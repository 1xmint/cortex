//! Storage trait — abstracts core database operations so backends
//! (SQLite today, PostgreSQL later) can be swapped without touching callers.
//!
//! Only the methods used by the scheduler, WebSocket handler, and key routes
//! are included here.  The full `Database` struct retains all ~100 methods;
//! consumers that need SQLite-specific helpers can still use `Database` directly.

use crate::db::VerifierReport;
use cortex_core::task::TaskContract;

/// Core storage operations required by the Cortex scheduler and API routes.
///
/// Implementors must be `Send + Sync` so the trait object can live inside
/// `Arc<AppState>` and be shared across Tokio tasks.
pub trait Storage: Send + Sync {
    // ── Runs ───────────────────────────────────────────────────────────

    /// Create a new run, returning the generated run ID.
    fn create_run(&self, user_id: &str, goal: &str, profile: &str, file_paths: &[String])
        -> String;

    /// Retrieve the goal text for a run.
    fn get_run_goal(&self, run_id: &str) -> Option<String>;

    /// Retrieve the user who owns a run.
    fn get_run_user_id(&self, run_id: &str) -> Option<String>;

    /// Check whether `user_id` owns `run_id`.
    fn verify_run_owner(&self, run_id: &str, user_id: &str) -> bool;

    /// Transition a run to a new status, optionally recording a failure reason.
    /// Returns `true` if a row was updated.
    fn update_run_status(&self, run_id: &str, status: &str, reason: Option<&str>) -> bool;

    /// Return IDs of runs in `planning` or `running` status.
    fn get_active_run_ids(&self) -> Vec<String>;

    /// List runs for a user (newest first), paginated.
    fn list_user_runs(&self, user_id: &str, limit: usize, offset: usize) -> Vec<serde_json::Value>;

    // ── Steps ──────────────────────────────────────────────────────────

    /// Create a step with a caller-supplied ID (used by the planner).
    fn create_step_with_id(
        &self,
        id: &str,
        run_id: &str,
        kind: &str,
        work_kind: &str,
        tier: &str,
        risk: &str,
        objective: &str,
        created_at: i64,
    );

    /// Get (kind, work_kind, tier, risk, objective) for a step.
    fn get_step_details(&self, step_id: &str) -> Option<(String, String, String, String, String)>;

    /// Get `(step_id, status)` for every step in a run.
    fn get_all_step_statuses(&self, run_id: &str) -> Vec<(String, String)>;

    /// Find steps that are ready to dispatch (all dependencies satisfied, not
    /// blocked by `earliest_dispatch_at`).
    fn find_ready_steps(&self, run_id: &str) -> Vec<String>;

    /// Attempt to lease a step to a worker.  Returns `Some(lease_gen)` on success,
    /// `None` if the CAS failed (step not in leasable state).
    fn lease_step(&self, step_id: &str, worker_id: &str, deadline: i64) -> Option<i64>;

    /// Reverse a lease (e.g. when the worker channel fails after leasing).
    /// Returns `true` if the step was successfully unleased.
    fn unlease_step(&self, step_id: &str, lease_gen: i64) -> bool;

    /// Record that a worker handed back a commit.  Returns `true` if a row was
    /// updated.
    ///
    /// The step becomes `delivered`, not succeeded — a worker's report is a
    /// diagnostic, and only our own verifier can produce `verified`.
    #[allow(clippy::too_many_arguments)]
    fn deliver_step(
        &self,
        step_id: &str,
        attempt_id: &str,
        lease_gen: i64,
        summary: Option<&str>,
        files: Option<&str>,
        base: Option<&str>,
        head: Option<&str>,
    ) -> bool;

    /// Hand a delivered step to our own verifier.
    ///
    /// The durable job is enqueued in the same transaction, so this trait
    /// deliberately has no separate enqueue: a job without a state, or a state
    /// without a job, is the failure it exists to make unrepresentable.
    fn begin_verifying_step(
        &self,
        step_id: &str,
        attempt_id: &str,
        lease_gen: i64,
        job: Option<crate::db::VerificationEnqueue<'_>>,
    ) -> bool;

    /// Seal a verdict our runner produced: `verified`, `failed`, or
    /// `inconclusive`.
    fn record_verification_outcome(
        &self,
        step_id: &str,
        attempt_id: &str,
        lease_gen: i64,
        state: &str,
        reason: Option<&str>,
    ) -> bool;

    /// Record that the delivery never happened — the sandbox or the harness
    /// failed before the step could produce a tree.
    fn record_execution_failure(
        &self,
        step_id: &str,
        attempt_id: &str,
        lease_gen: i64,
        reason: &str,
    ) -> bool;

    /// Mark a leased/running step as failed.  Returns `true` if a row was updated.
    fn fail_step(&self, step_id: &str, lease_gen: i64, error: &str, kind: Option<&str>) -> bool;

    /// Preserve rejected completion evidence on a failed step for heal context and UI inspection.
    fn record_failed_step_output(
        &self,
        step_id: &str,
        lease_gen: i64,
        summary: Option<&str>,
        files: Option<&str>,
        base: Option<&str>,
        head: Option<&str>,
    ) -> bool;

    /// Check that `worker_id` currently holds the lease on `step_id`.
    fn verify_step_worker(&self, step_id: &str, worker_id: &str) -> bool;

    /// Persist verifier output or a placeholder report for a worker completion.
    fn record_verifier_report(
        &self,
        step_id: &str,
        run_id: &str,
        lease_gen: i64,
        worker_id: Option<&str>,
        verifier: &str,
        status: &str,
        verdict: &str,
        evidence_json: &str,
    ) -> Option<String>;

    /// Return the latest verifier report for a step, if any.
    fn get_latest_verifier_report(&self, step_id: &str) -> Option<VerifierReport>;

    /// Persist the immutable work contract dispatched for a step lease.
    fn record_step_work_contract(
        &self,
        step_id: &str,
        run_id: &str,
        lease_gen: i64,
        contract: &TaskContract,
    ) -> bool;

    /// Return the work contract for a specific step lease, if one was recorded.
    fn get_step_work_contract(&self, step_id: &str, lease_gen: i64) -> Option<TaskContract>;

    /// Return the latest work contract for a step, if one was recorded.
    fn get_latest_step_work_contract(&self, step_id: &str) -> Option<TaskContract>;

    // ── Usage ──────────────────────────────────────────────────────────

    /// Record a usage event (tokens consumed by a step execution).
    fn record_usage(
        &self,
        user_id: &str,
        provider: &str,
        tier: &str,
        model: &str,
        worker_id: Option<&str>,
        tokens_in: Option<i64>,
        tokens_out: Option<i64>,
        duration_ms: Option<i64>,
    );

    /// Get a user's estimated cost for the current UTC day, plus step count.
    fn get_user_daily_cost(&self, user_id: &str) -> (f64, i64);

    /// Get a user's estimated cost for the current UTC month.
    fn get_user_monthly_cost(&self, user_id: &str) -> f64;

    // ── Pressure ───────────────────────────────────────────────────────

    /// Compute per-(provider, tier) pressure within `window_ms` using
    /// exponential time-decay.  Returns `(provider, tier, weighted_tokens)`.
    fn pressure_for_user(&self, user_id: &str, window_ms: i64) -> Vec<(String, String, i64)>;
}

// ── SQLite implementation ──────────────────────────────────────────────

use crate::db::Database;

impl Storage for Database {
    fn create_run(
        &self,
        user_id: &str,
        goal: &str,
        profile: &str,
        file_paths: &[String],
    ) -> String {
        Database::create_run(self, user_id, goal, profile, file_paths)
    }

    fn get_run_goal(&self, run_id: &str) -> Option<String> {
        Database::get_run_goal(self, run_id)
    }

    fn get_run_user_id(&self, run_id: &str) -> Option<String> {
        Database::get_run_user_id(self, run_id)
    }

    fn verify_run_owner(&self, run_id: &str, user_id: &str) -> bool {
        Database::verify_run_owner(self, run_id, user_id)
    }

    fn update_run_status(&self, run_id: &str, status: &str, reason: Option<&str>) -> bool {
        Database::update_run_status(self, run_id, status, reason)
    }

    fn get_active_run_ids(&self) -> Vec<String> {
        Database::get_active_run_ids(self)
    }

    fn list_user_runs(&self, user_id: &str, limit: usize, offset: usize) -> Vec<serde_json::Value> {
        Database::list_user_runs(self, user_id, limit, offset)
    }

    fn create_step_with_id(
        &self,
        id: &str,
        run_id: &str,
        kind: &str,
        work_kind: &str,
        tier: &str,
        risk: &str,
        objective: &str,
        created_at: i64,
    ) {
        Database::create_step_with_id(
            self, id, run_id, kind, work_kind, tier, risk, objective, created_at,
        )
    }

    fn get_step_details(&self, step_id: &str) -> Option<(String, String, String, String, String)> {
        Database::get_step_details(self, step_id)
    }

    fn get_all_step_statuses(&self, run_id: &str) -> Vec<(String, String)> {
        Database::get_all_step_statuses(self, run_id)
    }

    fn find_ready_steps(&self, run_id: &str) -> Vec<String> {
        Database::find_ready_steps(self, run_id)
    }

    fn lease_step(&self, step_id: &str, worker_id: &str, deadline: i64) -> Option<i64> {
        Database::lease_step(self, step_id, worker_id, deadline)
    }

    fn unlease_step(&self, step_id: &str, lease_gen: i64) -> bool {
        Database::unlease_step(self, step_id, lease_gen)
    }

    fn deliver_step(
        &self,
        step_id: &str,
        attempt_id: &str,
        lease_gen: i64,
        summary: Option<&str>,
        files: Option<&str>,
        base: Option<&str>,
        head: Option<&str>,
    ) -> bool {
        Database::deliver_step(
            self, step_id, attempt_id, lease_gen, summary, files, base, head,
        )
    }

    fn begin_verifying_step(
        &self,
        step_id: &str,
        attempt_id: &str,
        lease_gen: i64,
        job: Option<crate::db::VerificationEnqueue<'_>>,
    ) -> bool {
        Database::begin_verifying_step(self, step_id, attempt_id, lease_gen, job)
    }

    fn record_verification_outcome(
        &self,
        step_id: &str,
        attempt_id: &str,
        lease_gen: i64,
        state: &str,
        reason: Option<&str>,
    ) -> bool {
        Database::record_verification_outcome(self, step_id, attempt_id, lease_gen, state, reason)
    }

    fn record_execution_failure(
        &self,
        step_id: &str,
        attempt_id: &str,
        lease_gen: i64,
        reason: &str,
    ) -> bool {
        Database::record_execution_failure(self, step_id, attempt_id, lease_gen, reason)
    }

    fn fail_step(&self, step_id: &str, lease_gen: i64, error: &str, kind: Option<&str>) -> bool {
        Database::fail_step(self, step_id, lease_gen, error, kind)
    }

    fn record_failed_step_output(
        &self,
        step_id: &str,
        lease_gen: i64,
        summary: Option<&str>,
        files: Option<&str>,
        base: Option<&str>,
        head: Option<&str>,
    ) -> bool {
        Database::record_failed_step_output(self, step_id, lease_gen, summary, files, base, head)
    }

    fn verify_step_worker(&self, step_id: &str, worker_id: &str) -> bool {
        Database::verify_step_worker(self, step_id, worker_id)
    }

    fn record_verifier_report(
        &self,
        step_id: &str,
        run_id: &str,
        lease_gen: i64,
        worker_id: Option<&str>,
        verifier: &str,
        status: &str,
        verdict: &str,
        evidence_json: &str,
    ) -> Option<String> {
        Database::record_verifier_report(
            self,
            step_id,
            run_id,
            lease_gen,
            worker_id,
            verifier,
            status,
            verdict,
            evidence_json,
        )
    }

    fn get_latest_verifier_report(&self, step_id: &str) -> Option<VerifierReport> {
        Database::get_latest_verifier_report(self, step_id)
    }

    fn record_step_work_contract(
        &self,
        step_id: &str,
        run_id: &str,
        lease_gen: i64,
        contract: &TaskContract,
    ) -> bool {
        Database::record_step_work_contract(self, step_id, run_id, lease_gen, contract)
    }

    fn get_step_work_contract(&self, step_id: &str, lease_gen: i64) -> Option<TaskContract> {
        Database::get_step_work_contract(self, step_id, lease_gen)
    }

    fn get_latest_step_work_contract(&self, step_id: &str) -> Option<TaskContract> {
        Database::get_latest_step_work_contract(self, step_id)
    }

    fn record_usage(
        &self,
        user_id: &str,
        provider: &str,
        tier: &str,
        model: &str,
        worker_id: Option<&str>,
        tokens_in: Option<i64>,
        tokens_out: Option<i64>,
        duration_ms: Option<i64>,
    ) {
        Database::record_usage(
            self,
            user_id,
            provider,
            tier,
            model,
            worker_id,
            tokens_in,
            tokens_out,
            duration_ms,
        )
    }

    fn get_user_daily_cost(&self, user_id: &str) -> (f64, i64) {
        Database::get_user_daily_cost(self, user_id)
    }

    fn get_user_monthly_cost(&self, user_id: &str) -> f64 {
        Database::get_user_monthly_cost(self, user_id)
    }

    fn pressure_for_user(&self, user_id: &str, window_ms: i64) -> Vec<(String, String, i64)> {
        Database::pressure_for_user(self, user_id, window_ms)
    }
}
