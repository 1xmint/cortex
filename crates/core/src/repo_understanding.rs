//! Comprehension as a durable artifact, not a per-run expense.
//!
//! Phase 29.3. Every run explores, and today that exploration dies with the
//! worktree — the next run on the same repository pays for it again from zero.
//! That is the largest recurring waste in the system, and the reason the same
//! task costs the same on the hundredth run as on the first.
//!
//! Two design points decide whether making it durable works at all.
//!
//! # Negative results are the valuable half
//!
//! `dead_ends` — what was looked at and was *not* relevant — is what makes the
//! next exploration cheap, and it is exactly what every harness throws away.
//! Knowing that six plausible-looking modules are irrelevant to authentication
//! saves more tokens than knowing the two that are.
//!
//! So [`EntryKind::DeadEnd`] is an ordinary entry with the same durability rules
//! as everything else, not an annotation hanging off a positive finding.
//!
//! # Invalidate per file content hash, never per commit
//!
//! A commit-scoped cache is useless in a repository with 200 developers
//! committing: it is stale before it is written. Content-hash granularity means
//! a busy repository *degrades* its map gradually instead of discarding it,
//! which is the only version of this that survives Phase 33's scale.
//!
//! [`Understanding::commit_anchor`] is therefore recorded for provenance and is
//! **not** consulted by [`surviving`]. An entry anchored to files nobody touched
//! outlives any number of commits.
//!
//! # Provenance discipline is not optional
//!
//! Phase 13 already supplies it and it just has to be applied here: a prior
//! run's *executed outcome* enters as verified evidence, a prior run's
//! *conclusion* enters as unverified with its origin. A comprehension cache that
//! can override the repository is a defect generator, which is why there is no
//! constructor taking a [`Provenance`] — [`Entry::from_executed_outcome`] and
//! [`Entry::from_conclusion`] are the only two doors, and an agent's confident
//! description of its own work cannot come through the first one.
//!
//! Invariant 19 governs the rest: when the map disagrees with the tree, the tree
//! wins. That is already structural — every entry here carries a provenance
//! whose `may_instruct()` is false, so nothing in this artifact can direct a
//! step. It is orientation, and orientation is checked against the tree.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::provenance::Provenance;

/// A file this entry's truth depends on, and the content it depended on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAnchor {
    pub path: String,
    pub content_hash: String,
}

/// What kind of thing was learned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// Module boundaries, ownership, entry points.
    Subsystem,
    /// How this repo names things, tests things, wires DI.
    Convention,
    /// Where change concentrates.
    Hotspot,
    /// How this repo is verified: commands, cost, determinism record.
    BatteryShape,
    /// Looked at, not relevant. The half that makes the next run cheap.
    DeadEnd,
}

/// One thing a previous run learned about this repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub kind: EntryKind,
    pub claim: String,
    provenance: Provenance,
    /// The files whose content this claim was derived from.
    pub anchors: Vec<FileAnchor>,
}

impl Entry {
    /// A prior run's executed outcome — a check that ran, against a tree, at a
    /// commit. The only path to verified provenance.
    pub fn from_executed_outcome(
        kind: EntryKind,
        claim: impl Into<String>,
        verification_id: impl Into<String>,
        anchors: Vec<FileAnchor>,
    ) -> Self {
        Self {
            kind,
            claim: claim.into(),
            provenance: Provenance::VerifiedEvidence {
                verification_id: verification_id.into(),
            },
            anchors,
        }
    }

    /// A prior run's conclusion, with its origin. Unverified, and stays that way.
    ///
    /// Most of a comprehension artifact is this: an agent read some files and
    /// formed a view. Useful, and not evidence.
    pub fn from_conclusion(
        kind: EntryKind,
        claim: impl Into<String>,
        producer_step_id: impl Into<String>,
        anchors: Vec<FileAnchor>,
    ) -> Self {
        Self {
            kind,
            claim: claim.into(),
            provenance: Provenance::AgentOutput {
                producer_step_id: producer_step_id.into(),
            },
            anchors,
        }
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

/// What Cortex knows about one repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Understanding {
    pub repo_id: String,
    /// Recorded so a reader can find the tree these entries were derived
    /// against. Not the invalidation key — see [`surviving`].
    pub commit_anchor: String,
    pub entries: Vec<Entry>,
}

/// The entries still true of a tree with these file hashes.
///
/// An entry survives when every file it is anchored to is still present with the
/// same content. One file changing invalidates only the entries that depended on
/// it, which is the whole difference from a commit-scoped cache.
///
/// An entry with no anchors never survives. There is nothing to check it
/// against, so it cannot be shown to still hold — and an unanchored claim that
/// outlived the code it described is precisely the defect generator the
/// provenance rules exist to prevent. Re-deriving it is cheap; being wrong about
/// a repository is not.
pub fn surviving<'a>(
    entries: &'a [Entry],
    current_hashes: &HashMap<String, String>,
) -> Vec<&'a Entry> {
    entries
        .iter()
        .filter(|e| {
            !e.anchors.is_empty()
                && e.anchors
                    .iter()
                    .all(|a| current_hashes.get(&a.path) == Some(&a.content_hash))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(path: &str, hash: &str) -> FileAnchor {
        FileAnchor {
            path: path.to_string(),
            content_hash: hash.to_string(),
        }
    }

    fn hashes(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(p, h)| (p.to_string(), h.to_string()))
            .collect()
    }

    fn conclusion(kind: EntryKind, claim: &str, anchors: Vec<FileAnchor>) -> Entry {
        Entry::from_conclusion(kind, claim, "step-1", anchors)
    }

    #[test]
    fn one_file_changing_invalidates_only_what_depended_on_it() {
        // The design point. Under commit scoping every one of these dies on any
        // commit, and in a repo with 200 developers committing the cache is
        // stale before it is written.
        let entries = [
            conclusion(
                EntryKind::Subsystem,
                "auth lives in src/auth",
                vec![anchor("src/auth/mod.rs", "h1")],
            ),
            conclusion(
                EntryKind::Convention,
                "handlers return Result<Json<T>>",
                vec![anchor("src/api/handlers.rs", "h2")],
            ),
        ];
        let now = hashes(&[
            ("src/auth/mod.rs", "CHANGED"),
            ("src/api/handlers.rs", "h2"),
        ]);

        let alive = surviving(&entries, &now);
        assert_eq!(alive.len(), 1);
        assert_eq!(alive[0].claim, "handlers return Result<Json<T>>");
    }

    #[test]
    fn entries_outlive_commits_they_were_not_derived_from() {
        // `commit_anchor` is provenance, not the invalidation key. A thousand
        // commits touching other files leave this entry standing.
        let u = Understanding {
            repo_id: "acme/app".to_string(),
            commit_anchor: "abc123".to_string(),
            entries: vec![conclusion(
                EntryKind::Subsystem,
                "billing is in crates/billing",
                vec![anchor("crates/billing/src/lib.rs", "h1")],
            )],
        };
        let much_later = hashes(&[("crates/billing/src/lib.rs", "h1")]);
        assert_eq!(surviving(&u.entries, &much_later).len(), 1);
    }

    #[test]
    fn a_dead_end_is_as_durable_as_a_finding() {
        // The valuable half. "These six modules are irrelevant to auth" saves
        // more than knowing the two that are, and it must survive on the same
        // terms or the next run re-explores them.
        let entries = [conclusion(
            EntryKind::DeadEnd,
            "src/legacy/* has nothing to do with authentication",
            vec![anchor("src/legacy/session.rs", "h9")],
        )];
        let now = hashes(&[("src/legacy/session.rs", "h9")]);
        assert_eq!(surviving(&entries, &now).len(), 1);

        let after = hashes(&[("src/legacy/session.rs", "different")]);
        assert!(surviving(&entries, &after).is_empty());
    }

    #[test]
    fn a_deleted_file_invalidates_what_it_anchored() {
        let entries = [conclusion(
            EntryKind::Hotspot,
            "src/gone.rs changes constantly",
            vec![anchor("src/gone.rs", "h1")],
        )];
        assert!(surviving(&entries, &hashes(&[])).is_empty());
    }

    #[test]
    fn an_entry_anchored_to_several_files_needs_all_of_them() {
        let entries = [conclusion(
            EntryKind::Convention,
            "every handler is registered in router.rs",
            vec![
                anchor("src/router.rs", "h1"),
                anchor("src/handlers.rs", "h2"),
            ],
        )];
        assert_eq!(
            surviving(
                &entries,
                &hashes(&[("src/router.rs", "h1"), ("src/handlers.rs", "h2")])
            )
            .len(),
            1
        );
        assert!(surviving(
            &entries,
            &hashes(&[("src/router.rs", "h1"), ("src/handlers.rs", "CHANGED")])
        )
        .is_empty());
    }

    #[test]
    fn an_unanchored_claim_never_survives() {
        // There is nothing to check it against, so it cannot be shown to still
        // hold. A claim that outlived the code it described is the defect
        // generator the provenance rules exist to prevent, and re-deriving it
        // is cheap next to being wrong about a repository.
        let entries = [conclusion(
            EntryKind::Convention,
            "this repo prefers composition",
            vec![],
        )];
        assert!(surviving(&entries, &hashes(&[("anything", "h")])).is_empty());
    }

    #[test]
    fn a_conclusion_cannot_enter_as_evidence() {
        // The two doors. An agent describing its own work is the most common
        // source of a plausible false statement in the system, and there is no
        // constructor taking a `Provenance` for it to come through.
        let c = conclusion(EntryKind::Subsystem, "auth is in src/auth", vec![]);
        assert!(matches!(
            c.provenance(),
            Provenance::AgentOutput { producer_step_id } if producer_step_id == "step-1"
        ));

        let e = Entry::from_executed_outcome(
            EntryKind::BatteryShape,
            "cargo test -p api passes in 40s",
            "ver-7",
            vec![],
        );
        assert!(matches!(
            e.provenance(),
            Provenance::VerifiedEvidence { verification_id } if verification_id == "ver-7"
        ));
    }

    #[test]
    fn nothing_in_the_artifact_may_instruct_a_step() {
        // Invariant 19's structural half: when the map disagrees with the tree
        // the tree wins, and the way that is guaranteed is that no entry can
        // direct anything. Only a contract may.
        for entry in [
            conclusion(EntryKind::Subsystem, "x", vec![]),
            Entry::from_executed_outcome(EntryKind::BatteryShape, "y", "ver-1", vec![]),
        ] {
            assert!(
                !entry.provenance().may_instruct(),
                "a comprehension entry must never be readable as an instruction"
            );
        }
    }

    #[test]
    fn an_empty_understanding_survives_nothing_and_does_not_panic() {
        assert!(surviving(&[], &hashes(&[("a", "b")])).is_empty());
    }
}
