//! Checking Cortex's write set against the humans, and admitting what that
//! cannot see.
//!
//! Phase 33.2a. Phase 11 arbitrates paths between Cortex's own steps and Phase
//! 14 coordinates between Cortex plans. **Neither knows the humans exist**, and
//! in a repository with 200 developers the humans are the dominant source of
//! contention.
//!
//! The data is free: the forge already exposes open pull requests, their changed
//! paths, and their authors. Feeding that into the conflict checks turns
//! coordination from a Cortex-internal nicety into something a platform team can
//! see the value of on day one — and it is the feature the five-to-thirty-person
//! team of Phase 33.4 feels immediately.
//!
//! # The boundary that has to be stated rather than glossed
//!
//! > **Cortex coordinates against what is pushed.** Work sitting uncommitted in
//! > someone's working tree is invisible, and a claim of disjointness is a claim
//! > about the shared history, not about what everyone is doing right now.
//!
//! That is why [`check`] returns [`AgainstPushedWork`] and why the accessors are
//! [`overlaps_in_pushed_work`] and [`none_in_pushed_work`]. There is no
//! `is_clear()`, because there is no way to know that, and a method called
//! `is_clear` would be believed. The qualifier is in the name so it survives to
//! every call site and into whatever the user is eventually shown.
//!
//! [`overlaps_in_pushed_work`]: AgainstPushedWork::overlaps_in_pushed_work
//! [`none_in_pushed_work`]: AgainstPushedWork::none_in_pushed_work

use serde::{Deserialize, Serialize};

/// An open change already in the shared history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenChange {
    /// Whatever the forge calls it — a pull request number, a branch name.
    pub reference: String,
    pub author: String,
    /// Paths this change touches.
    pub paths: Vec<String>,
    /// Cortex opened this one.
    pub is_cortex: bool,
}

/// A path Cortex wants that somebody else already has open.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Overlap {
    pub reference: String,
    pub author: String,
    /// The specific paths in both, sorted so a receipt reproduces.
    pub paths: Vec<String>,
    /// Whether the other change is Cortex's own.
    pub is_cortex: bool,
}

/// The result of a contention check, named for what it could actually see.
///
/// Deliberately not `ContentionResult`. Every accessor carries the qualifier,
/// because the interesting answer — "nothing overlaps" — is the one somebody
/// will act on, and it is only true of pushed work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgainstPushedWork {
    overlaps: Vec<Overlap>,
}

impl AgainstPushedWork {
    pub fn overlaps_in_pushed_work(&self) -> &[Overlap] {
        &self.overlaps
    }

    pub fn none_in_pushed_work(&self) -> bool {
        self.overlaps.is_empty()
    }

    /// Overlaps with work a person is doing.
    ///
    /// Separate from the Cortex ones because the responses differ: Phase 14 can
    /// reschedule Cortex's own plan against itself, and cannot reschedule a
    /// colleague.
    pub fn with_humans(&self) -> impl Iterator<Item = &Overlap> {
        self.overlaps.iter().filter(|o| !o.is_cortex)
    }
}

/// Check a write set against everything open in the shared history.
pub fn check(write_set: &[String], open: &[OpenChange]) -> AgainstPushedWork {
    let overlaps = open
        .iter()
        .filter_map(|change| {
            let mut paths: Vec<String> = change
                .paths
                .iter()
                .filter(|p| write_set.contains(p))
                .cloned()
                .collect();
            if paths.is_empty() {
                return None;
            }
            paths.sort();
            Some(Overlap {
                reference: change.reference.clone(),
                author: change.author.clone(),
                paths,
                is_cortex: change.is_cortex,
            })
        })
        .collect();
    AgainstPushedWork { overlaps }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn human(reference: &str, author: &str, touched: &[&str]) -> OpenChange {
        OpenChange {
            reference: reference.to_string(),
            author: author.to_string(),
            paths: paths(touched),
            is_cortex: false,
        }
    }

    fn cortex(reference: &str, touched: &[&str]) -> OpenChange {
        OpenChange {
            is_cortex: true,
            ..human(reference, "cortex", touched)
        }
    }

    #[test]
    fn a_humans_open_branch_on_the_same_file_is_found() {
        // Phase 14 coordinates Cortex plans with each other while a colleague's
        // open branch touching the same file is invisible. In a 200-developer
        // repository that is the dominant source of contention.
        let result = check(
            &paths(&["src/auth.rs", "src/lib.rs"]),
            &[human("#812", "priya", &["src/auth.rs", "docs/auth.md"])],
        );
        assert_eq!(
            result.overlaps_in_pushed_work(),
            &[Overlap {
                reference: "#812".to_string(),
                author: "priya".to_string(),
                paths: paths(&["src/auth.rs"]),
                is_cortex: false,
            }]
        );
    }

    #[test]
    fn only_the_shared_paths_are_reported() {
        // A colleague's branch touching forty files, one of which is ours, is a
        // one-file conflict. Reporting all forty would make the signal useless
        // at exactly the scale it matters.
        let result = check(
            &paths(&["src/auth.rs"]),
            &[human(
                "#812",
                "priya",
                &["src/auth.rs", "a.rs", "b.rs", "c.rs"],
            )],
        );
        assert_eq!(
            result.overlaps_in_pushed_work()[0].paths,
            paths(&["src/auth.rs"])
        );
    }

    #[test]
    fn overlapping_paths_are_sorted_so_a_receipt_reproduces() {
        let result = check(
            &paths(&["z.rs", "a.rs", "m.rs"]),
            &[human("#1", "sam", &["z.rs", "m.rs", "a.rs"])],
        );
        assert_eq!(
            result.overlaps_in_pushed_work()[0].paths,
            paths(&["a.rs", "m.rs", "z.rs"])
        );
    }

    #[test]
    fn human_and_cortex_overlaps_are_distinguishable() {
        // The responses differ. Phase 14 can reschedule Cortex's own plan
        // against itself; it cannot reschedule a colleague.
        let result = check(
            &paths(&["src/auth.rs"]),
            &[
                human("#812", "priya", &["src/auth.rs"]),
                cortex("#900", &["src/auth.rs"]),
            ],
        );
        assert_eq!(result.overlaps_in_pushed_work().len(), 2);
        let humans: Vec<&Overlap> = result.with_humans().collect();
        assert_eq!(humans.len(), 1);
        assert_eq!(humans[0].author, "priya");
    }

    #[test]
    fn a_disjoint_write_set_reports_nothing_in_pushed_work() {
        // And the method is named for exactly that, because the answer somebody
        // will act on is this one.
        let result = check(
            &paths(&["src/billing.rs"]),
            &[human("#812", "priya", &["src/auth.rs"])],
        );
        assert!(result.none_in_pushed_work());
        assert!(result.overlaps_in_pushed_work().is_empty());
    }

    #[test]
    fn there_is_no_method_claiming_the_write_set_is_clear() {
        // The honest boundary, as an API shape. Cortex coordinates against what
        // is pushed; work sitting uncommitted in someone's working tree is
        // invisible. A method called `is_clear` would be believed, so the
        // qualifier lives in the name and survives to every call site.
        let result = check(&paths(&["src/billing.rs"]), &[]);
        assert!(result.none_in_pushed_work());
        // The type's own name carries the caveat into any struct that stores it.
        let json = serde_json::to_string(&result).unwrap();
        assert_eq!(json, r#"{"overlaps":[]}"#);
    }

    #[test]
    fn an_empty_write_set_conflicts_with_nothing() {
        let result = check(&[], &[human("#812", "priya", &["src/auth.rs"])]);
        assert!(result.none_in_pushed_work());
    }

    #[test]
    fn a_quiet_repository_reports_nothing() {
        assert!(check(&paths(&["src/auth.rs"]), &[]).none_in_pushed_work());
    }

    #[test]
    fn several_colleagues_on_one_file_are_all_reported() {
        // Three people already in a file is information about the file, not
        // only about the conflict, and collapsing it to "conflicted" loses the
        // part a planner should act on.
        let result = check(
            &paths(&["src/hot.rs"]),
            &[
                human("#1", "priya", &["src/hot.rs"]),
                human("#2", "sam", &["src/hot.rs"]),
                human("#3", "alex", &["src/hot.rs"]),
            ],
        );
        assert_eq!(result.with_humans().count(), 3);
    }
}
