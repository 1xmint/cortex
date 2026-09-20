//! What a step intends to write, and whether two steps can run at once.
//!
//! Today a run without explicit file paths takes a lease on `"."` — the whole
//! repository (`scheduler.rs`, "repo-wide run without explicit file paths").
//! That is correct and maximally pessimistic: `.` overlaps everything, so one
//! such run serialises every other run against the same repo. It is also the
//! common case, because most plans do not enumerate their paths.
//!
//! This module derives a write set from what the plan already knows, so the
//! repo-wide lease becomes the fallback rather than the default, and makes
//! disjointness a property that can be *checked* rather than hoped for.
//!
//! The overlap rule matches `path_keys_overlap` in `crates/api/src/db.rs`
//! exactly — same `.`-dominates-everything, same prefix-with-`/` boundary. Two
//! different answers to "do these paths conflict" is a scheduler that grants a
//! lease the conflict checker would have refused, which is the failure this
//! module must not introduce. See the parity test at the bottom.
//!
//! Part of PR R (Track B, Phase 11.2–11.3).

use serde::{Deserialize, Serialize};

/// The whole repository. Overlaps everything, including itself.
pub const REPO_WIDE: &str = ".";

/// The paths a step intends to write.
///
/// `Unknown` is a distinct variant rather than an empty set, and the
/// distinction is the one that keeps this safe: an empty set means *this step
/// writes nothing* and can run beside anything, while `Unknown` means *we do
/// not know what it writes* and must be treated as repo-wide. Collapsing them
/// would let a step with undeclared paths run concurrently with one that
/// touches the same file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WriteSet {
    /// Nothing is written — a read-only step.
    Empty,
    /// These paths, normalised and deduplicated.
    Paths(Vec<String>),
    /// Not derivable. Treated as the whole repository.
    Unknown,
}

impl WriteSet {
    /// The lease keys this write set needs.
    ///
    /// `Unknown` yields `["."]`, which is exactly today's behaviour — so a plan
    /// that declares nothing is no worse off than it is now, and a plan that
    /// declares something is better off. That property is what makes this
    /// deployable without a flag.
    pub fn lease_keys(&self) -> Vec<String> {
        match self {
            WriteSet::Empty => Vec::new(),
            WriteSet::Paths(paths) => paths.clone(),
            WriteSet::Unknown => vec![REPO_WIDE.to_string()],
        }
    }

    /// Whether this set can run beside `other` without serialising.
    pub fn is_disjoint_from(&self, other: &WriteSet) -> bool {
        overlapping_pairs(self, other).is_empty()
    }
}

/// Normalise one path to a lease key.
///
/// Trailing and leading slashes are stripped because `src/` and `src` are the
/// same directory but different strings, and a prefix test on the unnormalised
/// forms silently answers "no overlap" for a pair that plainly overlaps.
/// `.`-only and empty inputs collapse to repo-wide rather than to nothing —
/// failing safe, since an unparseable path is an unknown path.
pub fn normalise(path: &str) -> String {
    let trimmed = path.trim().trim_matches('/').trim();
    if trimmed.is_empty() || trimmed == "." {
        return REPO_WIDE.to_string();
    }
    // `./src` and `src` are the same place.
    trimmed
        .strip_prefix("./")
        .unwrap_or(trimmed)
        .trim_matches('/')
        .to_string()
}

/// Do two lease keys contend?
///
/// **Must stay identical to `path_keys_overlap` in `crates/api/src/db.rs`.**
/// The parity test at the bottom of this file is the guard.
pub fn keys_overlap(a: &str, b: &str) -> bool {
    a == REPO_WIDE
        || b == REPO_WIDE
        || a == b
        || a.strip_prefix(b)
            .is_some_and(|suffix| suffix.starts_with('/'))
        || b.strip_prefix(a)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

/// Derive a write set from what a step declares.
///
/// `target_paths` is what the work recipe says the step will change;
/// `allowed_paths` is the outer bound the contract permits. Targets are
/// preferred because they are narrower — the allowed set is a permission, not
/// an intention, and leasing the whole permission would serialise steps that
/// were never going to touch the same file.
///
/// `changes_tree` is the same predicate that decides whether verification
/// attaches. A step that cannot change the tree writes nothing, so it takes no
/// lease at all and never blocks anyone — which is most of the value here,
/// since Search/Think/Review steps are common and currently take a repo-wide
/// lease whenever the plan declares no paths.
pub fn derive_write_set(
    changes_tree: bool,
    target_paths: &[String],
    allowed_paths: &[String],
) -> WriteSet {
    if !changes_tree {
        return WriteSet::Empty;
    }

    let source = if !target_paths.is_empty() {
        target_paths
    } else if !allowed_paths.is_empty() {
        allowed_paths
    } else {
        // Nothing declared. This is the case that is repo-wide today and stays
        // repo-wide — declaring nothing must not be rewarded with more
        // concurrency than declaring something.
        return WriteSet::Unknown;
    };

    let mut keys: Vec<String> = source.iter().map(|p| normalise(p)).collect();
    keys.sort();
    keys.dedup();

    // A repo-wide entry among specific ones dominates: leasing both `.` and
    // `src/a.rs` is the same as leasing `.`, and carrying the redundant key
    // would make two identical write sets compare unequal.
    if keys.iter().any(|k| k == REPO_WIDE) {
        return WriteSet::Unknown;
    }

    // Drop any key already covered by a broader one in the same set, for the
    // same reason: `src` and `src/a.rs` together is just `src`.
    let mut minimal: Vec<String> = Vec::new();
    for key in keys {
        if minimal.iter().any(|kept| is_ancestor_or_equal(kept, &key)) {
            continue;
        }
        minimal.retain(|kept| !is_ancestor_or_equal(&key, kept));
        minimal.push(key);
    }
    minimal.sort();

    if minimal.is_empty() {
        WriteSet::Empty
    } else {
        WriteSet::Paths(minimal)
    }
}

fn is_ancestor_or_equal(ancestor: &str, descendant: &str) -> bool {
    ancestor == descendant
        || descendant
            .strip_prefix(ancestor)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

/// Which keys of two write sets contend.
pub fn overlapping_pairs(a: &WriteSet, b: &WriteSet) -> Vec<(String, String)> {
    let (ka, kb) = (a.lease_keys(), b.lease_keys());
    let mut pairs = Vec::new();
    for x in &ka {
        for y in &kb {
            if keys_overlap(x, y) {
                pairs.push((x.clone(), y.clone()));
            }
        }
    }
    pairs
}

/// Two steps that cannot run at the same time, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteSetOverlap {
    pub step_a: String,
    pub step_b: String,
    /// The contending keys, so the message names a path rather than saying
    /// "these conflict".
    pub keys: Vec<(String, String)>,
}

/// Check a whole plan for write-set disjointness.
///
/// Returns every overlapping pair rather than the first, because a plan with
/// four mutually overlapping steps should be reported once with four facts, not
/// fixed and re-run four times.
///
/// **Overlap is not an error.** It is the input to scheduling: overlapping
/// steps must be *serialised*, not rejected. A plan where every step writes the
/// same file is a perfectly good plan that happens to have no parallelism, and
/// failing it would reject most real work. The caller uses this to decide what
/// may run concurrently.
pub fn find_overlaps(steps: &[(String, WriteSet)]) -> Vec<WriteSetOverlap> {
    let mut overlaps = Vec::new();
    for (i, (id_a, set_a)) in steps.iter().enumerate() {
        for (id_b, set_b) in steps.iter().skip(i + 1) {
            let keys = overlapping_pairs(set_a, set_b);
            if !keys.is_empty() {
                overlaps.push(WriteSetOverlap {
                    step_a: id_a.clone(),
                    step_b: id_b.clone(),
                    keys,
                });
            }
        }
    }
    overlaps
}

/// Sort lease keys into the one order every acquirer uses.
///
/// Two transactions taking the same two locks in opposite orders can deadlock,
/// and the current code acquires in whatever order the caller assembled the
/// requests. That is latent rather than live today — acquisition happens inside
/// a single transaction that checks every conflict before inserting anything —
/// but step-scoped leases with queueing will hold locks across more decisions,
/// and at that point acquisition order stops being an implementation detail.
///
/// Plain lexicographic byte order: it is total, stable, and needs no shared
/// state between acquirers, which is the only property that matters.
pub fn canonical_order(keys: &mut [String]) {
    keys.sort();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|s| s.to_string()).collect()
    }

    /// The distinction the enum exists for. Collapsing `Empty` and `Unknown`
    /// would let a step with undeclared paths run beside one touching the same
    /// file.
    #[test]
    fn nothing_declared_is_repo_wide_but_nothing_written_is_free() {
        assert_eq!(derive_write_set(true, &[], &[]), WriteSet::Unknown);
        assert_eq!(WriteSet::Unknown.lease_keys(), vec![REPO_WIDE.to_string()]);

        assert_eq!(derive_write_set(false, &[], &[]), WriteSet::Empty);
        assert!(WriteSet::Empty.lease_keys().is_empty());
    }

    /// The main win: read-only steps stop taking a repo-wide lease.
    #[test]
    fn a_step_that_cannot_change_the_tree_takes_no_lease() {
        let ws = derive_write_set(false, &p(&["src/a.rs"]), &p(&["src"]));
        assert_eq!(ws, WriteSet::Empty);
        assert!(ws.is_disjoint_from(&WriteSet::Unknown));
    }

    #[test]
    fn targets_are_preferred_over_the_permitted_bound() {
        let ws = derive_write_set(true, &p(&["src/auth.rs"]), &p(&["src"]));
        assert_eq!(ws, WriteSet::Paths(p(&["src/auth.rs"])));
    }

    #[test]
    fn allowed_paths_are_the_fallback_when_no_target_is_declared() {
        let ws = derive_write_set(true, &[], &p(&["crates/api"]));
        assert_eq!(ws, WriteSet::Paths(p(&["crates/api"])));
    }

    #[test]
    fn a_repo_wide_entry_dominates_the_rest() {
        assert_eq!(
            derive_write_set(true, &p(&["src/a.rs", "."]), &[]),
            WriteSet::Unknown
        );
    }

    #[test]
    fn a_covered_path_is_dropped_in_favour_of_its_ancestor() {
        // `src` already covers `src/a.rs`; carrying both would make two equal
        // write sets compare unequal.
        assert_eq!(
            derive_write_set(true, &p(&["src/a.rs", "src", "src/b/c.rs"]), &[]),
            WriteSet::Paths(p(&["src"]))
        );
    }

    #[test]
    fn normalisation_makes_the_same_directory_the_same_key() {
        for form in ["src", "/src", "src/", "./src", " src ", "/src/"] {
            assert_eq!(normalise(form), "src", "{form:?}");
        }
        for form in ["", ".", "/", "  "] {
            assert_eq!(normalise(form), REPO_WIDE, "{form:?}");
        }
    }

    #[test]
    fn unnormalised_forms_would_otherwise_miss_an_overlap() {
        // The bug this normalisation prevents.
        assert!(!keys_overlap("src/", "src/a.rs"));
        assert!(keys_overlap(&normalise("src/"), &normalise("src/a.rs")));
    }

    #[test]
    fn overlap_is_by_directory_boundary_not_string_prefix() {
        assert!(keys_overlap("src", "src/a.rs"));
        // `src2` is not inside `src`, though it shares the prefix.
        assert!(!keys_overlap("src", "src2/a.rs"));
        assert!(!keys_overlap("src/a.rs", "src/b.rs"));
    }

    #[test]
    fn repo_wide_overlaps_everything_including_itself() {
        assert!(keys_overlap(REPO_WIDE, "anything/at/all"));
        assert!(keys_overlap("anything/at/all", REPO_WIDE));
        assert!(keys_overlap(REPO_WIDE, REPO_WIDE));
    }

    #[test]
    fn disjoint_sets_can_run_together() {
        let a = derive_write_set(true, &p(&["crates/api"]), &[]);
        let b = derive_write_set(true, &p(&["crates/worker"]), &[]);
        assert!(a.is_disjoint_from(&b));
        assert!(find_overlaps(&[("a".into(), a), ("b".into(), b)]).is_empty());
    }

    #[test]
    fn find_overlaps_reports_every_pair_not_just_the_first() {
        let shared = derive_write_set(true, &p(&["src/lib.rs"]), &[]);
        let steps = vec![
            ("s1".to_string(), shared.clone()),
            ("s2".to_string(), shared.clone()),
            ("s3".to_string(), shared),
        ];
        // Three steps, three pairs.
        assert_eq!(find_overlaps(&steps).len(), 3);
    }

    #[test]
    fn an_overlap_names_the_contending_paths() {
        let a = derive_write_set(true, &p(&["src"]), &[]);
        let b = derive_write_set(true, &p(&["src/auth.rs"]), &[]);
        let overlaps = find_overlaps(&[("a".into(), a), ("b".into(), b)]);
        assert_eq!(overlaps.len(), 1);
        assert_eq!(
            overlaps[0].keys,
            vec![("src".to_string(), "src/auth.rs".to_string())]
        );
    }

    /// An empty write set contends with nothing, including repo-wide.
    #[test]
    fn a_read_only_step_never_contends() {
        let reader = WriteSet::Empty;
        assert!(reader.is_disjoint_from(&WriteSet::Unknown));
        assert!(reader.is_disjoint_from(&derive_write_set(true, &p(&["src"]), &[])));
        assert!(reader.is_disjoint_from(&WriteSet::Empty));
    }

    #[test]
    fn canonical_order_is_total_and_agrees_between_acquirers() {
        let mut one = p(&["src/z.rs", "crates/api", "src/a.rs"]);
        let mut two = p(&["src/a.rs", "src/z.rs", "crates/api"]);
        canonical_order(&mut one);
        canonical_order(&mut two);
        assert_eq!(one, two, "two acquirers must derive the same order");
        assert_eq!(one, p(&["crates/api", "src/a.rs", "src/z.rs"]));
    }

    /// **Parity with the scheduler's conflict checker.**
    ///
    /// `path_keys_overlap` in `crates/api/src/db.rs` is what actually refuses a
    /// lease. If this module ever answers differently, the scheduler grants
    /// concurrency the conflict checker would have refused — two steps writing
    /// the same file, which is the exact failure PR R exists to prevent. This
    /// is a copy of that function; the test asserts they agree.
    #[test]
    fn keys_overlap_matches_the_schedulers_conflict_checker() {
        fn path_keys_overlap_from_db(a: &str, b: &str) -> bool {
            a == "."
                || b == "."
                || a == b
                || a.strip_prefix(b)
                    .is_some_and(|suffix| suffix.starts_with('/'))
                || b.strip_prefix(a)
                    .is_some_and(|suffix| suffix.starts_with('/'))
        }

        let keys = [
            ".",
            "src",
            "src2",
            "src/a.rs",
            "src/b/c.rs",
            "crates",
            "crates/api",
            "",
        ];
        for a in keys {
            for b in keys {
                assert_eq!(
                    keys_overlap(a, b),
                    path_keys_overlap_from_db(a, b),
                    "disagreement on ({a:?}, {b:?}) — the scheduler would grant \
                     concurrency the conflict checker refuses"
                );
            }
        }
    }
}
