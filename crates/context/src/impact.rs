//! Impact sets: what a change actually touches.
//!
//! Task **C4** of `cortex/plan/CONTEXT.md`, and the half of the enterprise
//! coherence claim Cortex could not previously make.
//!
//! Enforcement already exists — resource leases with conflict detection. What
//! was missing is *understanding*: a lease claims the paths someone thought to
//! list, so two runs editing different files that share a dependency collide
//! at merge time rather than at claim time.
//!
//! An impact set is the bounded closure over the symbol graph from a step's
//! edit surface. Two properties make it usable rather than merely correct:
//!
//! - **Bounded depth.** In a real monorepo the transitive closure of a common
//!   utility is the entire repository, and a lease over everything is a global
//!   mutex with extra steps. Depth is the dial between "claims too little" and
//!   "serialises the org".
//! - **Every entry carries its path.** A conflict a developer cannot explain
//!   is a conflict they will route around. `run A holds validateToken via
//!   checkout-service → auth-lib` is arguable; a bare rejection is not.

use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::index::Index;

/// Default closure depth.
///
/// One hop is the direct dependents of what you edited — the files that break
/// if you get it wrong. Two hops already reaches a large fraction of a
/// well-connected repository. Start conservative: a lease that claims too
/// little produces a merge conflict, while one that claims too much stops
/// other people working, and only the second gets Cortex uninstalled.
pub const DEFAULT_DEPTH: usize = 1;

/// One file in the impact set, with the route that put it there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImpactedFile {
    pub file: String,
    /// Hops from the edit surface. Zero means the step edits it directly.
    pub distance: usize,
    /// Human-readable route: `checkout-service.rs → auth-lib.rs`.
    pub path: Vec<String>,
}

impl ImpactedFile {
    /// The explanation a developer reads when their run is blocked.
    pub fn explain(&self) -> String {
        if self.distance == 0 {
            format!("{} (edited directly)", self.file)
        } else {
            format!("{} (via {})", self.file, self.path.join(" → "))
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImpactSet {
    pub files: Vec<ImpactedFile>,
    /// True when the closure stopped at the depth limit with more to explore.
    /// Surfaced rather than hidden: a truncated impact set means the lease is
    /// narrower than the real blast radius, and an operator deciding whether
    /// to widen depth needs to know that.
    pub truncated: bool,
}

impl ImpactSet {
    /// Just the paths, for the existing lease machinery, which speaks in
    /// resource keys rather than graphs.
    pub fn resource_keys(&self) -> Vec<String> {
        self.files.iter().map(|f| f.file.clone()).collect()
    }

    pub fn contains(&self, file: &str) -> bool {
        self.files.iter().any(|f| f.file == file)
    }

    /// Files claimed by both sets — the reason two runs cannot proceed
    /// together, with each side's route.
    pub fn overlap<'a>(&'a self, other: &'a ImpactSet) -> Vec<&'a ImpactedFile> {
        self.files
            .iter()
            .filter(|file| other.contains(&file.file))
            .collect()
    }
}

impl Index {
    /// Bounded closure over the symbol graph from `seed_files`.
    ///
    /// Breadth-first, so every file is reached by its shortest route and the
    /// recorded path is the most understandable one rather than whichever the
    /// traversal happened to find.
    pub fn impact_set(&self, seed_files: &[String], depth: usize) -> Result<ImpactSet, String> {
        let mut files: BTreeMap<String, ImpactedFile> = BTreeMap::new();
        let mut queue: VecDeque<(String, usize, Vec<String>)> = VecDeque::new();

        for seed in seed_files {
            if files.contains_key(seed) {
                continue;
            }
            files.insert(
                seed.clone(),
                ImpactedFile {
                    file: seed.clone(),
                    distance: 0,
                    path: vec![seed.clone()],
                },
            );
            queue.push_back((seed.clone(), 0, vec![seed.clone()]));
        }

        let mut truncated = false;

        while let Some((file, distance, path)) = queue.pop_front() {
            if distance >= depth {
                // Only a real truncation if there is something out there we
                // are choosing not to walk to.
                if self
                    .dependents_of(&file)?
                    .iter()
                    .any(|dependent| !files.contains_key(dependent))
                {
                    truncated = true;
                }
                continue;
            }

            for dependent in self.dependents_of(&file)? {
                if files.contains_key(&dependent) {
                    continue;
                }
                let mut next_path = path.clone();
                next_path.push(dependent.clone());
                files.insert(
                    dependent.clone(),
                    ImpactedFile {
                        file: dependent.clone(),
                        distance: distance + 1,
                        path: next_path.clone(),
                    },
                );
                queue.push_back((dependent, distance + 1, next_path));
            }
        }

        Ok(ImpactSet {
            files: files.into_values().collect(),
            truncated,
        })
    }

    /// Impact set at the default depth.
    pub fn impact_set_default(&self, seed_files: &[String]) -> Result<ImpactSet, String> {
        self.impact_set(seed_files, DEFAULT_DEPTH)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    struct Workspace {
        root: PathBuf,
    }

    impl Workspace {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "cortex-impact-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock after epoch")
                    .as_nanos()
            ));
            fs::create_dir_all(&root).expect("create workspace");
            Self { root }
        }

        fn write(&self, name: &str, contents: &str) {
            fs::write(self.root.join(name), contents).expect("write file");
        }

        fn indexed(&self) -> Index {
            let mut index =
                Index::open(&self.root.join(".cortex").join("index.sqlite")).expect("open index");
            index.sync(&self.root).expect("sync");
            index
        }
    }

    impl Drop for Workspace {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).ok();
        }
    }

    /// auth.rs defines; checkout.rs uses it; api.rs uses checkout.
    fn chain(tag: &str) -> Workspace {
        let ws = Workspace::new(tag);
        ws.write("auth.rs", "pub fn validate_token() {}\n");
        ws.write("checkout.rs", "pub fn charge() { validate_token(); }\n");
        ws.write("api.rs", "fn handler() { charge(); }\n");
        ws
    }

    #[test]
    fn the_edited_file_is_always_in_its_own_impact_set() {
        let ws = chain("self");
        let index = ws.indexed();
        let set = index
            .impact_set(&["auth.rs".to_string()], 0)
            .expect("impact set");
        assert!(set.contains("auth.rs"));
        assert_eq!(set.files[0].distance, 0);
    }

    #[test]
    fn one_hop_reaches_direct_dependents_only() {
        let ws = chain("one-hop");
        let index = ws.indexed();
        let set = index
            .impact_set(&["auth.rs".to_string()], 1)
            .expect("impact set");
        assert!(set.contains("checkout.rs"));
        assert!(
            !set.contains("api.rs"),
            "api.rs is two hops away and must not be claimed at depth 1"
        );
    }

    #[test]
    fn two_hops_reach_the_transitive_dependent() {
        let ws = chain("two-hop");
        let index = ws.indexed();
        let set = index
            .impact_set(&["auth.rs".to_string()], 2)
            .expect("impact set");
        assert!(set.contains("api.rs"));
        let api = set.files.iter().find(|f| f.file == "api.rs").unwrap();
        assert_eq!(api.distance, 2);
        assert_eq!(
            api.path,
            vec![
                "auth.rs".to_string(),
                "checkout.rs".to_string(),
                "api.rs".to_string()
            ]
        );
    }

    #[test]
    fn every_entry_explains_how_it_got_there() {
        let ws = chain("explain");
        let index = ws.indexed();
        let set = index
            .impact_set(&["auth.rs".to_string()], 2)
            .expect("impact set");
        let api = set.files.iter().find(|f| f.file == "api.rs").unwrap();
        // The message a blocked developer reads. A conflict nobody can explain
        // is a conflict they will route around.
        assert_eq!(api.explain(), "api.rs (via auth.rs → checkout.rs → api.rs)");
        let auth = set.files.iter().find(|f| f.file == "auth.rs").unwrap();
        assert_eq!(auth.explain(), "auth.rs (edited directly)");
    }

    #[test]
    fn a_bounded_closure_reports_that_it_stopped_early() {
        let ws = chain("truncated");
        let index = ws.indexed();
        let set = index
            .impact_set(&["auth.rs".to_string()], 1)
            .expect("impact set");
        assert!(
            set.truncated,
            "a lease narrower than the real blast radius must say so"
        );
    }

    #[test]
    fn a_complete_closure_is_not_reported_as_truncated() {
        let ws = chain("complete");
        let index = ws.indexed();
        let set = index
            .impact_set(&["auth.rs".to_string()], 5)
            .expect("impact set");
        assert!(!set.truncated);
    }

    #[test]
    fn two_runs_on_unrelated_files_do_not_overlap() {
        let ws = Workspace::new("disjoint");
        ws.write("a.rs", "pub fn alpha() {}\n");
        ws.write("b.rs", "pub fn beta() {}\n");
        let index = ws.indexed();

        let first = index.impact_set(&["a.rs".to_string()], 1).unwrap();
        let second = index.impact_set(&["b.rs".to_string()], 1).unwrap();
        assert!(first.overlap(&second).is_empty());
    }

    #[test]
    fn runs_sharing_a_dependency_collide_at_claim_time() {
        // The whole point: neither run edits the same file, but both depend on
        // auth.rs, so they must not proceed together. Previously this surfaced
        // as a merge conflict after both had already spent money.
        let ws = Workspace::new("shared");
        ws.write("auth.rs", "pub fn validate_token() {}\n");
        ws.write("checkout.rs", "pub fn charge() { validate_token(); }\n");
        ws.write("profile.rs", "pub fn show() { validate_token(); }\n");
        let index = ws.indexed();

        let editing_auth = index.impact_set(&["auth.rs".to_string()], 1).unwrap();
        let editing_checkout = index.impact_set(&["checkout.rs".to_string()], 1).unwrap();

        let overlap = editing_auth.overlap(&editing_checkout);
        assert!(
            !overlap.is_empty(),
            "a run editing auth.rs impacts checkout.rs, so the two conflict"
        );
    }

    #[test]
    fn resource_keys_are_plain_paths_for_the_lease_machinery() {
        let ws = chain("keys");
        let index = ws.indexed();
        let set = index.impact_set(&["auth.rs".to_string()], 1).unwrap();
        let keys = set.resource_keys();
        assert!(keys.contains(&"auth.rs".to_string()));
        assert!(keys.contains(&"checkout.rs".to_string()));
    }
}
