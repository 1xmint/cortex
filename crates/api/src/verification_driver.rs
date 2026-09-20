//! Run the frozen checks against a delivered tree and bind the verdict to the
//! ledger.
//!
//! This is the money path. Everything here exists to make two guarantees hold
//! at once: a verdict is produced by *us* executing checks rather than by a
//! worker reporting on itself, and the charge or refund that follows fires
//! exactly once no matter how many times this runs.
//!
//! The sequence, and why it is in this order:
//!
//! 1. Load the specs frozen at dispatch. Deriving them now would let the task
//!    influence its own exam.
//! 2. Claim the attempt. `UNIQUE(run_id, step_id, attempt)` is a CAS — losing
//!    it means another process owns this verdict, and the correct response is
//!    to stop, not to retry.
//! 3. Snapshot the delivered commit into a fresh checkout. Never the worker's
//!    working directory: the moment a worker can influence its own verdict the
//!    product claim is void.
//! 4. Execute, record, compute the verdict, seal it.
//! 5. Bind to the ledger, with the billing state derived from the ledger
//!    itself rather than from a column that could drift from the money.
//!
//! See `cortex/plan/VERIFIER.md` and `cortex/plan/V3-LAUNCH-SPEC.md`.

use std::path::{Path, PathBuf};

use cortex_core::billing_binding::{self, BillingEffect, BillingState};
use cortex_core::check_derivation::EcosystemFacts;
use cortex_core::diff_surface::{self, ClassOutcome, VerdictClass};
use cortex_core::verification::{
    CheckExecution, CheckOutcome, CheckRunner, CheckSpec, TreeSnapshot, Verdict,
};

use crate::db::Database;

/// How many extra passes a check gets when the *runner* fails, before the
/// check is recorded as `NotExecuted`. Infra problems cost us time, not the
/// customer money — but they cannot retry forever either.
const RUNNER_RETRIES: usize = 2;

/// What the completion handler knows once a worker has delivered.
#[derive(Debug, Clone)]
pub struct DeliveryFacts {
    pub run_id: String,
    pub step_id: String,
    /// The attempt this delivery belongs to. Carried so the verdict can be
    /// written against the same lifecycle row the delivery opened.
    pub attempt_id: String,
    pub attempt: i64,
    /// The repository the worker delivered into. Used only as the source for
    /// a detached checkout — never mounted directly.
    pub workspace_dir: PathBuf,
    /// The commit the worker delivered. This is what gets graded.
    pub head_commit: String,
    /// Credits quoted for this step. `None` means no quote was reachable, in
    /// which case the verdict is still recorded and the ledger is left alone.
    /// Inventing a price is never correct — pricing is a product decision.
    pub quoted_credits: Option<i64>,
}

/// The image checks execute in, pinned by digest in production.
///
/// "It passed" is only reproducible if *where* it passed is pinned, so this is
/// recorded on every execution. Overridable per deployment because Phase A
/// (containers) and Phase B (microVMs) use different images.
/// Which set of runner rules was in force when a job was enqueued.
///
/// Recorded on the job so a receipt can say what the rules *were* rather than
/// what they are now. Bumped whenever the runner's behaviour changes in a way
/// that would make two verdicts incomparable.
pub const RUNNER_POLICY_VERSION: &str = "runner-policy-1";

pub fn runner_image() -> String {
    std::env::var("CORTEX_RUNNER_IMAGE").unwrap_or_else(|_| "cortex/runner:phase-a".to_string())
}

/// A detached checkout of the delivered commit, removed on drop.
struct TreeCheckout {
    workspace_dir: PathBuf,
    path: PathBuf,
}

impl TreeCheckout {
    /// `git worktree add --detach` at the delivered commit. Cheap (it shares
    /// the object store) and, unlike a copy, guaranteed to be exactly the
    /// delivered tree with nothing the worker left lying around.
    fn create(workspace_dir: &Path, commit: &str) -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!("cortex-verify-{}", uuid::Uuid::new_v4()));
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(workspace_dir)
            .arg("worktree")
            .arg("add")
            .arg("--detach")
            .arg(&path)
            .arg(commit)
            .output()
            .map_err(|e| format!("could not run git worktree add: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "git worktree add failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(Self {
            workspace_dir: workspace_dir.to_path_buf(),
            path,
        })
    }
}

impl Drop for TreeCheckout {
    fn drop(&mut self) {
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.workspace_dir)
            .arg("worktree")
            .arg("remove")
            .arg("--force")
            .arg(&self.path)
            .output();
    }
}

/// Verify one delivery. Returns the verdict, or `None` if there was nothing to
/// do — the attempt was already claimed, or the tree could not be snapshotted.
///
/// Generic over the runner rather than taking `dyn CheckRunner`: the trait uses
/// RPITIT and is not object-safe.
pub async fn verify_delivery<R: CheckRunner>(
    db: &Database,
    runner: &R,
    facts: &DeliveryFacts,
) -> Option<Verdict> {
    let specs = db.load_check_specs(&facts.run_id, &facts.step_id);

    // Nothing was frozen for this step, so it is not a step this machinery
    // attaches to — read-only work (Search/Think/Review/Gate) never has specs
    // frozen for it. Claiming a verification here would mint a verdict of
    // `Unverified`, and `Unverified` *is* billable, so a Think step would
    // charge. Verification attaches to steps that change trees and to nothing
    // else (VERIFIER.md, "what not to do").
    if specs.is_empty() {
        tracing::debug!(
            run_id = %facts.run_id,
            step_id = %facts.step_id,
            "no frozen checks for this step; nothing to verify"
        );
        return None;
    }

    let verification_id = db.claim_verification(
        &facts.run_id,
        &facts.step_id,
        facts.attempt,
        &facts.head_commit,
        runner.runner_image(),
    )?;

    // Phase 27.2. Freezing the specs stopped a task rewriting its own *argv*.
    // It never stopped it rewriting what that argv reads, and the checkout
    // below is a clean checkout of the delivered commit -- the very commit
    // whose test files the agent controls.
    //
    // So ask what the delivery touched before grading it. A plan that declared
    // `strong` and then edited the exam has contradicted its own contract.
    // That is not a verdict, and the run does not get to choose the weaker
    // class after the fact.
    match exam_integrity(db, facts) {
        ExamIntegrity::Intact | ExamIntegrity::PermittedAuthoredWork => {}
        ExamIntegrity::ModifiedExam { paths } => {
            let detail = format!(
                "exam integrity: modified protected exam surface: {}",
                paths.join(", ")
            );
            tracing::error!(
                run_id = %facts.run_id,
                step_id = %facts.step_id,
                exam_paths = ?paths,
                "declared verdict_class=strong and then edited the exam; inconclusive"
            );
            finish_inconclusive_without_grading(db, &verification_id, facts, &detail);
            return Some(Verdict::Inconclusive);
        }
        ExamIntegrity::Unknown { reason } => {
            let detail = format!("exam integrity unknown: {reason}");
            tracing::error!(
                run_id = %facts.run_id,
                step_id = %facts.step_id,
                reason = %reason,
                "could not establish frozen exam integrity; grading refused"
            );
            finish_inconclusive_without_grading(db, &verification_id, facts, &detail);
            return Some(Verdict::Inconclusive);
        }
    }

    let checkout = match TreeCheckout::create(&facts.workspace_dir, &facts.head_commit) {
        Ok(c) => c,
        Err(e) => {
            // We could not produce a tree to grade. That is our failure, so it
            // is Inconclusive: no charge, no refund, and an operator hears
            // about it. Leaving the row 'pending' would strand the attempt.
            tracing::error!(
                run_id = %facts.run_id,
                step_id = %facts.step_id,
                error = %e,
                "could not snapshot the delivered tree; verification is inconclusive"
            );
            let _ = db.finish_verification(&verification_id, Verdict::Inconclusive);
            project_verdict(db, facts, Verdict::Inconclusive, Some(&e));
            return Some(Verdict::Inconclusive);
        }
    };

    let tree = TreeSnapshot {
        tree_hash: facts.head_commit.clone(),
        path: checkout.path.to_string_lossy().to_string(),
    };

    let mut executions: Vec<CheckExecution> = Vec::with_capacity(specs.len());
    for spec in &specs {
        let execution = run_with_retries(runner, &tree, spec).await;
        if let Err(e) = db.record_check_execution(&verification_id, spec, &execution) {
            tracing::error!(
                verification_id = %verification_id,
                spec_id = %spec.id,
                error = %e,
                "failed to record a check execution"
            );
        }
        executions.push(execution);
    }

    let report = cortex_core::verification::compute_verdict(&specs, &executions);
    tracing::info!(
        run_id = %facts.run_id,
        step_id = %facts.step_id,
        verdict = ?report.verdict,
        required_passed = report.required_passed,
        required_total = report.required_total,
        "verification complete"
    );

    finish_and_bill(db, &verification_id, report.verdict, facts).await;
    project_verdict(db, facts, report.verdict, None);
    Some(report.verdict)
}

/// Seal an integrity refusal without reaching either the runner or billing.
fn finish_inconclusive_without_grading(
    db: &Database,
    verification_id: &str,
    facts: &DeliveryFacts,
    detail: &str,
) {
    let _ = db.finish_verification(verification_id, Verdict::Inconclusive);
    project_verdict(db, facts, Verdict::Inconclusive, Some(detail));
}

/// Move the step itself to the state the verdict implies.
///
/// This is the line that makes verification mean something. Before it existed
/// the verdict was recorded beside the step and the step had already been
/// marked succeeded by the worker's own report; the grade was written on a
/// paper nobody read.
///
/// A verdict for a superseded attempt is a no-op — the transition carries the
/// `lease_gen` CAS, so a verifier that finishes after its step was re-leased
/// cannot move the live attempt.
fn project_verdict(db: &Database, facts: &DeliveryFacts, verdict: Verdict, detail: Option<&str>) {
    let state = match verdict {
        Verdict::Verified => "verified",
        Verdict::Failed => "failed",
        // Unverified means no executable ground truth existed, so nothing was
        // proven. It is not a pass. It stays visible as an unanswered question
        // rather than becoming a badge.
        Verdict::Unverified | Verdict::Inconclusive => "inconclusive",
    };
    let applied = db.record_verification_outcome(
        &facts.step_id,
        &facts.attempt_id,
        facts.attempt,
        state,
        detail,
    );
    if !applied {
        tracing::warn!(
            run_id = %facts.run_id,
            step_id = %facts.step_id,
            attempt = facts.attempt,
            state,
            "verdict did not move the step — the attempt was superseded or was not verifying"
        );
    }
}

/// Run one check, retrying only when the *runner* failed.
///
/// A check that runs and fails is a verdict about the work and is returned
/// immediately. Only `Err` — our infrastructure failing — is retried, and only
/// a bounded number of times, after which the check is `NotExecuted` and the
/// verdict becomes `Inconclusive` rather than a charge.
async fn run_with_retries<R: CheckRunner>(
    runner: &R,
    tree: &TreeSnapshot,
    spec: &CheckSpec,
) -> CheckExecution {
    let mut last_error = String::new();
    for attempt in 0..=RUNNER_RETRIES {
        match runner.run(tree, spec).await {
            Ok(execution) => return execution,
            Err(e) => {
                last_error = e.to_string();
                tracing::warn!(
                    spec_id = %spec.id,
                    attempt,
                    error = %last_error,
                    "check runner failed; retrying"
                );
            }
        }
    }

    tracing::error!(
        spec_id = %spec.id,
        error = %last_error,
        "check could not be executed after retries; verdict will be inconclusive"
    );
    CheckExecution {
        spec_id: spec.id.clone(),
        exit_code: None,
        outcome: CheckOutcome::NotExecuted,
        duration_ms: 0,
        output_digest: String::new(),
        output_tail: format!("runner error: {last_error}"),
        runner_image: runner.runner_image().to_string(),
    }
}

/// Seal the verdict, then move money if the verdict says to.
async fn finish_and_bill(
    db: &Database,
    verification_id: &str,
    verdict: Verdict,
    facts: &DeliveryFacts,
) {
    if let Err(e) = db.finish_verification(verification_id, verdict) {
        tracing::error!(verification_id, error = %e, "failed to seal verdict");
        return;
    }

    if !verdict.has_billing_effect() {
        // Inconclusive. Our problem, so it costs us time and not the
        // customer's money — but somebody should look at it.
        tracing::error!(
            run_id = %facts.run_id,
            step_id = %facts.step_id,
            "verification inconclusive; no ledger write, operator attention needed"
        );
        return;
    }

    let Some(user_id) = db.get_run_user_id(&facts.run_id) else {
        tracing::warn!(run_id = %facts.run_id, "no owner for run; skipping ledger write");
        return;
    };

    let charge_key = billing_binding::ChargeKey::for_verification(verification_id);
    let refund_key = billing_binding::RefundKey::for_verification(verification_id);

    // Derive the billing state from the ledger, not from a status column. The
    // ledger is where the money actually is, so it cannot disagree with itself.
    let state = if db.ledger_has_key(refund_key.as_str()) {
        BillingState::Refunded
    } else if db.ledger_has_key(charge_key.as_str()) {
        BillingState::Charged
    } else {
        BillingState::Unbilled
    };

    let reason = billing_binding::ledger_reason(verdict).unwrap_or("verdict");

    match billing_binding::billing_effect(verdict, state, verification_id) {
        BillingEffect::Charge { idempotency_key } => {
            let Some(amount) = facts.quoted_credits else {
                // Never invent a price. The verdict still stands; the charge
                // simply does not happen, and that is visible.
                tracing::warn!(
                    run_id = %facts.run_id,
                    step_id = %facts.step_id,
                    "verdict is billable but no quoted price was reachable; no charge written"
                );
                return;
            };
            match db.deduct_credits(&user_id, amount, reason, &idempotency_key) {
                Ok(_) => tracing::info!(
                    run_id = %facts.run_id,
                    verification_id,
                    amount,
                    "charged for a verified outcome"
                ),
                Err(e) => tracing::error!(
                    run_id = %facts.run_id,
                    verification_id,
                    error = %e,
                    "charge failed"
                ),
            }
        }
        BillingEffect::Refund { idempotency_key } => {
            match db.refund_credits(&user_id, &charge_key, &idempotency_key, reason) {
                Ok(_) => tracing::info!(
                    run_id = %facts.run_id,
                    verification_id,
                    "refunded a failed outcome"
                ),
                Err(e) => tracing::error!(
                    run_id = %facts.run_id,
                    verification_id,
                    error = %e,
                    "refund failed"
                ),
            }
        }
        BillingEffect::None => {
            tracing::debug!(verification_id, ?verdict, ?state, "no ledger effect");
        }
    }
}

/// What is known about the frozen exam before any grading process starts.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ExamIntegrity {
    Intact,
    PermittedAuthoredWork,
    ModifiedExam { paths: Vec<String> },
    Unknown { reason: String },
}

fn exam_integrity(db: &Database, facts: &DeliveryFacts) -> ExamIntegrity {
    let contract = match db.read_step_work_contract(&facts.step_id, facts.attempt) {
        Ok(Some(contract)) => contract,
        Ok(None) => {
            return ExamIntegrity::Unknown {
                reason: "missing work contract".to_string(),
            };
        }
        Err(err) => {
            return ExamIntegrity::Unknown {
                reason: format!("unreadable work contract: {err}"),
            };
        }
    };
    if contract.verdict_class != VerdictClass::Strong {
        return ExamIntegrity::PermittedAuthoredWork;
    }

    let Some(base) = contract.expected_base_commit.as_deref() else {
        return ExamIntegrity::Unknown {
            reason: "strong contract is missing its required base commit".to_string(),
        };
    };
    let changed = match changed_paths(&facts.workspace_dir, base, &facts.head_commit) {
        Ok(changed) => changed,
        Err(err) => {
            return ExamIntegrity::Unknown {
                reason: format!("diff inspection failed: {err}"),
            };
        }
    };
    let partition = diff_surface::partition(&changed, &ecosystem_facts_for(&facts.workspace_dir));

    match diff_surface::resolve_class(VerdictClass::Strong, &partition) {
        ClassOutcome::StrongContractBroken { exam_paths } => {
            ExamIntegrity::ModifiedExam { paths: exam_paths }
        }
        ClassOutcome::Honoured(_) => ExamIntegrity::Intact,
    }
}

/// `git diff --name-only base..head`, against the workspace.
///
/// Failure is an explicit unknown-integrity result. It is neither evidence of
/// tampering nor permission to grade.
fn changed_paths(workspace_dir: &Path, base: &str, head: &str) -> Result<Vec<String>, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace_dir)
        .arg("diff")
        .arg("--name-only")
        .arg(format!("{base}..{head}"))
        .output()
        .map_err(|err| format!("could not run git diff: {err}"))?;
    if !out.status.success() {
        return Err(format!(
            "git diff {base}..{head} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// What ecosystems the workspace root declares, for path classification.
fn ecosystem_facts_for(workspace_dir: &Path) -> EcosystemFacts {
    EcosystemFacts {
        has_cargo_manifest: workspace_dir.join("Cargo.toml").exists(),
        has_package_json: workspace_dir.join("package.json").exists(),
        npm_scripts: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_core::verification::{CheckSource, RunnerError};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// A runner we control, so the driver's sequencing can be tested without
    /// Docker. Generic bound is `R: CheckRunner`, so this substitutes freely.
    struct ScriptedRunner {
        /// Outcome per call, popped in order. `Err` simulates infra failure.
        script: Mutex<Vec<Result<CheckOutcome, RunnerError>>>,
        calls: AtomicUsize,
        image: String,
    }

    impl ScriptedRunner {
        fn new(script: Vec<Result<CheckOutcome, RunnerError>>) -> Self {
            let mut reversed = script;
            reversed.reverse();
            Self {
                script: Mutex::new(reversed),
                calls: AtomicUsize::new(0),
                image: "test/runner@sha256:0".to_string(),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl CheckRunner for ScriptedRunner {
        async fn run(
            &self,
            _tree: &TreeSnapshot,
            check: &CheckSpec,
        ) -> Result<CheckExecution, RunnerError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let next = self.script.lock().unwrap().pop();
            match next {
                Some(Ok(outcome)) => Ok(CheckExecution {
                    spec_id: check.id.clone(),
                    exit_code: Some(if outcome == CheckOutcome::Passed {
                        0
                    } else {
                        1
                    }),
                    outcome,
                    duration_ms: 5,
                    output_digest: "sha256:test".to_string(),
                    output_tail: String::new(),
                    runner_image: self.image.clone(),
                }),
                Some(Err(e)) => Err(e),
                None => Ok(CheckExecution {
                    spec_id: check.id.clone(),
                    exit_code: Some(0),
                    outcome: CheckOutcome::Passed,
                    duration_ms: 5,
                    output_digest: "sha256:test".to_string(),
                    output_tail: String::new(),
                    runner_image: self.image.clone(),
                }),
            }
        }

        fn runner_image(&self) -> &str {
            &self.image
        }
    }

    fn spec(id: &str) -> CheckSpec {
        CheckSpec {
            id: id.to_string(),
            source: CheckSource::Contract,
            command: vec!["true".to_string()],
            timeout_secs: 5,
            required: true,
        }
    }

    #[tokio::test]
    async fn runner_failure_is_retried_then_recorded_as_not_executed() {
        let runner = ScriptedRunner::new(vec![
            Err(RunnerError::ExecutionFailed("boom".into())),
            Err(RunnerError::ExecutionFailed("boom".into())),
            Err(RunnerError::ExecutionFailed("boom".into())),
        ]);
        let tree = TreeSnapshot {
            tree_hash: "abc".to_string(),
            path: ".".to_string(),
        };

        let execution = run_with_retries(&runner, &tree, &spec("c1")).await;
        assert_eq!(
            execution.outcome,
            CheckOutcome::NotExecuted,
            "exhausted retries must not become a pass or a fail"
        );
        assert!(execution.exit_code.is_none());
    }

    #[tokio::test]
    async fn a_check_that_runs_and_fails_is_not_retried() {
        // One Failed, then entries that would pass. If the driver retried a
        // real failure it would come back Passed, which would be a charge for
        // work that did not meet its contract.
        let runner = ScriptedRunner::new(vec![
            Ok(CheckOutcome::Failed),
            Ok(CheckOutcome::Passed),
            Ok(CheckOutcome::Passed),
        ]);
        let tree = TreeSnapshot {
            tree_hash: "abc".to_string(),
            path: ".".to_string(),
        };

        let execution = run_with_retries(&runner, &tree, &spec("c1")).await;
        assert_eq!(execution.outcome, CheckOutcome::Failed);
    }

    // --- End-to-end: the whole sequence against a real git repository ---

    fn test_db() -> crate::db::Database {
        let dir = tempfile::tempdir().unwrap().keep();
        crate::db::Database::open(&dir.join("cortex.sqlite"))
    }

    /// A real repository with one commit, so `git worktree add --detach` has
    /// something to check out. The tree snapshot is deliberately not mockable —
    /// shelling out to git is what guarantees the checks run against the
    /// delivered commit rather than against a worker's directory.
    fn repo_with_one_commit() -> (std::path::PathBuf, String) {
        let dir = tempfile::tempdir().unwrap().keep();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .expect("git runs");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        git(&["init", "--initial-branch=main"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(dir.join("file.txt"), "delivered\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "delivered work"]);
        let head = git(&["rev-parse", "HEAD"]);
        (dir, head)
    }

    fn facts(dir: &std::path::Path, head: &str) -> DeliveryFacts {
        DeliveryFacts {
            run_id: "run-1".to_string(),
            step_id: "step-1".to_string(),
            attempt_id: "attempt-1".to_string(),
            attempt: 1,
            workspace_dir: dir.to_path_buf(),
            head_commit: head.to_string(),
            // No quote is reachable yet, so the ledger is deliberately untouched.
            quoted_credits: None,
        }
    }

    #[tokio::test]
    async fn a_step_with_no_frozen_checks_is_not_verified_at_all() {
        let db = test_db();
        let (dir, head) = repo_with_one_commit();

        let runner = ScriptedRunner::new(vec![]);
        let verdict = verify_delivery(&db, &runner, &facts(&dir, &head)).await;

        assert!(
            verdict.is_none(),
            "read-only work has no frozen specs and must not mint a verdict — \
             Unverified is billable, so a Think step would otherwise charge"
        );
        assert!(db.get_receipt("run-1", "step-1").is_none());
    }

    #[tokio::test]
    async fn passing_checks_produce_a_verified_receipt() {
        let db = test_db();
        let (dir, head) = repo_with_one_commit();
        let run_id = seed_run_and_step(&db, "step-1");
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            1,
            &contract_declaring(VerdictClass::Strong, &head),
        ));
        let specs = vec![spec("c1"), spec("c2")];
        db.save_check_specs(&run_id, "step-1", &specs).unwrap();

        let runner = ScriptedRunner::new(vec![Ok(CheckOutcome::Passed), Ok(CheckOutcome::Passed)]);
        let verdict =
            verify_delivery(&db, &runner, &facts_for(&dir, &head, &run_id, "step-1")).await;
        assert_eq!(verdict, Some(Verdict::Verified));

        let receipt = db.get_receipt(&run_id, "step-1").expect("receipt exists");
        assert_eq!(
            receipt.tree_hash, head,
            "the receipt pins the graded commit"
        );
        assert_eq!(receipt.executions.len(), 2);
        assert_eq!(receipt.gate.required_total, 2);
        assert_eq!(receipt.gate.required_passed, 2);
    }

    #[tokio::test]
    async fn one_failing_check_fails_the_verdict() {
        let db = test_db();
        let (dir, head) = repo_with_one_commit();
        let run_id = seed_run_and_step(&db, "step-1");
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            1,
            &contract_declaring(VerdictClass::Strong, &head),
        ));
        let specs = vec![spec("c1"), spec("c2")];
        db.save_check_specs(&run_id, "step-1", &specs).unwrap();

        let runner = ScriptedRunner::new(vec![Ok(CheckOutcome::Passed), Ok(CheckOutcome::Failed)]);
        let verdict =
            verify_delivery(&db, &runner, &facts_for(&dir, &head, &run_id, "step-1")).await;
        assert_eq!(verdict, Some(Verdict::Failed));

        let receipt = db.get_receipt(&run_id, "step-1").expect("receipt exists");
        assert!(receipt.gate.failed.contains(&"c2".to_string()));
    }

    #[tokio::test]
    async fn a_runner_that_cannot_execute_is_inconclusive_and_never_bills() {
        let db = test_db();
        let (dir, head) = repo_with_one_commit();
        let run_id = seed_run_and_step(&db, "step-1");
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            1,
            &contract_declaring(VerdictClass::Strong, &head),
        ));
        db.save_check_specs(&run_id, "step-1", &[spec("c1")])
            .unwrap();

        // Every attempt fails as infrastructure, exhausting the retries.
        let runner = ScriptedRunner::new(vec![
            Err(RunnerError::ExecutionFailed("no docker".into())),
            Err(RunnerError::ExecutionFailed("no docker".into())),
            Err(RunnerError::ExecutionFailed("no docker".into())),
        ]);
        let verdict =
            verify_delivery(&db, &runner, &facts_for(&dir, &head, &run_id, "step-1")).await;

        assert_eq!(
            verdict,
            Some(Verdict::Inconclusive),
            "our infrastructure failing must cost us time, not the customer money"
        );
        assert!(!Verdict::Inconclusive.has_billing_effect());
    }

    #[tokio::test]
    async fn the_second_verifier_to_reach_an_attempt_does_nothing() {
        let db = test_db();
        let (dir, head) = repo_with_one_commit();
        let run_id = seed_run_and_step(&db, "step-1");
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            1,
            &contract_declaring(VerdictClass::Strong, &head),
        ));
        db.save_check_specs(&run_id, "step-1", &[spec("c1")])
            .unwrap();
        let f = facts_for(&dir, &head, &run_id, "step-1");

        let first = verify_delivery(
            &db,
            &ScriptedRunner::new(vec![Ok(CheckOutcome::Passed)]),
            &f,
        )
        .await;
        assert_eq!(first, Some(Verdict::Verified));

        // A duplicate delivery for the same attempt: the CAS must lose, so the
        // ledger key derived from the verification id is minted exactly once.
        let second = verify_delivery(
            &db,
            &ScriptedRunner::new(vec![Ok(CheckOutcome::Failed)]),
            &f,
        )
        .await;
        assert!(
            second.is_none(),
            "the second claim must not produce a verdict"
        );

        let receipt = db.get_receipt(&run_id, "step-1").expect("receipt");
        assert_eq!(
            receipt.gate.verdict,
            Verdict::Verified,
            "the duplicate must not overwrite the first verdict"
        );
        assert_eq!(receipt.executions.len(), 1, "and must not add executions");
    }

    /// A repo with a base commit and a delivered commit editing `paths`.
    fn repo_with_base_and_delivery(paths: &[&str]) -> (std::path::PathBuf, String, String) {
        let dir = tempfile::tempdir().unwrap().keep();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .expect("git runs");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        git(&["init", "--initial-branch=main"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]
",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("src/lib.rs"),
            "pub fn a() {}
",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("tests")).unwrap();
        std::fs::write(
            dir.join("tests/e2e.rs"),
            "#[test] fn t() {}
",
        )
        .unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "base"]);
        let base = git(&["rev-parse", "HEAD"]);

        for p in paths {
            std::fs::write(
                dir.join(p),
                "// delivered edit
",
            )
            .unwrap();
        }
        git(&["add", "."]);
        git(&["commit", "-m", "delivered"]);
        let head = git(&["rev-parse", "HEAD"]);
        (dir, base, head)
    }

    /// Seed a real run and step, because `step_work_contracts` has foreign
    /// keys to both. Returns the generated run id.
    fn seed_run_and_step(db: &Database, step_id: &str) -> String {
        db.create_run_with_steps(
            "user-1",
            "do the thing",
            "balanced",
            &[],
            None,
            None,
            None,
            &[(
                step_id.to_string(),
                "execute".to_string(),
                "modify".to_string(),
                None,
                "execute".to_string(),
                "low".to_string(),
                "do the thing".to_string(),
                0,
            )],
            &[],
        )
    }

    fn facts_for(dir: &std::path::Path, head: &str, run_id: &str, step_id: &str) -> DeliveryFacts {
        DeliveryFacts {
            run_id: run_id.to_string(),
            step_id: step_id.to_string(),
            attempt_id: "attempt-1".to_string(),
            attempt: 1,
            workspace_dir: dir.to_path_buf(),
            head_commit: head.to_string(),
            quoted_credits: None,
        }
    }

    fn seed_verifying_step(db: &Database, step_id: &str, head: &str) -> (String, i64) {
        let run_id = seed_run_and_step(db, step_id);
        db.update_run_status(&run_id, "running", None);
        db.register_worker("worker-1", "user-1");
        let lease_gen = db
            .lease_step(step_id, "worker-1", i64::MAX)
            .expect("step leases");
        assert!(db.start_step(step_id, lease_gen));
        assert!(db.deliver_step(
            step_id,
            "attempt-1",
            lease_gen,
            None,
            None,
            None,
            Some(head),
        ));
        assert!(db.begin_verifying_step(step_id, "attempt-1", lease_gen, None));
        (run_id, lease_gen)
    }

    fn verification_reason(db: &Database, step_id: &str, lease_gen: i64) -> String {
        db.conn()
            .query_row(
                "SELECT terminal_reason FROM step_verification_state
                 WHERE step_id = ?1 AND attempt_id = 'attempt-1' AND lease_gen = ?2",
                rusqlite::params![step_id, lease_gen],
                |row| row.get::<_, String>(0),
            )
            .expect("terminal diagnostic persisted")
    }

    fn contract_declaring(class: VerdictClass, base: &str) -> cortex_core::task::TaskContract {
        let mut c = cortex_core::task::TaskContract::new(
            "do the thing".to_string(),
            cortex_core::provider::Tier::Execute,
            cortex_core::routing::RiskLevel::Low,
        );
        c.verdict_class = class;
        c.expected_base_commit = Some(base.to_string());
        c
    }

    #[test]
    fn a_strong_declaration_that_edits_the_exam_is_a_contract_violation() {
        // The case Phase 27.2 exists for. Dispatch promised the battery would
        // be the customer's own; delivery rewrote it. Downgrading to
        // `authored` here would make `strong` mean "strong unless it was
        // inconvenient", so it is refused instead.
        let db = test_db();
        let (dir, base, head) = repo_with_base_and_delivery(&["src/lib.rs", "tests/e2e.rs"]);
        let run_id = seed_run_and_step(&db, "step-1");
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            1,
            &contract_declaring(VerdictClass::Strong, &base),
        ));

        assert_eq!(
            exam_integrity(&db, &facts_for(&dir, &head, &run_id, "step-1")),
            ExamIntegrity::ModifiedExam {
                paths: vec!["tests/e2e.rs".to_string()]
            },
            "the exam path is named, and only the exam path"
        );
    }

    #[test]
    fn a_strong_declaration_that_leaves_the_exam_alone_is_graded() {
        let db = test_db();
        let (dir, base, head) = repo_with_base_and_delivery(&["src/lib.rs"]);
        let run_id = seed_run_and_step(&db, "step-1");
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            1,
            &contract_declaring(VerdictClass::Strong, &base),
        ));

        assert_eq!(
            exam_integrity(&db, &facts_for(&dir, &head, &run_id, "step-1")),
            ExamIntegrity::Intact,
            "subject-only work under a strong declaration is what strong means"
        );
    }

    #[test]
    fn authored_work_may_edit_the_exam() {
        // Characterization testing and TDD both write the exam. If this ever
        // fires, Phase 24.2's brownfield on-ramp is broken.
        let db = test_db();
        let (dir, base, head) = repo_with_base_and_delivery(&["src/lib.rs", "tests/e2e.rs"]);
        let run_id = seed_run_and_step(&db, "step-1");
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            1,
            &contract_declaring(VerdictClass::Authored, &base),
        ));

        assert_eq!(
            exam_integrity(&db, &facts_for(&dir, &head, &run_id, "step-1")),
            ExamIntegrity::PermittedAuthoredWork,
            "an authored verdict is allowed to have written the exam"
        );
    }

    #[test]
    fn a_missing_contract_is_explicitly_unknown() {
        let db = test_db();
        let (dir, _base, head) = repo_with_base_and_delivery(&["tests/e2e.rs"]);
        assert_eq!(
            exam_integrity(&db, &facts(&dir, &head)),
            ExamIntegrity::Unknown {
                reason: "missing work contract".to_string()
            }
        );
    }

    #[tokio::test]
    async fn a_modified_protected_exam_is_inconclusive_without_running_checks() {
        let db = test_db();
        let (dir, base, head) = repo_with_base_and_delivery(&["src/lib.rs", "tests/e2e.rs"]);
        let (run_id, lease_gen) = seed_verifying_step(&db, "step-1", &head);
        db.save_check_specs(&run_id, "step-1", &[spec("c1")])
            .unwrap();
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            lease_gen,
            &contract_declaring(VerdictClass::Strong, &base),
        ));
        let runner = ScriptedRunner::new(vec![Ok(CheckOutcome::Passed)]);

        let verdict =
            verify_delivery(&db, &runner, &facts_for(&dir, &head, &run_id, "step-1")).await;

        assert_eq!(verdict, Some(Verdict::Inconclusive));
        assert_eq!(runner.calls(), 0, "a modified exam must not be graded");
        assert_eq!(
            db.get_step_status("step-1").as_deref(),
            Some("inconclusive")
        );
        assert!(verification_reason(&db, "step-1", lease_gen)
            .contains("modified protected exam surface: tests/e2e.rs"));
    }

    #[tokio::test]
    async fn a_missing_contract_is_inconclusive_unbilled_and_idempotent() {
        let db = test_db();
        let (dir, _base, head) = repo_with_base_and_delivery(&["src/lib.rs"]);
        let (run_id, lease_gen) = seed_verifying_step(&db, "step-1", &head);
        db.save_check_specs(&run_id, "step-1", &[spec("c1")])
            .unwrap();
        db.init_credit_balance("user-1", 50).expect("balance");
        let mut f = facts_for(&dir, &head, &run_id, "step-1");
        f.quoted_credits = Some(10);
        let runner = ScriptedRunner::new(vec![Ok(CheckOutcome::Passed)]);

        assert_eq!(
            verify_delivery(&db, &runner, &f).await,
            Some(Verdict::Inconclusive)
        );
        assert_eq!(runner.calls(), 0, "unknown integrity must run no checks");
        assert_eq!(db.get_credit_balance("user-1").subscription_remaining, 50);
        assert_eq!(db.credit_ledger_totals("user-1"), (0, 0));
        assert_eq!(
            db.get_step_status("step-1").as_deref(),
            Some("inconclusive")
        );
        assert!(verification_reason(&db, "step-1", lease_gen)
            .contains("exam integrity unknown: missing work contract"));

        assert_eq!(
            verify_delivery(&db, &runner, &f).await,
            None,
            "replay must lose the verification claim"
        );
        assert_eq!(runner.calls(), 0);
        assert_eq!(db.credit_ledger_totals("user-1"), (0, 0));
    }

    #[tokio::test]
    async fn an_unreadable_contract_is_inconclusive_with_its_own_diagnostic() {
        let db = test_db();
        let (dir, base, head) = repo_with_base_and_delivery(&["src/lib.rs"]);
        let (run_id, lease_gen) = seed_verifying_step(&db, "step-1", &head);
        db.save_check_specs(&run_id, "step-1", &[spec("c1")])
            .unwrap();
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            lease_gen,
            &contract_declaring(VerdictClass::Strong, &base),
        ));
        db.conn()
            .execute(
                "UPDATE step_work_contracts SET contract_json = '{' \
                 WHERE step_id = 'step-1' AND lease_gen = ?1",
                rusqlite::params![lease_gen],
            )
            .unwrap();
        let runner = ScriptedRunner::new(vec![Ok(CheckOutcome::Passed)]);

        assert_eq!(
            verify_delivery(&db, &runner, &facts_for(&dir, &head, &run_id, "step-1"),).await,
            Some(Verdict::Inconclusive)
        );
        assert_eq!(runner.calls(), 0);
        assert!(verification_reason(&db, "step-1", lease_gen).contains("unreadable work contract"));
    }

    #[tokio::test]
    async fn a_missing_required_base_is_inconclusive_with_its_own_diagnostic() {
        let db = test_db();
        let (dir, _base, head) = repo_with_base_and_delivery(&["src/lib.rs"]);
        let (run_id, lease_gen) = seed_verifying_step(&db, "step-1", &head);
        db.save_check_specs(&run_id, "step-1", &[spec("c1")])
            .unwrap();
        let mut contract = contract_declaring(VerdictClass::Strong, &head);
        contract.expected_base_commit = None;
        assert!(db.record_step_work_contract("step-1", &run_id, lease_gen, &contract,));
        let runner = ScriptedRunner::new(vec![Ok(CheckOutcome::Passed)]);

        assert_eq!(
            verify_delivery(&db, &runner, &facts_for(&dir, &head, &run_id, "step-1"),).await,
            Some(Verdict::Inconclusive)
        );
        assert_eq!(runner.calls(), 0);
        assert!(verification_reason(&db, "step-1", lease_gen)
            .contains("strong contract is missing its required base commit"));
    }

    #[tokio::test]
    async fn a_failed_diff_inspection_is_inconclusive_with_its_own_diagnostic() {
        let db = test_db();
        let (dir, _base, head) = repo_with_base_and_delivery(&["src/lib.rs"]);
        let (run_id, lease_gen) = seed_verifying_step(&db, "step-1", &head);
        db.save_check_specs(&run_id, "step-1", &[spec("c1")])
            .unwrap();
        assert!(db.record_step_work_contract(
            "step-1",
            &run_id,
            lease_gen,
            &contract_declaring(VerdictClass::Strong, "not-a-commit"),
        ));
        let runner = ScriptedRunner::new(vec![Ok(CheckOutcome::Passed)]);

        assert_eq!(
            verify_delivery(&db, &runner, &facts_for(&dir, &head, &run_id, "step-1"),).await,
            Some(Verdict::Inconclusive)
        );
        assert_eq!(runner.calls(), 0);
        assert!(verification_reason(&db, "step-1", lease_gen).contains("diff inspection failed"));
    }
}
