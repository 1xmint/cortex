//! The key Phase 24.1 already built, used for the thing it is also good for.
//!
//! Phase 24.1 introduced `(check, tree_hash, runner_digest)` as **flake
//! identity**: two executions sharing that triple should agree, and when they
//! disagree the check is non-deterministic. The same triple is a
//! **content-addressed cache key**, and using it as one is the largest cost
//! reduction available anywhere in the plan. It needs no new machinery — only
//! noticing that the key already exists.
//!
//! # The soundness condition is a measurement we already take
//!
//! Reuse is valid only if the check is a pure function of the tree, which is
//! precisely what flake scoring measures. A check with a clean determinism
//! record is cacheable; a quarantined one is not. One measurement drives both
//! mechanisms and they reinforce each other: the second execution of a triple is
//! simultaneously a cache miss and a free determinism probe, so early in a
//! repo's life Cortex pays for evidence it wants anyway, and later it stops
//! paying at all. [`reuse`] returns [`Reuse::ExecuteAsProbe`] for that case by
//! name, because a "miss" that is really a probe should not be counted as waste
//! when someone tunes the hit rate.
//!
//! # Where the savings are
//!
//! Concentrated exactly where the pain is: retry after a one-line fix, the
//! escalation ladder re-running the same battery, racing (N attempts share
//! unchanged subtrees), and integration re-verification — invariant 16 requires
//! a full battery on the integrated tree, and most of that tree is unchanged.
//! Phase 25.2's remote build cache is the same idea one layer down.
//!
//! It also *strengthens* the receipt rather than weakening it. A
//! content-addressed reuse is more reproducible than a re-execution: the receipt
//! names the exact prior execution, and any auditor can re-run it.
//!
//! # The scope rule is not a performance trade
//!
//! A cache keyed on tree hash alone, shared across tenants, is a membership
//! oracle for private source: hit means someone else has this exact tree.
//! [`CacheScope`] is therefore part of the key, not a filter applied afterwards,
//! and Phase 32.4 treats a leak here as existential rather than as a tuning
//! decision.

use serde::{Deserialize, Serialize};

use crate::verification::{CheckExecution, CheckOutcome};

/// The boundary a cache entry may never cross.
///
/// Part of the key rather than a filter over results. A filter is something a
/// later refactor can drop; a key component is something the lookup cannot be
/// performed without.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheScope {
    tenant: String,
    repo: String,
}

impl CacheScope {
    pub fn new(tenant: impl Into<String>, repo: impl Into<String>) -> Self {
        Self {
            tenant: tenant.into(),
            repo: repo.into(),
        }
    }
}

/// `(scope, check, tree_hash, runner_digest)` — flake identity plus the tenant
/// boundary.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ExecutionKey {
    encoded: String,
}

impl ExecutionKey {
    /// Build the key from the argv, not from [`CheckSpec::id`].
    ///
    /// The id is documented as stable *within a verification*, so keying on it
    /// would either miss across runs or, worse, collide two different commands
    /// that happened to reuse an id. The argv is what actually determines the
    /// outcome.
    ///
    /// [`CheckSpec::id`]: crate::verification::CheckSpec::id
    pub fn new(
        scope: &CacheScope,
        command: &[String],
        tree_hash: &str,
        runner_image: &str,
    ) -> Self {
        let mut parts: Vec<&str> = vec![&scope.tenant, &scope.repo, tree_hash, runner_image];
        parts.extend(command.iter().map(|s| s.as_str()));
        Self {
            encoded: length_prefixed(&parts),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.encoded
    }
}

/// Length-prefix every field so no field's *content* can imitate the structure.
///
/// A key joined on a separator is forgeable by anyone who can choose one of the
/// values: a tenant named `a:b` and a tenant named `a` with a repo named `b`
/// produce the same string. Here the length is written before the value, so the
/// decoder position never depends on the bytes inside a field.
fn length_prefixed(parts: &[&str]) -> String {
    let mut out = String::new();
    for p in parts {
        out.push_str(&p.len().to_string());
        out.push(':');
        out.push_str(p);
    }
    out
}

/// What the flake scorer knows about a check under one [`ExecutionKey`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Determinism {
    /// Executed more than once under identical keys and agreed every time.
    Clean,
    /// Executed at most once. Not evidence of determinism — evidence of
    /// nothing.
    Unproven,
    /// Disagreed with itself under an identical key. Never reusable, because
    /// its recorded outcome is not a fact about the tree.
    Quarantined,
}

/// What to do about one lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reuse {
    /// Serve the recorded outcome. The receipt names the prior execution.
    ServeRecorded,
    /// Execute. The result is worth recording.
    ExecuteAndRecord,
    /// Execute, and count this as buying determinism evidence rather than as a
    /// wasted miss. Recording is still correct; the entry only becomes servable
    /// once this execution makes the record `Clean`.
    ExecuteAsProbe,
    /// Execute, and record nothing. The check is not a function of the tree, so
    /// anything it produces is unsafe to reuse.
    ExecuteWithoutRecording,
}

impl Reuse {
    /// Whether the check runs. The one question the driver actually asks.
    pub fn must_execute(&self) -> bool {
        !matches!(self, Self::ServeRecorded)
    }
}

/// The reuse decision for one key.
///
/// `hit` is whether a recorded outcome exists for this exact key; `determinism`
/// is the flake scorer's verdict on the check.
pub fn reuse(hit: bool, determinism: Determinism) -> Reuse {
    match (determinism, hit) {
        (Determinism::Quarantined, _) => Reuse::ExecuteWithoutRecording,
        (Determinism::Clean, true) => Reuse::ServeRecorded,
        (Determinism::Clean, false) => Reuse::ExecuteAndRecord,
        // A hit we cannot yet trust. Executing anyway both delivers a sound
        // answer and produces the second observation that settles the record.
        (Determinism::Unproven, true) => Reuse::ExecuteAsProbe,
        (Determinism::Unproven, false) => Reuse::ExecuteAndRecord,
    }
}

/// A recorded outcome, which is what the receipt cites on a reuse.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheEntry {
    pub key: ExecutionKey,
    pub outcome: CheckOutcome,
    pub exit_code: Option<i32>,
    /// The execution this entry stands in for, so an auditor can re-run it.
    pub prior_output_digest: String,
}

/// Turn an execution into a cache entry, or refuse to.
///
/// `None` for a quarantined check, and `None` for [`CheckOutcome::NotExecuted`].
/// The second refusal is the one that would otherwise cost real money: a
/// `NotExecuted` says the runner had a problem, not that the tree has one, and
/// caching it would make a single Docker hiccup a permanent property of that
/// tree — `Inconclusive` on every retry, forever, for a repo that is fine.
pub fn record(
    key: ExecutionKey,
    execution: &CheckExecution,
    determinism: Determinism,
) -> Option<CacheEntry> {
    if matches!(determinism, Determinism::Quarantined) {
        return None;
    }
    if !execution.outcome.is_about_the_work() {
        return None;
    }
    Some(CacheEntry {
        key,
        outcome: execution.outcome,
        exit_code: execution.exit_code,
        prior_output_digest: execution.output_digest.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    fn key(scope: &CacheScope, command: &[&str], tree: &str, image: &str) -> ExecutionKey {
        ExecutionKey::new(scope, &cmd(command), tree, image)
    }

    fn execution(outcome: CheckOutcome) -> CheckExecution {
        CheckExecution {
            spec_id: "cargo-test".to_string(),
            exit_code: Some(0),
            outcome,
            duration_ms: 12,
            output_digest: "sha256:abc".to_string(),
            output_tail: String::new(),
            runner_image: "img@sha256:d".to_string(),
        }
    }

    #[test]
    fn the_same_work_in_two_tenants_does_not_share_a_key() {
        // The existential one. A cache keyed on tree hash alone answers "has
        // anyone else built this exact tree", which is a membership oracle over
        // private source.
        let a = key(
            &CacheScope::new("tenant-a", "repo"),
            &["cargo", "test"],
            "tree1",
            "img@sha256:d",
        );
        let b = key(
            &CacheScope::new("tenant-b", "repo"),
            &["cargo", "test"],
            "tree1",
            "img@sha256:d",
        );
        assert_ne!(a, b);
    }

    #[test]
    fn a_field_containing_the_separator_cannot_forge_another_key() {
        // Length prefixes, not delimiters. With `tenant:repo` joined on a
        // colon these two would be the same string, and a customer who can
        // name their own repo would be choosing another customer's cache
        // bucket.
        let a = key(
            &CacheScope::new("acme", "5:hello"),
            &["c"],
            "t",
            "img@sha256:d",
        );
        let b = key(&CacheScope::new("acme", ""), &["c"], "t", "img@sha256:d");
        assert_ne!(a, b);

        let c = key(&CacheScope::new("a", "b:c"), &["x"], "t", "i");
        let d = key(&CacheScope::new("a:b", "c"), &["x"], "t", "i");
        assert_ne!(c, d);
    }

    #[test]
    fn the_key_moves_when_any_part_of_the_triple_moves() {
        let scope = CacheScope::new("t", "r");
        let base = key(&scope, &["cargo", "test"], "tree1", "img@sha256:d");
        assert_ne!(
            base,
            key(&scope, &["cargo", "bench"], "tree1", "img@sha256:d")
        );
        assert_ne!(
            base,
            key(&scope, &["cargo", "test"], "tree2", "img@sha256:d")
        );
        assert_ne!(
            base,
            key(&scope, &["cargo", "test"], "tree1", "img@sha256:e")
        );
        assert_eq!(
            base,
            key(&scope, &["cargo", "test"], "tree1", "img@sha256:d")
        );
    }

    #[test]
    fn argv_boundaries_are_part_of_the_key() {
        // `["cargo test", "-p", "x"]` and `["cargo", "test", "-p", "x"]` are
        // different commands. A key that flattened argv would treat them as the
        // same and serve one's result for the other.
        let scope = CacheScope::new("t", "r");
        assert_ne!(
            key(&scope, &["cargo test"], "tree", "img"),
            key(&scope, &["cargo", "test"], "tree", "img")
        );
    }

    #[test]
    fn a_quarantined_check_always_runs_and_never_records() {
        // Its recorded outcome is not a fact about the tree, so there is
        // nothing sound to serve and nothing sound to store.
        for hit in [true, false] {
            assert_eq!(
                reuse(hit, Determinism::Quarantined),
                Reuse::ExecuteWithoutRecording
            );
        }
        let entry = record(
            key(&CacheScope::new("t", "r"), &["c"], "tree", "img"),
            &execution(CheckOutcome::Passed),
            Determinism::Quarantined,
        );
        assert!(entry.is_none());
    }

    #[test]
    fn a_hit_on_an_unproven_check_is_a_probe_not_a_saving() {
        // The reinforcing case. We do not yet know the check is deterministic,
        // so we run it -- and that run is the second observation the flake
        // scorer needs. Naming it a probe stops it being counted as a wasted
        // miss by whoever tunes the hit rate later.
        assert_eq!(reuse(true, Determinism::Unproven), Reuse::ExecuteAsProbe);
        assert!(reuse(true, Determinism::Unproven).must_execute());
    }

    #[test]
    fn only_a_clean_record_with_a_hit_is_served() {
        assert_eq!(reuse(true, Determinism::Clean), Reuse::ServeRecorded);
        assert!(!reuse(true, Determinism::Clean).must_execute());
        for (hit, det) in [
            (false, Determinism::Clean),
            (true, Determinism::Unproven),
            (false, Determinism::Unproven),
            (true, Determinism::Quarantined),
            (false, Determinism::Quarantined),
        ] {
            assert!(
                reuse(hit, det).must_execute(),
                "hit={hit} {det:?} must not be served from cache"
            );
        }
    }

    #[test]
    fn an_infrastructure_failure_is_never_cached() {
        // `NotExecuted` says the runner had a problem, not that the tree does.
        // Caching it would make one Docker hiccup a permanent property of the
        // tree: `Inconclusive` on every retry, forever, for a repo that is fine.
        let entry = record(
            key(&CacheScope::new("t", "r"), &["c"], "tree", "img"),
            &execution(CheckOutcome::NotExecuted),
            Determinism::Clean,
        );
        assert!(entry.is_none());
    }

    #[test]
    fn a_failure_and_a_timeout_are_both_worth_caching() {
        // Both are evidence about the work. Re-running a battery after a
        // one-line fix should not re-pay for the twenty checks the fix did not
        // touch, and that includes the ones that were red.
        for outcome in [
            CheckOutcome::Passed,
            CheckOutcome::Failed,
            CheckOutcome::TimedOut,
        ] {
            let entry = record(
                key(&CacheScope::new("t", "r"), &["c"], "tree", "img"),
                &execution(outcome),
                Determinism::Clean,
            )
            .expect("an outcome about the work is cacheable");
            assert_eq!(entry.outcome, outcome);
        }
    }

    #[test]
    fn an_entry_names_the_execution_it_stands_in_for() {
        // What makes a reuse *more* reproducible than a re-run: the receipt can
        // cite the exact earlier execution, and an auditor can go re-run it.
        let entry = record(
            key(&CacheScope::new("t", "r"), &["c"], "tree", "img"),
            &execution(CheckOutcome::Passed),
            Determinism::Clean,
        )
        .expect("cacheable");
        assert_eq!(entry.prior_output_digest, "sha256:abc");
    }
}
