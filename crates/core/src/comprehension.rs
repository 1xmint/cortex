//! Invariant 28: comprehension quality is declared, never assumed.
//!
//! Phase 29.1 found the repo map is real and better than the plan implied —
//! `crates/context` does tree-sitter symbol and reference extraction, ranks a
//! map, and `scheduler.rs` renders it into a step under a token budget. Two of
//! its defects matter far more together than apart:
//!
//! - **The comprehension layer degrades silently.** `with_repo_map` documents
//!   failure as silent by design: an unparseable repo "should cost a step its
//!   orientation and nothing else."
//! - **Grammar coverage is four languages** — Rust, TypeScript, TSX, JavaScript.
//!   Python, Go, Java, C#, Ruby, PHP, Kotlin, Swift and C/C++ get an empty map.
//!
//! Silently. So a Python repository produces a run that looks exactly like a
//! well-oriented Rust one, and:
//!
//! > Cortex currently cannot tell the difference between a well-oriented run and
//! > a blind one, and neither can the customer.
//!
//! That is invariant 10 — a receipt never claims a setting the backend did not
//! apply — violated on context instead of effort. Every capability claim in
//! Phase 28 is conditioned on the agent knowing where it is, so a blind run
//! quietly invalidates the thing the product is sold on.
//!
//! > **Invariant 28.** Every attempt records which map, retrieval, and
//! > prior-experience layers were actually available; a degraded or absent map
//! > appears on the Plan Receipt before approval; and no verdict class is raised
//! > on the strength of context the backend did not supply.
//!
//! # Why this mirrors `EffortApplication`
//!
//! `execution_job::EffortApplication` exists because a backend that cannot
//! honour an effort request must not be able to accept it silently. The same
//! failure, one layer over: a context layer that cannot read a repository must
//! not be able to report the same thing as one that can.
//!
//! So [`ComprehensionRecord`] has no `Default`. There is no value meaning
//! "assume it worked" — the caller supplies what each layer actually did, and
//! [`Availability::Failed`] carries the reason so a receipt can say *why* a run
//! was blind rather than only that it was.

use serde::{Deserialize, Serialize};

/// What the structural map actually was for this attempt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum MapQuality {
    /// A real tree-sitter symbol and reference map.
    Structural,
    /// Imports, file tree, and definition-shaped regex, because no grammar
    /// covers this language. Better than nothing and **labelled**, never passed
    /// off as a map.
    Degraded { language: String },
    /// No orientation at all.
    Absent { reason: String },
}

impl MapQuality {
    /// The value written to the Plan Receipt.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Structural => "structural",
            Self::Degraded { .. } => "degraded",
            Self::Absent { .. } => "absent",
        }
    }

    /// Whether this must be shown to a human before the plan is approved.
    ///
    /// Both non-structural cases must. A customer approving a plan is approving
    /// an estimate, and an estimate produced by a blind agent is a different
    /// estimate — they are entitled to know which one they are looking at.
    pub fn must_appear_before_approval(&self) -> bool {
        !matches!(self, Self::Structural)
    }
}

/// Whether a supporting layer was there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Availability {
    Available,
    /// The layer is not wired into this path at all. Distinct from `Failed`:
    /// nothing went wrong, the capability simply was not offered.
    NotWired,
    Failed {
        reason: String,
    },
}

impl Availability {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }
}

/// What one attempt actually had to work with.
///
/// No `Default`, deliberately. There is no value here meaning "assume it
/// worked", because the whole defect is a layer failing to the same record a
/// working one would write.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComprehensionRecord {
    pub map: MapQuality,
    /// Targeted retrieval — the symbols this task's write set actually touches.
    pub retrieval: Availability,
    /// What earlier runs on this repository learned.
    pub prior_experience: Availability,
}

impl ComprehensionRecord {
    /// Whether the agent knew where it was.
    ///
    /// Only a structural map counts. Retrieval and prior experience make an
    /// oriented run better; they do not make a blind one oriented, because both
    /// are refinements over a map that is not there.
    pub fn is_oriented(&self) -> bool {
        matches!(self.map, MapQuality::Structural)
    }
}

/// The grammars `crates/context/src/extract.rs` actually has.
///
/// Listed by extension because that is what `grammar_for` matches on. Kept here
/// so the capability cliff is a value a receipt and a test can both read — the
/// defect in Phase 29.1 is not that coverage is four languages, it is that
/// coverage being four languages is invisible from outside.
pub const COVERED_EXTENSIONS: &[&str] = &["rs", "ts", "tsx", "js", "jsx", "mjs", "cjs"];

/// What map a file of this extension can get.
///
/// The `language` on a `Degraded` result is the extension itself, which is the
/// most specific true thing available without a grammar to ask.
pub fn map_quality_for_extension(extension: &str) -> MapQuality {
    if COVERED_EXTENSIONS.contains(&extension) {
        MapQuality::Structural
    } else {
        MapQuality::Degraded {
            language: extension.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oriented() -> ComprehensionRecord {
        ComprehensionRecord {
            map: MapQuality::Structural,
            retrieval: Availability::Available,
            prior_experience: Availability::Available,
        }
    }

    #[test]
    fn a_blind_run_and_an_oriented_run_do_not_record_the_same_thing() {
        // The defect, stated as the thing that must not be possible. Today a
        // Python repository and a Rust one produce indistinguishable runs,
        // because the map fails silently and the receipt says nothing.
        let blind = ComprehensionRecord {
            map: MapQuality::Absent {
                reason: "tree-sitter parse failed".to_string(),
            },
            ..oriented()
        };
        assert_ne!(blind, oriented());
        assert!(!blind.is_oriented());
        assert!(oriented().is_oriented());
    }

    #[test]
    fn a_degraded_map_is_not_an_oriented_run() {
        // A degraded fallback is better than nothing and it is not a map. The
        // plan is explicit that it is labelled rather than passed off as one.
        let record = ComprehensionRecord {
            map: MapQuality::Degraded {
                language: "py".to_string(),
            },
            ..oriented()
        };
        assert!(!record.is_oriented());
        assert_eq!(record.map.as_str(), "degraded");
    }

    #[test]
    fn anything_less_than_a_structural_map_reaches_the_customer_before_approval() {
        // Approving a plan is approving an estimate, and an estimate produced
        // by a blind agent is a different estimate.
        for map in [
            MapQuality::Degraded {
                language: "go".to_string(),
            },
            MapQuality::Absent {
                reason: "no worktree".to_string(),
            },
        ] {
            assert!(
                map.must_appear_before_approval(),
                "{map:?} must be disclosed before the customer approves"
            );
        }
        assert!(!MapQuality::Structural.must_appear_before_approval());
    }

    #[test]
    fn supporting_layers_cannot_rescue_a_missing_map() {
        // Retrieval and prior experience are refinements over a map. With no
        // map there is nothing to refine, and counting them would let a run
        // claim orientation it does not have.
        let record = ComprehensionRecord {
            map: MapQuality::Absent {
                reason: "unreadable".to_string(),
            },
            retrieval: Availability::Available,
            prior_experience: Availability::Available,
        };
        assert!(!record.is_oriented());
    }

    #[test]
    fn a_layer_that_was_never_wired_is_not_a_failure() {
        // `retrieval.rs` and `impact.rs` are built, tested, and not on the
        // dispatch path. That is a different fact from a retrieval attempt
        // going wrong, and a receipt that conflated them would send someone
        // debugging a working module.
        let not_wired = Availability::NotWired;
        let failed = Availability::Failed {
            reason: "index unreadable".to_string(),
        };
        assert_ne!(not_wired, failed);
        assert!(!not_wired.is_available());
        assert!(!failed.is_available());
    }

    #[test]
    fn an_absent_map_carries_why() {
        // "The agent was blind" is not actionable. "The agent was blind because
        // the worktree had no readable source files" is.
        let map = MapQuality::Absent {
            reason: "no files matched a grammar".to_string(),
        };
        let json = serde_json::to_value(&map).unwrap();
        assert_eq!(json["kind"], "absent");
        assert_eq!(json["reason"], "no files matched a grammar");
    }

    #[test]
    fn the_covered_languages_are_the_ones_extract_actually_has() {
        for ext in ["rs", "ts", "tsx", "js", "jsx", "mjs", "cjs"] {
            assert_eq!(
                map_quality_for_extension(ext),
                MapQuality::Structural,
                "{ext} has a grammar in extract.rs"
            );
        }
    }

    #[test]
    fn the_capability_cliff_is_a_value_not_a_surprise() {
        // Every one of these is a language a target customer uses, and every
        // one gets a degraded map today. The point of asserting it is that the
        // cliff becomes something a receipt can state and a test can see,
        // rather than something a customer discovers.
        for ext in [
            "py", "go", "java", "cs", "rb", "php", "kt", "swift", "c", "cpp",
        ] {
            assert_eq!(
                map_quality_for_extension(ext),
                MapQuality::Degraded {
                    language: ext.to_string()
                },
                "{ext} has no grammar and must be labelled degraded"
            );
        }
    }
}
