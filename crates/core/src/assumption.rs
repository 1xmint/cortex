//! Claims a plan depends on that nothing has checked.
//!
//! A plan says "fix the failing test in `auth.rs`". That sentence carries
//! premises nobody stated: that `auth.rs` exists, that there is a test in it,
//! that the test currently fails. If any is false the plan cannot succeed, and
//! today the way we find out is that a worker is dispatched, burns an attempt,
//! and reports something confusing.
//!
//! An `Assumption` makes those premises explicit and attaches a **mechanical**
//! check — one that runs at plan time, before dispatch, before a customer is
//! charged for discovering the plan was built on air.
//!
//! Structured the way `check_derivation` is, and for the same reason: the rules
//! are pure and unit-testable without a disk, and the filesystem lives at the
//! edge in one small probe. See `cortex/plan/briefs/PR-U-provenance-typing.md`.

use serde::{Deserialize, Serialize};

/// A premise a plan depends on, and the mechanical test for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assumption {
    /// The claim, in the words a human would use. Goes on the receipt.
    pub claim: String,
    pub check: AssumptionCheck,
}

/// How to test a premise without asking a model.
///
/// Every variant is decidable by looking at the repository. That is the whole
/// constraint: an assumption whose check is "ask an agent whether this is true"
/// is not an assumption, it is another unverified claim, and adding it here
/// would launder a guess into something that looks checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssumptionCheck {
    /// A path exists in the repository.
    PathExists { path: String },
    /// A path does not exist — for a plan that means to create something.
    PathAbsent { path: String },
    /// A file contains a substring. Used for "this symbol is defined here".
    FileContains { path: String, needle: String },
    /// The repository has the manifest for an ecosystem, e.g. `Cargo.toml`.
    ManifestPresent { manifest: String },
}

/// What a check found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssumptionOutcome {
    Held,
    Violated,
    /// The check could not be run — the workspace was unreadable.
    ///
    /// Distinct from `Violated` on purpose, and the distinction is the same one
    /// the verdict model makes between `Failed` and `Inconclusive`: our inability
    /// to look is not evidence that the premise is false. Blocking a plan on it
    /// would fail customers' work for our outage.
    Indeterminate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssumptionResult {
    pub claim: String,
    pub outcome: AssumptionOutcome,
    /// Why, when it did not hold. Phrased for whoever reads the receipt.
    pub detail: Option<String>,
}

/// What the filesystem said, gathered once at the edge.
///
/// The evaluator takes this rather than a path, so every rule below is a pure
/// function testable without touching a disk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceFacts {
    /// Paths that exist, repo-relative.
    pub existing_paths: Vec<String>,
    /// Contents of the files the assumptions asked about. Absent means the file
    /// could not be read, which is `Indeterminate` rather than `Violated`.
    pub file_contents: Vec<(String, String)>,
    /// True when the workspace itself could not be read at all.
    pub unreadable: bool,
}

impl WorkspaceFacts {
    fn has_path(&self, path: &str) -> bool {
        self.existing_paths.iter().any(|p| p == path)
    }

    fn content_of(&self, path: &str) -> Option<&str> {
        self.file_contents
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, c)| c.as_str())
    }
}

/// Evaluate one assumption against gathered facts.
pub fn evaluate(assumption: &Assumption, facts: &WorkspaceFacts) -> AssumptionResult {
    let result = |outcome, detail: Option<String>| AssumptionResult {
        claim: assumption.claim.clone(),
        outcome,
        detail,
    };

    if facts.unreadable {
        return result(
            AssumptionOutcome::Indeterminate,
            Some("the workspace could not be read".to_string()),
        );
    }

    match &assumption.check {
        AssumptionCheck::PathExists { path } => {
            if facts.has_path(path) {
                result(AssumptionOutcome::Held, None)
            } else {
                result(
                    AssumptionOutcome::Violated,
                    Some(format!("{path} is not in the repository")),
                )
            }
        }
        AssumptionCheck::PathAbsent { path } => {
            if facts.has_path(path) {
                result(
                    AssumptionOutcome::Violated,
                    Some(format!("{path} already exists")),
                )
            } else {
                result(AssumptionOutcome::Held, None)
            }
        }
        AssumptionCheck::FileContains { path, needle } => match facts.content_of(path) {
            Some(content) if content.contains(needle) => result(AssumptionOutcome::Held, None),
            Some(_) => result(
                AssumptionOutcome::Violated,
                Some(format!("{path} does not contain {needle:?}")),
            ),
            // The file was named but not read. We do not know, and saying
            // "violated" would be asserting something we did not check.
            None => result(
                AssumptionOutcome::Indeterminate,
                Some(format!("{path} could not be read")),
            ),
        },
        AssumptionCheck::ManifestPresent { manifest } => {
            if facts.has_path(manifest) {
                result(AssumptionOutcome::Held, None)
            } else {
                result(
                    AssumptionOutcome::Violated,
                    Some(format!("{manifest} is not in the repository")),
                )
            }
        }
    }
}

/// Which paths a set of assumptions needs read, so the probe fetches exactly
/// those and no more.
///
/// A plan-time check that walked the tree would be a plan-time cost proportional
/// to repository size, on every plan.
pub fn paths_to_read(assumptions: &[Assumption]) -> Vec<String> {
    let mut paths: Vec<String> = assumptions
        .iter()
        .filter_map(|a| match &a.check {
            AssumptionCheck::FileContains { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// Evaluate all of them, and say whether the plan may proceed.
///
/// **Only `Violated` blocks.** `Indeterminate` does not, for the same reason an
/// inconclusive verdict does not penalise a model: not being able to look is our
/// problem, and blocking a customer's plan on our inability to read a file
/// converts an infrastructure failure into a product failure.
pub fn evaluate_all(assumptions: &[Assumption], facts: &WorkspaceFacts) -> AssumptionReport {
    let results: Vec<AssumptionResult> = assumptions.iter().map(|a| evaluate(a, facts)).collect();
    let violated = results
        .iter()
        .filter(|r| r.outcome == AssumptionOutcome::Violated)
        .count();
    let indeterminate = results
        .iter()
        .filter(|r| r.outcome == AssumptionOutcome::Indeterminate)
        .count();
    AssumptionReport {
        results,
        violated,
        indeterminate,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssumptionReport {
    pub results: Vec<AssumptionResult>,
    pub violated: usize,
    pub indeterminate: usize,
}

impl AssumptionReport {
    /// Whether the plan may be dispatched.
    pub fn may_proceed(&self) -> bool {
        self.violated == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(paths: &[&str]) -> WorkspaceFacts {
        WorkspaceFacts {
            existing_paths: paths.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn assume(claim: &str, check: AssumptionCheck) -> Assumption {
        Assumption {
            claim: claim.to_string(),
            check,
        }
    }

    #[test]
    fn a_missing_path_violates_and_blocks() {
        let a = assume(
            "auth.rs exists",
            AssumptionCheck::PathExists {
                path: "src/auth.rs".into(),
            },
        );
        let report = evaluate_all(&[a], &facts(&["src/main.rs"]));
        assert_eq!(report.violated, 1);
        assert!(!report.may_proceed());
        assert!(report.results[0]
            .detail
            .as_ref()
            .unwrap()
            .contains("not in the repository"));
    }

    #[test]
    fn a_present_path_holds() {
        let a = assume(
            "auth.rs exists",
            AssumptionCheck::PathExists {
                path: "src/auth.rs".into(),
            },
        );
        let report = evaluate_all(&[a], &facts(&["src/auth.rs"]));
        assert!(report.may_proceed());
        assert_eq!(report.results[0].outcome, AssumptionOutcome::Held);
    }

    #[test]
    fn path_absent_catches_a_plan_that_would_overwrite() {
        let a = assume(
            "config.toml does not exist yet",
            AssumptionCheck::PathAbsent {
                path: "config.toml".into(),
            },
        );
        assert!(!evaluate_all(std::slice::from_ref(&a), &facts(&["config.toml"])).may_proceed());
        assert!(evaluate_all(&[a], &facts(&["other.toml"])).may_proceed());
    }

    /// The distinction the outcome type exists to preserve. Not being able to
    /// look is not evidence the premise is false.
    #[test]
    fn an_unreadable_workspace_is_indeterminate_and_does_not_block() {
        let a = assume(
            "auth.rs exists",
            AssumptionCheck::PathExists {
                path: "src/auth.rs".into(),
            },
        );
        let unreadable = WorkspaceFacts {
            unreadable: true,
            ..Default::default()
        };
        let report = evaluate_all(&[a], &unreadable);
        assert_eq!(report.results[0].outcome, AssumptionOutcome::Indeterminate);
        assert_eq!(report.violated, 0);
        assert_eq!(report.indeterminate, 1);
        assert!(
            report.may_proceed(),
            "our inability to read must not fail a customer's plan"
        );
    }

    #[test]
    fn an_unread_file_is_indeterminate_not_violated() {
        let a = assume(
            "login is defined in auth.rs",
            AssumptionCheck::FileContains {
                path: "src/auth.rs".into(),
                needle: "fn login".into(),
            },
        );
        // The path exists but no content was gathered for it.
        let report = evaluate_all(&[a], &facts(&["src/auth.rs"]));
        assert_eq!(report.results[0].outcome, AssumptionOutcome::Indeterminate);
        assert!(report.may_proceed());
    }

    #[test]
    fn file_contains_holds_and_violates_on_content() {
        let a = assume(
            "login is defined in auth.rs",
            AssumptionCheck::FileContains {
                path: "src/auth.rs".into(),
                needle: "fn login".into(),
            },
        );
        let mut f = facts(&["src/auth.rs"]);
        f.file_contents
            .push(("src/auth.rs".into(), "pub fn login() {}".into()));
        assert_eq!(evaluate(&a, &f).outcome, AssumptionOutcome::Held);

        let mut g = facts(&["src/auth.rs"]);
        g.file_contents
            .push(("src/auth.rs".into(), "pub fn logout() {}".into()));
        assert_eq!(evaluate(&a, &g).outcome, AssumptionOutcome::Violated);
    }

    /// A plan-time check must not cost a tree walk on every plan.
    #[test]
    fn only_the_named_files_are_requested() {
        let assumptions = vec![
            assume(
                "a",
                AssumptionCheck::FileContains {
                    path: "src/a.rs".into(),
                    needle: "x".into(),
                },
            ),
            assume(
                "b",
                AssumptionCheck::PathExists {
                    path: "src/b.rs".into(),
                },
            ),
            assume(
                "a again",
                AssumptionCheck::FileContains {
                    path: "src/a.rs".into(),
                    needle: "y".into(),
                },
            ),
        ];
        // Only FileContains needs a read, and the duplicate collapses.
        assert_eq!(paths_to_read(&assumptions), vec!["src/a.rs".to_string()]);
    }

    #[test]
    fn a_violation_among_holds_still_blocks() {
        let assumptions = vec![
            assume(
                "main exists",
                AssumptionCheck::PathExists {
                    path: "src/main.rs".into(),
                },
            ),
            assume(
                "auth exists",
                AssumptionCheck::PathExists {
                    path: "src/auth.rs".into(),
                },
            ),
        ];
        let report = evaluate_all(&assumptions, &facts(&["src/main.rs"]));
        assert_eq!(report.violated, 1);
        assert!(!report.may_proceed());
    }

    #[test]
    fn no_assumptions_proceeds() {
        assert!(evaluate_all(&[], &facts(&[])).may_proceed());
    }
}
