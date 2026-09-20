//! Files that everything touches, and numbers nobody should pick.
//!
//! Real repositories have paths every change ends up editing: lockfiles,
//! manifests, route tables, DI registries, i18n catalogs, and single-integer
//! migration counters. Two agents working on unrelated features will still
//! collide there, and the collision is not a scheduling failure — it is
//! inherent to the file.
//!
//! Two different problems live here and they have different answers:
//!
//! - A **hotspot** is a file with genuine content that many changes touch. It
//!   needs a short exclusive hold for the edit itself, not for the whole step.
//! - A **sequence** is a number that must be unique and increasing — a
//!   migration version, a port. Nothing is being merged; a value is being
//!   chosen. Letting an agent choose it creates a conflict that need not exist,
//!   because the scheduler can simply hand out the next one.
//!
//! This organisation has already lost time to the second kind on this very
//! repository: two branches each picked the next migration number, and the one
//! that merged second had its migration silently skipped. That is what
//! `Sequence` exists to make impossible.
//!
//! Part of PR R (Track B, Phase 11.5).

use serde::{Deserialize, Serialize};

/// Why a path needs special handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotspotKind {
    /// Machine-generated and regenerable. Two edits conflict textually but the
    /// resolution is to regenerate, not to merge.
    Lockfile,
    /// Dependency manifests. Concurrent edits usually merge, but not always,
    /// and a bad merge breaks the build for everyone.
    Manifest,
    /// A registry every feature appends to — routes, DI, i18n. Textual
    /// conflicts are near-certain with concurrent work.
    Registry,
    /// A single increasing number. **Not mergeable in any useful sense** — see
    /// `Sequence`.
    Counter,
}

impl HotspotKind {
    /// Whether concurrent edits can be reconciled by merging text at all.
    ///
    /// `Counter` cannot: two branches picking "the next number" both produce a
    /// syntactically clean file, and the merge succeeds while the meaning is
    /// wrong. That silent success is exactly why counters get their own
    /// mechanism rather than a stricter lease.
    pub fn is_textually_mergeable(&self) -> bool {
        !matches!(self, HotspotKind::Counter)
    }
}

/// A path that needs a short exclusive hold rather than a step-long one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hotspot {
    pub path: String,
    pub kind: HotspotKind,
}

/// Hotspots present in essentially every repository of a given ecosystem.
///
/// A starting set, not a policy: a repo declaring its own should extend this,
/// not replace it. Matching is on the file name so a path anywhere in the tree
/// is caught — a workspace has many `Cargo.toml`s and all of them are
/// manifests.
pub const WELL_KNOWN_HOTSPOTS: &[(&str, HotspotKind)] = &[
    ("Cargo.lock", HotspotKind::Lockfile),
    ("package-lock.json", HotspotKind::Lockfile),
    ("pnpm-lock.yaml", HotspotKind::Lockfile),
    ("yarn.lock", HotspotKind::Lockfile),
    ("poetry.lock", HotspotKind::Lockfile),
    ("go.sum", HotspotKind::Lockfile),
    ("Cargo.toml", HotspotKind::Manifest),
    ("package.json", HotspotKind::Manifest),
    ("pyproject.toml", HotspotKind::Manifest),
    ("go.mod", HotspotKind::Manifest),
];

/// Classify a path, if it is a well-known hotspot.
///
/// Declared hotspots are checked first so a repository can say "this file is a
/// registry" about a path this list would otherwise call a manifest, or would
/// not recognise at all.
pub fn classify(path: &str, declared: &[Hotspot]) -> Option<HotspotKind> {
    let normalised = crate::write_set::normalise(path);
    if let Some(hit) = declared
        .iter()
        .find(|h| crate::write_set::normalise(&h.path) == normalised)
    {
        return Some(hit.kind);
    }
    let file_name = normalised.rsplit('/').next().unwrap_or(&normalised);
    WELL_KNOWN_HOTSPOTS
        .iter()
        .find(|(name, _)| *name == file_name)
        .map(|(_, kind)| *kind)
}

/// A value the scheduler hands out so nobody has to pick one.
///
/// The name is the sequence, not the file: two repositories both have
/// `migration_version`, and they are unrelated sequences. Callers key by
/// `(repo, name)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequenceAllocation {
    pub sequence: String,
    pub value: i64,
    /// The step the value was issued to, so an allocation can be traced back to
    /// the work that asked for it rather than appearing from nowhere in a diff.
    pub step_id: String,
}

/// The sequence a migration counter lives on.
///
/// Named rather than stringly-typed at call sites because a typo here produces
/// a *second independent counter* that silently hands out numbers already in
/// use — the same failure the whole mechanism exists to prevent, reintroduced
/// by a misspelling.
pub const MIGRATION_SEQUENCE: &str = "migration_version";

#[cfg(test)]
mod tests {
    use super::*;

    fn declared(path: &str, kind: HotspotKind) -> Hotspot {
        Hotspot {
            path: path.to_string(),
            kind,
        }
    }

    #[test]
    fn well_known_lockfiles_and_manifests_are_recognised_anywhere_in_the_tree() {
        assert_eq!(classify("Cargo.lock", &[]), Some(HotspotKind::Lockfile));
        assert_eq!(
            classify("crates/api/Cargo.toml", &[]),
            Some(HotspotKind::Manifest)
        );
        assert_eq!(
            classify("heyvera/package.json", &[]),
            Some(HotspotKind::Manifest)
        );
        assert_eq!(classify("src/main.rs", &[]), None);
    }

    #[test]
    fn a_declared_hotspot_overrides_the_well_known_classification() {
        let declared = vec![declared("crates/api/Cargo.toml", HotspotKind::Registry)];
        assert_eq!(
            classify("crates/api/Cargo.toml", &declared),
            Some(HotspotKind::Registry)
        );
    }

    #[test]
    fn a_declared_hotspot_can_name_a_path_the_list_does_not_know() {
        let declared = vec![declared("src/routes/mod.rs", HotspotKind::Registry)];
        assert_eq!(
            classify("src/routes/mod.rs", &declared),
            Some(HotspotKind::Registry)
        );
        assert_eq!(classify("src/routes/other.rs", &declared), None);
    }

    #[test]
    fn declaration_matching_survives_path_spelling() {
        let declared = vec![declared("/src/routes/mod.rs", HotspotKind::Registry)];
        assert_eq!(
            classify("./src/routes/mod.rs", &declared),
            Some(HotspotKind::Registry)
        );
    }

    /// The distinction that earns `Counter` its own mechanism: two branches
    /// each picking "the next number" both produce a clean file, and the merge
    /// succeeds while the meaning is wrong.
    #[test]
    fn a_counter_is_the_one_kind_that_cannot_be_merged() {
        assert!(!HotspotKind::Counter.is_textually_mergeable());
        for kind in [
            HotspotKind::Lockfile,
            HotspotKind::Manifest,
            HotspotKind::Registry,
        ] {
            assert!(kind.is_textually_mergeable(), "{kind:?}");
        }
    }
}
