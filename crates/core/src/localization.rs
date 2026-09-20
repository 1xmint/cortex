//! Scoring the map against what the change actually touched.
//!
//! Phase 29.5. Cortex can score its own comprehension retroactively at **zero
//! labelling cost**, because every completed run reveals the ground truth: the
//! set of files the change actually touched. Compare that against what the agent
//! was given and what it opened.
//!
//! | Metric | Definition |
//! |---|---|
//! | Context recall | share of finally-changed files present in the assembled context |
//! | Context precision | share of assembled context that was touched or read |
//! | Exploration cost | tokens spent before the first edit |
//!
//! Map availability is the fourth metric in that table and is not computed here
//! — it is `comprehension::MapQuality`, recorded by the layer that produced (or
//! failed to produce) the map rather than derived after the fact.
//!
//! # Where these go, and why each one is currently guessed
//!
//! **The estimator.** Poor localization is a leading cause of the 3×–30× cost
//! variance the forecast keeps missing. An agent that has to discover where the
//! work is pays for the discovery, and nothing upstream currently predicts that.
//!
//! **The router.** Context quality is a confound the router presently attributes
//! to the model. A run that went badly because the agent was handed the wrong
//! files teaches the router that the *model* is weak, and it will keep learning
//! that for as long as the two are indistinguishable in the record.
//!
//! **The Plan Receipt.** "Cortex has a strong map of this repository" against
//! "Cortex is working without a symbol map in this language" is something a
//! customer is entitled to know *before* approving spend.
//!
//! # Why the scores are optional
//!
//! A run that changed no files has no ground truth, and a run with no assembled
//! context has no denominator. Both return `None` rather than a number.
//!
//! That is the whole reason [`Localization`] holds `Option<f64>`. Scoring an
//! empty context as precision `0.0` records a precision failure where the real
//! event was an absent map, and scoring it `1.0` flatters it — either way the
//! router and the estimator learn something false from a run that measured
//! nothing. `None` says "not measured", which is the true thing.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// What one completed run can be scored against.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunObservation {
    /// Files the delivered change actually touched. The free ground truth.
    pub changed: BTreeSet<String>,
    /// Files present in the context the agent was handed.
    pub assembled: BTreeSet<String>,
    /// Files the agent opened for itself during the run.
    pub opened: BTreeSet<String>,
    /// Tokens spent before the first edit.
    pub tokens_before_first_edit: u64,
}

/// How well the context matched the work.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Localization {
    /// Share of finally-changed files that were in the assembled context.
    ///
    /// `None` when the run changed nothing: there is no ground truth to score
    /// against, which is different from scoring badly.
    pub recall: Option<f64>,
    /// Share of assembled context that was touched or read.
    ///
    /// `None` when nothing was assembled. An empty context is an absent map,
    /// not a precision failure.
    pub precision: Option<f64>,
    /// Tokens spent before the first edit. The number the repository corpus
    /// should drive down over time, which is only visible if it is recorded from
    /// the first run rather than from the run someone starts optimising.
    pub exploration_tokens: u64,
}

/// Score one completed run.
pub fn score(observation: &RunObservation) -> Localization {
    let recall = if observation.changed.is_empty() {
        None
    } else {
        let found = observation
            .changed
            .intersection(&observation.assembled)
            .count();
        Some(found as f64 / observation.changed.len() as f64)
    };

    let precision = if observation.assembled.is_empty() {
        None
    } else {
        // "Touched or read": a file the agent did not edit but did open was
        // still doing work -- ruling something out is what half of a good
        // context is for, and counting it as waste would optimise toward
        // starving the agent.
        let useful = observation
            .assembled
            .iter()
            .filter(|f| observation.changed.contains(*f) || observation.opened.contains(*f))
            .count();
        Some(useful as f64 / observation.assembled.len() as f64)
    };

    Localization {
        recall,
        precision,
        exploration_tokens: observation.tokens_before_first_edit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn close(a: Option<f64>, b: f64) -> bool {
        matches!(a, Some(v) if (v - b).abs() < 1e-9)
    }

    #[test]
    fn a_perfect_map_scores_one_on_both() {
        let o = RunObservation {
            changed: set(&["a.rs", "b.rs"]),
            assembled: set(&["a.rs", "b.rs"]),
            opened: BTreeSet::new(),
            tokens_before_first_edit: 400,
        };
        let s = score(&o);
        assert!(close(s.recall, 1.0));
        assert!(close(s.precision, 1.0));
        assert_eq!(s.exploration_tokens, 400);
    }

    #[test]
    fn a_file_the_agent_had_to_find_shows_up_as_missing_recall() {
        // Below 1, the agent had to discover -- and the gap is pure cost, which
        // is exactly the cost variance the estimator keeps missing.
        let o = RunObservation {
            changed: set(&["a.rs", "hidden.rs"]),
            assembled: set(&["a.rs"]),
            opened: set(&["hidden.rs"]),
            tokens_before_first_edit: 9_000,
        };
        assert!(close(score(&o).recall, 0.5));
    }

    #[test]
    fn context_nobody_looked_at_is_a_precision_loss() {
        // Low precision is context rot and wasted budget, not merely a bigger
        // bill: more tokens make the outcome worse as well as dearer.
        let o = RunObservation {
            changed: set(&["a.rs"]),
            assembled: set(&["a.rs", "junk1.rs", "junk2.rs", "junk3.rs"]),
            opened: BTreeSet::new(),
            tokens_before_first_edit: 100,
        };
        assert!(close(score(&o).precision, 0.25));
    }

    #[test]
    fn a_file_that_was_read_but_not_edited_still_counts_as_useful() {
        // Ruling something out is what half of a good context is for. Counting
        // only edited files would score a careful run as wasteful and optimise
        // toward starving the agent.
        let o = RunObservation {
            changed: set(&["a.rs"]),
            assembled: set(&["a.rs", "considered.rs"]),
            opened: set(&["considered.rs"]),
            tokens_before_first_edit: 100,
        };
        assert!(close(score(&o).precision, 1.0));
    }

    #[test]
    fn an_empty_context_is_not_a_precision_failure() {
        // The defensible-`None` case. A blind run measured nothing. Recording
        // `0.0` would teach the router that a *model* did badly on a run where
        // the real event was an absent map, and it would keep learning that for
        // as long as the two look the same in the record.
        let o = RunObservation {
            changed: set(&["a.rs"]),
            assembled: BTreeSet::new(),
            opened: set(&["a.rs"]),
            tokens_before_first_edit: 20_000,
        };
        let s = score(&o);
        assert_eq!(s.precision, None);
        assert!(
            close(s.recall, 0.0),
            "recall is genuinely zero and measured"
        );
        assert_eq!(s.exploration_tokens, 20_000);
    }

    #[test]
    fn a_run_that_changed_nothing_has_no_recall_to_report() {
        // No ground truth. Not a score of zero -- an absence of a score, and a
        // mean taken over these would otherwise drift down every time an
        // exploratory run finished.
        let o = RunObservation {
            changed: BTreeSet::new(),
            assembled: set(&["a.rs"]),
            opened: set(&["a.rs"]),
            tokens_before_first_edit: 50,
        };
        let s = score(&o);
        assert_eq!(s.recall, None);
        assert!(close(s.precision, 1.0));
    }

    #[test]
    fn a_run_with_neither_scores_neither() {
        let s = score(&RunObservation::default());
        assert_eq!(s.recall, None);
        assert_eq!(s.precision, None);
        assert_eq!(s.exploration_tokens, 0);
    }

    #[test]
    fn exploration_cost_is_recorded_even_when_the_scores_are_not() {
        // The one number that is always available, and the one the repository
        // corpus is supposed to drive down. It has to be recorded from the
        // first run, not from the run someone starts optimising.
        let o = RunObservation {
            changed: BTreeSet::new(),
            assembled: BTreeSet::new(),
            opened: BTreeSet::new(),
            tokens_before_first_edit: 12_345,
        };
        assert_eq!(score(&o).exploration_tokens, 12_345);
    }

    #[test]
    fn a_wide_context_that_was_all_used_beats_a_narrow_one_that_missed() {
        // Precision and recall move independently, which is the point of
        // keeping both: "assemble less" is only an improvement if recall holds.
        let wide = RunObservation {
            changed: set(&["a.rs", "b.rs"]),
            assembled: set(&["a.rs", "b.rs", "c.rs"]),
            opened: set(&["c.rs"]),
            tokens_before_first_edit: 100,
        };
        let narrow = RunObservation {
            changed: set(&["a.rs", "b.rs"]),
            assembled: set(&["a.rs"]),
            opened: BTreeSet::new(),
            tokens_before_first_edit: 100,
        };
        assert!(close(score(&wide).recall, 1.0));
        assert!(close(score(&wide).precision, 1.0));
        assert!(close(score(&narrow).recall, 0.5));
        assert!(close(score(&narrow).precision, 1.0));
    }
}
