//! A small in-process cache for rendered repo maps.
//!
//! Building a map walks and parses the whole tree. That is cheap next to a
//! model call but not free, and a run dispatches many steps against one
//! workspace — rebuilding per step would pay the cost dozens of times for an
//! identical answer.
//!
//! Deliberately a cache and not an index. The real index (`C2`) is persistent,
//! incremental, and invalidated per file by content hash. This is the
//! stopgap that makes C1 usable in the dispatch path today, and it is written
//! so that swapping it for C2 touches one call site.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::extract::extract_repo;
use crate::repo_map::RepoMap;

/// How long a rendered map stays fresh. Short enough that a long-running
/// server notices a repository changing under it; long enough that the steps
/// of one run share a single build.
const DEFAULT_TTL: Duration = Duration::from_secs(300);

struct Entry {
    rendered: Arc<str>,
    built_at: Instant,
}

/// Keyed by workspace path *and* token budget: the same tree rendered for a
/// 4k budget and a 40k budget are different strings, and silently returning
/// one for the other would blow a prompt budget.
type Key = (PathBuf, usize);

#[derive(Default)]
pub struct RepoMapCache {
    entries: Mutex<HashMap<Key, Entry>>,
}

impl RepoMapCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rendered map for `root` under `token_budget`, building if needed.
    ///
    /// Returns `None` when the tree yields no symbols at all — a workspace
    /// with nothing parseable in it should contribute no prompt section
    /// rather than an empty heading.
    pub fn get(&self, root: &Path, token_budget: usize) -> Option<Arc<str>> {
        self.get_with_ttl(root, token_budget, DEFAULT_TTL)
    }

    pub fn get_with_ttl(
        &self,
        root: &Path,
        token_budget: usize,
        ttl: Duration,
    ) -> Option<Arc<str>> {
        let key = (root.to_path_buf(), token_budget);

        // A poisoned lock means another thread panicked mid-build. The cache
        // is derived data, so recovering it is correct — losing a repo map
        // must never take down dispatch.
        {
            let entries = match self.entries.lock() {
                Ok(entries) => entries,
                Err(poisoned) => poisoned.into_inner(),
            };
            if let Some(entry) = entries.get(&key) {
                if entry.built_at.elapsed() < ttl {
                    return Some(Arc::clone(&entry.rendered));
                }
            }
        }

        // Built outside the lock: parsing a large repository must not block
        // every other step's dispatch. Two threads racing here duplicate work
        // once and then agree, which is cheaper than serialising all of them.
        let extraction = extract_repo(root);
        if extraction.symbols.is_empty() {
            return None;
        }
        let rendered: Arc<str> = Arc::from(
            RepoMap::build(extraction.symbols, &extraction.references)
                .render(token_budget)
                .as_str(),
        );

        let mut entries = match self.entries.lock() {
            Ok(entries) => entries,
            Err(poisoned) => poisoned.into_inner(),
        };
        entries.insert(
            key,
            Entry {
                rendered: Arc::clone(&rendered),
                built_at: Instant::now(),
            },
        );
        Some(rendered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// `Instant::now().elapsed()` is ~0 by construction, so the first version
    /// of this helper handed every test in this module the *same* directory.
    /// They run in parallel, so one test deleted the tree another was mid-way
    /// through reading — which is exactly how it failed in CI, and only in CI.
    fn workspace(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cortex-map-cache-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create workspace");
        dir
    }

    #[test]
    fn an_empty_workspace_contributes_no_section() {
        let dir = workspace("empty");
        let cache = RepoMapCache::new();
        assert!(cache.get(&dir, 1000).is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_built_map_is_returned_and_reused() {
        let dir = workspace("reuse");
        fs::write(dir.join("lib.rs"), "pub fn exported() {}\n").unwrap();

        let cache = RepoMapCache::new();
        let first = cache.get(&dir, 1000).expect("map built");
        let second = cache.get(&dir, 1000).expect("map served from cache");
        assert!(first.contains("exported"));
        assert!(Arc::ptr_eq(&first, &second), "second call should be cached");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn different_budgets_do_not_share_an_entry() {
        let dir = workspace("budgets");
        fs::write(dir.join("lib.rs"), "pub fn exported() {}\n").unwrap();

        let cache = RepoMapCache::new();
        let small = cache.get(&dir, 4).expect("small map");
        let large = cache.get(&dir, 4000).expect("large map");
        assert!(
            !Arc::ptr_eq(&small, &large),
            "a map rendered for one budget must never be served for another"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_expired_entry_is_rebuilt() {
        let dir = workspace("expiry");
        fs::write(dir.join("lib.rs"), "pub fn first() {}\n").unwrap();

        let cache = RepoMapCache::new();
        let before = cache
            .get_with_ttl(&dir, 1000, Duration::ZERO)
            .expect("first build");
        assert!(before.contains("first"));

        fs::write(dir.join("lib.rs"), "pub fn second() {}\n").unwrap();
        let after = cache
            .get_with_ttl(&dir, 1000, Duration::ZERO)
            .expect("rebuild");
        assert!(
            after.contains("second"),
            "a zero TTL must rebuild rather than serve a stale tree"
        );

        fs::remove_dir_all(&dir).ok();
    }
}
