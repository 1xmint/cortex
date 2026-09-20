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
                        cortex_core::execution_job::CapabilityGrant::ReachProvider { .. }
                        | cortex_core::execution_job::CapabilityGrant::ReadSecret { .. } => {
                            Vec::new()
                        }
                    })
                    .collect();
                let granted_provider = grants.iter().find_map(|grant| match grant {
                    cortex_core::execution_job::CapabilityGrant::ReachProvider { provider } => {
                        Some(provider.clone())
                    }
                    cortex_core::execution_job::CapabilityGrant::ResolveDependencies { .. }
                    | cortex_core::execution_job::CapabilityGrant::ReadSecret { .. } => None,
                });
                Some(EgressReceipt {
                    granted_registries,
                    granted_provider,
                    endpoints,
                    mediator_image: mediator,
                })
            });

        Some(Receipt {
            verification_id,
            run_id: run_id.to_string(),
            step_id: step_id.to_string(),
            attempt,
            tree_hash,
            gate: compute_verdict(&specs, &executions),
            executions,
            egress,
        })
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

    pub fn add_pack_credits(&self, clerk_user_id: &str, amount: i64) -> Result<(), String> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO credit_balances (clerk_user_id, subscription_remaining, subscription_total, pack_remaining)
             VALUES (?1, 200, 200, ?2)
             ON CONFLICT(clerk_user_id) DO UPDATE SET pack_remaining = pack_remaining + ?2",
            params![clerk_user_id, amount],
        )
        .map_err(|e| format!("failed to add pack credits: {e}"))?;
        Ok(())
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
}
