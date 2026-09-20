//! The loop that makes verification durable.
//!
//! Delivery enqueues a row; this claims it, runs the frozen checks, and seals
//! the verdict. Nothing here is the source of durability — the database is.
//! This process can die at any line below and the work is still on disk, still
//! claimable, and still counted.
//!
//! # What each part is defending against
//!
//! - **Claim with a token, heartbeat while working.** A dispatcher that dies
//!   mid-check leaves a claim whose heartbeat stops; after the lease window
//!   another dispatcher takes it. Without the token, the second dispatcher
//!   could not tell its own claim from a corpse's.
//! - **Reclaim counts as an attempt.** A job that reliably kills whatever picks
//!   it up reaches the dead-letter state instead of eating the queue forever.
//! - **Reconcile on startup, before serving.** The jobs that matter most are the
//!   ones the *previous* deploy left behind, so reconciliation is not scoped to
//!   what this process enqueued.
//! - **Infrastructure failure retries; an executed check does not.** A runner
//!   that could not start is our problem and gets another go. A check that ran
//!   and came back inconclusive is a fact about the work, and retrying it until
//!   it goes green is how flakiness gets laundered into a pass.
//!
//! See `cortex/plan/briefs/PR-B-durable-verifier.md` and
//! `docs/adr/ADR-0001-step-truth-model.md`.

use std::sync::Arc;
use std::time::Duration;

use crate::db::{Database, VerificationJob, VERIFICATION_LEASE_MS};
use crate::state::AppState;
use crate::verification_driver::{self, DeliveryFacts};

/// How often the dispatcher looks for work when the queue is empty.
const IDLE_POLL: Duration = Duration::from_secs(5);

/// How often a claim is refreshed while a check runs. Comfortably inside the
/// lease so an ordinary GC pause cannot lose a claim mid-verification.
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(VERIFICATION_LEASE_MS as u64 / 3);

/// How often expired claims are swept and non-terminal jobs re-examined.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(30);

/// Backoff before a retryable job is offered again. Fixed rather than
/// exponential: the thing being waited on is almost always a container runtime
/// coming back, and an exponential curve turns a thirty-second outage into a
/// twenty-minute one.
const RETRY_BACKOFF: Duration = Duration::from_secs(60);

/// Refuse to start a second dispatcher on SQLite.
///
/// The store is a `Mutex<Connection>`; two dispatchers in two processes do not
/// share that mutex, and the claim CAS is the only thing between them and
/// double-dispatch. The CAS is correct, but the rest of the write path is not
/// yet safe for two writers, so this is a hard failure and not a warning.
///
/// A deployment that genuinely runs one node asserts it. One that cannot make
/// that promise should not be running this at all, which is the point of
/// refusing rather than logging.
pub fn assert_single_node() -> Result<(), String> {
    match std::env::var("CORTEX_SINGLE_NODE").as_deref() {
        Ok("1") | Ok("true") => Ok(()),
        _ => Err(
            "the verification dispatcher requires CORTEX_SINGLE_NODE=1. The SQLite store is a \
             Mutex<Connection> and two dispatchers double-dispatch. Set it only where exactly \
             one node runs; a Postgres dispatcher removes this constraint."
                .to_string(),
        ),
    }
}

/// Recover everything the previous process left behind.
///
/// Runs before the server accepts traffic. Expired claims go back to the queue,
/// and what remains is reported — a queue nobody can see is a queue that
/// silently grows.
pub fn reconcile_on_startup(db: &Database) {
    let reclaimed = db.reclaim_expired_verification_jobs();
    let pending = db.non_terminal_verification_jobs();

    if reclaimed > 0 {
        tracing::warn!(
            reclaimed,
            "recovered verification claims held by a process that is no longer running"
        );
    }
    if !pending.is_empty() {
        tracing::info!(
            pending = pending.len(),
            depth = ?db.verification_queue_depth(),
            "verification jobs carried over from the previous run"
        );
    }
}

/// Start the dispatcher. One per process, and one process, per
/// [`assert_single_node`].
pub fn spawn(state: Arc<AppState>) {
    let dispatcher_id = format!("verifier-{}", uuid::Uuid::new_v4());
    tracing::info!(dispatcher_id, "verification dispatcher starting");
    tokio::spawn(dispatch_loop(state, dispatcher_id));
}

async fn dispatch_loop(state: Arc<AppState>, dispatcher_id: String) {
    let mut reconcile = tokio::time::interval(RECONCILE_INTERVAL);
    reconcile.tick().await; // The first tick is immediate; startup already ran.

    loop {
        let Some(db) = state.db.as_ref() else {
            tokio::time::sleep(IDLE_POLL).await;
            continue;
        };

        tokio::select! {
            _ = reconcile.tick() => {
                let reclaimed = db.reclaim_expired_verification_jobs();
                if reclaimed > 0 {
                    tracing::warn!(
                        reclaimed,
                        "reclaimed verification claims whose lease expired"
                    );
                }
                crate::metrics::set_verification_queue_depth(&db.verification_queue_depth());
            }
            _ = tokio::time::sleep(IDLE_POLL) => {}
        }

        // Drain what is runnable, then go back to waiting. One at a time: the
        // checks run in containers and the point of the queue is that work is
        // not lost, not that it is parallel.
        while let Some(job) = db.claim_verification_job(&dispatcher_id) {
            run_claimed_job(&state, db, &dispatcher_id, job).await;
        }
    }
}

/// Run one claimed job to a terminal state, or hand it back.
async fn run_claimed_job(
    state: &AppState,
    db: &Database,
    dispatcher_id: &str,
    job: VerificationJob,
) {
    let started = std::time::Instant::now();
    tracing::info!(
        job_id = %job.job_id,
        step_id = %job.step_id,
        attempt = job.attempt_count,
        "claimed a verification job"
    );

    // The exam must be the one the job was promised. A spec set that changed
    // between enqueue and claim means we would be grading against a different
    // exam than the customer was told about, which is not a verdict we are
    // entitled to issue.
    let specs = db.load_check_specs(&job.run_id, &job.step_id);
    let digest = crate::db::spec_set_digest(&specs);
    if job.spec_set_digest != "unknown" && digest != job.spec_set_digest {
        seal(
            state,
            db,
            dispatcher_id,
            &job,
            "inconclusive",
            "the frozen check specs changed between enqueue and claim",
        )
        .await;
        return;
    }

    // Our infrastructure, not the customer's work. It gets another go.
    let runner =
        match crate::check_runner::ContainerCheckRunner::new(verification_driver::runner_image()) {
            Ok(runner) => runner,
            Err(e) => {
                tracing::warn!(
                    job_id = %job.job_id,
                    error = %e,
                    "no container runner available; the job waits rather than being lost"
                );
                if db.retry_verification_job(
                    &job.job_id,
                    dispatcher_id,
                    RETRY_BACKOFF.as_millis() as i64,
                ) {
                    crate::metrics::record_verification_retry();
                }
                return;
            }
        };

    let facts = DeliveryFacts {
        run_id: job.run_id.clone(),
        step_id: job.step_id.clone(),
        attempt_id: job.attempt_id.clone(),
        attempt: job.lease_gen,
        workspace_dir: state.workspace_dir.clone(),
        // Frozen at enqueue. Never re-resolved — a job that looked this up
        // again could grade a tree the worker never delivered.
        head_commit: job.delivered_commit.clone(),
        // The quote frozen at dispatch, if there was one.
        //
        // `billable_credits()` rather than `quoted_credits`, deliberately. A
        // provisional class publishes a number so the number can be argued
        // with, and does not move money — Phase 31.3's graduation gate. Reading
        // the raw field here would charge against a price seeded from a model
        // with zero measured outcomes behind it, which is the one thing the
        // gate exists to prevent.
        //
        // `None` is still a real state and still the honest one: a step
        // dispatched before any price list was published has no price, and the
        // driver records the verdict and leaves the ledger alone.
        quoted_credits: db
            .get_step_quote(&job.run_id, &job.step_id)
            .and_then(|quote| quote.billable_credits()),
    };

    // Keep the claim alive while the checks run, so a slow container does not
    // look like a dead dispatcher.
    let heartbeat = spawn_heartbeat(state, job.job_id.clone(), dispatcher_id.to_string());

    let verdict = verification_driver::verify_delivery(db, &runner, &facts).await;
    heartbeat.abort();

    // The routing signal, restored against a real verdict.
    //
    // PR A suspended it because `record_step_completed` derived its outcome
    // from the worker's exit code, which invariant 6 forbids from improving a
    // score. This is the only place a positive reward can now originate, and
    // the verdict here was produced by checks that ran against the delivered
    // tree in a runner the task did not choose.
    //
    // The spend is the whole attempt chain, read at verdict time: a step
    // verified on its fourth try cost four dispatches, and a signal that only
    // sees the winning attempt cannot tell a model that gets it right first
    // time from one that needs coaxing.
    if let Some(verdict) = verdict {
        match db.get_run_user_id(&job.run_id) {
            Some(user_id) => {
                let chain = db.attempt_chain_spend(&job.step_id);
                let emitted = state.vera_tracker.record_verdict(
                    &user_id,
                    verdict,
                    chain,
                    db.get_step_intent(&job.step_id),
                );
                tracing::debug!(
                    step_id = %job.step_id,
                    verdict = ?verdict,
                    attempts = chain.attempts,
                    chain_ms = chain.total_duration_ms,
                    emitted,
                    "routing signal from verdict"
                );
            }
            // No owner means no heart to attribute the interaction to. Dropping
            // the signal is right: attributing it to a placeholder would put a
            // synthetic actor in the diversity term, and diversity is exactly
            // what stops a single repeated observer reaching high coherence.
            None => tracing::warn!(
                run_id = %job.run_id,
                step_id = %job.step_id,
                "no owner for this run; routing signal dropped rather than misattributed"
            ),
        }
    }

    let (state_name, reason) = match verdict {
        Some(cortex_core::verification::Verdict::Verified) => {
            ("succeeded", "required checks passed")
        }
        Some(cortex_core::verification::Verdict::Failed) => ("failed", "a required check failed"),
        Some(cortex_core::verification::Verdict::Inconclusive) => {
            ("inconclusive", "no verdict could be obtained")
        }
        Some(cortex_core::verification::Verdict::Unverified) => {
            ("inconclusive", "no executable ground truth was derivable")
        }
        // The driver declined: another process already owns this verdict, or
        // there was nothing frozen to verify. Either way the job is done and
        // must not be retried into one.
        None => ("succeeded", "nothing to verify for this step"),
    };

    seal(state, db, dispatcher_id, &job, state_name, reason).await;
    crate::metrics::record_time_to_verdict(started.elapsed().as_secs_f64(), state_name);
}

/// Refresh the claim on an interval until aborted.
fn spawn_heartbeat(
    state: &AppState,
    job_id: String,
    dispatcher_id: String,
) -> tokio::task::JoinHandle<()> {
    let db_path = crate::state::cortex_db_path(&state.workspace_dir);
    tokio::spawn(async move {
        let db = Database::open(&db_path);
        loop {
            tokio::time::sleep(HEARTBEAT_INTERVAL).await;
            if !db.heartbeat_verification_job(&job_id, &dispatcher_id) {
                // Our claim is gone. Say so — a dispatcher that keeps working
                // on a reclaimed job is producing a verdict nobody will accept.
                tracing::warn!(
                    job_id = %job_id,
                    "lost the claim on a job that is still running here"
                );
                return;
            }
        }
    })
}

/// Seal the job and record the recovery in the operations timeline.
async fn seal(
    state: &AppState,
    db: &Database,
    dispatcher_id: &str,
    job: &VerificationJob,
    state_name: &str,
    reason: &str,
) {
    if !db.finish_verification_job(&job.job_id, dispatcher_id, state_name, Some(reason)) {
        // The claim moved while we worked. The verdict belongs to whoever holds
        // it now, and writing ours anyway is the double-verdict this whole
        // mechanism exists to prevent.
        tracing::warn!(
            job_id = %job.job_id,
            "could not seal the job; the claim was reclaimed while it ran"
        );
        return;
    }

    tracing::info!(
        job_id = %job.job_id,
        step_id = %job.step_id,
        state = state_name,
        attempt = job.attempt_count,
        "verification job sealed"
    );

    // A retried job that eventually succeeds is correct behaviour that nobody
    // can see unless it is written down.
    if job.attempt_count > 1 {
        db.record_step_operations_event(
            &job.step_id,
            "verification.recovered",
            &serde_json::json!({
                "job_id": job.job_id,
                "attempt": job.attempt_count,
                "state": state_name,
                "reason": reason,
                "source": "cortex_independent",
            }),
        );
    }

    let _ = state;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_node_must_be_asserted_not_assumed() {
        // Two dispatchers on a Mutex<Connection> double-dispatch. An unset
        // variable is not a promise, so it is a refusal.
        let saved = std::env::var("CORTEX_SINGLE_NODE").ok();

        std::env::remove_var("CORTEX_SINGLE_NODE");
        let err = assert_single_node().expect_err("an unset variable must not start a dispatcher");
        assert!(
            err.contains("CORTEX_SINGLE_NODE"),
            "and must say what to set"
        );

        std::env::set_var("CORTEX_SINGLE_NODE", "0");
        assert!(
            assert_single_node().is_err(),
            "and must not accept a denial"
        );

        std::env::set_var("CORTEX_SINGLE_NODE", "1");
        assert!(assert_single_node().is_ok());

        match saved {
            Some(value) => std::env::set_var("CORTEX_SINGLE_NODE", value),
            None => std::env::remove_var("CORTEX_SINGLE_NODE"),
        }
    }

    #[test]
    fn the_heartbeat_fits_inside_the_lease() {
        // A heartbeat slower than the lease loses claims for no reason, and the
        // symptom is verifications that mysteriously run twice.
        assert!(
            (HEARTBEAT_INTERVAL.as_millis() as i64) < VERIFICATION_LEASE_MS,
            "a heartbeat must refresh a claim before it expires"
        );
    }
}
