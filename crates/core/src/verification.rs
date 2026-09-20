//! What "verified" means, as types and one deterministic function.
//!
//! This is task **V1** of `cortex/plan/VERIFIER.md`: the contract the verifier
//! core sees, and the verdict rule it computes. Deliberately free of Docker,
//! HTTP, and the database — a verdict is a pure function of executed check
//! results, and keeping it that way is what makes it testable and auditable.
//!
//! Three properties this module exists to enforce:
//!
//! - **Independent** — a [`CheckExecution`] can only be produced by a
//!   [`CheckRunner`], which runs on infrastructure the worker cannot touch.
//!   Worker-reported evidence never becomes a `CheckExecution`.
//! - **Deterministic** — [`compute_verdict`] takes checks and executions and
//!   returns a verdict. No model, no heuristic, no clock.
//! - **Attributable** — every execution carries the digest of its output and
//!   a human-readable tail, so a charge can be defended after the fact.

use serde::{Deserialize, Serialize};
use std::future::Future;

/// Where a required check came from. Recorded because a dispute usually turns
/// on *why* a check was required, not on whether it ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckSource {
    /// Derived from the delivered tree: `Cargo.toml` implies `cargo test`,
    /// `package.json` implies the scripts it declares.
    Ecosystem,
    /// Acceptance criteria from the `TaskContract`, frozen before execution.
    Contract,
    /// Added by risk level — allowed-path enforcement, lint with warnings
    /// denied — at High and Critical.
    Risk,
}

/// One check to execute. Immutable once a run starts: a check added after the
/// worker has seen the task is advisory, never required, or the optimizing
/// process gets to choose its own exam.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckSpec {
    /// Stable within a verification; used to join executions back to specs.
    pub id: String,
    pub source: CheckSource,
    /// Argv, not a shell string — nothing here goes through `sh -c`.
    pub command: Vec<String>,
    pub timeout_secs: u64,
    /// Advisory checks run and are recorded, but cannot change the verdict.
    pub required: bool,
}

/// An immutable view of the delivered work. The runner mounts this read-only;
/// it is a clean checkout of the delivered commit, never a worker's working
/// directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeSnapshot {
    /// Commit or tree hash the checks ran against. Half of "it passed" —
    /// the other half is [`CheckExecution::runner_image`].
    pub tree_hash: String,
    /// Host path of the checkout the runner mounts.
    pub path: String,
}

/// What happened to one check. Distinct from a verdict: a single check does
/// not decide anything on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcome {
    Passed,
    Failed,
    /// Exceeded `timeout_secs`. Counts as a failure, not an infra error — a
    /// hung test is exactly the defect a customer pays to be protected from.
    TimedOut,
    /// The runner could not execute it at all. This is the only outcome that
    /// can produce [`Verdict::Inconclusive`], and it must never bill.
    NotExecuted,
}

impl CheckOutcome {
    /// True when this outcome is evidence about the *work* rather than about
    /// our infrastructure.
    pub fn is_about_the_work(&self) -> bool {
        matches!(self, Self::Passed | Self::Failed | Self::TimedOut)
    }
}

/// The record of running one [`CheckSpec`]. Produced only by a
/// [`CheckRunner`]; this type is the boundary the trust model draws.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckExecution {
    pub spec_id: String,
    /// `None` when the check could not be executed.
    pub exit_code: Option<i32>,
    pub outcome: CheckOutcome,
    pub duration_ms: u64,
    /// Hash of the full output. The full log is stored separately and
    /// retention-limited; the digest is what makes it tamper-evident.
    pub output_digest: String,
    /// Last few KB, human-readable. What a dispute reads first.
    pub output_tail: String,
    /// Image digest of the environment. "It passed" is only reproducible if
    /// *where* it passed is pinned.
    pub runner_image: String,
}

/// Why a check could not be executed. Every variant here means
/// [`Verdict::Inconclusive`] — never a charge, never a refund.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum RunnerError {
    #[error("check runner unavailable: {0}")]
    Unavailable(String),
    #[error("failed to prepare tree snapshot {tree_hash}: {reason}")]
    SnapshotFailed { tree_hash: String, reason: String },
    #[error("runner refused the check spec: {0}")]
    InvalidSpec(String),
    #[error("runner failed while executing: {0}")]
    ExecutionFailed(String),
}

/// The only interface the verifier core sees. Phase A (containers) and
/// Phase B (Firecracker microVMs) both implement it, so upgrading isolation
/// is a deployment decision rather than a redesign.
///
/// Implementations must guarantee, and are the only place these can be
/// guaranteed: no network, no upstream provider keys in the environment, a
/// read-only tree mount, and enforcement of `timeout_secs`.
pub trait CheckRunner: Send + Sync {
    /// Execute one check against an immutable snapshot of the delivered tree.
    ///
    /// `Err` is what [`Verdict::Inconclusive`] is made of: return it only for
    /// *our* failures. A check that runs and fails is `Ok` with
    /// [`CheckOutcome::Failed`].
    fn run(
        &self,
        tree: &TreeSnapshot,
        check: &CheckSpec,
    ) -> impl Future<Output = Result<CheckExecution, RunnerError>> + Send;

    /// Image digest this runner executes in, recorded on every execution.
    fn runner_image(&self) -> &str;
}

/// The verdict. Note that `Unverified` is a label rather than a judgement:
/// the work was sold without a verification promise.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Every required check executed and passed. Charge fires, exactly once.
    Verified,
    /// A required check executed and failed. No charge; refund if charged.
    Failed,
    /// A required check could not be executed. No billing effect at all —
    /// bounded retry, then an operator is paged.
    Inconclusive,
    /// No required checks were derivable. Charged at task rate, sold without
    /// the verified badge and without the refund promise.
    Unverified,
}

impl Verdict {
    /// Whether this verdict may move money in either direction.
    pub fn has_billing_effect(&self) -> bool {
        !matches!(self, Self::Inconclusive)
    }
}

/// A verdict plus the reasoning that produced it, so the receipt can explain
/// itself without re-deriving anything.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerdictReport {
    pub verdict: Verdict,
    pub required_total: usize,
    pub required_passed: usize,
    /// Spec ids of required checks that failed or timed out.
    pub failed: Vec<String>,
    /// Spec ids of required checks that could not be executed.
    pub not_executed: Vec<String>,
}

/// Compute the verdict. This is the gate, and it is the whole gate.
///
/// Precedence, and the reasoning behind the one case the spec table leaves
/// implicit: if one required check *failed* and another *could not run*, the
/// verdict is `Failed`. A failing test is positive evidence that the work is
/// defective, and that evidence does not become less true because a second
/// check had an infrastructure problem. Ordering it the other way would let a
/// flaky runner launder a genuine failure into a free retry.
///
/// Advisory (`required: false`) checks are executed and recorded but never
/// change the outcome.
/// Combine two outcomes for the same check, pessimistically.
///
/// Ordering: a failure beats everything, then "could not run", then a pass.
/// Failing closed is the only defensible direction here — the alternative is a
/// verdict that depends on which duplicate row arrived first.
fn worst_of(a: CheckOutcome, b: CheckOutcome) -> CheckOutcome {
    fn severity(outcome: CheckOutcome) -> u8 {
        match outcome {
            CheckOutcome::Failed | CheckOutcome::TimedOut => 2,
            CheckOutcome::NotExecuted => 1,
            CheckOutcome::Passed => 0,
        }
    }
    if severity(b) > severity(a) {
        b
    } else {
        a
    }
}

pub fn compute_verdict(specs: &[CheckSpec], executions: &[CheckExecution]) -> VerdictReport {
    let required: Vec<&CheckSpec> = specs.iter().filter(|spec| spec.required).collect();

    if required.is_empty() {
        return VerdictReport {
            verdict: Verdict::Unverified,
            required_total: 0,
            required_passed: 0,
            failed: Vec::new(),
            not_executed: Vec::new(),
        };
    }

    let mut passed = 0usize;
    let mut failed = Vec::new();
    let mut not_executed = Vec::new();

    for spec in &required {
        // Fold over EVERY execution for this spec, worst outcome winning —
        // not `find`, which takes the first.
        //
        // One verdict per attempt is supposed to be guaranteed by the CAS on
        // (run_id, step_id, attempt), so duplicate rows for one spec should
        // be impossible. "Should be impossible" is not a security property:
        // a retry that re-ran a check and appended, a runner emitting twice,
        // or a replay concatenating attempts all produce duplicates without
        // anyone acting maliciously — and with `find`, whichever landed first
        // decided the verdict. A passing row ahead of a failing one would
        // have masked a genuine failure and charged the customer for it.
        //
        // A required check with no execution row at all is treated as one the
        // runner could not run. Silently dropping it is the hollow-gate
        // failure mode.
        let outcome = executions
            .iter()
            .filter(|exec| exec.spec_id == spec.id)
            .map(|exec| exec.outcome)
            .reduce(worst_of);

        match outcome {
            Some(CheckOutcome::Passed) => passed += 1,
            Some(CheckOutcome::Failed | CheckOutcome::TimedOut) => failed.push(spec.id.clone()),
            Some(CheckOutcome::NotExecuted) | None => not_executed.push(spec.id.clone()),
        }
    }

    let verdict = if !failed.is_empty() {
        Verdict::Failed
    } else if !not_executed.is_empty() {
        Verdict::Inconclusive
    } else {
        Verdict::Verified
    };

    VerdictReport {
        verdict,
        required_total: required.len(),
        required_passed: passed,
        failed,
        not_executed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str, required: bool) -> CheckSpec {
        CheckSpec {
            id: id.to_string(),
            source: CheckSource::Ecosystem,
            command: vec!["cargo".into(), "test".into(), "--locked".into()],
            timeout_secs: 600,
            required,
        }
    }

    fn execution(spec_id: &str, outcome: CheckOutcome) -> CheckExecution {
        CheckExecution {
            spec_id: spec_id.to_string(),
            exit_code: match outcome {
                CheckOutcome::Passed => Some(0),
                CheckOutcome::Failed => Some(1),
                CheckOutcome::TimedOut | CheckOutcome::NotExecuted => None,
            },
            outcome,
            duration_ms: 1_234,
            output_digest: "sha256:deadbeef".into(),
            output_tail: "test result: ok".into(),
            runner_image: "cortex-checkrunner@sha256:abc".into(),
        }
    }

    #[test]
    fn all_required_passing_is_verified() {
        let specs = vec![spec("a", true), spec("b", true)];
        let executions = vec![
            execution("a", CheckOutcome::Passed),
            execution("b", CheckOutcome::Passed),
        ];
        let report = compute_verdict(&specs, &executions);
        assert_eq!(report.verdict, Verdict::Verified);
        assert_eq!(report.required_passed, 2);
        assert!(report.failed.is_empty());
    }

    #[test]
    fn a_failing_required_check_fails_the_verdict() {
        let specs = vec![spec("a", true), spec("b", true)];
        let executions = vec![
            execution("a", CheckOutcome::Passed),
            execution("b", CheckOutcome::Failed),
        ];
        let report = compute_verdict(&specs, &executions);
        assert_eq!(report.verdict, Verdict::Failed);
        assert_eq!(report.failed, vec!["b".to_string()]);
    }

    #[test]
    fn a_timeout_is_a_failure_not_an_infra_error() {
        let specs = vec![spec("a", true)];
        let executions = vec![execution("a", CheckOutcome::TimedOut)];
        let report = compute_verdict(&specs, &executions);
        assert_eq!(report.verdict, Verdict::Failed);
        assert!(report.not_executed.is_empty());
    }

    #[test]
    fn an_unrunnable_check_is_inconclusive_and_never_bills() {
        let specs = vec![spec("a", true)];
        let executions = vec![execution("a", CheckOutcome::NotExecuted)];
        let report = compute_verdict(&specs, &executions);
        assert_eq!(report.verdict, Verdict::Inconclusive);
        assert!(!report.verdict.has_billing_effect());
    }

    #[test]
    fn a_missing_execution_row_is_inconclusive_not_a_pass() {
        // The hollow-gate failure mode: a required check that simply never
        // reported must not silently count as satisfied.
        let specs = vec![spec("a", true), spec("b", true)];
        let executions = vec![execution("a", CheckOutcome::Passed)];
        let report = compute_verdict(&specs, &executions);
        assert_eq!(report.verdict, Verdict::Inconclusive);
        assert_eq!(report.not_executed, vec!["b".to_string()]);
    }

    #[test]
    fn a_real_failure_outranks_a_runner_problem() {
        let specs = vec![spec("a", true), spec("b", true)];
        let executions = vec![
            execution("a", CheckOutcome::Failed),
            execution("b", CheckOutcome::NotExecuted),
        ];
        let report = compute_verdict(&specs, &executions);
        assert_eq!(report.verdict, Verdict::Failed);
    }

    #[test]
    fn no_required_checks_means_unverified_not_verified() {
        let specs = vec![spec("a", false)];
        let executions = vec![execution("a", CheckOutcome::Passed)];
        let report = compute_verdict(&specs, &executions);
        assert_eq!(report.verdict, Verdict::Unverified);
        assert_eq!(report.required_total, 0);
    }

    #[test]
    fn advisory_failures_cannot_fail_a_verdict() {
        let specs = vec![spec("a", true), spec("b", false)];
        let executions = vec![
            execution("a", CheckOutcome::Passed),
            execution("b", CheckOutcome::Failed),
        ];
        let report = compute_verdict(&specs, &executions);
        assert_eq!(report.verdict, Verdict::Verified);
    }

    #[test]
    fn every_verdict_except_inconclusive_moves_money() {
        assert!(Verdict::Verified.has_billing_effect());
        assert!(Verdict::Failed.has_billing_effect());
        assert!(Verdict::Unverified.has_billing_effect());
        assert!(!Verdict::Inconclusive.has_billing_effect());
    }
}

/// V7 red-team: the ways a worker or a bug could try to buy a passing verdict.
///
/// These are properties rather than examples. Each one names an attack, so a
/// future change that reopens it fails with the reason attached instead of an
/// assertion nobody can interpret.
#[cfg(test)]
mod adversarial {
    use super::*;

    fn spec(id: &str, required: bool) -> CheckSpec {
        CheckSpec {
            id: id.to_string(),
            source: CheckSource::Ecosystem,
            command: vec!["cargo".into(), "test".into()],
            timeout_secs: 600,
            required,
        }
    }

    fn execution(spec_id: &str, outcome: CheckOutcome) -> CheckExecution {
        CheckExecution {
            spec_id: spec_id.to_string(),
            exit_code: Some(0),
            outcome,
            duration_ms: 1,
            output_digest: "sha256:x".into(),
            output_tail: String::new(),
            runner_image: "img@sha256:y".into(),
        }
    }

    /// Attack: get a passing execution recorded ahead of the real, failing one
    /// — by retrying, by emitting twice, or by replaying an earlier attempt —
    /// so the first row read wins.
    #[test]
    fn a_duplicate_pass_cannot_mask_a_failure_regardless_of_order() {
        let specs = vec![spec("a", true)];

        for executions in [
            vec![
                execution("a", CheckOutcome::Passed),
                execution("a", CheckOutcome::Failed),
            ],
            vec![
                execution("a", CheckOutcome::Failed),
                execution("a", CheckOutcome::Passed),
            ],
        ] {
            assert_eq!(
                compute_verdict(&specs, &executions).verdict,
                Verdict::Failed,
                "ordering of duplicate executions must not decide the verdict"
            );
        }
    }

    /// Attack: bury an unrunnable check under a passing duplicate to turn an
    /// INCONCLUSIVE (no billing) into a VERIFIED (charge).
    #[test]
    fn a_duplicate_pass_cannot_hide_a_check_that_never_ran() {
        let specs = vec![spec("a", true)];
        let executions = vec![
            execution("a", CheckOutcome::Passed),
            execution("a", CheckOutcome::NotExecuted),
        ];
        assert_eq!(
            compute_verdict(&specs, &executions).verdict,
            Verdict::Inconclusive
        );
    }

    /// Attack: propose extra checks that trivially pass, hoping they dilute or
    /// outvote the real ones. The gate is a conjunction, so they cannot — but
    /// only as long as it stays a conjunction.
    #[test]
    fn adding_trivially_passing_checks_cannot_rescue_a_failure() {
        let mut specs = vec![spec("real", true)];
        let mut executions = vec![execution("real", CheckOutcome::Failed)];
        for i in 0..50 {
            let id = format!("padding_{i}");
            specs.push(spec(&id, true));
            executions.push(execution(&id, CheckOutcome::Passed));
        }
        assert_eq!(
            compute_verdict(&specs, &executions).verdict,
            Verdict::Failed
        );
    }

    /// Attack: mark the checks that would fail as advisory so they cannot
    /// bite. Permitted by construction — advisory checks never gate — which is
    /// exactly why `required` must be set during derivation, before the worker
    /// sees the task, and never afterwards. This test pins the consequence so
    /// the constraint cannot be quietly relaxed.
    #[test]
    fn advisory_downgrades_are_only_safe_because_derivation_is_frozen() {
        let specs = vec![spec("a", false)];
        let executions = vec![execution("a", CheckOutcome::Failed)];
        let report = compute_verdict(&specs, &executions);
        assert_eq!(report.verdict, Verdict::Unverified);
        assert_eq!(
            report.required_total, 0,
            "a step with no required checks is UNVERIFIED — sold without the \
             badge and without the refund promise, never silently VERIFIED"
        );
    }

    /// Attack: report executions for checks nobody asked for, hoping the count
    /// of passes is what gets compared.
    #[test]
    fn unsolicited_executions_do_not_satisfy_required_checks() {
        let specs = vec![spec("required", true)];
        let executions = vec![
            execution("something_else", CheckOutcome::Passed),
            execution("also_not_it", CheckOutcome::Passed),
        ];
        let report = compute_verdict(&specs, &executions);
        assert_eq!(report.verdict, Verdict::Inconclusive);
        assert_eq!(report.required_passed, 0);
    }

    /// Attack: submit no executions at all and hope an empty set reads as
    /// "nothing failed".
    #[test]
    fn silence_is_not_success() {
        let specs = vec![spec("a", true)];
        assert_eq!(compute_verdict(&specs, &[]).verdict, Verdict::Inconclusive);
    }
}
