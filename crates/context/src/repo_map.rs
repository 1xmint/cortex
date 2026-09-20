//! The repo map: a ranked one-page skeleton of a repository.
//!
//! Task **C1** of `cortex/plan/CONTEXT.md`. The decomposer currently passes
//! along whatever `file_paths` the user typed, so plans are blind to anything
//! the user did not think to name. This is the cheapest fix with the largest
//! effect: a ranked skeleton prepended to planning prompts, so the model sees
//! what the repository *is* before deciding what to touch.
//!
//! The ranking rule, which is the whole idea: **a symbol matters in
//! proportion to how many other files reference it.** A function called from
//! nine modules is load-bearing; one called from its own file is an
//! implementation detail. Cross-file references are what separate the two,
//! and self-references are deliberately excluded — otherwise a long file full
//! of internal calls outranks the interface everything depends on.
//!
//! This module is pure: it takes symbols and references and produces text.
//! Parsing lives in [`crate::extract`], so every rule here is unit testable
//! without a repository on disk.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Rough characters-per-token, used only to fit the map inside an existing
/// planning budget. Deliberately conservative: overshooting the budget costs a
/// truncated prompt, undershooting costs a few lines of map.
const CHARS_PER_TOKEN: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Interface,
    Module,
    Macro,
    Other,
}

impl SymbolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Function => "fn",
            Self::Method => "method",
            Self::Class => "class",
            Self::Interface => "interface",
            Self::Module => "mod",
            Self::Macro => "macro",
            Self::Other => "sym",
        }
    }

    /// Map a tree-sitter tags capture (`definition.function`, …) onto a kind.
    /// Grammars agree on this vocabulary, which is why the tags queries are
    /// worth using instead of hand-written ones per language.
    pub fn from_capture(capture: &str) -> Option<Self> {
        let suffix = capture.strip_prefix("definition.")?;
        Some(match suffix {
            "function" => Self::Function,
            "method" => Self::Method,
            "class" | "struct" | "enum" => Self::Class,
            "interface" | "trait" => Self::Interface,
            "module" => Self::Module,
            "macro" => Self::Macro,
            _ => Self::Other,
        })
    }
}

/// A definition found in the tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    /// Repo-relative, forward-slashed, so a map generated on Windows reads
    /// the same as one generated in a Linux container.
    pub file: String,
    pub line: usize,
}

/// A use of a name, wherever it occurred.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    pub name: String,
    pub file: String,
}

/// A symbol with its computed importance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RankedSymbol {
    pub symbol: Symbol,
    /// Number of *other* files that reference this name.
    pub referencing_files: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoMap {
    /// Ranked most-referenced first; ties broken deterministically so the same
    /// tree always produces the same map. A map that reshuffles between runs
    /// would poison prompt caching for no benefit.
    pub symbols: Vec<RankedSymbol>,
    pub files_scanned: usize,
}

impl RepoMap {
    pub fn build(symbols: Vec<Symbol>, references: &[Reference]) -> Self {
        let files_scanned = symbols
            .iter()
            .map(|symbol| symbol.file.as_str())
            .chain(references.iter().map(|reference| reference.file.as_str()))
            .collect::<std::collections::BTreeSet<_>>()
            .len();

        // name -> set of files referencing it. A file referencing a name ten
        // times counts once: breadth of use is the signal, not chattiness.
        let mut referencing: HashMap<&str, std::collections::BTreeSet<&str>> = HashMap::new();
        for reference in references {
            referencing
                .entry(reference.name.as_str())
                .or_default()
                .insert(reference.file.as_str());
        }

        let mut ranked: Vec<RankedSymbol> = symbols
            .iter()
            .map(|symbol| {
                let count = referencing
                    .get(symbol.name.as_str())
                    .map(|files| {
                        files
                            .iter()
                            .filter(|file| **file != symbol.file.as_str())
                            .count()
                    })
                    .unwrap_or(0);
                RankedSymbol {
                    symbol: symbol.clone(),
                    referencing_files: count,
                }
            })
            .collect();

        ranked.sort_by(|a, b| {
            b.referencing_files
                .cmp(&a.referencing_files)
                .then_with(|| a.symbol.file.cmp(&b.symbol.file))
                .then_with(|| a.symbol.line.cmp(&b.symbol.line))
                .then_with(|| a.symbol.name.cmp(&b.symbol.name))
        });

        Self {
            symbols: ranked,
            files_scanned,
        }
    }

    /// Render the map to fit within `token_budget`, grouped by file and
    /// ordered by the most important symbol each file contains.
    ///
    /// Truncation is explicit. A silently shortened map would make a model
    /// confidently wrong about what a repository contains, which is worse
    /// than a model that knows it is looking at an excerpt.
    pub fn render(&self, token_budget: usize) -> String {
        let char_budget = token_budget.saturating_mul(CHARS_PER_TOKEN);
        if char_budget == 0 || self.symbols.is_empty() {
            return String::new();
        }

        // Keep files in first-appearance order of the ranked list, so the most
        // referenced code appears first.
        let mut order: Vec<&str> = Vec::new();
        let mut by_file: BTreeMap<&str, Vec<&RankedSymbol>> = BTreeMap::new();
        for ranked in &self.symbols {
            let file = ranked.symbol.file.as_str();
            if !by_file.contains_key(file) {
                order.push(file);
            }
            by_file.entry(file).or_default().push(ranked);
        }

        let header = format!("Repository map ({} files)\n", self.files_scanned);
        let mut out = String::new();
        out.push_str(&header);

        let mut truncated = false;
        'files: for file in order {
            let entries = &by_file[file];
            let file_line = format!("\n{file}\n");
            if out.len() + file_line.len() > char_budget {
                truncated = true;
                break;
            }
            out.push_str(&file_line);

            for ranked in entries {
                let line = format!(
                    "  {} {} :{}{}\n",
                    ranked.symbol.kind.as_str(),
                    ranked.symbol.name,
                    ranked.symbol.line,
                    if ranked.referencing_files > 0 {
                        format!("  ({} refs)", ranked.referencing_files)
                    } else {
                        String::new()
                    }
                );
                if out.len() + line.len() > char_budget {
                    truncated = true;
                    break 'files;
                }
                out.push_str(&line);
            }
        }

        if truncated {
            out.push_str("… map truncated to fit the context budget\n");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(name: &str, file: &str, line: usize) -> Symbol {
        Symbol {
            name: name.to_string(),
            kind: SymbolKind::Function,
            file: file.to_string(),
            line,
        }
    }

    fn reference(name: &str, file: &str) -> Reference {
        Reference {
            name: name.to_string(),
            file: file.to_string(),
        }
    }

    #[test]
    fn a_widely_used_symbol_outranks_a_local_one() {
        let symbols = vec![
            symbol("validate_token", "auth.rs", 10),
            symbol("helper", "auth.rs", 90),
        ];
        let references = vec![
            reference("validate_token", "api.rs"),
            reference("validate_token", "worker.rs"),
            reference("helper", "auth.rs"),
        ];
        let map = RepoMap::build(symbols, &references);
        assert_eq!(map.symbols[0].symbol.name, "validate_token");
        assert_eq!(map.symbols[0].referencing_files, 2);
    }

    #[test]
    fn self_references_do_not_inflate_a_symbol() {
        // Otherwise a long file full of internal calls outranks the interface
        // every other module depends on.
        let symbols = vec![symbol("inner", "util.rs", 5)];
        let references = vec![
            reference("inner", "util.rs"),
            reference("inner", "util.rs"),
            reference("inner", "util.rs"),
        ];
        let map = RepoMap::build(symbols, &references);
        assert_eq!(map.symbols[0].referencing_files, 0);
    }

    #[test]
    fn repeated_use_within_one_file_counts_once() {
        let symbols = vec![symbol("parse", "parse.rs", 1)];
        let references = vec![reference("parse", "main.rs"), reference("parse", "main.rs")];
        let map = RepoMap::build(symbols, &references);
        assert_eq!(map.symbols[0].referencing_files, 1);
    }

    #[test]
    fn ranking_is_stable_for_the_same_tree() {
        let symbols = vec![
            symbol("b", "z.rs", 1),
            symbol("a", "a.rs", 1),
            symbol("c", "a.rs", 2),
        ];
        let first = RepoMap::build(symbols.clone(), &[]);
        let second = RepoMap::build(symbols, &[]);
        assert_eq!(first, second);
        // Deterministic tie-break: file, then line.
        let order: Vec<&str> = first
            .symbols
            .iter()
            .map(|r| r.symbol.name.as_str())
            .collect();
        assert_eq!(order, vec!["a", "c", "b"]);
    }

    #[test]
    fn rendering_groups_by_file_and_leads_with_the_most_referenced() {
        let symbols = vec![symbol("minor", "b.rs", 1), symbol("major", "a.rs", 4)];
        let references = vec![reference("major", "z.rs")];
        let map = RepoMap::build(symbols, &references);
        let text = map.render(1000);
        let a = text.find("a.rs").expect("a.rs present");
        let b = text.find("b.rs").expect("b.rs present");
        assert!(a < b, "the referenced file should come first:\n{text}");
        assert!(text.contains("(1 refs)"));
    }

    #[test]
    fn a_tight_budget_truncates_visibly_rather_than_silently() {
        let symbols: Vec<Symbol> = (0..200)
            .map(|i| symbol(&format!("symbol_{i}"), &format!("file_{i}.rs"), i))
            .collect();
        let map = RepoMap::build(symbols, &[]);
        let text = map.render(50);
        assert!(text.len() <= 50 * CHARS_PER_TOKEN + 64);
        assert!(
            text.contains("truncated"),
            "a shortened map must say so, or the model is confidently wrong"
        );
    }

    #[test]
    fn a_zero_budget_yields_nothing_rather_than_a_header() {
        let map = RepoMap::build(vec![symbol("a", "a.rs", 1)], &[]);
        assert!(map.render(0).is_empty());
    }

    #[test]
    fn an_empty_repository_renders_nothing() {
        assert!(RepoMap::build(Vec::new(), &[]).render(1000).is_empty());
    }

    #[test]
    fn capture_names_map_onto_kinds() {
        assert_eq!(
            SymbolKind::from_capture("definition.function"),
            Some(SymbolKind::Function)
        );
        assert_eq!(
            SymbolKind::from_capture("definition.trait"),
            Some(SymbolKind::Interface)
        );
        assert_eq!(SymbolKind::from_capture("reference.call"), None);
    }
}
