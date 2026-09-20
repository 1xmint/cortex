//! The persistent symbol index.
//!
//! Task **C2** of `cortex/plan/CONTEXT.md`. Replaces the in-process cache with
//! a real index: parse once, store, and reparse only what changed.
//!
//! # It deliberately does not join the migration chain
//!
//! This index lives in its own SQLite file (`.cortex/context-index.sqlite`)
//! with its own [`SCHEMA_TAG`]. It is a **derived cache**, not application
//! state: every row can be rebuilt from a checkout, so a schema change drops
//! and rebuilds rather than migrating.
//!
//! That is a deliberate opt-out of the single-integer migration counter that
//! currently couples two products — the constraint that has the whole verifier
//! line waiting on a Socials PR. A derived cache has no business inside it.
//!
//! # Incremental by construction
//!
//! `file_meta.content_hash` makes staleness detectable per file: a worker's
//! diff invalidates exactly the files it touched. An index pinned to a hash is
//! also evidence — planning against a stale index becomes a context bug with a
//! name rather than a mystery.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

use crate::extract::extract_file;
use crate::repo_map::{Reference, RepoMap, Symbol, SymbolKind};

/// Bumped whenever the shape below changes. On mismatch the index is dropped
/// and rebuilt — correct for derived data, and far safer than a migration
/// nobody will test.
pub const SCHEMA_TAG: &str = "context-index-v1";

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS file_meta (
    file         TEXT PRIMARY KEY,
    content_hash TEXT NOT NULL,
    indexed_at   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS symbols (
    id    INTEGER PRIMARY KEY,
    name  TEXT NOT NULL,
    kind  TEXT NOT NULL,
    file  TEXT NOT NULL,
    line  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file);

-- A resolved use of a name, from one file to one definition.
--
-- Resolution is by name, which is honest about its own ambiguity: two
-- definitions sharing a name both get an edge. Scope-accurate resolution needs
-- enclosing-scope tracking and belongs to C3's retrieval work — claiming it
-- here would make the table look more precise than it is.
CREATE TABLE IF NOT EXISTS symbol_refs (
    symbol_id INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
    from_file TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_symbol_refs_symbol ON symbol_refs(symbol_id);
CREATE INDEX IF NOT EXISTS idx_symbol_refs_file   ON symbol_refs(from_file);

-- Unresolved names, kept because a reference to something not yet indexed is
-- still information — and becomes an edge the moment its definition appears.
CREATE TABLE IF NOT EXISTS pending_refs (
    name      TEXT NOT NULL,
    from_file TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_pending_refs_name ON pending_refs(name);
CREATE INDEX IF NOT EXISTS idx_pending_refs_file ON pending_refs(from_file);
"#;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncStats {
    pub files_indexed: usize,
    pub files_unchanged: usize,
    pub files_removed: usize,
}

pub struct Index {
    conn: Connection,
}

impl Index {
    /// Open (or create) the index at `path`, rebuilding it if the schema tag
    /// does not match.
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create index directory: {e}"))?;
        }

        let conn =
            Connection::open(path).map_err(|e| format!("failed to open context index: {e}"))?;
        conn.execute_batch(SCHEMA)
            .map_err(|e| format!("failed to create context index schema: {e}"))?;

        let mut index = Self { conn };
        let stored: Option<String> = index
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_tag'",
                [],
                |row| row.get(0),
            )
            .ok();

        match stored.as_deref() {
            Some(SCHEMA_TAG) => {}
            Some(other) => {
                tracing::info!(
                    found = other,
                    expected = SCHEMA_TAG,
                    "context index schema changed; rebuilding from scratch"
                );
                index.clear()?;
            }
            None => index.set_schema_tag()?,
        }

        Ok(index)
    }

    /// Read access for sibling modules that query the index (C3's retrieval).
    /// Crate-internal: the connection is an implementation detail, and callers
    /// outside this crate go through typed methods.
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    fn set_schema_tag(&self) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_tag', ?1)",
                params![SCHEMA_TAG],
            )
            .map(|_| ())
            .map_err(|e| format!("failed to record schema tag: {e}"))
    }

    /// Drop everything. Safe precisely because this is derived data.
    pub fn clear(&mut self) -> Result<(), String> {
        self.conn
            .execute_batch(
                "DELETE FROM symbol_refs; DELETE FROM pending_refs; \
                 DELETE FROM symbols; DELETE FROM file_meta;",
            )
            .map_err(|e| format!("failed to clear context index: {e}"))?;
        self.set_schema_tag()
    }

    /// Bring the index in line with the tree at `root`.
    ///
    /// Reparses only files whose content hash changed, and drops files that no
    /// longer exist. A file that fails to parse is indexed as "seen, no
    /// symbols" rather than retried every sync.
    pub fn sync(&mut self, root: &Path) -> Result<SyncStats, String> {
        let mut stats = SyncStats::default();

        let known: HashMap<String, String> = {
            let mut stmt = self
                .conn
                .prepare("SELECT file, content_hash FROM file_meta")
                .map_err(|e| format!("failed to read file_meta: {e}"))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| format!("failed to read file_meta rows: {e}"))?;
            rows.filter_map(Result::ok).collect()
        };

        let mut seen: Vec<String> = Vec::new();
        let files = crate::extract::source_files(root);

        for (absolute, relative) in files {
            seen.push(relative.clone());
            let Ok(source) = std::fs::read_to_string(&absolute) else {
                continue;
            };
            let hash = content_hash(&source);

            if known.get(&relative).map(String::as_str) == Some(hash.as_str()) {
                stats.files_unchanged += 1;
                continue;
            }

            let mut extraction = crate::extract::Extraction::default();
            extract_file(&absolute, &relative, &source, &mut extraction);
            self.replace_file(&relative, &hash, &extraction)?;
            stats.files_indexed += 1;
        }

        for file in known.keys() {
            if !seen.contains(file) {
                self.forget_file(file)?;
                stats.files_removed += 1;
            }
        }

        Ok(stats)
    }

    /// Replace one file's contribution atomically. Anything less risks a
    /// half-indexed file that reads as a file with missing symbols — which is
    /// worse than one that is absent, because nothing looks wrong.
    fn replace_file(
        &mut self,
        relative: &str,
        hash: &str,
        extraction: &crate::extract::Extraction,
    ) -> Result<(), String> {
        let tx = self
            .conn
            .transaction()
            .map_err(|e| format!("failed to begin index transaction: {e}"))?;

        tx.execute(
            "DELETE FROM symbol_refs WHERE symbol_id IN (SELECT id FROM symbols WHERE file = ?1)",
            params![relative],
        )
        .map_err(|e| format!("failed to clear refs for {relative}: {e}"))?;
        tx.execute("DELETE FROM symbols WHERE file = ?1", params![relative])
            .map_err(|e| format!("failed to clear symbols for {relative}: {e}"))?;
        tx.execute(
            "DELETE FROM symbol_refs WHERE from_file = ?1",
            params![relative],
        )
        .map_err(|e| format!("failed to clear outgoing refs for {relative}: {e}"))?;
        tx.execute(
            "DELETE FROM pending_refs WHERE from_file = ?1",
            params![relative],
        )
        .map_err(|e| format!("failed to clear pending refs for {relative}: {e}"))?;

        for symbol in &extraction.symbols {
            tx.execute(
                "INSERT INTO symbols (name, kind, file, line) VALUES (?1, ?2, ?3, ?4)",
                params![
                    symbol.name,
                    symbol.kind.as_str(),
                    symbol.file,
                    symbol.line as i64
                ],
            )
            .map_err(|e| format!("failed to insert symbol {}: {e}", symbol.name))?;
        }

        // This file's new definitions may resolve references other files
        // parked earlier. Promote them now, or an index built in an unlucky
        // order would under-count forever.
        for symbol in &extraction.symbols {
            tx.execute(
                "INSERT INTO symbol_refs (symbol_id, from_file)
                 SELECT s.id, p.from_file
                 FROM symbols s JOIN pending_refs p ON p.name = s.name
                 WHERE s.name = ?1 AND s.file = ?2",
                params![symbol.name, relative],
            )
            .map_err(|e| format!("failed to promote pending refs: {e}"))?;
        }
        tx.execute(
            "DELETE FROM pending_refs WHERE name IN (SELECT name FROM symbols)",
            [],
        )
        .map_err(|e| format!("failed to prune promoted refs: {e}"))?;

        for reference in &extraction.references {
            let resolved = tx
                .execute(
                    "INSERT INTO symbol_refs (symbol_id, from_file)
                     SELECT id, ?2 FROM symbols WHERE name = ?1",
                    params![reference.name, relative],
                )
                .map_err(|e| format!("failed to insert reference {}: {e}", reference.name))?;
            if resolved == 0 {
                tx.execute(
                    "INSERT INTO pending_refs (name, from_file) VALUES (?1, ?2)",
                    params![reference.name, relative],
                )
                .map_err(|e| format!("failed to park unresolved reference: {e}"))?;
            }
        }

        tx.execute(
            "INSERT OR REPLACE INTO file_meta (file, content_hash, indexed_at)
             VALUES (?1, ?2, strftime('%s','now'))",
            params![relative, hash],
        )
        .map_err(|e| format!("failed to record file_meta for {relative}: {e}"))?;

        tx.commit()
            .map_err(|e| format!("failed to commit index transaction: {e}"))
    }

    fn forget_file(&self, relative: &str) -> Result<(), String> {
        self.conn
            .execute(
                "DELETE FROM symbol_refs WHERE symbol_id IN (SELECT id FROM symbols WHERE file = ?1)",
                params![relative],
            )
            .map_err(|e| format!("failed to drop refs for {relative}: {e}"))?;
        self.conn
            .execute("DELETE FROM symbols WHERE file = ?1", params![relative])
            .map_err(|e| format!("failed to drop symbols for {relative}: {e}"))?;
        self.conn
            .execute(
                "DELETE FROM symbol_refs WHERE from_file = ?1",
                params![relative],
            )
            .map_err(|e| format!("failed to drop outgoing refs for {relative}: {e}"))?;
        self.conn
            .execute(
                "DELETE FROM pending_refs WHERE from_file = ?1",
                params![relative],
            )
            .map_err(|e| format!("failed to drop pending refs for {relative}: {e}"))?;
        self.conn
            .execute("DELETE FROM file_meta WHERE file = ?1", params![relative])
            .map(|_| ())
            .map_err(|e| format!("failed to drop file_meta for {relative}: {e}"))
    }

    /// Files that reference anything defined in `file`. The seed of C4's
    /// impact set: a lease should claim what a change actually touches, not
    /// the paths someone thought to list.
    pub fn dependents_of(&self, file: &str) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT DISTINCT r.from_file
                 FROM symbol_refs r JOIN symbols s ON s.id = r.symbol_id
                 WHERE s.file = ?1 AND r.from_file != ?1
                 ORDER BY r.from_file",
            )
            .map_err(|e| format!("failed to prepare dependents query: {e}"))?;
        let rows = stmt
            .query_map(params![file], |row| row.get::<_, String>(0))
            .map_err(|e| format!("failed to query dependents: {e}"))?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Build a [`RepoMap`] from stored rows rather than by reparsing.
    pub fn repo_map(&self) -> Result<RepoMap, String> {
        let mut symbol_stmt = self
            .conn
            .prepare("SELECT name, kind, file, line FROM symbols")
            .map_err(|e| format!("failed to prepare symbols query: {e}"))?;
        let symbols: Vec<Symbol> = symbol_stmt
            .query_map([], |row| {
                Ok(Symbol {
                    name: row.get(0)?,
                    kind: kind_from_str(&row.get::<_, String>(1)?),
                    file: row.get(2)?,
                    line: row.get::<_, i64>(3)? as usize,
                })
            })
            .map_err(|e| format!("failed to query symbols: {e}"))?
            .filter_map(Result::ok)
            .collect();

        let mut ref_stmt = self
            .conn
            .prepare(
                "SELECT s.name, r.from_file
                 FROM symbol_refs r JOIN symbols s ON s.id = r.symbol_id",
            )
            .map_err(|e| format!("failed to prepare refs query: {e}"))?;
        let references: Vec<Reference> = ref_stmt
            .query_map([], |row| {
                Ok(Reference {
                    name: row.get(0)?,
                    file: row.get(1)?,
                })
            })
            .map_err(|e| format!("failed to query refs: {e}"))?
            .filter_map(Result::ok)
            .collect();

        Ok(RepoMap::build(symbols, &references))
    }
}

fn kind_from_str(raw: &str) -> SymbolKind {
    match raw {
        "fn" => SymbolKind::Function,
        "method" => SymbolKind::Method,
        "class" => SymbolKind::Class,
        "interface" => SymbolKind::Interface,
        "mod" => SymbolKind::Module,
        "macro" => SymbolKind::Macro,
        _ => SymbolKind::Other,
    }
}

fn content_hash(source: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source.as_bytes());
    hex::encode(hasher.finalize())
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
                "cortex-index-{tag}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&root).expect("create workspace");
            Self { root }
        }

        fn write(&self, name: &str, contents: &str) {
            fs::write(self.root.join(name), contents).expect("write file");
        }

        fn index(&self) -> Index {
            Index::open(&self.root.join(".cortex").join("context-index.sqlite"))
                .expect("open index")
        }
    }

    impl Drop for Workspace {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).ok();
        }
    }

    #[test]
    fn a_first_sync_indexes_everything() {
        let ws = Workspace::new("first");
        ws.write("lib.rs", "pub fn exported() {}\n");
        let mut index = ws.index();

        let stats = index.sync(&ws.root).expect("sync");
        assert_eq!(stats.files_indexed, 1);
        assert_eq!(stats.files_unchanged, 0);

        let map = index.repo_map().expect("map");
        assert!(map.symbols.iter().any(|s| s.symbol.name == "exported"));
    }

    #[test]
    fn an_unchanged_file_is_not_reparsed() {
        let ws = Workspace::new("unchanged");
        ws.write("lib.rs", "pub fn exported() {}\n");
        let mut index = ws.index();

        index.sync(&ws.root).expect("first sync");
        let stats = index.sync(&ws.root).expect("second sync");
        assert_eq!(stats.files_indexed, 0);
        assert_eq!(stats.files_unchanged, 1);
    }

    #[test]
    fn a_changed_file_is_reindexed_and_old_symbols_go() {
        let ws = Workspace::new("changed");
        ws.write("lib.rs", "pub fn before() {}\n");
        let mut index = ws.index();
        index.sync(&ws.root).expect("first sync");

        ws.write("lib.rs", "pub fn after() {}\n");
        let stats = index.sync(&ws.root).expect("second sync");
        assert_eq!(stats.files_indexed, 1);

        let map = index.repo_map().expect("map");
        let names: Vec<&str> = map.symbols.iter().map(|s| s.symbol.name.as_str()).collect();
        assert!(names.contains(&"after"));
        assert!(
            !names.contains(&"before"),
            "a stale symbol is worse than a missing one — nothing looks wrong"
        );
    }

    #[test]
    fn a_deleted_file_is_forgotten() {
        let ws = Workspace::new("deleted");
        ws.write("lib.rs", "pub fn gone() {}\n");
        let mut index = ws.index();
        index.sync(&ws.root).expect("first sync");

        fs::remove_file(ws.root.join("lib.rs")).expect("remove");
        let stats = index.sync(&ws.root).expect("second sync");
        assert_eq!(stats.files_removed, 1);
        assert!(index.repo_map().expect("map").symbols.is_empty());
    }

    #[test]
    fn dependents_are_found_across_files() {
        let ws = Workspace::new("dependents");
        ws.write("auth.rs", "pub fn validate_token() {}\n");
        ws.write("api.rs", "fn handler() { validate_token(); }\n");
        let mut index = ws.index();
        index.sync(&ws.root).expect("sync");

        let dependents = index.dependents_of("auth.rs").expect("dependents");
        assert_eq!(dependents, vec!["api.rs".to_string()]);
    }

    #[test]
    fn a_reference_indexed_before_its_definition_still_resolves() {
        // Index order is alphabetical, so api.rs is parsed before auth.rs and
        // its call has nothing to point at yet. Without promotion of parked
        // references the edge would be lost permanently.
        let ws = Workspace::new("ordering");
        ws.write("api.rs", "fn handler() { later_defined(); }\n");
        let mut index = ws.index();
        index.sync(&ws.root).expect("first sync");

        ws.write("zz_defs.rs", "pub fn later_defined() {}\n");
        index.sync(&ws.root).expect("second sync");

        let dependents = index.dependents_of("zz_defs.rs").expect("dependents");
        assert_eq!(dependents, vec!["api.rs".to_string()]);
    }

    #[test]
    fn a_schema_tag_mismatch_rebuilds_rather_than_migrates() {
        let ws = Workspace::new("schema");
        ws.write("lib.rs", "pub fn exported() {}\n");
        let index_path = ws.root.join(".cortex").join("context-index.sqlite");

        {
            let mut index = Index::open(&index_path).expect("open");
            index.sync(&ws.root).expect("sync");
            index
                .conn
                .execute(
                    "UPDATE meta SET value = 'context-index-v0' WHERE key = 'schema_tag'",
                    [],
                )
                .expect("downgrade tag");
        }

        let index = Index::open(&index_path).expect("reopen");
        assert!(
            index.repo_map().expect("map").symbols.is_empty(),
            "a tag mismatch must drop derived data, not migrate it"
        );
    }
}
