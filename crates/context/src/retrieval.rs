//! Hybrid retrieval over the symbol index.
//!
//! Task **C3** of `cortex/plan/CONTEXT.md`. C2 stores what a repository
//! contains; this decides which slice of it a step should actually see.
//!
//! Two signals, combined, in the order CONTEXT.md specifies:
//!
//! - **Lexical** — does the name look like what the task is about?
//! - **Structural** — how much of the repository depends on it, and is it
//!   reachable from the files the task already names?
//!
//! Neither alone is enough. Lexical matching alone surfaces a test helper
//! called `validate` ahead of the `validate_token` everything imports.
//! Structural ranking alone surfaces the most-referenced symbols in the
//! repository regardless of what was asked. The product of the two is what
//! makes "the right 500k tokens" different from "the nearest ones".
//!
//! Deliberately no embeddings. CONTEXT.md defers them until the first two
//! signals show a measured recall gap, because they add per-index COGS, a
//! provider dependency, and a data-handling surface over customer code.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::index::Index;
use crate::repo_map::{Symbol, SymbolKind};

/// Weight applied to an exact name match versus a substring one. An exact hit
/// is a much stronger claim about intent than a shared prefix.
const EXACT_MATCH_BONUS: f64 = 4.0;
/// Weight on a symbol living in, or reachable from, a file the task named.
const SEED_PROXIMITY_BONUS: f64 = 3.0;
/// Diminishing weight on how widely a symbol is referenced. Logarithmic
/// because the step from 1 to 10 dependents means far more than 90 to 100.
const REFERENCE_WEIGHT: f64 = 1.5;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    /// Words from the task objective, already lowercased and split.
    pub terms: Vec<String>,
    /// Files the task explicitly names. Not a filter — a starting point.
    pub seed_files: Vec<String>,
}

impl Query {
    /// Split an objective into search terms.
    ///
    /// Stop words are dropped because "the", "and", "fix" match everything and
    /// therefore rank nothing. Short fragments go too: a two-letter substring
    /// matches most identifiers in any codebase.
    pub fn from_objective(objective: &str, seed_files: Vec<String>) -> Self {
        const STOP_WORDS: [&str; 24] = [
            "the", "and", "for", "with", "that", "this", "from", "into", "when", "then", "fix",
            "add", "make", "use", "using", "should", "would", "code", "file", "files", "please",
            "need", "want", "get",
        ];

        let terms = objective
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|word| word.len() > 2)
            .map(|word| word.to_ascii_lowercase())
            .filter(|word| !STOP_WORDS.contains(&word.as_str()))
            .collect::<Vec<_>>();

        Self { terms, seed_files }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.seed_files.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    pub symbol: Symbol,
    pub score: f64,
    /// Why this was retrieved, in words. Carried so a plan that went wrong can
    /// be debugged without re-running retrieval — a ranked list with no
    /// reasoning is impossible to argue with.
    pub reason: String,
}

impl Index {
    /// Rank symbols against a query. Highest score first, deterministic ties.
    pub fn search(&self, query: &Query, limit: usize) -> Result<Vec<Hit>, String> {
        if query.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        // Files reachable from the seeds: the seeds themselves, plus anything
        // that depends on them. A change to a seed file can break its
        // dependents, so they are part of the neighbourhood by definition.
        let mut neighbourhood: HashSet<String> = query.seed_files.iter().cloned().collect();
        for seed in &query.seed_files {
            for dependent in self.dependents_of(seed)? {
                neighbourhood.insert(dependent);
            }
        }

        let mut stmt = self
            .conn()
            .prepare(
                "SELECT s.name, s.kind, s.file, s.line, COUNT(DISTINCT r.from_file)
                 FROM symbols s
                 LEFT JOIN symbol_refs r ON r.symbol_id = s.id AND r.from_file != s.file
                 GROUP BY s.id",
            )
            .map_err(|e| format!("failed to prepare search query: {e}"))?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    Symbol {
                        name: row.get(0)?,
                        kind: kind_from_str(&row.get::<_, String>(1)?),
                        file: row.get(2)?,
                        line: row.get::<_, i64>(3)? as usize,
                    },
                    row.get::<_, i64>(4)? as usize,
                ))
            })
            .map_err(|e| format!("failed to run search query: {e}"))?;

        let mut hits: Vec<Hit> = Vec::new();
        for row in rows.filter_map(Result::ok) {
            let (symbol, referencing_files) = row;
            let lowered = symbol.name.to_ascii_lowercase();

            let mut score = 0.0;
            let mut reasons: Vec<String> = Vec::new();

            for term in &query.terms {
                if lowered == *term {
                    score += EXACT_MATCH_BONUS;
                    reasons.push(format!("name matches '{term}'"));
                } else if lowered.contains(term.as_str()) {
                    score += 1.0;
                    reasons.push(format!("name contains '{term}'"));
                }
            }

            if neighbourhood.contains(&symbol.file) {
                score += SEED_PROXIMITY_BONUS;
                reasons.push(if query.seed_files.contains(&symbol.file) {
                    "in a file the task names".to_string()
                } else {
                    "in a file that depends on one the task names".to_string()
                });
            }

            // Nothing matched textually or structurally: this symbol is simply
            // popular, which is not a reason to show it.
            if score == 0.0 {
                continue;
            }

            if referencing_files > 0 {
                score += REFERENCE_WEIGHT * ((referencing_files + 1) as f64).ln();
                reasons.push(format!("referenced by {referencing_files} files"));
            }

            hits.push(Hit {
                symbol,
                score,
                reason: reasons.join("; "),
            });
        }

        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.symbol.file.cmp(&b.symbol.file))
                .then_with(|| a.symbol.line.cmp(&b.symbol.line))
                .then_with(|| a.symbol.name.cmp(&b.symbol.name))
        });
        hits.truncate(limit);
        Ok(hits)
    }
}

/// Render ranked hits into a prompt section that fits `token_budget`.
///
/// Grouped by file, because a step reasons about files, and a flat list of
/// symbols scattered across a repository reads as noise.
pub fn pack(hits: &[Hit], token_budget: usize) -> String {
    const CHARS_PER_TOKEN: usize = 4;
    let char_budget = token_budget.saturating_mul(CHARS_PER_TOKEN);
    if char_budget == 0 || hits.is_empty() {
        return String::new();
    }

    let mut order: Vec<&str> = Vec::new();
    let mut by_file: BTreeMap<&str, Vec<&Hit>> = BTreeMap::new();
    for hit in hits {
        let file = hit.symbol.file.as_str();
        if !by_file.contains_key(file) {
            order.push(file);
        }
        by_file.entry(file).or_default().push(hit);
    }

    let mut out = String::from("Relevant code for this task\n");
    let mut truncated = false;

    'files: for file in order {
        let heading = format!("\n{file}\n");
        if out.len() + heading.len() > char_budget {
            truncated = true;
            break;
        }
        out.push_str(&heading);

        for hit in &by_file[file] {
            let line = format!(
                "  {} {} :{}  — {}\n",
                hit.symbol.kind.as_str(),
                hit.symbol.name,
                hit.symbol.line,
                hit.reason
            );
            if out.len() + line.len() > char_budget {
                truncated = true;
                break 'files;
            }
            out.push_str(&line);
        }
    }

    if truncated {
        out.push_str("… retrieval truncated to fit the context budget\n");
    }
    out
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
                "cortex-retrieval-{tag}-{}-{}",
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

    #[test]
    fn objective_parsing_drops_noise() {
        let query = Query::from_objective("Please fix the validate_token function", Vec::new());
        assert!(query.terms.contains(&"validate_token".to_string()));
        assert!(!query.terms.contains(&"the".to_string()));
        assert!(!query.terms.contains(&"fix".to_string()));
    }

    #[test]
    fn an_exact_name_match_outranks_a_substring_one() {
        let ws = Workspace::new("exact");
        ws.write("auth.rs", "pub fn validate() {}\n");
        ws.write("helper.rs", "pub fn validate_token_helper_thing() {}\n");
        let index = ws.indexed();

        let query = Query::from_objective("validate", Vec::new());
        let hits = index.search(&query, 10).expect("search");
        assert_eq!(hits[0].symbol.name, "validate");
        assert!(hits[0].reason.contains("matches"));
    }

    #[test]
    fn a_popular_but_irrelevant_symbol_is_not_retrieved() {
        // Structural ranking alone would surface this; it has nothing to do
        // with the task.
        let ws = Workspace::new("popular");
        ws.write("util.rs", "pub fn everywhere() {}\n");
        ws.write("a.rs", "fn a() { everywhere(); }\n");
        ws.write("b.rs", "fn b() { everywhere(); }\n");
        ws.write("auth.rs", "pub fn validate_token() {}\n");
        let index = ws.indexed();

        let query = Query::from_objective("validate_token", Vec::new());
        let hits = index.search(&query, 10).expect("search");
        assert!(hits.iter().all(|hit| hit.symbol.name != "everywhere"));
        assert!(hits.iter().any(|hit| hit.symbol.name == "validate_token"));
    }

    #[test]
    fn seed_files_pull_in_their_dependents() {
        let ws = Workspace::new("seeds");
        ws.write("auth.rs", "pub fn validate_token() {}\n");
        ws.write("api.rs", "fn handler() { validate_token(); }\n");
        let index = ws.indexed();

        // No search terms at all — everything here comes from the seed.
        let query = Query {
            terms: Vec::new(),
            seed_files: vec!["auth.rs".to_string()],
        };
        let hits = index.search(&query, 10).expect("search");
        let files: Vec<&str> = hits.iter().map(|h| h.symbol.file.as_str()).collect();
        assert!(files.contains(&"auth.rs"));
        assert!(
            files.contains(&"api.rs"),
            "a file that depends on a seed is part of the neighbourhood"
        );
    }

    #[test]
    fn an_empty_query_retrieves_nothing() {
        let ws = Workspace::new("empty");
        ws.write("auth.rs", "pub fn validate_token() {}\n");
        let index = ws.indexed();
        assert!(index
            .search(&Query::default(), 10)
            .expect("search")
            .is_empty());
    }

    #[test]
    fn results_are_capped_by_the_limit() {
        let ws = Workspace::new("limit");
        let source: String = (0..30)
            .map(|i| format!("pub fn validate_{i}() {{}}\n"))
            .collect();
        ws.write("many.rs", &source);
        let index = ws.indexed();

        let query = Query::from_objective("validate", Vec::new());
        assert_eq!(index.search(&query, 5).expect("search").len(), 5);
    }

    #[test]
    fn packing_explains_itself_and_respects_the_budget() {
        let hits = vec![Hit {
            symbol: Symbol {
                name: "validate_token".to_string(),
                kind: SymbolKind::Function,
                file: "auth.rs".to_string(),
                line: 12,
            },
            score: 7.0,
            reason: "name matches 'validate_token'; referenced by 3 files".to_string(),
        }];

        let packed = pack(&hits, 1000);
        assert!(packed.contains("auth.rs"));
        assert!(packed.contains("validate_token"));
        assert!(
            packed.contains("referenced by 3 files"),
            "a ranked list with no reasoning is impossible to argue with"
        );

        assert!(pack(&hits, 0).is_empty());
    }

    #[test]
    fn packing_truncates_visibly() {
        let hits: Vec<Hit> = (0..200)
            .map(|i| Hit {
                symbol: Symbol {
                    name: format!("symbol_{i}"),
                    kind: SymbolKind::Function,
                    file: format!("file_{i}.rs"),
                    line: i,
                },
                score: 1.0,
                reason: "matched".to_string(),
            })
            .collect();
        let packed = pack(&hits, 40);
        assert!(packed.contains("truncated"));
    }
}
