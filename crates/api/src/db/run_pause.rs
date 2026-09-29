//! Pause a run when its owner's credits run out, and resume it on top-up.
//!
//! There are no holds here. Credits are charged as each attempt settles, at
//! exact cost, and nothing is ever locked. What this file adds is the answer
//! to "the next call does not fit in the balance": the attempt ends, its
//! calls already made are charged as usual, the step goes back to the queue
//! without being failed or retried, and the run waits in `awaiting_top_up`
//! until the owner tops up and the run is resumed.
//!
//! Every fact about a pause is an existing column: the run's `status`, the
//! step's `orphaned` status, and an `attempt_endings` row with cause
//! `out_of_credits`. Nothing is added to the schema, so a paused run survives
//! a restart the same way any other row does.

use super::*;
use rusqlite::OptionalExtension;

/// A step that was paused because its owner ran out of credits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PausedAttempt {
    pub user_id: String,
    pub run_id: String,
    pub step_id: String,
    /// The worker that was running the step, so the caller can tell it to
    /// stop. `None` when the step had no worker recorded.
    pub worker_id: Option<String>,
}

/// What `resume_run_after_top_up` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeOutcome {
    /// The run is running again; the caller should let the scheduler
    /// re-dispatch its paused steps.
    Resumed,
    /// The balance still does not cover one maximum per-call reservation.
    Insufficient {
        need_credits: i64,
        available_credits: i64,
    },
    /// The run is not waiting for a top-up.
    NotPaused,
}

impl Database {
    /// What a user can still pay for, in micro-USD: the whole balance
    /// converted at the active price list, minus the fractional carry already
    /// owed. The same formula `chat_paid` gates on. `None` when the user has
    /// no balance row (unmetered) or no price list is published.
    pub fn payable_micro_usd(&self, user_id: &str) -> Option<i64> {
        let balance = self.get_credit_balance_row(user_id)?;
        let price_list = self.active_price_list()?;
        let carry = self.get_credit_carry_micro_usd(user_id) as i64;
        Some(
            (balance.subscription_remaining + balance.pack_remaining)
                .saturating_mul(price_list.micros_per_credit)
                .saturating_sub(carry),
        )
    }

    /// The largest reservation one provider call can ask for on the active
    /// price list: a call whose input plus output fills the model's whole
    /// context window at the dearer of its two rates. The gateway refuses a
    /// call whose input bound plus `max_tokens` exceeds the window, so no call
    /// can reserve more than this.
    pub fn max_call_reservation_micros(&self) -> Option<i64> {
        let price_list = self.active_price_list()?;
        price_list
            .models
            .iter()
            .filter(|m| m.provider == "claude" || m.provider == "openai")
            .filter_map(|m| {
                let rate = m.input_micros_per_1k.max(m.output_micros_per_1k);
                m.context_window
                    .checked_mul(rate)?
                    .checked_add(999)
                    // The gateway rounds input and output up separately, so
                    // one micro of headroom covers the two roundings.
                    .map(|v| v / 1_000 + 1)
            })
            .max()
    }

    /// `(need_credits, available_credits)` for resuming a paused run: the
    /// credits that cover one maximum per-call reservation plus the carry
    /// already owed, against the credits the user has now. `None` when no
    /// price list is published.
    pub fn top_up_requirement(&self, user_id: &str) -> Option<(i64, i64)> {
        let price_list = self.active_price_list()?;
        let max_reservation = self.max_call_reservation_micros()?;
        let carry = self.get_credit_carry_micro_usd(user_id) as i64;
        let available = self
            .get_credit_balance_row(user_id)
            .map(|b| b.subscription_remaining + b.pack_remaining)
            .unwrap_or(0);
        let per_credit = price_list.micros_per_credit.max(1);
        let need = max_reservation
            .saturating_add(carry)
            .saturating_add(per_credit - 1)
            / per_credit;
        Some((need, available))
    }

    /// End the attempt that just hit its balance-capped authorization and
    /// park its run.
    ///
    /// Only an authorization whose cap sits below the operator's own cap
    /// (`env_max_micro_usd`) was capped by the balance; an attempt that hit
    /// the operator's cap did not run out of credits and is left alone.
    ///
    /// In one transaction: the step goes to `orphaned` (dispatchable again,
    /// not failed, no retry counted), its path leases are released, an
    /// `out_of_credits` ending is recorded so the calls already made are
    /// charged exactly as settled, and the run goes to `awaiting_top_up`.
    /// `None` means nothing was paused.
    pub fn pause_attempt_for_top_up(
        &self,
        run_id: &str,
        attempt_id: &str,
        authorization_id: &str,
        env_max_micro_usd: i64,
    ) -> Option<PausedAttempt> {
        let paused = {
            let mut conn = self.conn();
            let now = chrono::Utc::now().timestamp_millis();
            let tx = conn.transaction().ok()?;

            let authorized_max: i64 = tx
                .query_row(
                    "SELECT max_micro_usd FROM provider_spend_authorizations
                     WHERE id = ?1 AND run_id = ?2 AND attempt_id = ?3",
                    params![authorization_id, run_id, attempt_id],
                    |row| row.get(0),
                )
                .ok()?;
            if authorized_max >= env_max_micro_usd {
                return None;
            }

            let run_status: String = tx
                .query_row(
                    "SELECT status FROM runs WHERE id = ?1",
                    params![run_id],
                    |row| row.get(0),
                )
                .ok()?;
            if !matches!(
                run_status.as_str(),
                "planning" | "running" | "awaiting_top_up"
            ) {
                return None;
            }

            let (step_id, worker_id, worker_owned_by_cortex): (String, Option<String>, bool) = tx
                .query_row(
                    "SELECT s.id, s.assigned_worker, COALESCE(w.owned_by_cortex, 0)
                     FROM steps s
                     LEFT JOIN workers w ON w.id = s.assigned_worker
                     WHERE s.run_id = ?1 AND s.server_attempt_id = ?2
                       AND s.status IN ('leased', 'running')",
                    params![run_id, attempt_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .ok()??;
            let user_id = Self::run_user_id_in_tx(&tx, run_id).ok()?;

            let rows = tx
                .execute(
                    "UPDATE steps SET status = 'orphaned', assigned_worker = NULL,
                         lease_deadline = NULL, updated_at = ?1, version = version + 1
                     WHERE id = ?2 AND status IN ('leased', 'running')",
                    params![now, step_id],
                )
                .ok()?;
            if rows == 0 {
                return None;
            }
            insert_step_operations_event(
                &tx,
                &step_id,
                "step.paused_for_top_up",
                &serde_json::json!({
                    "status": "orphaned",
                    "attempt_id": attempt_id,
                }),
            );
            Self::insert_attempt_ending_in_tx(
                &tx,
                attempt_id,
                &user_id,
                &step_id,
                cortex_core::billing_binding::AttemptEndCause::OutOfCredits,
                worker_owned_by_cortex,
                now,
            )
            .ok()?;
            release_step_leases(&tx, &step_id, now);
            if run_status != "awaiting_top_up" {
                update_run_status_tx(&tx, run_id, "awaiting_top_up", None, now).ok()?;
            }
            tx.commit().ok()?;

            PausedAttempt {
                user_id,
                run_id: run_id.to_string(),
                step_id,
                worker_id,
            }
        };

        // Charge what the attempt's calls actually cost, now rather than on
        // the next tick, so the run's recorded spend is current when the
        // owner looks at it. A failure here is retried by the regular tick.
        if let Err(err) = self.settle_pending_attempts() {
            tracing::warn!(run_id, error = %err, "pause: settle_pending_attempts failed");
        }
        Some(paused)
    }

    /// Put a paused run back to `running` if the balance now covers one
    /// maximum per-call reservation. The caller emits the scheduler event
    /// that re-dispatches the paused steps.
    pub fn resume_run_after_top_up(
        &self,
        run_id: &str,
        user_id: &str,
    ) -> Result<ResumeOutcome, String> {
        if self.get_run_status(run_id).as_deref() != Some("awaiting_top_up") {
            return Ok(ResumeOutcome::NotPaused);
        }
        let (need_credits, available_credits) = self
            .top_up_requirement(user_id)
            .ok_or_else(|| "no price list is published".to_string())?;
        if available_credits < need_credits {
            return Ok(ResumeOutcome::Insufficient {
                need_credits,
                available_credits,
            });
        }

        let mut conn = self.conn();
        let now = chrono::Utc::now().timestamp_millis();
        let tx = conn
            .transaction()
            .map_err(|e| format!("failed to begin resume transaction: {e}"))?;
        let status: Option<String> = tx
            .query_row(
                "SELECT status FROM runs WHERE id = ?1",
                params![run_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("failed to read run status: {e}"))?;
        if status.as_deref() != Some("awaiting_top_up") {
            return Ok(ResumeOutcome::NotPaused);
        }
        update_run_status_tx(&tx, run_id, "running", None, now)
            .map_err(|e| format!("failed to resume run: {e}"))?;
        tx.commit()
            .map_err(|e| format!("failed to commit resume: {e}"))?;
        Ok(ResumeOutcome::Resumed)
    }

    /// Resume every paused run of `user_id` that the balance now covers, and
    /// return the ids resumed. Used when a top-up lands.
    pub fn resume_awaiting_runs_for_user(&self, user_id: &str) -> Vec<String> {
        let ids: Vec<String> = {
            let conn = self.conn();
            let Ok(mut stmt) = conn.prepare(
                "SELECT id FROM runs WHERE user_id = ?1 AND status = 'awaiting_top_up'
                 ORDER BY created_at ASC",
            ) else {
                return Vec::new();
            };
            let Ok(rows) = stmt.query_map(params![user_id], |row| row.get::<_, String>(0)) else {
                return Vec::new();
            };
            rows.filter_map(|r| r.ok()).collect()
        };
        ids.into_iter()
            .filter(|id| {
                matches!(
                    self.resume_run_after_top_up(id, user_id),
                    Ok(ResumeOutcome::Resumed)
                )
            })
            .collect()
    }

    /// Credits charged to a run so far, from the ledger: every spend row
    /// keyed to one of the run's attempts. Settled charges only, so it is
    /// exact and never an estimate.
    pub(super) fn run_spent_credits_in(conn: &Connection, run_id: &str) -> i64 {
        conn.query_row(
            "SELECT COALESCE(-SUM(t.amount), 0)
             FROM credit_transactions t
             JOIN attempt_endings e
               ON t.idempotency_key IN ('attempt:' || e.attempt_id,
                                        'attempt:' || e.attempt_id || ':pack')
             JOIN steps s ON s.id = e.step_id
             WHERE s.run_id = ?1 AND t.reason = 'spend'",
            params![run_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
    }

    /// [`Self::run_spent_credits_in`] on the shared connection.
    pub fn run_spent_credits(&self, run_id: &str) -> i64 {
        let conn = self.conn();
        Self::run_spent_credits_in(&conn, run_id)
    }
}
