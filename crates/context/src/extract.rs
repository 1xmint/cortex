//! Pulling symbols and references out of source with tree-sitter.
//!
//! Uses each grammar's own **tags query** rather than hand-written patterns.
//! Those queries are maintained upstream alongside the grammar, they already
//! agree on a capture vocabulary (`definition.function`, `reference.call`,
//! `@name`), and they are the same mechanism ctags-style tooling has used for
//! years. Writing our own would mean maintaining three of them badly.
//!
//! Everything here is best-effort by design: a file that fails to parse
//! contributes nothing and does not fail the walk. A repo map is an aid to
//! planning, not a gate — an unparseable file should cost us one file's worth
//! of context, never a run.

use std::path::{Path, PathBuf};

use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Parser, Query, QueryCursor};

use crate::repo_map::{Reference, Symbol, SymbolKind};

/// Directories never worth indexing: build output, dependencies, and history.
/// `archive/` is repo-specific and deliberate — it holds 6,000+ files marked
/// "reference only, do not build from".
const SKIP_DIRS: [&str; 8] = [
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    "archive",
    "vendor",
];

/// Files above this size are skipped: generated bundles and lockfiles dominate
/// a map without informing it.
const MAX_FILE_BYTES: u64 = 512 * 1024;

#[derive(Default)]
pub struct Extraction {
    pub symbols: Vec<Symbol>,
    pub references: Vec<Reference>,
}

struct GrammarSpec {
    language: Language,
    tags_query: String,
}

/// TypeScript's own tags query covers only TypeScript-specific constructs —
/// signatures, interfaces, abstract classes. Plain `function` and `class`
/// declarations are JavaScript nodes, and the TS grammar is a superset of the
/// JS one, so a TypeScript file needs **both** queries or an ordinary
/// `export function foo()` produces no symbol at all.
///
/// Caught by a test, not by reading: the first version of this file used the
/// TypeScript query alone and found nothing in `export function fetchUser`.
fn typescript_tags_query() -> String {
    format!(
        "{}\n{}",
        tree_sitter_javascript::TAGS_QUERY,
        tree_sitter_typescript::TAGS_QUERY
    )
}

fn grammar_for(path: &Path) -> Option<GrammarSpec> {
    let extension = path.extension()?.to_str()?;
    match extension {
        "rs" => Some(GrammarSpec {
            language: tree_sitter_rust::LANGUAGE.into(),
            tags_query: tree_sitter_rust::TAGS_QUERY.to_string(),
        }),
        "ts" => Some(GrammarSpec {
            language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            tags_query: typescript_tags_query(),
        }),
        "tsx" => Some(GrammarSpec {
            language: tree_sitter_typescript::LANGUAGE_TSX.into(),
            tags_query: typescript_tags_query(),
        }),
        "js" | "jsx" | "mjs" | "cjs" => Some(GrammarSpec {
            language: tree_sitter_javascript::LANGUAGE.into(),
            tags_query: tree_sitter_javascript::TAGS_QUERY.to_string(),
        }),
        _ => None,
    }
}

/// Every source file under `root` a grammar can read, as
/// `(absolute, repo-relative)` pairs, in a stable order.
///
/// Public so the incremental index can walk without extracting: most files in
/// a sync are unchanged and must not be reparsed.
pub fn source_files(root: &Path) -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files);
    // Sorted so a sync visits files in the same order every time, which makes
    // the reference-promotion path deterministic and its tests meaningful.
    files.sort_by(|a, b| a.1.cmp(&b.1));
    files
}

/// Walk `root` and extract every symbol and reference we can parse.
pub fn extract_repo(root: &Path) -> Extraction {
    let mut extraction = Extraction::default();
    let files = source_files(root);

    for (absolute, relative) in files {
        match std::fs::read_to_string(&absolute) {
            Ok(source) => extract_file(&absolute, &relative, &source, &mut extraction),
            Err(error) => {
                // Binary or unreadable. Not worth a warning per file.
                tracing::debug!(path = %relative, %error, "skipping unreadable file");
            }
        }
    }

    extraction
}

/// Parse one file's contents. Separated from the walk so it can be tested
/// without a directory tree.
pub fn extract_file(path: &Path, relative: &str, source: &str, extraction: &mut Extraction) {
    let Some(spec) = grammar_for(path) else {
        return;
    };

    let mut parser = Parser::new();
    if parser.set_language(&spec.language).is_err() {
        tracing::warn!(path = %relative, "grammar rejected by parser; skipping");
        return;
    }
    let Some(tree) = parser.parse(source, None) else {
        tracing::debug!(path = %relative, "file did not parse; skipping");
        return;
    };
    let Ok(query) = Query::new(&spec.language, &spec.tags_query) else {
        tracing::warn!(path = %relative, "tags query failed to compile; skipping");
        return;
    };

    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());

    while let Some(matched) = matches.next() {
        // A tags match pairs a `@name` capture (the identifier) with a
        // `@definition.*` or `@reference.*` capture (what it is). Both are
        // needed; a match missing either is not usable.
        let mut name: Option<(&str, usize)> = None;
        let mut role: Option<&str> = None;

        for capture in matched.captures() {
            let capture_name = capture_names[capture.index as usize];
            if capture_name == "name" {
                if let Ok(text) = capture.node.utf8_text(source.as_bytes()) {
                    name = Some((text, capture.node.start_position().row + 1));
                }
            } else if capture_name.starts_with("definition.")
                || capture_name.starts_with("reference.")
            {
                role = Some(capture_name);
            }
        }

        let (Some((text, line)), Some(role)) = (name, role) else {
            continue;
        };

        if role.starts_with("definition.") {
            extraction.symbols.push(Symbol {
                name: text.to_string(),
                kind: SymbolKind::from_capture(role).unwrap_or(SymbolKind::Other),
                file: relative.to_string(),
                line,
            });
        } else {
            extraction.references.push(Reference {
                name: text.to_string(),
                file: relative.to_string(),
            });
        }
    }
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(PathBuf, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        if file_type.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
                continue;
            }
            collect_files(root, &path, out);
        } else if file_type.is_file() {
            if grammar_for(&path).is_none() {
                continue;
            }
            if entry.metadata().map(|m| m.len()).unwrap_or(0) > MAX_FILE_BYTES {
                continue;
            }
            if let Some(relative) = relative_path(root, &path) {
                out.push((path, relative));
            }
        }
    }
}

/// Repo-relative and forward-slashed, so a map built on Windows reads exactly
/// like one built in a Linux container — the same tree must produce the same
/// map, or prompt caching is wasted and diffs are noise.
fn relative_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    Some(
        relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract(name: &str, source: &str) -> Extraction {
        let mut extraction = Extraction::default();
        extract_file(Path::new(name), name, source, &mut extraction);
        extraction
    }

    #[test]
    fn rust_definitions_are_found() {
        let extraction = extract(
            "auth.rs",
            r#"
pub fn validate_token(raw: &str) -> bool { helper(raw) }
fn helper(raw: &str) -> bool { !raw.is_empty() }
"#,
        );
        let names: Vec<&str> = extraction.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"validate_token"), "got {names:?}");
        assert!(names.contains(&"helper"), "got {names:?}");
    }

    #[test]
    fn rust_calls_are_recorded_as_references() {
        let extraction = extract("api.rs", "fn caller() { validate_token(\"x\"); }");
        let names: Vec<&str> = extraction
            .references
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert!(names.contains(&"validate_token"), "got {names:?}");
    }

    #[test]
    fn typescript_definitions_are_found() {
        let extraction = extract(
            "api.ts",
            "export function fetchUser(id: string) { return id; }",
        );
        let names: Vec<&str> = extraction.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"fetchUser"), "got {names:?}");
    }

    #[test]
    fn symbols_carry_a_one_based_line() {
        let extraction = extract("a.rs", "\n\nfn third_line() {}\n");
        let symbol = extraction
            .symbols
            .iter()
            .find(|s| s.name == "third_line")
            .expect("symbol found");
        assert_eq!(symbol.line, 3);
    }

    #[test]
    fn an_unknown_extension_is_ignored() {
        let extraction = extract("notes.md", "# not code");
        assert!(extraction.symbols.is_empty());
        assert!(extraction.references.is_empty());
    }

    #[test]
    fn a_file_that_does_not_parse_costs_one_file_not_a_run() {
        let extraction = extract("broken.rs", "fn (((( {{{{ ????");
        // The point is that this returns rather than panicking; whatever the
        // error recovery salvages is a bonus.
        assert!(extraction.symbols.len() < 10);
    }

    #[test]
    fn paths_are_forward_slashed_regardless_of_platform() {
        let root = Path::new("repo");
        let nested = root.join("crates").join("api").join("src.rs");
        assert_eq!(
            relative_path(root, &nested).as_deref(),
            Some("crates/api/src.rs")
        );
    }
}
