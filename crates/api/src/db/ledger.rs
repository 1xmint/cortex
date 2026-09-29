//! The ledger and the verdict it answers to.
//!
//! Split out of `db.rs` because this is the surface Cortex sells correctness
//! of. In one 29,643-line file the credit writes sat between social feeds and
//! deploy status, reachable from anywhere and greppable only by luck. Here
//! they are a named boundary with a short list of callers.
//!
//! **This is a move, not a redesign.** Every function below is byte-identical
//! to what it was, still `impl Database`, still reached the same way from the
//! same call sites. Narrowing the write surface so the compiler enforces it --
//! `deduct_credits` takes a bare `&str` key today, when it should demand proof
//! the key came from `cortex_core::billing_binding` -- is a behavioural change
//! and belongs in its own review.
//!
//! What lives here: credit balances and their writes, the price list, frozen
//! step quotes, check specs, the verification claim/execute/seal cycle, the
//! receipt assembled from it, and billing events.

use super::*;
use rusqlite::OptionalExtension;

/// What [`Database::charge_settled_cost`] actually did: how many whole
/// credits it took and what fractional remainder carries forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettledCharge {
    /// Whole credits actually debited from the balance this call. `0` when
    /// the cost (plus whatever carry preceded it) is still under one credit,
    /// or when this call replayed an already-charged key.
    pub credits_charged: i64,
    /// `credit_balances.carry_micro_usd` after this charge: the fractional
    /// micro-USD remainder still owed, always `< micros_per_credit`.
    pub new_carry_micro_usd: u64,
}

impl Database {
    /// Honest credit balance read: returns `None` when no ledger row exists.
    /// Never invents a default (e.g. 200) — product APIs must use this.
    pub fn get_credit_balance_row(&self, clerk_user_id: &str) -> Option<CreditBalanceRecord> {
        let conn = self.conn();
        conn.query_row(
            "SELECT subscription_remaining, subscription_total, pack_remaining
             FROM credit_balances WHERE clerk_user_id = ?1",
            params![clerk_user_id],
            |row| {
                Ok(CreditBalanceRecord {
                    subscription_remaining: row.get(0)?,
                    subscription_total: row.get(1)?,
                    pack_remaining: row.get(2)?,
                })
            },
        )
        .ok()
    }

    /// Legacy helper that invents a default of 200 when no row exists.
    /// **Do not use for product billing/usage or Pulse metering** — prefer
    /// [`Self::get_credit_balance_row`] which returns `None` when unmetered.
    pub fn get_credit_balance(&self, clerk_user_id: &str) -> CreditBalanceRecord {
        self.get_credit_balance_row(clerk_user_id)
            .unwrap_or(CreditBalanceRecord {
                subscription_remaining: 200,
                subscription_total: 200,
                pack_remaining: 0,
            })
    }

    /// Deduct whole credits from an **existing** balance row. Does not invent a
    /// row or a default balance — returns an error when no balance row exists.
    ///
    /// **Exactly-once.** `idempotency_key` must uniquely identify the unit of
    /// work being charged; for task work use `run_id:step_id:attempt_id`.
    /// Replaying a key that has already been charged is a no-op that returns the
    /// current balance, because the orchestrator retries by design — steps carry
    /// `max_attempts: 3` and orphaned steps requeue on lease expiry, so without
    /// this a step charged and then rejected for a stale `lease_gen` is charged
    /// twice.
    ///
    /// A deduction can span both buckets, and two rows cannot share one UNIQUE
    /// key, so each bucket records under a derived key (`<key>:subscription`,
    /// `<key>:pack`). The UNIQUE constraint is the real guarantee; the explicit
    /// replay check below exists to return a balance rather than an error.
    pub fn deduct_credits(
        &self,
        clerk_user_id: &str,
        amount: i64,
        description: &str,
        key: &cortex_core::billing_binding::ChargeKey,
    ) -> Result<CreditBalanceRecord, String> {
        let idempotency_key = key.as_str();
        if amount < 0 {
            return Err("credit amount must be non-negative".into());
        }
        if idempotency_key.trim().is_empty() {
            return Err("idempotency key is required for a credit deduction".into());
        }

        let sub_key = format!("{idempotency_key}:subscription");
        let pack_key = format!("{idempotency_key}:pack");

        let conn = self.conn();

        conn.execute("BEGIN IMMEDIATE", [])
            .map_err(|e| format!("failed to begin transaction: {e}"))?;

        let read_balance = |conn: &Connection| {
            conn.query_row(
                "SELECT subscription_remaining, pack_remaining, subscription_total
                 FROM credit_balances WHERE clerk_user_id = ?1",
                params![clerk_user_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
        };

        let (sub_rem, pack_rem, sub_total) = match read_balance(&conn) {
            Ok(v) => v,
            Err(_) => {
                conn.execute("ROLLBACK", []).ok();
                return Err(
                    "no credit balance row — unmetered (refusing to invent a balance)".into(),
                );
            }
        };

        // Replay check, inside the transaction so it cannot race a concurrent
        // charge of the same key.
        let already: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM credit_transactions
                 WHERE idempotency_key = ?1 OR idempotency_key = ?2",
                params![sub_key, pack_key],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if already > 0 {
            conn.execute("ROLLBACK", []).ok();
            tracing::debug!(
                user_id = clerk_user_id,
                idempotency_key,
                "credit deduction replayed; balance unchanged"
            );
            return Ok(CreditBalanceRecord {
                subscription_remaining: sub_rem,
                subscription_total: sub_total,
                pack_remaining: pack_rem,
            });
        }

        if amount == 0 {
            conn.execute("ROLLBACK", []).ok();
            return Ok(CreditBalanceRecord {
                subscription_remaining: sub_rem,
                subscription_total: sub_total,
                pack_remaining: pack_rem,
            });
        }

        let total_available = sub_rem + pack_rem;
        if total_available < amount {
            conn.execute("ROLLBACK", []).ok();
            return Err(format!(
                "insufficient credits: need {amount}, have {total_available}"
            ));
        }

        // Spend the monthly allotment before purchased packs — the allotment
        // expires, packs do not.
        let from_sub = amount.min(sub_rem);
        let from_pack = amount - from_sub;

        let new_sub_rem = sub_rem - from_sub;
        let new_pack_rem = pack_rem - from_pack;

        // UPDATE only — a deduction must never create an account.
        let updated = conn
            .execute(
                "UPDATE credit_balances
                 SET subscription_remaining = ?1, pack_remaining = ?2
                 WHERE clerk_user_id = ?3",
                params![new_sub_rem, new_pack_rem, clerk_user_id],
            )
            .map_err(|e| {
                conn.execute("ROLLBACK", []).ok();
                format!("failed to update credit balance: {e}")
            })?;
        if updated == 0 {
            conn.execute("ROLLBACK", []).ok();
            return Err("no credit balance row — unmetered (refusing to invent a balance)".into());
        }

        let insert_tx = |bucket: &str, delta: i64, key: &str| -> Result<(), String> {
            let tx_id = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO credit_transactions
                    (id, clerk_user_id, amount, balance_type, reason, description, idempotency_key)
                 VALUES (?1, ?2, ?3, ?4, 'spend', ?5, ?6)",
                params![tx_id, clerk_user_id, delta, bucket, description, key],
            )
            .map_err(|e| format!("failed to record {bucket} transaction: {e}"))?;
            Ok(())
        };

        if from_sub > 0 {
            if let Err(e) = insert_tx("subscription", -from_sub, &sub_key) {
                conn.execute("ROLLBACK", []).ok();
                return Err(e);
            }
        }
        if from_pack > 0 {
            if let Err(e) = insert_tx("pack", -from_pack, &pack_key) {
                conn.execute("ROLLBACK", []).ok();
                return Err(e);
            }
        }

        conn.execute("COMMIT", [])
            .map_err(|e| format!("failed to commit transaction: {e}"))?;

        Ok(CreditBalanceRecord {
            subscription_remaining: new_sub_rem,
            subscription_total: sub_total,
            pack_remaining: new_pack_rem,
        })
    }

    /// Like [`Self::deduct_credits`], but never fails for insufficient
    /// funds: it clamps the charge to `min(amount, available)` in the same
    /// `BEGIN IMMEDIATE` transaction and the same idempotency check, so a
    /// reply that ran up a bill larger than the balance still gets charged
    /// once, for whatever the user actually had, rather than the caller
    /// having to choose between discarding the reply or eating the loss.
    ///
    /// Returns the number of credits actually charged (may be less than
    /// `amount`, or `0` if there was nothing left). Logs the shortfall via
    /// `tracing::warn` when the clamp bites.
    pub fn deduct_credits_up_to(
        &self,
        clerk_user_id: &str,
        amount: i64,
        description: &str,
        key: &cortex_core::billing_binding::ChargeKey,
    ) -> Result<i64, String> {
        let idempotency_key = key.as_str();
        if amount < 0 {
            return Err("credit amount must be non-negative".into());
        }
        if idempotency_key.trim().is_empty() {
            return Err("idempotency key is required for a credit deduction".into());
        }

        let sub_key = format!("{idempotency_key}:subscription");
        let pack_key = format!("{idempotency_key}:pack");

        let conn = self.conn();

        conn.execute("BEGIN IMMEDIATE", [])
            .map_err(|e| format!("failed to begin transaction: {e}"))?;

        let read_balance = |conn: &Connection| {
            conn.query_row(
                "SELECT subscription_remaining, pack_remaining, subscription_total
                 FROM credit_balances WHERE clerk_user_id = ?1",
                params![clerk_user_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
        };

        let (sub_rem, pack_rem, _sub_total) = match read_balance(&conn) {
            Ok(v) => v,
            Err(_) => {
                conn.execute("ROLLBACK", []).ok();
                return Err(
                    "no credit balance row — unmetered (refusing to invent a balance)".into(),
                );
            }
        };

        // Replay check, inside the transaction so it cannot race a concurrent
        // charge of the same key.
        let already: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM credit_transactions
                 WHERE idempotency_key = ?1 OR idempotency_key = ?2",
                params![sub_key, pack_key],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if already > 0 {
            conn.execute("ROLLBACK", []).ok();
            tracing::debug!(
                user_id = clerk_user_id,
                idempotency_key,
                "credit deduction replayed; balance unchanged"
            );
            return Ok(0);
        }

        let total_available = sub_rem + pack_rem;
        let clamped_amount = amount.min(total_available).max(0);
        if clamped_amount < amount {
            tracing::warn!(
                user_id = clerk_user_id,
                idempotency_key,
                wanted = amount,
                available = total_available,
                charged = clamped_amount,
                "final charge exceeds balance; clamping instead of discarding the reply"
            );
        }

        if clamped_amount == 0 {
            conn.execute("ROLLBACK", []).ok();
            return Ok(0);
        }

        let from_sub = clamped_amount.min(sub_rem);
        let from_pack = clamped_amount - from_sub;

        let new_sub_rem = sub_rem - from_sub;
        let new_pack_rem = pack_rem - from_pack;

        // UPDATE only — a deduction must never create an account.
        let updated = conn
            .execute(
                "UPDATE credit_balances
                 SET subscription_remaining = ?1, pack_remaining = ?2
                 WHERE clerk_user_id = ?3",
                params![new_sub_rem, new_pack_rem, clerk_user_id],
            )
            .map_err(|e| {
                conn.execute("ROLLBACK", []).ok();
                format!("failed to update credit balance: {e}")
            })?;
        if updated == 0 {
            conn.execute("ROLLBACK", []).ok();
            return Err("no credit balance row — unmetered (refusing to invent a balance)".into());
        }

        let insert_tx = |bucket: &str, delta: i64, key: &str| -> Result<(), String> {
            let tx_id = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO credit_transactions
                    (id, clerk_user_id, amount, balance_type, reason, description, idempotency_key)
                 VALUES (?1, ?2, ?3, ?4, 'spend', ?5, ?6)",
                params![tx_id, clerk_user_id, delta, bucket, description, key],
            )
            .map_err(|e| format!("failed to record {bucket} transaction: {e}"))?;
            Ok(())
        };

        if from_sub > 0 {
            if let Err(e) = insert_tx("subscription", -from_sub, &sub_key) {
                conn.execute("ROLLBACK", []).ok();
                return Err(e);
            }
        }
        if from_pack > 0 {
            if let Err(e) = insert_tx("pack", -from_pack, &pack_key) {
                conn.execute("ROLLBACK", []).ok();
                return Err(e);
            }
        }

        conn.execute("COMMIT", [])
            .map_err(|e| format!("failed to commit transaction: {e}"))?;

        Ok(clamped_amount)
    }

    /// Settle a chat reply's exact observed cost against the user's balance
    /// and carry, in one atomic transaction: reads `credit_balances`
    /// (including `carry_micro_usd`), runs `pricing::charge(carry, cost,
    /// micros_per_credit)` to decide how many whole credits are owed and what
    /// remainder carries forward, debits the credits, writes the new carry,
    /// and inserts one `credit_transactions` row per bucket actually drawn
    /// from, matching the convention `deduct_credits`/`deduct_credits_up_to`
    /// use elsewhere.
    ///
    /// The bare `key` always gets a `'subscription'`-typed row (amount
    /// `-from_sub`, `0` allowed) carrying the full `cost_micro_usd`, since
    /// that is the row the replay check below looks up; when the charge also
    /// draws from the pack, `{key}:pack` gets a second, `'pack'`-typed row
    /// (amount `-from_pack`, `cost_micro_usd` left `NULL` so the full cost
    /// isn't double-counted by any reader that sums it). A row is written
    /// even when it debits `0` whole credits — a reply costing less than one
    /// credit still must record its cost and advance the carry. This keeps
    /// `credit_ledger_totals`'s per-bucket sums in agreement with the balance
    /// cache; a single `'mixed'`-typed row would be invisible to that sum.
    ///
    /// In normal operation `credits_owed` can never exceed the balance:
    /// callers must reserve a spend cap that fits *before* making the call
    /// this charges for. If it ever does anyway (e.g. a concurrent charge
    /// shrank the balance after this call's own reservation succeeded), the
    /// deduction is clamped to what's left — balances never go negative and
    /// this never invents credit. `cost_micro_usd` on the ledger row still
    /// records the full, true cost, and the carry still advances by the
    /// pricing math's full total (carry + cost) — but the whole credits this
    /// call could not collect are simply not collected from anyone, ever:
    /// Cortex absorbs them. That absorption is recorded, not hidden — the
    /// shortfall (owed vs. charged) is written into the ledger row's own
    /// `description`, alongside a `tracing::warn!` that marks the clamp.
    ///
    /// Idempotent on `key`: a replay changes nothing and returns
    /// `credits_charged: 0` with the balance's current carry.
    pub fn charge_settled_cost(
        &self,
        clerk_user_id: &str,
        cost_micro_usd: u64,
        micros_per_credit: i64,
        description: &str,
        key: &cortex_core::billing_binding::ChargeKey,
    ) -> Result<SettledCharge, String> {
        let conn = self.conn();

        conn.execute("BEGIN IMMEDIATE", [])
            .map_err(|e| format!("failed to begin transaction: {e}"))?;

        match Self::charge_settled_cost_in_tx(
            &conn,
            clerk_user_id,
            cost_micro_usd,
            micros_per_credit,
            description,
            key,
        ) {
            Ok(settled) => {
                conn.execute("COMMIT", [])
                    .map_err(|e| format!("failed to commit transaction: {e}"))?;
                Ok(settled)
            }
            Err(e) => {
                conn.execute("ROLLBACK", []).ok();
                Err(e)
            }
        }
    }

    /// The core of [`Self::charge_settled_cost`], for a caller that already
    /// holds a transaction it controls the boundaries of — namely
    /// [`Self::settle_one_pending_attempt`], which must run this as one step
    /// of a single larger transaction rather than its own. Does no `BEGIN`,
    /// `COMMIT`, or `ROLLBACK` of its own: an `Err` here leaves the caller's
    /// transaction exactly as it found it, for the caller to roll back.
    fn charge_settled_cost_in_tx(
        conn: &Connection,
        clerk_user_id: &str,
        cost_micro_usd: u64,
        micros_per_credit: i64,
        description: &str,
        key: &cortex_core::billing_binding::ChargeKey,
    ) -> Result<SettledCharge, String> {
        let idempotency_key = key.as_str();
        if idempotency_key.trim().is_empty() {
            return Err("idempotency key is required for a settled charge".into());
        }
        if micros_per_credit <= 0 {
            return Err("micros_per_credit must be positive".into());
        }

        let read_balance = |conn: &Connection| {
            conn.query_row(
                "SELECT subscription_remaining, pack_remaining, carry_micro_usd
                 FROM credit_balances WHERE clerk_user_id = ?1",
                params![clerk_user_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
        };

        let (sub_rem, pack_rem, carry_micro_usd) = read_balance(conn).map_err(|_| {
            "no credit balance row — unmetered (refusing to invent a balance)".to_string()
        })?;

        // Replay check inside the transaction, so it can't race a concurrent
        // charge of the same key.
        let already: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM credit_transactions WHERE idempotency_key = ?1",
                params![idempotency_key],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if already > 0 {
            tracing::debug!(
                user_id = clerk_user_id,
                idempotency_key,
                "settled charge replayed; balance and carry unchanged"
            );
            return Ok(SettledCharge {
                credits_charged: 0,
                new_carry_micro_usd: carry_micro_usd as u64,
            });
        }

        let (credits_owed, new_carry) = crate::pricing::charge(
            carry_micro_usd as u64,
            cost_micro_usd,
            micros_per_credit as u64,
        );

        let total_available = sub_rem + pack_rem;
        let credits_owed_i64 = i64::try_from(credits_owed).unwrap_or(i64::MAX);
        let actual_charged = credits_owed_i64.min(total_available).max(0);
        if actual_charged < credits_owed_i64 {
            tracing::warn!(
                user_id = clerk_user_id,
                idempotency_key,
                owed = credits_owed_i64,
                available = total_available,
                charged = actual_charged,
                "settled cost exceeds balance; clamping the deduction, not the recorded cost"
            );
        }

        let from_sub = actual_charged.min(sub_rem);
        let from_pack = actual_charged - from_sub;
        let new_sub_rem = sub_rem - from_sub;
        let new_pack_rem = pack_rem - from_pack;
        let new_carry_i64 = i64::try_from(new_carry).unwrap_or(i64::MAX);

        let updated = conn
            .execute(
                "UPDATE credit_balances
                 SET subscription_remaining = ?1, pack_remaining = ?2, carry_micro_usd = ?3
                 WHERE clerk_user_id = ?4",
                params![new_sub_rem, new_pack_rem, new_carry_i64, clerk_user_id],
            )
            .map_err(|e| format!("failed to update credit balance: {e}"))?;
        if updated == 0 {
            return Err("no credit balance row — unmetered (refusing to invent a balance)".into());
        }

        // Unpaid whole credits (owed > charged) are never collected from
        // anyone — Cortex absorbs them. Record that in the row's own
        // description rather than only in a log line, so it's durable and
        // auditable from the ledger itself.
        let cost_i64 = i64::try_from(cost_micro_usd).unwrap_or(i64::MAX);
        let row_description = if actual_charged < credits_owed_i64 {
            format!(
                "{description} (owed {credits_owed_i64}, charged {actual_charged}; \
                 shortfall absorbed by Cortex)"
            )
        } else {
            description.to_string()
        };

        let tx_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO credit_transactions
                (id, clerk_user_id, amount, balance_type, reason, description,
                 idempotency_key, cost_micro_usd)
             VALUES (?1, ?2, ?3, 'subscription', 'spend', ?4, ?5, ?6)",
            params![
                tx_id,
                clerk_user_id,
                -from_sub,
                row_description,
                idempotency_key,
                cost_i64
            ],
        )
        .map_err(|e| format!("failed to record settled charge: {e}"))?;

        if from_pack > 0 {
            let pack_tx_id = Uuid::new_v4().to_string();
            let pack_key = format!("{idempotency_key}:pack");
            conn.execute(
                "INSERT INTO credit_transactions
                    (id, clerk_user_id, amount, balance_type, reason, description,
                     idempotency_key, cost_micro_usd)
                 VALUES (?1, ?2, ?3, 'pack', 'spend', ?4, ?5, NULL)",
                params![
                    pack_tx_id,
                    clerk_user_id,
                    -from_pack,
                    row_description,
                    pack_key
                ],
            )
            .map_err(|e| format!("failed to record settled pack charge: {e}"))?;
        }

        Ok(SettledCharge {
            credits_charged: actual_charged,
            new_carry_micro_usd: new_carry,
        })
    }

    /// Sum of one task attempt's confirmed observed cost, across every
    /// `provider_request_reservations` row `record_gateway_request`/
    /// `settle_provider_request` wrote for it.
    ///
    /// Only `status = 'settled'` rows count. `reserved` means the call may
    /// still be in flight; `released` means it never happened; `unresolved`
    /// means the gateway never heard back from the supplier to confirm a
    /// price; `mismatch` means the reconciliation contradicted itself or blew
    /// past the reservation and is sitting in `admin.rs`'s stuck-reservation
    /// queue for an operator to resolve. None of those four are a number
    /// Cortex has actually confirmed — charging from one of them risks
    /// billing for a call that didn't happen, or missing a call that did and
    /// silently absorbing it forever. `unresolved` and `mismatch` rows are
    /// logged so the gap is visible instead of silent; the settlement itself
    /// waits for reconciliation rather than guessing.
    ///
    /// An attempt with zero reservations at all (no provider call was ever
    /// made — e.g. it never got past a lease before the customer cancelled)
    /// returns `0`, which the caller turns into "no ledger row" rather than a
    /// zero-amount charge.
    pub fn attempt_settled_cost_micro_usd(&self, attempt_id: &str) -> u64 {
        let conn = self.conn();

        let stuck: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM provider_request_reservations
                 WHERE attempt_id = ?1 AND status IN ('unresolved', 'mismatch')",
                params![attempt_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if stuck > 0 {
            tracing::warn!(
                attempt_id,
                stuck_reservations = stuck,
                "attempt has unresolved/mismatched provider reservations; \
                 excluded from its settled cost pending reconciliation"
            );
        }

        let settled: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(observed_micro_usd), 0)
                 FROM provider_request_reservations
                 WHERE attempt_id = ?1 AND status = 'settled'",
                params![attempt_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        settled.max(0) as u64
    }

    /// Record that Cortex, not the customer, absorbed a task attempt's
    /// settled observed cost — an infrastructure failure ended the attempt
    /// before a verdict could be reached, so there is nothing to charge for
    /// and nothing to refund.
    ///
    /// Writes one `credit_transactions` row with `amount = 0`: the balance
    /// and carry are untouched, but the cost is on the record, against
    /// Cortex, exactly the way a settled charge would record it against the
    /// customer. Idempotent on `key` (the same `ChargeKey::for_attempt` used
    /// for a charge, since exactly one of charge-or-absorb ever happens for a
    /// given attempt) — a replay is a silent no-op, not a second zero row.
    ///
    /// Takes `clerk_user_id` purely for the record — `credit_transactions`
    /// requires one on every row, and every call site of this function
    /// already has it (it's the same run/user lookup `charge_settled_cost`
    /// needs for the charge path, resolved before either branch is chosen).
    /// The row's `amount` is always `0`: this never touches a balance.
    pub fn absorb_attempt_cost(
        &self,
        clerk_user_id: &str,
        cost_micro_usd: u64,
        description: &str,
        key: &cortex_core::billing_binding::ChargeKey,
    ) -> Result<(), String> {
        let conn = self.conn();

        conn.execute("BEGIN IMMEDIATE", [])
            .map_err(|e| format!("failed to begin transaction: {e}"))?;

        match Self::absorb_attempt_cost_in_tx(
            &conn,
            clerk_user_id,
            cost_micro_usd,
            description,
            key,
        ) {
            Ok(()) => {
                conn.execute("COMMIT", [])
                    .map_err(|e| format!("failed to commit transaction: {e}"))?;
                Ok(())
            }
            Err(e) => {
                conn.execute("ROLLBACK", []).ok();
                Err(e)
            }
        }
    }

    /// The core of [`Self::absorb_attempt_cost`], for a caller
    /// ([`Self::settle_one_pending_attempt`]) that already holds a
    /// transaction it controls the boundaries of. Does no `BEGIN`, `COMMIT`,
    /// or `ROLLBACK` of its own.
    fn absorb_attempt_cost_in_tx(
        conn: &Connection,
        clerk_user_id: &str,
        cost_micro_usd: u64,
        description: &str,
        key: &cortex_core::billing_binding::ChargeKey,
    ) -> Result<(), String> {
        let idempotency_key = key.as_str();
        if idempotency_key.trim().is_empty() {
            return Err("idempotency key is required to record an absorbed cost".into());
        }

        let already: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM credit_transactions WHERE idempotency_key = ?1",
                params![idempotency_key],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if already > 0 {
            tracing::debug!(
                idempotency_key,
                "absorbed-cost record replayed; nothing written again"
            );
            return Ok(());
        }

        let cost_i64 = i64::try_from(cost_micro_usd).unwrap_or(i64::MAX);
        let tx_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO credit_transactions
                (id, clerk_user_id, amount, balance_type, reason, description,
                 idempotency_key, cost_micro_usd)
             VALUES (?1, ?2, 0, 'subscription', 'absorbed', ?3, ?4, ?5)",
            params![tx_id, clerk_user_id, description, idempotency_key, cost_i64],
        )
        .map_err(|e| format!("failed to record absorbed cost: {e}"))?;

        Ok(())
    }

    /// The same sum [`Self::attempt_settled_cost_micro_usd`] computes, for a
    /// caller ([`Self::settle_one_pending_attempt`]) that already holds the
    /// settler's transaction and must see a real error rather than a
    /// swallowed `0` if the read fails — a silent `0` here would read as "no
    /// cost" and skip billing entirely, instead of retrying next tick.
    ///
    /// Returns `Ok(None)` — not an `Err` — when a reservation is still
    /// `reserved`/`unresolved`/`mismatch`: this is an ordinary "not ready
    /// yet" outcome the settler retries next tick, not a failure worth
    /// logging as one every tick until the gateway reconciles (F7 of the
    /// money-review fix pass). `Err` is reserved for a genuine read failure.
    fn attempt_settled_cost_micro_usd_in_tx(
        conn: &Connection,
        attempt_id: &str,
    ) -> Result<Option<u64>, String> {
        let blocking: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM provider_request_reservations
                 WHERE attempt_id = ?1 AND status IN ('reserved', 'unresolved', 'mismatch')",
                params![attempt_id],
                |row| row.get(0),
            )
            .map_err(|e| {
                format!("failed to check reservation readiness for attempt {attempt_id}: {e}")
            })?;
        if blocking > 0 {
            return Ok(None);
        }

        let settled: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(observed_micro_usd), 0)
                 FROM provider_request_reservations
                 WHERE attempt_id = ?1 AND status = 'settled'",
                params![attempt_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("failed to sum settled cost for attempt {attempt_id}: {e}"))?;
        Ok(Some(settled.max(0) as u64))
    }

    /// Mirrors [`Self::active_price_list`]'s `ORDER BY version DESC LIMIT 1`
    /// selection, reading only the one column [`Self::settle_one_pending_attempt`]
    /// needs, for a caller that already holds the settler's transaction (and
    /// so cannot call `active_price_list`, which opens its own `self.conn()`
    /// and would deadlock against the plain, non-reentrant connection mutex).
    fn active_micros_per_credit_in_tx(conn: &Connection) -> Option<i64> {
        conn.query_row(
            "SELECT micros_per_credit FROM price_lists ORDER BY version DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .ok()
    }

    /// Settle exactly one unsettled `attempt_endings` row: charge or absorb
    /// its settled observed cost per `billing_binding::settle_attempt`, then
    /// mark it settled — all inside one `BEGIN IMMEDIATE` transaction, so a
    /// crash between the ledger write and the `settled_at` stamp is
    /// impossible; either both happen or neither does, and
    /// `settle_pending_attempts` retries it next tick.
    ///
    /// Order inside the transaction: (1) re-read the row, so a concurrent
    /// settle of the same attempt (a race that shouldn't happen given the
    /// single connection mutex, but is cheap to make impossible anyway) is a
    /// silent no-op if it's already settled or gone; (2) the settled-cost
    /// sum, which also enforces that no reservation for this attempt is
    /// still `reserved`/`unresolved`/`mismatch` — if one is, this returns
    /// `Ok(())` without settling, so the next tick retries once the gateway
    /// finishes reconciling; (3) the charge or the zero-amount absorb
    /// insert, or neither at all when the settled cost is exactly zero; (4)
    /// stamp `settled_at` and clear `last_error`.
    ///
    /// `Ok(())` covers two distinct outcomes on purpose (F7 of the
    /// money-review fix pass): the attempt settled just now, or it is still
    /// waiting on a reservation and will be retried next tick — neither is a
    /// failure, so neither should spam an error log every tick. Only a
    /// genuine problem (a DB error, an unreadable row, a missing price list)
    /// is an `Err`, and only an `Err` here is persisted to
    /// `attempt_endings.last_error` (by the caller, after the rollback below,
    /// as a separate write outside this now-rolled-back transaction).
    fn settle_one_pending_attempt(&self, attempt_id: &str) -> Result<(), String> {
        let conn = self.conn();

        conn.execute("BEGIN IMMEDIATE", [])
            .map_err(|e| format!("failed to begin transaction: {e}"))?;

        let outcome = Self::settle_one_pending_attempt_in_tx(&conn, attempt_id);

        match outcome {
            Ok(()) => {
                conn.execute("COMMIT", [])
                    .map_err(|e| format!("failed to commit transaction: {e}"))?;
                Ok(())
            }
            Err(e) => {
                conn.execute("ROLLBACK", []).ok();
                // A separate write, deliberately outside the transaction that
                // just rolled back: the failure itself must survive on the
                // row so it's visible (and so a human or a test can see why
                // an attempt is stuck) even though nothing else about this
                // attempt could be committed.
                if let Err(update_err) = conn.execute(
                    "UPDATE attempt_endings SET last_error = ?1 WHERE attempt_id = ?2",
                    params![e.clone(), attempt_id],
                ) {
                    tracing::error!(
                        attempt_id,
                        error = %update_err,
                        "failed to persist last_error after a settlement failure"
                    );
                }
                Err(e)
            }
        }
    }

    /// The core of [`Self::settle_one_pending_attempt`]: the four ordered
    /// steps documented there, run against a transaction its caller already
    /// opened. Does no `BEGIN`, `COMMIT`, or `ROLLBACK` of its own.
    fn settle_one_pending_attempt_in_tx(conn: &Connection, attempt_id: &str) -> Result<(), String> {
        use cortex_core::billing_binding::{self, AttemptSettlement};

        let row: Option<(String, String, String, bool)> = conn
            .query_row(
                "SELECT user_id, step_id, cause, worker_owned_by_cortex
                 FROM attempt_endings WHERE attempt_id = ?1 AND settled_at IS NULL",
                params![attempt_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)? != 0,
                    ))
                },
            )
            .optional()
            .map_err(|e| format!("failed to read attempt_endings row for {attempt_id}: {e}"))?;

        let Some((user_id, _step_id, cause_str, _worker_owned_by_cortex)) = row else {
            // Already settled (a race) or no such attempt at all: nothing to
            // do.
            return Ok(());
        };
        let end_cause = super::attempt_end_cause_from_str(&cause_str).ok_or_else(|| {
            format!("attempt {attempt_id} has unrecognised stored cause {cause_str:?}")
        })?;

        let now = Utc::now().timestamp_millis();
        let Some(cost_micro_usd) = Self::attempt_settled_cost_micro_usd_in_tx(conn, attempt_id)?
        else {
            // Still waiting on a reservation to reconcile — not an error (see
            // this function's doc comment), just not ready yet. Leave
            // `settled_at` and `last_error` untouched and let the next tick
            // try again.
            return Ok(());
        };

        if cost_micro_usd > 0 {
            let key = billing_binding::ChargeKey::for_attempt(attempt_id);
            match billing_binding::settle_attempt(end_cause) {
                AttemptSettlement::Charge => {
                    let micros_per_credit =
                        Self::active_micros_per_credit_in_tx(conn).ok_or_else(|| {
                            "no active price list; cannot convert the ended attempt's \
                             observed cost to credits"
                                .to_string()
                        })?;
                    Self::charge_settled_cost_in_tx(
                        conn,
                        &user_id,
                        cost_micro_usd,
                        micros_per_credit,
                        billing_binding::reason::TASK_ATTEMPT_CHARGED,
                        &key,
                    )?;
                }
                AttemptSettlement::Absorb(cause) => {
                    let description = format!(
                        "{}: {cause:?}",
                        billing_binding::reason::TASK_ATTEMPT_ABSORBED
                    );
                    Self::absorb_attempt_cost_in_tx(
                        conn,
                        &user_id,
                        cost_micro_usd,
                        &description,
                        &key,
                    )?;
                }
            }
        }

        conn.execute(
            "UPDATE attempt_endings SET settled_at = ?1, last_error = NULL WHERE attempt_id = ?2",
            params![now, attempt_id],
        )
        .map_err(|e| format!("failed to stamp settled_at for attempt {attempt_id}: {e}"))?;

        Ok(())
    }

    /// The one place that turns a durable `attempt_endings` record into a
    /// ledger effect. Driven by the scheduler's tick loop and a startup pass,
    /// not by any end path directly — every end path (a sealed verdict,
    /// `cancel_run`, `expire_stale_leases`, a `ws.rs` failure/rejection/block
    /// path) only ever writes the durable record
    /// (`Database::insert_attempt_ending_in_tx` /
    /// `Database::record_attempt_ended`); this is what reads it back and
    /// charges or absorbs.
    ///
    /// A stuck reservation is not a failure — `settle_one_pending_attempt`
    /// returns `Ok(())` for it and simply leaves the row unsettled for the
    /// next tick to retry. What genuinely fails here (a missing price list,
    /// a DB error) is logged and skipped, not propagated, so one bad attempt
    /// can't block every other attempt's tick; it is also written to that
    /// row's own `last_error` for visibility (F7 of the money-review fix
    /// pass), rather than existing only in the log.
    pub fn settle_pending_attempts(&self) -> Result<(), String> {
        let pending: Vec<String> = {
            let conn = self.conn();
            let mut stmt = conn
                .prepare("SELECT attempt_id FROM attempt_endings WHERE settled_at IS NULL")
                .map_err(|e| format!("failed to list pending attempt endings: {e}"))?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| format!("failed to list pending attempt endings: {e}"))?;
            // Every row is a real attempt id this same query just produced --
            // a read failure here (a corrupt row, a type mismatch) is a
            // genuine problem, not something to drop silently the way
            // `filter_map(|r| r.ok())` used to (F7): it would have hidden an
            // attempt from settlement with no trace of why.
            let mut ids = Vec::new();
            for row in rows {
                match row {
                    Ok(id) => ids.push(id),
                    Err(e) => tracing::error!(
                        error = %e,
                        "failed to read a pending attempt id; skipping it this tick"
                    ),
                }
            }
            ids
        };

        for attempt_id in pending {
            if let Err(e) = self.settle_one_pending_attempt(&attempt_id) {
                tracing::error!(
                    attempt_id,
                    error = %e,
                    "failed to settle pending attempt; will retry next tick"
                );
            }
        }

        Ok(())
    }

    /// The carry alone: the fractional micro-USD remainder from previous
    /// exact charges that hasn't yet added up to one whole credit. `0` for a
    /// user with no balance row.
    pub fn get_credit_carry_micro_usd(&self, clerk_user_id: &str) -> u64 {
        self.conn()
            .query_row(
                "SELECT carry_micro_usd FROM credit_balances WHERE clerk_user_id = ?1",
                params![clerk_user_id],
                |row| row.get::<_, i64>(0),
            )
            .map(|v| v.max(0) as u64)
            .unwrap_or(0)
    }

    /// Give back exactly what a charge took, when a verdict says the work
    /// failed. See `cortex/plan/VERIFIER.md` ("Billing binding").
    ///
    /// This takes the *charge's* idempotency key rather than an amount, and
    /// that is deliberate. `deduct_credits` splits one charge across the
    /// subscription and pack buckets according to what was left in each at the
    /// time, and only the rows it wrote know how the split fell. Passing an
    /// amount would force this function to guess the split, and a wrong guess
    /// silently moves credits between an expiring bucket and a permanent one.
    /// So a refund reads the spend rows and mirrors them.
    ///
    /// Append-only: a refund is a positive row with reason
    /// `task_failed_refund`, never an UPDATE of the spend row.
    /// Return credits taken under `charge`.
    ///
    /// The two keys are different types on purpose. They used to be two bare
    /// `&str` in a row, so passing them the wrong way round compiled: the
    /// refund would look for a charge under its own key, find none, and move
    /// no money while reporting success. Now that is a compile error.
    pub fn refund_credits(
        &self,
        clerk_user_id: &str,
        charge: &cortex_core::billing_binding::ChargeKey,
        refund: &cortex_core::billing_binding::RefundKey,
        description: &str,
    ) -> Result<CreditBalanceRecord, String> {
        let charge_idempotency_key = charge.as_str();
        let refund_idempotency_key = refund.as_str();
        if charge_idempotency_key.trim().is_empty() || refund_idempotency_key.trim().is_empty() {
            return Err("both charge and refund idempotency keys are required".into());
        }

        let charge_sub_key = format!("{charge_idempotency_key}:subscription");
        let charge_pack_key = format!("{charge_idempotency_key}:pack");
        let refund_sub_key = format!("{refund_idempotency_key}:subscription");
        let refund_pack_key = format!("{refund_idempotency_key}:pack");

        let conn = self.conn();

        conn.execute("BEGIN IMMEDIATE", [])
            .map_err(|e| format!("failed to begin transaction: {e}"))?;

        let read_balance = |conn: &Connection| {
            conn.query_row(
                "SELECT subscription_remaining, pack_remaining, subscription_total
                 FROM credit_balances WHERE clerk_user_id = ?1",
                params![clerk_user_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
        };

        let (sub_rem, pack_rem, sub_total) = match read_balance(&conn) {
            Ok(v) => v,
            Err(_) => {
                conn.execute("ROLLBACK", []).ok();
                return Err(
                    "no credit balance row — unmetered (refusing to invent a balance)".into(),
                );
            }
        };

        let unchanged = CreditBalanceRecord {
            subscription_remaining: sub_rem,
            subscription_total: sub_total,
            pack_remaining: pack_rem,
        };

        // Replay check first: the ledger's UNIQUE key is the backstop if the
        // process died between verdict and refund, and re-deriving the same
        // key must make the write a no-op rather than an error.
        let already: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM credit_transactions
                 WHERE idempotency_key = ?1 OR idempotency_key = ?2",
                params![refund_sub_key, refund_pack_key],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if already > 0 {
            conn.execute("ROLLBACK", []).ok();
            tracing::debug!(
                user_id = clerk_user_id,
                refund_idempotency_key,
                "credit refund replayed; balance unchanged"
            );
            return Ok(unchanged);
        }

        // Mirror the spend rows. `amount` is negative on a spend, so negating
        // it yields what to give back, per bucket.
        let read_spend = |key: &str| -> i64 {
            conn.query_row(
                "SELECT COALESCE(SUM(amount), 0) FROM credit_transactions
                 WHERE idempotency_key = ?1 AND amount < 0",
                params![key],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
        };
        let to_sub = -read_spend(&charge_sub_key);
        let to_pack = -read_spend(&charge_pack_key);

        if to_sub == 0 && to_pack == 0 {
            // Nothing was ever charged under that key. Not an error — the
            // billing state machine only asks for a refund when it believes a
            // charge landed, and disagreeing with it must not take the caller
            // down. It is worth an operator's attention.
            conn.execute("ROLLBACK", []).ok();
            tracing::warn!(
                user_id = clerk_user_id,
                charge_idempotency_key,
                "refund requested but no matching spend rows; nothing to give back"
            );
            return Ok(unchanged);
        }

        let new_sub_rem = sub_rem + to_sub;
        let new_pack_rem = pack_rem + to_pack;

        // UPDATE only — a refund must never create an account.
        let updated = conn
            .execute(
                "UPDATE credit_balances
                 SET subscription_remaining = ?1, pack_remaining = ?2
                 WHERE clerk_user_id = ?3",
                params![new_sub_rem, new_pack_rem, clerk_user_id],
            )
            .map_err(|e| {
                conn.execute("ROLLBACK", []).ok();
                format!("failed to update credit balance: {e}")
            })?;
        if updated == 0 {
            conn.execute("ROLLBACK", []).ok();
            return Err("no credit balance row — unmetered (refusing to invent a balance)".into());
        }

        let insert_tx = |bucket: &str, delta: i64, key: &str| -> Result<(), String> {
            let tx_id = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO credit_transactions
                    (id, clerk_user_id, amount, balance_type, reason, description, idempotency_key)
                 VALUES (?1, ?2, ?3, ?4, 'task_failed_refund', ?5, ?6)",
                params![tx_id, clerk_user_id, delta, bucket, description, key],
            )
            .map_err(|e| format!("failed to record {bucket} refund: {e}"))?;
            Ok(())
        };

        if to_sub > 0 {
            if let Err(e) = insert_tx("subscription", to_sub, &refund_sub_key) {
                conn.execute("ROLLBACK", []).ok();
                return Err(e);
            }
        }
        if to_pack > 0 {
            if let Err(e) = insert_tx("pack", to_pack, &refund_pack_key) {
                conn.execute("ROLLBACK", []).ok();
                return Err(e);
            }
        }

        conn.execute("COMMIT", [])
            .map_err(|e| format!("failed to commit transaction: {e}"))?;

        tracing::info!(
            user_id = clerk_user_id,
            refund_idempotency_key,
            subscription = to_sub,
            pack = to_pack,
            "refunded credits for a failed verdict"
        );

        Ok(CreditBalanceRecord {
            subscription_remaining: new_sub_rem,
            subscription_total: sub_total,
            pack_remaining: new_pack_rem,
        })
    }

    /// Sum of the append-only transaction log per bucket, for reconciling
    /// against `credit_balances`. The balance columns are a cache; this is the
    /// derivation they must agree with.
    pub fn credit_ledger_totals(&self, clerk_user_id: &str) -> (i64, i64) {
        let conn = self.conn();
        let sum = |bucket: &str| -> i64 {
            conn.query_row(
                "SELECT COALESCE(SUM(amount), 0) FROM credit_transactions
                 WHERE clerk_user_id = ?1 AND balance_type = ?2",
                params![clerk_user_id, bucket],
                |r| r.get(0),
            )
            .unwrap_or(0)
        };
        (sum("subscription"), sum("pack"))
    }

    // --- V3: verdict persistence (see cortex/plan/VERIFIER.md) ---

    /// Whether the ledger already carries a transaction under this key.
    ///
    /// `deduct_credits` and `refund_credits` both suffix the key per bucket,
    /// so this asks about either. Used to derive `BillingState` from the
    /// ledger itself rather than from a status column that could drift out of
    /// agreement with the money.
    pub fn ledger_has_key(&self, idempotency_key: &str) -> bool {
        let conn = self.conn();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM credit_transactions
                 WHERE idempotency_key = ?1 OR idempotency_key = ?2",
                params![
                    format!("{idempotency_key}:subscription"),
                    format!("{idempotency_key}:pack")
                ],
                |r| r.get(0),
            )
            .unwrap_or(0);
        count > 0
    }

    /// What the ledger actually moved for one verification's charge and any
    /// refund of it, net -- not the quote, and not a status column.
    ///
    /// `deduct_credits` writes negative amounts under
    /// `verify:<id>:{subscription,pack}`; `refund_credits` mirrors them
    /// positive under `refund:<id>:{subscription,pack}`. Summing all four and
    /// negating gives the amount still standing against the customer: the
    /// full charge while unrefunded, and exactly `0` once refunded, without
    /// this function having to know which state it is in.
    ///
    /// `None` means no charge was ever written for this verification --
    /// distinct from `Some(0)`, which means one was written and then refunded
    /// in full.
    pub fn ledger_net_charge_for_verification(&self, verification_id: &str) -> Option<i64> {
        let charge_key = cortex_core::billing_binding::ChargeKey::for_verification(verification_id);
        let refund_key = cortex_core::billing_binding::RefundKey::for_verification(verification_id);
        let conn = self.conn();
        let keys = [
            format!("{}:subscription", charge_key.as_str()),
            format!("{}:pack", charge_key.as_str()),
            format!("{}:subscription", refund_key.as_str()),
            format!("{}:pack", refund_key.as_str()),
        ];
        // One query for both facts, so a torn read between "how many rows"
        // and "what do they sum to" cannot happen. A DB error is logged and
        // surfaced as `None` -- "no charge" and "could not read the ledger"
        // must not look identical to a caller deciding whether to display a
        // charge, so the difference has to at least reach the logs.
        let row = conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(amount), 0) FROM credit_transactions
             WHERE idempotency_key IN (?1, ?2, ?3, ?4)",
            params![keys[0], keys[1], keys[2], keys[3]],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        );
        let (count, sum) = match row {
            Ok(v) => v,
            Err(err) => {
                tracing::error!(
                    verification_id,
                    error = %err,
                    "ledger_net_charge_for_verification: query failed"
                );
                return None;
            }
        };
        if count == 0 {
            return None;
        }
        Some(-sum)
    }

    /// What the ledger actually moved for one task attempt's settled-cost
    /// charge, net.
    ///
    /// The attempt-billing counterpart to [`Self::ledger_net_charge_for_verification`],
    /// for the observed-cost charge `finish_and_bill` writes under
    /// `ChargeKey::for_attempt`. There is no refund key to sum here: nothing
    /// in `settle_attempt` ever refunds an attempt charge, so the sum of the
    /// charge rows alone is the amount still standing.
    ///
    /// `None` means no charge was ever written for this attempt (including:
    /// it was absorbed, not charged).
    pub fn ledger_net_charge_for_attempt(&self, attempt_id: &str) -> Option<i64> {
        let charge_key = cortex_core::billing_binding::ChargeKey::for_attempt(attempt_id);
        let conn = self.conn();
        // `charge_settled_cost` writes the subscription-bucket row under the
        // BARE idempotency key (never a `:subscription` suffix) and, only
        // when `from_pack > 0`, a second pack-bucket row under `{key}:pack`.
        // A previous version of this query looked for `{key}:subscription`,
        // which `charge_settled_cost` never writes, so it silently missed
        // every attempt's subscription-bucket charge. `reason = 'spend'`
        // excludes an `absorbed` (zero-amount) row from the sum, which
        // matters once `absorb_attempt_cost` and this share the same
        // `attempt:{id}` key prefix.
        let keys = [
            charge_key.as_str().to_string(),
            format!("{}:pack", charge_key.as_str()),
        ];
        let row = conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(amount), 0) FROM credit_transactions
             WHERE idempotency_key IN (?1, ?2) AND reason = 'spend'",
            params![keys[0], keys[1]],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        );
        let (count, sum) = match row {
            Ok(v) => v,
            Err(err) => {
                tracing::error!(
                    attempt_id,
                    error = %err,
                    "ledger_net_charge_for_attempt: query failed"
                );
                return None;
            }
        };
        if count == 0 {
            return None;
        }
        Some(-sum)
    }

    /// Freeze the derived checks for a step at dispatch time.
    ///
    /// Derivation must happen before the worker sees the task, and the checks
    /// must be executed after delivery. This is where they wait. Writing twice
    /// for the same step is a no-op rather than an overwrite: the frozen set is
    /// the exam, and re-deriving it later would let a task influence its own.
    pub fn save_check_specs(
        &self,
        run_id: &str,
        step_id: &str,
        specs: &[CheckSpec],
    ) -> Result<(), String> {
        let specs_json = serde_json::to_string(specs)
            .map_err(|e| format!("failed to serialize check specs: {e}"))?;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO verification_specs (run_id, step_id, specs_json, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(run_id, step_id) DO NOTHING",
            params![run_id, step_id, specs_json, Utc::now().timestamp()],
        )
        .map_err(|e| format!("failed to freeze check specs: {e}"))?;
        Ok(())
    }

    // --- Price lists and step quotes (PR I) ---

    /// Publish a price list. There is no update path, by design.
    ///
    /// Invariant 23: a shared artifact is immutable and versioned. The triggers
    /// in migration v66 make an `UPDATE` or `DELETE` on any of these tables an
    /// error at the storage layer, so this is the only way a price ever changes
    /// — by a new version existing beside the old one, which every prior receipt
    /// still names.
    ///
    /// Fails rather than overwrites when the version already exists. A
    /// republish that silently replaced a version would be the edit the
    /// invariant forbids, wearing an insert's clothes.
    pub fn publish_price_list(&self, list: &crate::pricing::PriceList) -> Result<(), String> {
        let mut conn = self.conn();
        let tx = conn
            .transaction()
            .map_err(|e| format!("failed to open a transaction: {e}"))?;

        tx.execute(
            "INSERT INTO price_lists
                (id, version, status, micros_per_credit, basis, published_at, published_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                list.id,
                list.version,
                list.status.as_str(),
                list.micros_per_credit,
                list.basis,
                list.published_at,
                list.published_by,
            ],
        )
        .map_err(|e| format!("failed to publish price list v{}: {e}", list.version))?;

        for model in &list.models {
            tx.execute(
                "INSERT INTO price_list_models
                    (price_list_id, provider, model_id, input_micros_per_1k,
                     output_micros_per_1k, cache_read_bp, context_window, capability_class)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    list.id,
                    model.provider,
                    model.model_id,
                    model.input_micros_per_1k,
                    model.output_micros_per_1k,
                    model.cache_read_bp,
                    model.context_window,
                    model.capability_class,
                ],
            )
            .map_err(|e| format!("failed to publish model {}: {e}", model.model_id))?;
        }

        for class in &list.classes {
            tx.execute(
                "INSERT INTO price_list_task_classes
                    (price_list_id, task_class, quoted_credits, status,
                     sample_count, measured_cost_micros, margin_bp)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    list.id,
                    class.task_class,
                    class.quoted_credits,
                    class.status.as_str(),
                    class.sample_count,
                    class.measured_cost_micros,
                    class.margin_bp,
                ],
            )
            .map_err(|e| format!("failed to publish class {}: {e}", class.task_class))?;
        }

        tx.commit()
            .map_err(|e| format!("failed to commit price list: {e}"))?;
        Ok(())
    }

    /// The highest published version, whole.
    ///
    /// "Highest version" rather than "the one marked current": a current-flag
    /// column would be mutable state about immutable rows, which is the shape
    /// invariant 23 exists to remove.
    pub fn active_price_list(&self) -> Option<crate::pricing::PriceList> {
        let conn = self.conn();
        let (id, version, status, micros_per_credit, basis, published_at, published_by): (
            String,
            i64,
            String,
            i64,
            String,
            i64,
            String,
        ) = conn
            .query_row(
                "SELECT id, version, status, micros_per_credit, basis, published_at, published_by
                 FROM price_lists ORDER BY version DESC LIMIT 1",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .ok()?;

        let mut models = Vec::new();
        if let Ok(mut stmt) = conn.prepare(
            "SELECT provider, model_id, input_micros_per_1k, output_micros_per_1k,
                    cache_read_bp, context_window, capability_class
             FROM price_list_models WHERE price_list_id = ?1 ORDER BY provider, model_id",
        ) {
            if let Ok(rows) = stmt.query_map(params![id], |r| {
                Ok(crate::pricing::ModelPrice {
                    provider: r.get(0)?,
                    model_id: r.get(1)?,
                    input_micros_per_1k: r.get(2)?,
                    output_micros_per_1k: r.get(3)?,
                    cache_read_bp: r.get(4)?,
                    context_window: r.get(5)?,
                    capability_class: r.get(6)?,
                })
            }) {
                models.extend(rows.flatten());
            }
        }

        let mut classes = Vec::new();
        if let Ok(mut stmt) = conn.prepare(
            "SELECT task_class, quoted_credits, status, sample_count,
                    measured_cost_micros, margin_bp
             FROM price_list_task_classes WHERE price_list_id = ?1 ORDER BY task_class",
        ) {
            if let Ok(rows) = stmt.query_map(params![id], |r| {
                let status: String = r.get(2)?;
                Ok(crate::pricing::ClassPrice {
                    task_class: r.get(0)?,
                    quoted_credits: r.get(1)?,
                    // An unrecognised status reads as `Provisional`, which is
                    // the direction that cannot charge. A parse failure here
                    // must never resolve toward billing.
                    status: crate::pricing::PriceStatus::from_str(&status)
                        .unwrap_or(crate::pricing::PriceStatus::Provisional),
                    sample_count: r.get(3)?,
                    measured_cost_micros: r.get(4)?,
                    margin_bp: r.get(5)?,
                })
            }) {
                classes.extend(rows.flatten());
            }
        }

        Some(crate::pricing::PriceList {
            id,
            version,
            // Same rule as above, for the same reason.
            status: crate::pricing::PriceStatus::from_str(&status)
                .unwrap_or(crate::pricing::PriceStatus::Provisional),
            micros_per_credit,
            basis,
            published_at,
            published_by,
            models,
            classes,
        })
    }

    /// Freeze a quote for one step, at dispatch.
    ///
    /// `ON CONFLICT DO NOTHING` on `(run_id, step_id)`: a retry keeps the
    /// original price. Cortex absorbing the cost of its own second attempt is
    /// the entire content of an outcome guarantee, and re-quoting on retry
    /// would quietly bill the customer for Cortex having been wrong the first
    /// time.
    pub fn freeze_step_quote(&self, quote: &crate::pricing::StepQuote) -> Result<(), String> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO step_quotes
                (quote_id, run_id, step_id, task_class, quoted_credits,
                 price_list_id, price_list_version, billable, frozen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(run_id, step_id) DO NOTHING",
            params![
                quote.quote_id,
                quote.run_id,
                quote.step_id,
                quote.task_class,
                quote.quoted_credits,
                quote.price_list_id,
                quote.price_list_version,
                if quote.billable { 1 } else { 0 },
                quote.frozen_at,
            ],
        )
        .map_err(|e| format!("failed to freeze a step quote: {e}"))?;
        Ok(())
    }

    /// The quote frozen for a step, if one was.
    ///
    /// `None` is a real state and the honest one: a step dispatched before any
    /// price list existed has no price, and the verdict is still recorded while
    /// the ledger is left alone.
    pub fn get_step_quote(&self, run_id: &str, step_id: &str) -> Option<crate::pricing::StepQuote> {
        let conn = self.conn();
        conn.query_row(
            "SELECT quote_id, run_id, step_id, task_class, quoted_credits,
                    price_list_id, price_list_version, billable, frozen_at
             FROM step_quotes WHERE run_id = ?1 AND step_id = ?2",
            params![run_id, step_id],
            |r| {
                Ok(crate::pricing::StepQuote {
                    quote_id: r.get(0)?,
                    run_id: r.get(1)?,
                    step_id: r.get(2)?,
                    task_class: r.get(3)?,
                    quoted_credits: r.get(4)?,
                    price_list_id: r.get(5)?,
                    price_list_version: r.get(6)?,
                    billable: r.get::<_, i64>(7)? != 0,
                    frozen_at: r.get(8)?,
                })
            },
        )
        .ok()
    }

    /// The frozen checks for a step, or an empty vec if none were derived.
    ///
    /// An empty result is meaningful, not an error: `compute_verdict` maps an
    /// empty required set to `Unverified`, which is a real product state.
    pub fn load_check_specs(&self, run_id: &str, step_id: &str) -> Vec<CheckSpec> {
        let conn = self.conn();
        let raw: Option<String> = conn
            .query_row(
                "SELECT specs_json FROM verification_specs WHERE run_id = ?1 AND step_id = ?2",
                params![run_id, step_id],
                |r| r.get(0),
            )
            .ok();
        raw.and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Claim the right to verify one attempt, returning its verification id.
    ///
    /// This is the CAS the whole money path rests on. `UNIQUE(run_id, step_id,
    /// attempt)` plus `ON CONFLICT DO NOTHING` means two verifier processes
    /// racing the same delivery cannot both produce a verdict, so the ledger
    /// key derived from the verification id is minted exactly once.
    ///
    /// `None` means someone else already claimed it — the correct response is
    /// to do nothing at all, not to retry.
    pub fn claim_verification(
        &self,
        run_id: &str,
        step_id: &str,
        attempt: i64,
        tree_hash: &str,
        runner_image: &str,
    ) -> Option<String> {
        let id = Uuid::new_v4().to_string();
        let conn = self.conn();
        let inserted = conn
            .execute(
                "INSERT INTO verification_runs
                    (id, run_id, step_id, attempt, tree_hash, runner_image, verdict, started_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7)
                 ON CONFLICT(run_id, step_id, attempt) DO NOTHING",
                params![
                    id,
                    run_id,
                    step_id,
                    attempt,
                    tree_hash,
                    runner_image,
                    Utc::now().timestamp()
                ],
            )
            .unwrap_or(0);
        if inserted == 1 {
            Some(id)
        } else {
            tracing::debug!(run_id, step_id, attempt, "verification already claimed");
            None
        }
    }

    /// Record one executed check. Append-only; the runner is the only writer.
    pub fn record_check_execution(
        &self,
        verification_id: &str,
        spec: &CheckSpec,
        execution: &CheckExecution,
    ) -> Result<(), String> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO verification_checks
                (id, verification_id, spec_id, source, command, outcome,
                 exit_code, duration_ms, output_digest, output_tail, runner_image)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                Uuid::new_v4().to_string(),
                verification_id,
                execution.spec_id,
                check_source_str(spec.source),
                spec.command.join(" "),
                check_outcome_str(execution.outcome),
                execution.exit_code,
                execution.duration_ms as i64,
                execution.output_digest,
                execution.output_tail,
                execution.runner_image,
            ],
        )
        .map_err(|e| format!("failed to record check execution: {e}"))?;
        Ok(())
    }

    /// Seal a verification with its verdict. Only ever called once per id.
    pub fn finish_verification(
        &self,
        verification_id: &str,
        verdict: Verdict,
    ) -> Result<(), String> {
        let conn = self.conn();
        conn.execute(
            "UPDATE verification_runs SET verdict = ?1, finished_at = ?2 WHERE id = ?3",
            params![
                verdict_str(verdict),
                Utc::now().timestamp(),
                verification_id
            ],
        )
        .map_err(|e| format!("failed to finish verification: {e}"))?;
        Ok(())
    }

    /// Atomic sibling of [`Self::finish_verification`] that also records why
    /// the attempt ended, in the same transaction as sealing the verdict (F6
    /// of the money-review fix pass). `verification_driver.rs`'s
    /// `finish_and_bill` calls this instead of `finish_verification` followed
    /// by a separate `record_attempt_ended`, so a crash between sealing the
    /// verdict and recording why the attempt ended can no longer happen —
    /// either both land, or neither does and the caller (whose worker message
    /// or verification job is still outstanding) can be retried.
    #[allow(clippy::too_many_arguments)]
    pub fn finish_verification_and_end(
        &self,
        verification_id: &str,
        verdict: Verdict,
        run_id: &str,
        step_id: &str,
        attempt_id: &str,
        cause: cortex_core::billing_binding::AttemptEndCause,
        worker_owned_by_cortex: bool,
    ) -> Result<(), String> {
        let mut conn = self.conn();
        let tx = conn
            .transaction()
            .map_err(|e| format!("failed to begin transaction: {e}"))?;

        tx.execute(
            "UPDATE verification_runs SET verdict = ?1, finished_at = ?2 WHERE id = ?3",
            params![
                verdict_str(verdict),
                Utc::now().timestamp(),
                verification_id
            ],
        )
        .map_err(|e| format!("failed to finish verification: {e}"))?;

        let user_id = Database::run_user_id_in_tx(&tx, run_id)?;
        let now = Utc::now().timestamp_millis();
        Database::insert_attempt_ending_in_tx(
            &tx,
            attempt_id,
            &user_id,
            step_id,
            cause,
            worker_owned_by_cortex,
            now,
        )
        .map_err(|e| format!("failed to record attempt ending: {e}"))?;

        tx.commit()
            .map_err(|e| format!("failed to commit transaction: {e}"))?;
        Ok(())
    }

    /// The receipt for a step's most recent **sealed** verification attempt.
    ///
    /// The gate is recomputed from the frozen specs and the stored executions
    /// rather than read from a column. Storing a `VerdictReport` would create a
    /// second source of truth that could drift from the evidence beneath it;
    /// `compute_verdict` is pure, so deriving it costs nothing and cannot lie.
    ///
    /// `finished_at IS NOT NULL` is what makes that safe, and it is load-bearing
    /// (F18). Recomputing from the executions *recorded so far* means an
    /// unsealed verification yields a receipt whose verdict changes as checks
    /// land: `Inconclusive` with no executions the moment the run row is
    /// created, then `Failed`, then `Verified`. A caller polling for "a receipt
    /// exists" catches whichever it happens to hit, which is how the same input
    /// produced two different verdicts on consecutive runs. Reading the
    /// verdict from the column instead would not have fixed it — the row says
    /// `pending` until the same moment.
    ///
    /// A receipt is the record of a verification that finished. Until
    /// `finish_verification` seals it, there is no receipt, and this returns
    /// `None` rather than a preview of one.
    pub fn get_receipt(&self, run_id: &str, step_id: &str) -> Option<Receipt> {
        let specs = self.load_check_specs(run_id, step_id);

        // Scoped so `conn` (a std MutexGuard, non-reentrant) drops before the
        // later self.* calls below, each of which locks it again. Holding it
        // across those calls deadlocked on this thread.
        let (verification_id, attempt, tree_hash, executions, egress, attempt_id) = {
            let conn = self.conn();

            let (verification_id, attempt, tree_hash) = conn
                .query_row(
                    "SELECT id, attempt, tree_hash FROM verification_runs
                     WHERE run_id = ?1 AND step_id = ?2 AND finished_at IS NOT NULL
                     ORDER BY attempt DESC LIMIT 1",
                    params![run_id, step_id],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, i64>(1)?,
                            r.get::<_, String>(2)?,
                        ))
                    },
                )
                .ok()?;

            let mut stmt = conn
                .prepare(
                    "SELECT spec_id, exit_code, outcome, duration_ms,
                            output_digest, output_tail, runner_image
                     FROM verification_checks WHERE verification_id = ?1",
                )
                .ok()?;
            let executions: Vec<CheckExecution> = stmt
                .query_map(params![verification_id], |r| {
                    Ok(CheckExecution {
                        spec_id: r.get::<_, String>(0)?,
                        exit_code: r.get::<_, Option<i32>>(1)?,
                        outcome: check_outcome_from_str(&r.get::<_, String>(2)?),
                        duration_ms: r.get::<_, i64>(3)? as u64,
                        output_digest: r.get::<_, String>(4)?,
                        output_tail: r.get::<_, String>(5)?,
                        runner_image: r.get::<_, String>(6)?,
                    })
                })
                .ok()?
                .filter_map(|row| row.ok())
                .collect();
            drop(stmt);

            // The egress the sandbox actually ran under, read from the job rather
            // than recomputed. Ordered by lease generation because a step that was
            // re-leased ran more than once, and the last lease is the one whose
            // sandbox produced the tree this verdict is about.
            //
            // A missing row is `None`, not an empty allowlist: a step executed
            // before scoped egress existed has no record of what it reached, and
            // reporting that as "reached nothing" would be a claim we cannot make.
            let egress = conn
                .query_row(
                    "SELECT capability_grants, effective_egress, egress_mediator
                     FROM execution_jobs WHERE run_id = ?1 AND step_id = ?2
                     ORDER BY lease_gen DESC, submitted_at DESC LIMIT 1",
                    params![run_id, step_id],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, Option<String>>(1)?,
                            r.get::<_, Option<String>>(2)?,
                        ))
                    },
                )
                .ok()
                .and_then(|(grants_json, endpoints_json, mediator)| {
                    // Only a job that recorded its effective egress can produce a
                    // receipt for it. `NULL` predates the feature.
                    let endpoints: Vec<String> =
                        serde_json::from_str(endpoints_json.as_deref()?).ok()?;
                    let grants: Vec<cortex_core::execution_job::CapabilityGrant> =
                        serde_json::from_str(&grants_json).unwrap_or_default();
                    // The two grants are read out separately, from the same
                    // persisted list, because that list is where they stayed
                    // distinct. Flattening them into one set of names at any point
                    // between derivation and here would have made this
                    // unrecoverable.
                    let granted_registries = grants
                        .iter()
                        .flat_map(|grant| match grant {
                            cortex_core::execution_job::CapabilityGrant::ResolveDependencies {
                                registries,
                            } => registries.clone(),
                            cortex_core::execution_job::CapabilityGrant::ReachProvider {
                                ..
                            }
                            | cortex_core::execution_job::CapabilityGrant::ReadSecret { .. } => {
                                Vec::new()
                            }
                        })
                        .collect();
                    let granted_provider = grants.iter().find_map(|grant| match grant {
                        cortex_core::execution_job::CapabilityGrant::ReachProvider { provider } => {
                            Some(provider.clone())
                        }
                        cortex_core::execution_job::CapabilityGrant::ResolveDependencies {
                            ..
                        }
                        | cortex_core::execution_job::CapabilityGrant::ReadSecret { .. } => None,
                    });
                    Some(EgressReceipt {
                        granted_registries,
                        granted_provider,
                        endpoints,
                        mediator_image: mediator,
                    })
                });

            // The attempt id this *sealed verification* ran under -- not
            // whatever `execution_jobs` row happens to be the latest lease
            // right now, which can already belong to a later re-lease than
            // the one this verdict is about.
            //
            // `verification_runs.attempt` is the lease generation the
            // verifier claimed under (`verify_delivery` passes
            // `facts.attempt == job.lease_gen` into `claim_verification`), so
            // the `verification_jobs` row for this exact `(run_id, step_id,
            // lease_gen)` is the one that opened this verdict, and its
            // `attempt_id` is the scheduler-minted id the settler and receipts
            // key off of.
            //
            // Best effort: a step verified before `verification_jobs`
            // existed has no row, and the charge lookup below falls back to
            // the pre-attempt-billing key.
            let attempt_id: Option<String> = conn
                .query_row(
                    "SELECT attempt_id FROM verification_jobs
                     WHERE run_id = ?1 AND step_id = ?2 AND lease_gen = ?3
                     LIMIT 1",
                    params![run_id, step_id, attempt],
                    |r| r.get(0),
                )
                .ok();

            (
                verification_id,
                attempt,
                tree_hash,
                executions,
                egress,
                attempt_id,
            )
        };

        // Declared at plan time, before this step ran -- read back from the
        // frozen work contract rather than re-derived, for the same reason
        // the gate is recomputed from frozen specs: a receipt shows what was
        // decided before delivery, not a guess made after it.
        let verdict_class = self
            .read_step_work_contract(step_id, attempt)
            .ok()
            .flatten()
            .and_then(|contract| contract.verdict_class);
        // What the step was quoted at dispatch, before any verdict existed.
        let quoted_credits = self
            .get_step_quote(run_id, step_id)
            .map(|q| q.quoted_credits);
        // What the ledger actually moved -- not the quote. A quote is a plan;
        // `credit_transactions` is what happened. Task attempts are charged
        // under an attempt-derived key now (`ChargeKey::for_attempt`), not a
        // verification-derived one, so that key is tried first; the
        // verification-derived lookup remains as a fallback for rows written
        // before this change (and for the refund it could carry, which an
        // attempt charge never can).
        let charged_credits = attempt_id
            .as_deref()
            .and_then(|aid| self.ledger_net_charge_for_attempt(aid))
            .or_else(|| self.ledger_net_charge_for_verification(&verification_id));

        Some(Receipt {
            verification_id,
            run_id: run_id.to_string(),
            step_id: step_id.to_string(),
            attempt,
            tree_hash,
            gate: compute_verdict(&specs, &executions),
            executions,
            egress,
            verdict_class,
            quoted_credits,
            charged_credits,
        })
    }

    /// The verdict `finish_verification` actually sealed for the latest
    /// finished attempt of a step, read straight from the `verification_runs`
    /// row rather than recomputed.
    ///
    /// `record_check_execution` failures are only logged (see
    /// `verification_driver.rs`), so the executions recomputing a verdict
    /// would read from can be missing a row the runner believes it wrote —
    /// recomputing in that case can turn a sealed `Failed` into
    /// `Inconclusive` and let a run through that should have been blocked.
    /// The sealed column is what the runner actually decided and is not
    /// subject to that gap, so it is the one this gate trusts.
    ///
    /// `None` when there is no sealed attempt, or (defensively) when the
    /// stored string is not one `verdict_str` ever writes.
    fn get_sealed_verdict(&self, run_id: &str, step_id: &str) -> Option<Verdict> {
        let conn = self.conn();
        let raw: Option<String> = conn
            .query_row(
                "SELECT verdict FROM verification_runs
                 WHERE run_id = ?1 AND step_id = ?2 AND finished_at IS NOT NULL
                 ORDER BY attempt DESC LIMIT 1",
                params![run_id, step_id],
                |r| r.get(0),
            )
            .ok();
        raw.and_then(|s| match s.as_str() {
            "verified" => Some(Verdict::Verified),
            "failed" => Some(Verdict::Failed),
            "inconclusive" => Some(Verdict::Inconclusive),
            "unverified" => Some(Verdict::Unverified),
            _ => None,
        })
    }

    /// Whether the latest **sealed** verdict for any step of this run is
    /// `Failed`.
    ///
    /// A run whose latest sealed verdict for any step is `Failed` is still
    /// delivered — the customer paid for the calls the attempt used and gets
    /// the work either way — but `create_pr_core` (`crate::routes`) opens it
    /// as a draft PR titled with a `[failed checks]` prefix instead of a
    /// normal one, whatever the ledger says about billing (today a failed
    /// verdict is simply unbilled; that is expected to change to charging raw
    /// cost, and this gate must not depend on which of those is currently
    /// true). This walks every step the run ever had and reads each one's
    /// *latest* sealed verdict straight from the `verification_runs` row
    /// (`get_sealed_verdict`), falling back to recomputing it from the frozen
    /// specs and recorded executions (the way `get_receipt` does) only when
    /// no sealed value is stored at all — that keeps this gate from trusting
    /// a recomputation that a partially-recorded execution set could have
    /// gotten wrong. Either path orders by `attempt DESC` over sealed
    /// (`finished_at IS NOT NULL`) attempts, so a step that failed and was
    /// then retried to a `Verified` attempt reads as `Verified` here, not
    /// `Failed` — latest attempt wins.
    ///
    /// One step's latest verdict reading `Failed` marks the whole run's PR
    /// as a draft with failed checks — a run is a single deliverable, so a
    /// partial marking is not offered as a fallback.
    pub fn run_has_failed_step(&self, run_id: &str) -> bool {
        for (step_id, _status) in self.get_all_step_statuses(run_id) {
            let verdict = match self.get_sealed_verdict(run_id, &step_id) {
                Some(verdict) => verdict,
                None => {
                    let Some(receipt) = self.get_receipt(run_id, &step_id) else {
                        continue;
                    };
                    receipt.gate.verdict
                }
            };
            if verdict == Verdict::Failed {
                return true;
            }
        }
        false
    }

    /// The check spec ids that failed on the latest sealed attempt of every
    /// step whose sealed verdict is `Failed`, for the "failed checks" line on
    /// a draft PR's body. Best-effort: a step counted `Failed` by
    /// `run_has_failed_step`'s recompute fallback but with no receipt
    /// available here (which should not happen in practice — the fallback
    /// itself comes from `get_receipt`) simply contributes no names rather
    /// than erroring.
    pub fn run_failed_check_names(&self, run_id: &str) -> Vec<String> {
        let mut names = Vec::new();
        for (step_id, _status) in self.get_all_step_statuses(run_id) {
            let Some(receipt) = self.get_receipt(run_id, &step_id) else {
                continue;
            };
            if receipt.gate.verdict != Verdict::Failed {
                continue;
            }
            for execution in &receipt.executions {
                if !matches!(execution.outcome, CheckOutcome::Passed) {
                    names.push(execution.spec_id.clone());
                }
            }
        }
        names
    }

    /// Whether any step of this run has frozen check specs and verification
    /// actually in flight for it — i.e. the step could still land on
    /// `Failed`.
    ///
    /// Specs are frozen at dispatch, before the first attempt is claimed
    /// (`load_check_specs` is non-empty exactly when a verification was
    /// requested for the step). But specs surviving on a step is not the same
    /// as that step's verification being unresolved: a heal
    /// (`try_heal`) can flip the original step to `recovered` and spin up a
    /// new retry step id for the same work, or a step can be `cancelled`
    /// after dispatch — in both cases the *original* step keeps its frozen
    /// specs forever, never gets a receipt, and would otherwise block the
    /// run's PR indefinitely even though the retry (or nothing) is what
    /// actually determines the outcome now. So this only counts a step as
    /// pending when its own status says verification is currently running
    /// for it (`delivered`, about to be handed to the verifier, or
    /// `verifying`, already with it) — any other status, sealed or not, is
    /// not "in flight" and must not block delivery.
    pub fn run_has_pending_verification(&self, run_id: &str) -> bool {
        for (step_id, status) in self.get_all_step_statuses(run_id) {
            if !matches!(status.as_str(), "delivered" | "verifying") {
                continue;
            }
            if self.load_check_specs(run_id, &step_id).is_empty() {
                continue;
            }
            if self.get_receipt(run_id, &step_id).is_none() {
                return true;
            }
        }
        false
    }

    /// Returns `Result` rather than panicking: these run in request paths, and
    /// `.expect()` on a database error took the handler down with it.
    pub fn reset_subscription_credits(
        &self,
        clerk_user_id: &str,
        total: i64,
    ) -> Result<(), String> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO credit_balances (clerk_user_id, subscription_remaining, subscription_total, pack_remaining, last_reset_at)
             VALUES (?1, ?2, ?2, 0, datetime('now'))
             ON CONFLICT(clerk_user_id) DO UPDATE SET
                subscription_remaining = ?2, subscription_total = ?2, last_reset_at = datetime('now')",
            params![clerk_user_id, total],
        )
        .map_err(|e| format!("failed to reset subscription credits: {e}"))?;
        Ok(())
    }

    pub fn init_credit_balance(
        &self,
        clerk_user_id: &str,
        subscription_total: i64,
    ) -> Result<(), String> {
        let conn = self.conn();
        conn.execute(
            "INSERT OR IGNORE INTO credit_balances (clerk_user_id, subscription_remaining, subscription_total, pack_remaining)
             VALUES (?1, ?2, ?2, 0)",
            params![clerk_user_id, subscription_total],
        )
        .map_err(|e| format!("failed to init credit balance: {e}"))?;
        Ok(())
    }

    /// Non-idempotent pack-credit top-up used by tests to fund a balance.
    /// **Never for product billing** — it writes no ledger row and can be
    /// called twice for the same money. Product code that grants purchased
    /// credits must use [`Self::grant_topup_credits`], which is exactly-once
    /// and durable.
    ///
    /// On a missing balance row this creates one with `subscription_remaining
    /// = 0` / `subscription_total = 0` — never inventing a free monthly
    /// allotment for a user who only bought a pack.
    pub fn add_pack_credits(&self, clerk_user_id: &str, amount: i64) -> Result<(), String> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO credit_balances (clerk_user_id, subscription_remaining, subscription_total, pack_remaining)
             VALUES (?1, 0, 0, ?2)
             ON CONFLICT(clerk_user_id) DO UPDATE SET pack_remaining = pack_remaining + ?2",
            params![clerk_user_id, amount],
        )
        .map_err(|e| format!("failed to add pack credits: {e}"))?;
        Ok(())
    }

    /// Grant purchased pack credits for one completed Stripe Checkout
    /// Session, exactly once.
    ///
    /// **Idempotent on `checkout_session_id`**, not on the Stripe event id:
    /// Stripe may replay `checkout.session.completed` (same event id, at-least-
    /// once delivery) and, for delayed payment methods, follow it with a
    /// separate `checkout.session.async_payment_succeeded` for the *same*
    /// session — a different event id describing the same money. Keying on
    /// the session id under `topup:{checkout_session_id}` and relying on
    /// `credit_transactions.idempotency_key`'s UNIQUE constraint (the same
    /// exactly-once mechanism `deduct_credits`/`charge_settled_cost` use)
    /// makes every one of those deliveries after the first a no-op.
    ///
    /// One transaction: increments `pack_remaining` (creating the balance row
    /// with `subscription_remaining = 0` / `subscription_total = 0` if it
    /// doesn't exist yet — a top-up never invents subscription credits) and
    /// inserts one positive `'pack'`-typed `credit_transactions` row with
    /// reason `'purchase'`.
    ///
    /// Returns `Ok(true)` when this call granted the credits, `Ok(false)`
    /// when the session had already been granted (replay).
    pub fn grant_topup_credits(
        &self,
        clerk_user_id: &str,
        checkout_session_id: &str,
        credits: i64,
        description: &str,
    ) -> Result<bool, String> {
        if credits <= 0 {
            return Err("topup credits must be positive".into());
        }
        if checkout_session_id.trim().is_empty() {
            return Err("checkout session id is required for a topup grant".into());
        }

        let idempotency_key = format!("topup:{checkout_session_id}");
        let conn = self.conn();

        conn.execute("BEGIN IMMEDIATE", [])
            .map_err(|e| format!("failed to begin transaction: {e}"))?;

        // Replay check inside the transaction, so it can't race a concurrent
        // grant for the same session (e.g. `completed` and
        // `async_payment_succeeded` arriving back to back).
        let already: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM credit_transactions WHERE idempotency_key = ?1",
                params![idempotency_key],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if already > 0 {
            conn.execute("ROLLBACK", []).ok();
            tracing::debug!(
                user_id = clerk_user_id,
                checkout_session_id,
                "credit topup replayed; no additional credits granted"
            );
            return Ok(false);
        }

        if let Err(e) = conn.execute(
            "INSERT INTO credit_balances (clerk_user_id, subscription_remaining, subscription_total, pack_remaining)
             VALUES (?1, 0, 0, ?2)
             ON CONFLICT(clerk_user_id) DO UPDATE SET pack_remaining = pack_remaining + ?2",
            params![clerk_user_id, credits],
        ) {
            conn.execute("ROLLBACK", []).ok();
            return Err(format!("failed to credit pack balance: {e}"));
        }

        let tx_id = Uuid::new_v4().to_string();
        if let Err(e) = conn.execute(
            "INSERT INTO credit_transactions
                (id, clerk_user_id, amount, balance_type, reason, description,
                 idempotency_key, cost_micro_usd)
             VALUES (?1, ?2, ?3, 'pack', 'purchase', ?4, ?5, NULL)",
            params![tx_id, clerk_user_id, credits, description, idempotency_key],
        ) {
            conn.execute("ROLLBACK", []).ok();
            return Err(format!("failed to record topup transaction: {e}"));
        }

        if let Err(e) = conn.execute("COMMIT", []) {
            conn.execute("ROLLBACK", []).ok();
            return Err(format!("failed to commit transaction: {e}"));
        }

        Ok(true)
    }

    pub fn record_billing_event(
        &self,
        clerk_user_id: &str,
        stripe_event_id: &str,
        amount_cents: i64,
        description: &str,
        status: &str,
    ) -> bool {
        let conn = self.conn();
        let id = Uuid::new_v4().to_string();
        let rows = conn.execute(
            "INSERT OR IGNORE INTO billing_history (id, clerk_user_id, stripe_event_id, amount_cents, description, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, clerk_user_id, stripe_event_id, amount_cents, description, status],
        ).unwrap_or(0);
        rows > 0
    }

    /// Purchase/billing ledger rows for a user, newest first.
    /// `offset` skips that many rows; `limit` caps the page size.
    pub fn get_billing_history(
        &self,
        clerk_user_id: &str,
        limit: i64,
        offset: i64,
    ) -> Vec<BillingHistoryRecord> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, amount_cents, description, status, created_at
             FROM billing_history WHERE clerk_user_id = ?1
             ORDER BY created_at DESC LIMIT ?2 OFFSET ?3",
            )
            .unwrap();

        stmt.query_map(params![clerk_user_id, limit, offset], |row| {
            Ok(BillingHistoryRecord {
                id: row.get(0)?,
                amount_cents: row.get(1)?,
                description: row.get(2)?,
                status: row.get(3)?,
                created_at: row.get(4)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::test_db;
    use super::*;
    use cortex_core::billing_binding::{ChargeKey, RefundKey};

    /// A user with a monthly allotment and no purchased pack.
    fn subscriber(db: &Database, subscription: i64) -> &'static str {
        db.init_credit_balance("user-1", subscription)
            .expect("balance row");
        "user-1"
    }

    #[test]
    fn a_deduction_never_invents_a_balance() {
        // "UPDATE only -- a deduction must never create an account." An
        // unmetered user is not a user with zero credits; charging one would
        // mint an account nobody bought and start a relationship on a row we
        // made up.
        let db = test_db();
        let err = db
            .deduct_credits("nobody", 10, "work", &ChargeKey::for_verification("v-1"))
            .expect_err("an unmetered user cannot be charged");
        assert!(err.contains("unmetered"), "{err}");
        assert_eq!(db.credit_ledger_totals("nobody"), (0, 0));
    }

    #[test]
    fn the_expiring_bucket_is_spent_before_the_permanent_one() {
        // The monthly allotment expires and purchased packs do not, so drawing
        // from packs first would silently destroy credits the customer paid
        // cash for while the free ones lapsed unused.
        let db = test_db();
        db.init_credit_balance("user-1", 100).expect("balance");
        db.add_pack_credits("user-1", 50).expect("pack");

        let after = db
            .deduct_credits("user-1", 30, "work", &ChargeKey::for_verification("v-1"))
            .expect("charge");
        assert_eq!(after.subscription_remaining, 70);
        assert_eq!(
            after.pack_remaining, 50,
            "packs are untouched while the allotment holds"
        );
    }

    #[test]
    fn a_charge_larger_than_the_allotment_spills_into_the_pack() {
        let db = test_db();
        db.init_credit_balance("user-1", 100).expect("balance");
        db.add_pack_credits("user-1", 50).expect("pack");

        let after = db
            .deduct_credits("user-1", 120, "work", &ChargeKey::for_verification("v-1"))
            .expect("charge");
        assert_eq!(after.subscription_remaining, 0);
        assert_eq!(
            after.pack_remaining, 30,
            "the overflow came out of the pack"
        );
        assert_eq!(db.credit_ledger_totals("user-1"), (-100, -20));
    }

    #[test]
    fn a_refund_mirrors_the_split_rather_than_the_amount() {
        // The argument in `refund_credits`' own doc comment, which had no test.
        // Only the spend rows know how a charge fell across the two buckets.
        // Refunding the total to one bucket would move 20 credits from a
        // permanent pack into an allotment that expires at the end of the
        // month -- the customer is made whole this week and short next.
        let db = test_db();
        db.init_credit_balance("user-1", 100).expect("balance");
        db.add_pack_credits("user-1", 50).expect("pack");

        let charge = ChargeKey::for_verification("v-1");
        db.deduct_credits("user-1", 120, "work", &charge)
            .expect("charge");

        let refunded = db
            .refund_credits(
                "user-1",
                &charge,
                &RefundKey::for_verification("v-1"),
                "failed",
            )
            .expect("refund");
        assert_eq!(refunded.subscription_remaining, 100);
        assert_eq!(refunded.pack_remaining, 50);
        assert_eq!(
            db.credit_ledger_totals("user-1"),
            (0, 0),
            "each bucket's append-only log nets to zero on its own"
        );
    }

    #[test]
    fn settled_charges_accumulate_carry_and_deduct_exactly_when_it_crosses_a_credit() {
        // Exact pass-through billing: a reply that costs far less than one
        // credit deducts nothing and leaves the whole cost sitting in carry.
        // Repeating it keeps accumulating carry until it crosses one whole
        // credit (100_000 micro-USD here), at which point exactly one credit
        // is deducted and the remainder keeps carrying forward.
        let db = test_db();
        let user = subscriber(&db, 100);
        let mpc: i64 = 100_000;

        let first = db
            .charge_settled_cost(
                user,
                12_345,
                mpc,
                "Cortex-paid chat reply",
                &ChargeKey::for_chat_reply("reply-1"),
            )
            .expect("first charge");
        assert_eq!(first.credits_charged, 0);
        assert_eq!(first.new_carry_micro_usd, 12_345);
        assert_eq!(db.get_credit_balance(user).subscription_remaining, 100);
        assert_eq!(db.get_credit_carry_micro_usd(user), 12_345);

        let second = db
            .charge_settled_cost(
                user,
                12_345,
                mpc,
                "Cortex-paid chat reply",
                &ChargeKey::for_chat_reply("reply-2"),
            )
            .expect("second charge");
        assert_eq!(second.credits_charged, 0);
        assert_eq!(second.new_carry_micro_usd, 24_690);
        assert_eq!(db.get_credit_carry_micro_usd(user), 24_690);

        // Keep charging the same amount until the accumulated carry crosses
        // one whole credit (100_000 micro-USD): 24_690 + 12_345*7 = 111_105,
        // which owes exactly 1 credit and carries the 11_105 remainder.
        let mut reply_n = 3;
        loop {
            let charge = db
                .charge_settled_cost(
                    user,
                    12_345,
                    mpc,
                    "Cortex-paid chat reply",
                    &ChargeKey::for_chat_reply(&format!("reply-{reply_n}")),
                )
                .expect("repeated charge");
            if charge.credits_charged > 0 {
                assert_eq!(
                    charge.credits_charged, 1,
                    "one carry-crossing is one credit"
                );
                assert_eq!(charge.new_carry_micro_usd, 11_105);
                break;
            }
            reply_n += 1;
            assert!(reply_n < 20, "carry should have crossed a credit by now");
        }
        assert_eq!(db.get_credit_balance(user).subscription_remaining, 99);
        assert_eq!(db.get_credit_carry_micro_usd(user), 11_105);
    }

    #[test]
    fn a_settled_charge_replayed_by_key_charges_once() {
        // The same reply id must never be charged twice: a retry, a page
        // refresh mid-stream, or a crashed-then-resumed request must all
        // collapse onto the single row the first attempt wrote.
        let db = test_db();
        let user = subscriber(&db, 100);
        let key = ChargeKey::for_chat_reply("reply-1");

        let first = db
            .charge_settled_cost(user, 60, 100_000, "Cortex-paid chat reply", &key)
            .expect("first charge");
        assert_eq!(first.credits_charged, 0);
        assert_eq!(first.new_carry_micro_usd, 60);

        let replay = db
            .charge_settled_cost(user, 60, 100_000, "Cortex-paid chat reply", &key)
            .expect("replay is a no-op, not an error");
        assert_eq!(replay.credits_charged, 0, "a replay charges nothing more");
        assert_eq!(
            replay.new_carry_micro_usd, 60,
            "a replay must not advance the carry a second time"
        );
        assert_eq!(db.get_credit_balance(user).subscription_remaining, 100);
        assert_eq!(db.get_credit_carry_micro_usd(user), 60);

        let row_count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM credit_transactions WHERE idempotency_key = ?1",
                params![key.as_str()],
                |r| r.get(0),
            )
            .expect("count query");
        assert_eq!(row_count, 1, "a replay must not write a second row");
    }

    #[test]
    fn a_charge_spanning_both_buckets_writes_one_row_per_bucket_not_mixed() {
        // Every other writer (`deduct_credits`, `refund_credits`) writes one
        // row per bucket under `<key>`/`<key>:pack`, and `credit_ledger_totals`
        // sums per bucket -- the documented derivation the balance cache must
        // agree with. A single `'mixed'`-typed row is invisible to that sum,
        // so reconciliation would silently disagree with the cache the moment
        // a chat reply's exact cost happened to straddle both buckets.
        let db = test_db();
        let user = subscriber(&db, 1);
        db.add_pack_credits(user, 5).expect("pack");
        db.conn()
            .execute(
                "UPDATE credit_balances SET carry_micro_usd = 90000 WHERE clerk_user_id = ?1",
                params![user],
            )
            .expect("seed carry");

        let key = ChargeKey::for_chat_reply("reply-mixed");
        // total = carry 90_000 + cost 150_000 = 240_000; 240_000 / 100_000 =
        // 2 credits owed, 40_000 carried forward. 1 comes from the lone
        // subscription credit, the other spills into the pack.
        let settled = db
            .charge_settled_cost(user, 150_000, 100_000, "Cortex-paid chat reply", &key)
            .expect("settle");
        assert_eq!(settled.credits_charged, 2);
        assert_eq!(settled.new_carry_micro_usd, 40_000);

        let balance = db.get_credit_balance(user);
        assert_eq!(
            balance.subscription_remaining, 0,
            "the one subscription credit is spent first"
        );
        assert_eq!(
            balance.pack_remaining, 4,
            "the other owed credit spills into the pack"
        );
        assert_eq!(db.get_credit_carry_micro_usd(user), 40_000);
        assert_eq!(
            db.credit_ledger_totals(user),
            (-1, -1),
            "one credit came from each bucket, not one 'mixed' row invisible to this sum"
        );

        let pack_key = format!("{}:pack", key.as_str());
        let pack_rows: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM credit_transactions WHERE idempotency_key = ?1",
                params![pack_key],
                |r| r.get(0),
            )
            .expect("count query");
        assert_eq!(pack_rows, 1, "exactly one pack row for this charge");

        let (bare_type, bare_cost): (String, Option<i64>) = db
            .conn()
            .query_row(
                "SELECT balance_type, cost_micro_usd FROM credit_transactions
                 WHERE idempotency_key = ?1",
                params![key.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("bare row");
        assert_eq!(
            bare_type, "subscription",
            "the replay-checked key is never 'mixed'"
        );
        assert_eq!(
            bare_cost,
            Some(150_000),
            "the bare row carries the full nominal cost"
        );

        // Replaying the same key charges nothing and inserts no further rows.
        let replay = db
            .charge_settled_cost(user, 150_000, 100_000, "Cortex-paid chat reply", &key)
            .expect("replay is a no-op, not an error");
        assert_eq!(replay.credits_charged, 0);
        assert_eq!(replay.new_carry_micro_usd, 40_000);
        assert_eq!(
            db.credit_ledger_totals(user),
            (-1, -1),
            "a replay must not add more rows"
        );

        let total_rows: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM credit_transactions
                 WHERE idempotency_key = ?1 OR idempotency_key = ?2",
                params![key.as_str(), pack_key],
                |r| r.get(0),
            )
            .expect("count query");
        assert_eq!(total_rows, 2, "replay must not insert additional rows");
    }

    #[test]
    fn a_charge_that_cannot_be_afforded_moves_nothing() {
        // Not a partial charge, and not a negative balance. The append-only log
        // must show that nothing happened, because a half-charged customer with
        // no delivered work is the worst of both.
        let db = test_db();
        let user = subscriber(&db, 10);

        let err = db
            .deduct_credits(user, 50, "work", &ChargeKey::for_verification("v-1"))
            .expect_err("insufficient");
        assert!(err.contains("insufficient"), "{err}");
        assert_eq!(db.get_credit_balance(user).subscription_remaining, 10);
        assert_eq!(db.credit_ledger_totals(user), (0, 0));
    }

    #[test]
    fn a_zero_charge_is_free_rather_than_an_error() {
        // A step that cost nothing still resolves. Erroring would turn a
        // successful free task into a failed one.
        let db = test_db();
        let user = subscriber(&db, 10);

        let after = db
            .deduct_credits(user, 0, "free work", &ChargeKey::for_verification("v-1"))
            .expect("zero is not an error");
        assert_eq!(after.subscription_remaining, 10);
        assert_eq!(db.credit_ledger_totals(user), (0, 0), "no row for no money");
    }

    #[test]
    fn a_negative_charge_is_refused_rather_than_treated_as_a_credit() {
        // A deduction is the only thing this function does. A negative amount
        // reaching it is a caller bug, and quietly adding credits would be the
        // most expensive possible interpretation of one.
        let db = test_db();
        let user = subscriber(&db, 10);

        assert!(db
            .deduct_credits(user, -50, "work", &ChargeKey::for_verification("v-1"))
            .is_err());
        assert_eq!(db.credit_ledger_totals(user), (0, 0));
    }

    #[test]
    fn a_replayed_charge_takes_the_money_once() {
        // The worker retried after the process died between charging and
        // recording. The second call must report the same balance without
        // taking anything.
        let db = test_db();
        let user = subscriber(&db, 100);
        let charge = ChargeKey::for_verification("v-1");

        let first = db
            .deduct_credits(user, 30, "work", &charge)
            .expect("charge");
        let replay = db
            .deduct_credits(user, 30, "work", &charge)
            .expect("replay");
        assert_eq!(first.subscription_remaining, 70);
        assert_eq!(replay.subscription_remaining, 70);
        assert_eq!(db.credit_ledger_totals(user), (-30, 0));
    }

    #[test]
    fn the_ledger_answers_whether_a_key_was_charged() {
        // `BillingState` is derived from the money rather than from a status
        // column, so this is the question that keeps the two from drifting.
        let db = test_db();
        let user = subscriber(&db, 100);
        let charge = ChargeKey::for_verification("v-1");

        assert!(!db.ledger_has_key(charge.as_str()));
        db.deduct_credits(user, 30, "work", &charge)
            .expect("charge");
        assert!(db.ledger_has_key(charge.as_str()));
        assert!(!db.ledger_has_key("v-2"), "an unrelated key is not charged");
    }

    #[test]
    fn a_key_charged_only_against_the_pack_is_still_found() {
        // The key is suffixed per bucket, so a lookup that only checked
        // `:subscription` would miss a charge that fell entirely on a pack and
        // report an unbilled step that was in fact billed.
        let db = test_db();
        db.init_credit_balance("user-1", 0).expect("balance");
        db.add_pack_credits("user-1", 50).expect("pack");

        let charge = ChargeKey::for_verification("v-1");
        db.deduct_credits("user-1", 20, "work", &charge)
            .expect("charge");
        assert_eq!(db.credit_ledger_totals("user-1"), (0, -20));
        assert!(db.ledger_has_key(charge.as_str()));
    }

    #[test]
    fn an_empty_idempotency_key_is_refused() {
        // Without a key there is no replay protection, so a retry would charge
        // twice. Refusing is the only safe reading.
        let db = test_db();
        let user = subscriber(&db, 100);

        assert!(db
            .deduct_credits(user, 10, "work", &ChargeKey::per_unit("   "))
            .is_err());
        assert_eq!(db.credit_ledger_totals(user), (0, 0));
    }

    #[test]
    fn deduct_credits_up_to_replay_of_the_same_key_charges_nothing_twice() {
        // Same idempotency-key protection as `deduct_credits`: a retried
        // final charge must not double-spend.
        let db = test_db();
        let user = subscriber(&db, 100);
        let key = ChargeKey::for_chat_reply("reply-1");

        let first = db
            .deduct_credits_up_to(user, 30, "work", &key)
            .expect("first charge");
        assert_eq!(first, 30);

        let second = db
            .deduct_credits_up_to(user, 30, "work", &key)
            .expect("replayed charge");
        assert_eq!(second, 0, "a replayed key must charge nothing further");

        let balance = db.get_credit_balance_row(user).expect("balance row");
        assert_eq!(
            balance.subscription_remaining, 70,
            "the balance must reflect only the first charge"
        );
    }

    #[test]
    fn deduct_credits_up_to_draws_the_subscription_bucket_before_the_pack() {
        let db = test_db();
        let user = subscriber(&db, 20);
        db.add_pack_credits(user, 50).expect("pack");

        let charged = db
            .deduct_credits_up_to(user, 40, "work", &ChargeKey::for_chat_reply("reply-2"))
            .expect("charge");
        assert_eq!(charged, 40, "enough credit exists across both buckets");

        let balance = db.get_credit_balance_row(user).expect("balance row");
        assert_eq!(
            balance.subscription_remaining, 0,
            "the expiring bucket is drained first"
        );
        assert_eq!(
            balance.pack_remaining, 30,
            "only the remainder after the subscription spills into the pack"
        );
    }

    #[test]
    fn deduct_credits_up_to_clamps_to_the_total_balance_instead_of_failing() {
        let db = test_db();
        let user = subscriber(&db, 10);
        db.add_pack_credits(user, 5).expect("pack");

        let charged = db
            .deduct_credits_up_to(user, 1_000, "work", &ChargeKey::for_chat_reply("reply-3"))
            .expect("charge");
        assert_eq!(
            charged, 15,
            "the charge is clamped to whatever was actually available"
        );

        let balance = db.get_credit_balance_row(user).expect("balance row");
        assert_eq!(balance.subscription_remaining, 0);
        assert_eq!(balance.pack_remaining, 0);
    }

    // --- run_has_failed_step / run_has_pending_verification ---

    fn verdict_spec(id: &str) -> CheckSpec {
        CheckSpec {
            id: id.to_string(),
            source: CheckSource::Contract,
            command: vec!["cargo".into(), "test".into()],
            timeout_secs: 60,
            required: true,
        }
    }

    fn verdict_execution(spec_id: &str, outcome: CheckOutcome) -> CheckExecution {
        CheckExecution {
            spec_id: spec_id.to_string(),
            exit_code: if matches!(outcome, CheckOutcome::Passed) {
                Some(0)
            } else {
                Some(1)
            },
            outcome,
            duration_ms: 1200,
            output_digest: "sha256:deadbeef".to_string(),
            output_tail: "ok".to_string(),
            runner_image: "cortex/runner@sha256:abc".to_string(),
        }
    }

    /// A run row and one step under it, the minimum `run_has_failed_step` /
    /// `run_has_pending_verification` need to have anything to walk --
    /// `get_all_step_statuses` reads from `steps`, not from the verification
    /// tables.
    fn insert_run_and_step(db: &Database, run_id: &str, step_id: &str) {
        let conn = db.conn();
        conn.execute(
            "INSERT INTO runs (id, user_id, goal, created_at, updated_at)
             VALUES (?1, 'user-1', 'g', 0, 0)",
            params![run_id],
        )
        .expect("insert run");
        conn.execute(
            "INSERT INTO steps (id, run_id, kind, tier, risk, objective, created_at, updated_at)
             VALUES (?1, ?2, 'execute', 'standard', 'low', 'o', 0, 0)",
            params![step_id, run_id],
        )
        .expect("insert step");
    }

    /// Sets a step's status directly, bypassing the lifecycle CAS in
    /// `transition_verification` — tests use this to plant the step in
    /// whatever state a scenario needs without wiring up a whole heal or
    /// cancellation path.
    fn set_step_status(db: &Database, step_id: &str, status: &str) {
        db.conn()
            .execute(
                "UPDATE steps SET status = ?1 WHERE id = ?2",
                params![status, step_id],
            )
            .expect("set step status");
    }

    /// Seals `attempt` for the given step with the given verdict, driving the
    /// real claim/record/finish path, and returns the verification id.
    fn seal_verification_attempt(
        db: &Database,
        run_id: &str,
        step_id: &str,
        attempt: i64,
        verdict: Verdict,
    ) -> String {
        let specs = vec![verdict_spec("check-1")];
        db.save_check_specs(run_id, step_id, &specs)
            .expect("freeze specs");
        let vid = db
            .claim_verification(run_id, step_id, attempt, "tree-hash", "img@sha256:1")
            .expect("claim verification");
        let outcome = if matches!(verdict, Verdict::Failed) {
            CheckOutcome::Failed
        } else {
            CheckOutcome::Passed
        };
        db.record_check_execution(&vid, &specs[0], &verdict_execution("check-1", outcome))
            .expect("record execution");
        db.finish_verification(&vid, verdict).expect("seal");
        vid
    }

    fn seal_verification(db: &Database, run_id: &str, step_id: &str, verdict: Verdict) -> String {
        seal_verification_attempt(db, run_id, step_id, 1, verdict)
    }

    #[test]
    fn a_failed_step_marks_the_run_for_a_draft_pr() {
        // No billing state is seeded here: a `Failed` verdict is unbilled in
        // production, and the gate must trip on the verdict alone.
        let db = test_db();
        insert_run_and_step(&db, "run-failed", "step-failed");
        seal_verification(&db, "run-failed", "step-failed", Verdict::Failed);

        assert!(db.run_has_failed_step("run-failed"));
    }

    #[test]
    fn a_verified_step_does_not_mark_the_run_for_a_draft_pr() {
        let db = test_db();
        insert_run_and_step(&db, "run-verified", "step-verified");
        seal_verification(&db, "run-verified", "step-verified", Verdict::Verified);

        assert!(!db.run_has_failed_step("run-verified"));
    }

    #[test]
    fn a_step_failed_then_retried_to_verified_does_not_mark_the_run_for_a_draft_pr() {
        // Latest attempt wins: `get_receipt` orders by attempt DESC over
        // sealed attempts, so a retried step reads as its newest verdict.
        let db = test_db();
        insert_run_and_step(&db, "run-retried", "step-retried");
        seal_verification_attempt(&db, "run-retried", "step-retried", 1, Verdict::Failed);
        seal_verification_attempt(&db, "run-retried", "step-retried", 2, Verdict::Verified);

        assert!(!db.run_has_failed_step("run-retried"));
    }

    #[test]
    fn a_step_with_frozen_specs_and_no_sealed_verdict_is_pending() {
        let db = test_db();
        insert_run_and_step(&db, "run-pending", "step-pending");
        set_step_status(&db, "step-pending", "verifying");
        db.save_check_specs("run-pending", "step-pending", &[verdict_spec("check-1")])
            .expect("freeze specs");
        db.claim_verification(
            "run-pending",
            "step-pending",
            1,
            "tree-hash",
            "img@sha256:1",
        )
        .expect("claim verification");

        assert!(db.run_has_pending_verification("run-pending"));
        assert!(
            !db.run_has_failed_step("run-pending"),
            "an unsealed attempt must not read as a failed verdict"
        );
    }

    /// A heal (`try_heal`) flips the original failed step to `recovered` and
    /// mints a new retry step id for the same work. The original keeps its
    /// frozen specs forever with no receipt, but its verification is not "in
    /// flight" any more — the retry's is. This is the F1 regression: before
    /// the fix, the recovered original blocked the run's PR forever even
    /// though the retry sealed `Verified` and was charged.
    #[test]
    fn a_recovered_step_does_not_block_the_run_once_its_retry_is_verified() {
        let db = test_db();
        insert_run_and_step(&db, "run-healed", "step-original");
        db.save_check_specs("run-healed", "step-original", &[verdict_spec("check-1")])
            .expect("freeze specs on the original, as scheduler.rs does at dispatch");
        set_step_status(&db, "step-original", "verifying");

        // The worker reports StepFailed; try_heal marks the original
        // recovered without ever sealing a verdict for it.
        set_step_status(&db, "step-original", "recovered");

        // The heal chain's new retry step, sealed Verified.
        let conn = db.conn();
        conn.execute(
            "INSERT INTO steps (id, run_id, kind, tier, risk, objective, created_at, updated_at)
             VALUES ('step-retry', 'run-healed', 'execute', 'standard', 'low', 'o', 0, 0)",
            [],
        )
        .expect("insert retry step");
        drop(conn);
        seal_verification(&db, "run-healed", "step-retry", Verdict::Verified);

        assert!(
            !db.run_has_pending_verification("run-healed"),
            "the recovered original must not read as still verifying"
        );
        assert!(
            !db.run_has_failed_step("run-healed"),
            "neither the recovered original (never sealed) nor the verified retry blocks the PR"
        );
    }

    /// A step cancelled after dispatch keeps its frozen specs too, and must
    /// not block delivery either.
    #[test]
    fn a_cancelled_step_with_frozen_specs_does_not_block_the_run() {
        let db = test_db();
        insert_run_and_step(&db, "run-cancelled", "step-cancelled");
        db.save_check_specs(
            "run-cancelled",
            "step-cancelled",
            &[verdict_spec("check-1")],
        )
        .expect("freeze specs");
        set_step_status(&db, "step-cancelled", "verifying");
        set_step_status(&db, "step-cancelled", "cancelled");

        assert!(!db.run_has_pending_verification("run-cancelled"));
        assert!(!db.run_has_failed_step("run-cancelled"));
    }

    #[test]
    fn a_step_with_no_frozen_specs_is_not_pending() {
        // A step with no verification requested at all (e.g. a read-only
        // step) must not be mistaken for one still being verified. Status is
        // set to `verifying` so this actually exercises the "no frozen
        // specs" branch rather than being short-circuited by the
        // `delivered`/`verifying` status filter first.
        let db = test_db();
        insert_run_and_step(&db, "run-no-checks", "step-no-checks");
        set_step_status(&db, "step-no-checks", "verifying");

        assert!(!db.run_has_pending_verification("run-no-checks"));
    }

    #[test]
    fn a_sealed_step_is_not_pending() {
        let db = test_db();
        insert_run_and_step(&db, "run-sealed", "step-sealed");
        set_step_status(&db, "step-sealed", "verifying");
        seal_verification(&db, "run-sealed", "step-sealed", Verdict::Verified);

        assert!(!db.run_has_pending_verification("run-sealed"));
    }

    // --- settle_pending_attempts / attempt_settled_cost_micro_usd ---
    //
    // These settle real `provider_request_reservations` rows through the same
    // two calls a gateway `forward()` makes (`reserve_provider_request` then
    // `settle_provider_request`/`mark_provider_request_unresolved`), not a
    // hand-rolled substitute, so they exercise the actual state machine the
    // production end paths (a sealed verdict, `cancel_run`,
    // `expire_stale_leases`) all read from.

    use crate::db::SpendAuthorization;
    use crate::provider_gateway::GatewayCapability;
    use cortex_core::billing_binding::AttemptEndCause;

    const ATTEMPT_NOW: i64 = 1_800_000_000_000;

    /// Drives the real two-step production path instead of the deleted
    /// direct-dispatch `settle_ended_attempt`: write the durable
    /// `attempt_endings` record (exactly what `cancel_run` /
    /// `expire_stale_leases` / a sealed verdict do), then run the settler
    /// that turns unsettled records into ledger effects. `step_id` has no FK
    /// constraint on `attempt_endings`, so any distinguishing string works.
    fn end_and_settle_attempt(
        db: &Database,
        user_id: &str,
        attempt_id: &str,
        step_id: &str,
        cause: AttemptEndCause,
    ) {
        {
            let mut conn = db.conn();
            let tx = conn.transaction().expect("begin");
            Database::insert_attempt_ending_in_tx(
                &tx,
                attempt_id,
                user_id,
                step_id,
                cause,
                false,
                ATTEMPT_NOW,
            )
            .expect("insert ending");
            tx.commit().expect("commit");
        }
        db.settle_pending_attempts().expect("settle");
    }

    /// A funded spend authorization scoped to one attempt, ready for
    /// `reserve_provider_request`.
    fn attempt_capability(db: &Database, attempt_id: &str) -> GatewayCapability {
        let price_list_id = db.active_price_list().unwrap().id;
        db.set_supplier_capacity("claude", 10_000_000, ATTEMPT_NOW)
            .unwrap();
        let authorization = SpendAuthorization {
            id: format!("auth-{attempt_id}"),
            user_id: "user-1".into(),
            run_id: "run-1".into(),
            attempt_id: attempt_id.into(),
            provider: "claude".into(),
            model: "claude-sonnet-5".into(),
            price_list_id,
            max_micro_usd: 10_000_000,
            expires_at_ms: ATTEMPT_NOW + 60_000,
        };
        db.create_spend_authorization(&authorization, ATTEMPT_NOW)
            .unwrap();
        GatewayCapability::new(
            authorization.id,
            authorization.user_id,
            authorization.run_id,
            authorization.attempt_id,
            authorization.provider,
            authorization.model,
            authorization.expires_at_ms,
        )
    }

    /// Reserve and settle one provider call for `claims.attempt_id`, exactly
    /// as a real gateway `forward()` would once the supplier confirms a cost.
    fn settle_call(db: &Database, claims: &GatewayCapability, request_key: &str, observed: i64) {
        db.reserve_provider_request(claims, request_key, "digest", observed, ATTEMPT_NOW)
            .expect("reserve");
        let settled = db
            .settle_provider_request(request_key, observed, Some("upstream-1"), ATTEMPT_NOW)
            .expect("settle");
        assert_eq!(settled.status, "settled");
    }

    /// Reserve one provider call and leave it unresolved -- the supplier
    /// never confirmed a cost, so it must never contribute to what an
    /// attempt owes.
    fn leave_unresolved(db: &Database, claims: &GatewayCapability, request_key: &str) {
        db.reserve_provider_request(claims, request_key, "digest", 999_999, ATTEMPT_NOW)
            .expect("reserve");
        db.mark_provider_request_unresolved(request_key, None, "stub timeout", ATTEMPT_NOW)
            .expect("mark unresolved");
    }

    #[test]
    fn attempt_settled_cost_sums_every_settled_call_and_excludes_unresolved_ones() {
        let db = test_db();
        let claims = attempt_capability(&db, "attempt-sum");
        settle_call(&db, &claims, "call-1", 300_000);
        settle_call(&db, &claims, "call-2", 150_000);
        leave_unresolved(&db, &claims, "call-3");

        assert_eq!(db.attempt_settled_cost_micro_usd("attempt-sum"), 450_000);
    }

    #[test]
    fn an_attempt_with_no_provider_calls_owes_nothing() {
        let db = test_db();
        assert_eq!(
            db.attempt_settled_cost_micro_usd("attempt-never-dispatched"),
            0
        );
    }

    #[test]
    fn settle_pending_attempts_charges_the_exact_settled_sum_for_a_chargeable_cause() {
        // 300_000 + 150_000 micro-USD at the seeded 100_000-micro-USD credit
        // (`SEED_MICROS_PER_CREDIT`) is exactly 4 whole credits with a
        // 50_000 remainder -- pass-through billing, no rounding up.
        let db = test_db();
        let user = subscriber(&db, 1_000);
        let claims = attempt_capability(&db, "attempt-charge");
        settle_call(&db, &claims, "charge-1", 300_000);
        settle_call(&db, &claims, "charge-2", 150_000);

        end_and_settle_attempt(
            &db,
            user,
            "attempt-charge",
            "step-charge",
            AttemptEndCause::CustomerCancel,
        );

        assert_eq!(db.get_credit_balance(user).subscription_remaining, 996);
        assert_eq!(db.get_credit_carry_micro_usd(user), 50_000);
    }

    #[test]
    fn settle_pending_attempts_replay_is_a_noop_not_a_double_charge() {
        let db = test_db();
        let user = subscriber(&db, 1_000);
        let claims = attempt_capability(&db, "attempt-replay");
        settle_call(&db, &claims, "replay-1", 300_000);

        end_and_settle_attempt(
            &db,
            user,
            "attempt-replay",
            "step-replay",
            AttemptEndCause::Verified,
        );
        assert_eq!(db.get_credit_balance(user).subscription_remaining, 997);
        assert_eq!(db.get_credit_carry_micro_usd(user), 0);

        // Same attempt, same cause, settled again (e.g. a cancel racing a
        // verdict that already settled): `INSERT OR IGNORE` makes the second
        // `attempt_endings` insert a no-op on the same primary key, and even
        // if it weren't, `ChargeKey::for_attempt` makes the second charge see
        // its own idempotency key already spent and write nothing further.
        end_and_settle_attempt(
            &db,
            user,
            "attempt-replay",
            "step-replay",
            AttemptEndCause::Verified,
        );
        assert_eq!(db.get_credit_balance(user).subscription_remaining, 997);
        assert_eq!(db.get_credit_carry_micro_usd(user), 0);
    }

    #[test]
    fn settle_pending_attempts_with_no_settled_calls_writes_no_ledger_row_at_all() {
        let db = test_db();
        let user = subscriber(&db, 1_000);

        // Never dispatched far enough to make a priced call -- there is
        // nothing to charge and nothing to absorb, so this must be silent:
        // no ledger row, whether the cause would have charged or absorbed.
        end_and_settle_attempt(
            &db,
            user,
            "attempt-nothing-spent",
            "step-nothing-spent",
            AttemptEndCause::CustomerCancel,
        );
        end_and_settle_attempt(
            &db,
            user,
            "attempt-nothing-spent-2",
            "step-nothing-spent-2",
            AttemptEndCause::RunnerDown,
        );

        assert_eq!(db.get_credit_balance(user).subscription_remaining, 1_000);
        assert_eq!(db.get_credit_carry_micro_usd(user), 0);
        assert_eq!(db.credit_ledger_totals(user), (0, 0));

        // `credit_ledger_totals` alone would read the same 0 whether no row
        // was written or a zero-amount absorb row was: count rows directly
        // to prove this case is silent, not merely balance-neutral.
        let rows: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM credit_transactions WHERE clerk_user_id = ?1",
                params![user],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            rows, 0,
            "an attempt with no settled calls must write no ledger row"
        );
    }

    #[test]
    fn settle_pending_attempts_absorbs_as_a_zero_amount_row_never_a_refund() {
        // An infrastructure-caused end (the worker vanished, or Cortex itself
        // crashed) must never touch the customer's balance -- Cortex eats the
        // cost. This is recorded as a `credit_transactions` row with
        // `amount = 0`, on the record against Cortex, not as a refund of any
        // kind (there is nothing to refund: nothing was ever charged).
        let db = test_db();
        let user = subscriber(&db, 1_000);
        let claims = attempt_capability(&db, "attempt-absorb");
        settle_call(&db, &claims, "absorb-1", 300_000);

        end_and_settle_attempt(
            &db,
            user,
            "attempt-absorb",
            "step-absorb",
            AttemptEndCause::RunnerDown,
        );

        assert_eq!(
            db.get_credit_balance(user).subscription_remaining,
            1_000,
            "an absorbed attempt must not touch the customer's balance"
        );
        assert_eq!(db.get_credit_carry_micro_usd(user), 0);
        assert_eq!(db.credit_ledger_totals(user), (0, 0));

        let key = cortex_core::billing_binding::ChargeKey::for_attempt("attempt-absorb");
        let (amount, reason): (i64, String) = db
            .conn()
            .query_row(
                "SELECT amount, reason FROM credit_transactions WHERE idempotency_key = ?1",
                params![key.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("absorbed-cost row");
        assert_eq!(amount, 0, "an absorbed cost is never a nonzero refund row");
        assert_eq!(reason, "absorbed");
    }

    #[test]
    fn settle_pending_attempts_waits_while_a_reservation_is_still_reserved_then_settles_exactly() {
        // H2: while any reservation for the attempt is still `reserved` (the
        // gateway hasn't heard back from the supplier yet), the settler must
        // not guess -- it leaves the ending unsettled and writes nothing.
        // Once every reservation resolves, the very next tick charges the
        // exact sum, floored to whole credits with the remainder carried
        // (same 300_000 + 150_000 -> 4 credits, 50_000 carry arithmetic as
        // `settle_pending_attempts_charges_the_exact_settled_sum_for_a_chargeable_cause`).
        let db = test_db();
        let user = subscriber(&db, 1_000);
        let claims = attempt_capability(&db, "attempt-waiting");

        // Reserved but never settled: the supplier hasn't confirmed a cost.
        db.reserve_provider_request(&claims, "wait-1", "digest", 300_000, ATTEMPT_NOW)
            .expect("reserve");

        end_and_settle_attempt(
            &db,
            user,
            "attempt-waiting",
            "step-waiting",
            AttemptEndCause::CustomerCancel,
        );

        assert_eq!(db.get_credit_balance(user).subscription_remaining, 1_000);
        assert_eq!(db.credit_ledger_totals(user), (0, 0));
        let settled_at: Option<i64> = db
            .conn()
            .query_row(
                "SELECT settled_at FROM attempt_endings WHERE attempt_id = ?1",
                params!["attempt-waiting"],
                |row| row.get(0),
            )
            .expect("ending row exists");
        assert!(
            settled_at.is_none(),
            "an attempt with a reservation still `reserved` must not be marked settled"
        );

        // The supplier confirms the first call, and a second call settles
        // cleanly too.
        let settled = db
            .settle_provider_request("wait-1", 300_000, Some("upstream-1"), ATTEMPT_NOW)
            .expect("settle");
        assert_eq!(settled.status, "settled");
        settle_call(&db, &claims, "wait-2", 150_000);

        db.settle_pending_attempts().expect("settle");
        assert_eq!(db.get_credit_balance(user).subscription_remaining, 996);
        assert_eq!(db.get_credit_carry_micro_usd(user), 50_000);

        // A second settle pass, now that the row is already settled, changes
        // nothing further.
        db.settle_pending_attempts().expect("settle");
        assert_eq!(db.get_credit_balance(user).subscription_remaining, 996);
        assert_eq!(db.get_credit_carry_micro_usd(user), 50_000);
    }

    #[test]
    fn settle_pending_attempts_with_no_price_list_records_the_error_then_recovers() {
        // H3: with no active price list there is no rate to convert a
        // settled micro-USD cost into credits. The settler must not guess --
        // it leaves the ending unsettled, records the failure on the row's
        // own `last_error` (F7 of the money-review fix pass), and writes no
        // ledger row. Once a price list exists again, the next pass charges
        // the exact sum and clears the error.
        let db = test_db();
        let user = subscriber(&db, 1_000);
        let claims = attempt_capability(&db, "attempt-no-price-list");
        settle_call(&db, &claims, "no-price-1", 300_000);

        db.conn()
            .execute("DELETE FROM price_lists", [])
            .expect("remove every price list");

        end_and_settle_attempt(
            &db,
            user,
            "attempt-no-price-list",
            "step-no-price-list",
            AttemptEndCause::CustomerCancel,
        );

        assert_eq!(db.get_credit_balance(user).subscription_remaining, 1_000);
        assert_eq!(db.credit_ledger_totals(user), (0, 0));

        let (settled_at, last_error): (Option<i64>, Option<String>) = db
            .conn()
            .query_row(
                "SELECT settled_at, last_error FROM attempt_endings WHERE attempt_id = ?1",
                params!["attempt-no-price-list"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("ending row exists");
        assert!(
            settled_at.is_none(),
            "a settlement failure must not be marked settled"
        );
        assert!(
            last_error.is_some(),
            "a missing price list must record why the settle failed"
        );

        // Publish a price list again and retry: the exact same tick that
        // previously failed now succeeds and clears the error.
        db.publish_price_list(&crate::pricing::seed_provisional(
            2,
            "test:recovered",
            0,
            crate::pricing::seed_models(),
        ))
        .expect("publish recovered price list");

        db.settle_pending_attempts().expect("settle");
        assert_eq!(db.get_credit_balance(user).subscription_remaining, 997);
        assert_eq!(db.get_credit_carry_micro_usd(user), 0);

        let (settled_at, last_error): (Option<i64>, Option<String>) = db
            .conn()
            .query_row(
                "SELECT settled_at, last_error FROM attempt_endings WHERE attempt_id = ?1",
                params!["attempt-no-price-list"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("ending row exists");
        assert!(settled_at.is_some(), "the retried settle must succeed");
        assert!(
            last_error.is_none(),
            "a successful retry must clear the prior error"
        );
    }

    // --- get_receipt: charged_credits ---

    #[test]
    fn get_receipt_reports_the_exact_charge_when_paid_from_subscription_alone() {
        // H8: `charged_credits` is what the ledger actually moved for this
        // verification, not the quote. Paid entirely out of the monthly
        // allotment, it must read back as exactly that whole-credit amount.
        let db = test_db();
        insert_run_and_step(&db, "run-receipt-sub", "step-receipt-sub");
        let vid = seal_verification(&db, "run-receipt-sub", "step-receipt-sub", Verdict::Verified);

        let user = subscriber(&db, 1_000);
        let charge_key = ChargeKey::for_verification(&vid);
        db.deduct_credits(user, 4, "verified verdict charge", &charge_key)
            .expect("charge succeeds");

        let receipt = db
            .get_receipt("run-receipt-sub", "step-receipt-sub")
            .expect("sealed verification has a receipt");
        assert_eq!(receipt.charged_credits, Some(4));
    }

    #[test]
    fn get_receipt_reports_the_exact_total_when_the_charge_splits_across_subscription_and_pack() {
        // H8: a charge larger than the remaining monthly allotment spills
        // into the purchased-pack bucket, writing two `credit_transactions`
        // rows under the same verification key. The receipt must still show
        // one exact total, not just the subscription-bucket half of it.
        let db = test_db();
        insert_run_and_step(&db, "run-receipt-split", "step-receipt-split");
        let vid = seal_verification(
            &db,
            "run-receipt-split",
            "step-receipt-split",
            Verdict::Verified,
        );

        let user = subscriber(&db, 2);
        let granted = db
            .grant_topup_credits(user, "cs_receipt_split", 250, "Top-up $25")
            .expect("grant");
        assert!(granted);

        let charge_key = ChargeKey::for_verification(&vid);
        db.deduct_credits(user, 4, "verified verdict charge", &charge_key)
            .expect("charge succeeds");

        let (sub_total, pack_total) = db.credit_ledger_totals(user);
        assert_eq!(sub_total, -2, "the allotment covers only 2 of the 4 credits");
        assert_eq!(pack_total, -2, "the remaining 2 credits spill into the pack bucket");

        let receipt = db
            .get_receipt("run-receipt-split", "step-receipt-split")
            .expect("sealed verification has a receipt");
        assert_eq!(
            receipt.charged_credits,
            Some(4),
            "the receipt must show the full split charge as one exact total"
        );
    }

    // --- grant_topup_credits ---

    #[test]
    fn a_topup_grant_credits_the_pack_bucket_and_writes_one_row() {
        let db = test_db();
        let user = subscriber(&db, 0);

        let granted = db
            .grant_topup_credits(user, "cs_test_1", 250, "Top-up $25")
            .expect("grant");
        assert!(granted, "a fresh session must grant credits");

        let balance = db.get_credit_balance_row(user).expect("balance row");
        assert_eq!(balance.pack_remaining, 250);
        assert_eq!(
            balance.subscription_remaining, 0,
            "a top-up must never touch the subscription bucket"
        );

        let row_count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM credit_transactions WHERE idempotency_key = ?1",
                params!["topup:cs_test_1"],
                |r| r.get(0),
            )
            .expect("count query");
        assert_eq!(row_count, 1);
    }

    #[test]
    fn a_topup_grant_never_invents_subscription_credits_for_a_new_user() {
        // A pack purchase from a user with no balance row at all must not
        // mint a free monthly allotment — only the pack bucket is credited.
        let db = test_db();
        let granted = db
            .grant_topup_credits("brand-new-user", "cs_test_new", 250, "Top-up $25")
            .expect("grant");
        assert!(granted);

        let balance = db
            .get_credit_balance_row("brand-new-user")
            .expect("a balance row must now exist");
        assert_eq!(balance.subscription_remaining, 0);
        assert_eq!(balance.subscription_total, 0);
        assert_eq!(balance.pack_remaining, 250);
    }

    #[test]
    fn a_replayed_topup_session_grants_nothing_further() {
        // Stripe redelivers `checkout.session.completed` (same event id) and
        // may separately fire `checkout.session.async_payment_succeeded` for
        // the same session (a different event id, same money) -- both must
        // be no-ops after the first grant.
        let db = test_db();
        let user = subscriber(&db, 0);

        let first = db
            .grant_topup_credits(user, "cs_test_2", 250, "Top-up $25")
            .expect("first grant");
        assert!(first);

        let replay_same_event = db
            .grant_topup_credits(user, "cs_test_2", 250, "Top-up $25")
            .expect("replayed grant must not error");
        assert!(!replay_same_event, "a replay must grant nothing further");

        let balance = db.get_credit_balance_row(user).expect("balance row");
        assert_eq!(balance.pack_remaining, 250, "credited only once");

        let row_count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM credit_transactions WHERE idempotency_key = ?1",
                params!["topup:cs_test_2"],
                |r| r.get(0),
            )
            .expect("count query");
        assert_eq!(row_count, 1, "a replay must not write a second row");
    }

    #[test]
    fn a_topped_up_pack_is_spent_only_after_the_subscription_runs_out() {
        // Before `grant_topup_credits` existed, a pack purchase from a brand
        // new user went through `add_pack_credits`, which minted a phantom
        // 200-credit subscription allotment on the missing balance row --
        // invisible to a spend test, since a charge would "succeed" against
        // credits nobody paid for while the pack sat untouched. This proves
        // the real path end to end: a subscriber tops up, and
        // `charge_settled_cost` still drains the (real, small) subscription
        // bucket before touching the pack credits they bought, and a
        // zero-subscription top-up customer can still be charged at all out
        // of the pack alone.
        let db = test_db();
        let user = subscriber(&db, 2);
        db.grant_topup_credits(user, "cs_test_spend", 250, "Top-up $25")
            .expect("grant");

        let before = db.get_credit_balance_row(user).expect("balance row");
        assert_eq!(before.subscription_remaining, 2);
        assert_eq!(before.pack_remaining, 250);

        let key = ChargeKey::for_chat_reply("reply-topup");
        let settled = db
            .charge_settled_cost(user, 300_000, 100_000, "Cortex-paid chat reply", &key)
            .expect("settle");
        assert_eq!(settled.credits_charged, 3);

        let after = db.get_credit_balance_row(user).expect("balance row");
        assert_eq!(
            after.subscription_remaining, 0,
            "the 2 subscription credits are drawn down first"
        );
        assert_eq!(
            after.pack_remaining, 249,
            "only the 1 remaining credit spills into the purchased pack"
        );

        // And a customer with no subscription at all -- purely a top-up --
        // can still be charged: everything comes out of the pack.
        db.grant_topup_credits("topup-only-user", "cs_test_spend_2", 250, "Top-up $25")
            .expect("grant");
        let zero_sub_settled = db
            .charge_settled_cost(
                "topup-only-user",
                200_000,
                100_000,
                "Cortex-paid chat reply",
                &ChargeKey::for_chat_reply("reply-topup-only"),
            )
            .expect("settle");
        assert_eq!(zero_sub_settled.credits_charged, 2);
        let zero_sub_after = db
            .get_credit_balance_row("topup-only-user")
            .expect("balance row");
        assert_eq!(zero_sub_after.subscription_remaining, 0);
        assert_eq!(zero_sub_after.pack_remaining, 248);
    }
}
