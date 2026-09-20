//! The context budget is a decision, and over-retrieval is a defect.
//!
//! Phase 29.2. Today the budget is a constant:
//!
//! ```text
//! const REPO_MAP_BUDGET_FRACTION: u64 = 20;
//! let budget = token_budget(provider, tier) / REPO_MAP_BUDGET_FRACTION;
//! ```
//!
//! It should be an allocation the estimator makes and the receipt records,
//! **because over-retrieval is a measurable defect rather than merely a cost**:
//! more tokens means more context rot means a worse outcome. A budget that is
//! only a spending limit gets tuned upward whenever someone is worried, and that
//! is the wrong direction as often as it is the right one.
//!
//! # The loop this closes
//!
//! Phase 29.5 scores every completed run for free, and those scores say what the
//! *next* allocation should be. [`suggest`] turns them into a decision.
//!
//! The case a naive rule gets wrong is low recall together with low precision.
//! The tempting reading is "the budget was too small — the agent had to go
//! looking". But precision is also low, so the budget was not full of the wrong
//! amount of context, it was full of the wrong context. Enlarging it buys more
//! of what already was not helping. That is [`Adjustment::Reselect`], and it
//! points at the selection — retrieval, impact analysis, the ranking — rather
//! than at the size.

use serde::{Deserialize, Serialize};

/// A context budget, and where the number came from.
///
/// No `Default`, and no way to read the tokens without the allocation existing.
/// A bare `usize` threaded through a call chain is indistinguishable from a
/// constant by the time it reaches a receipt, which is the state this replaces.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetAllocation {
    tokens: u64,
    /// What this number was derived from, for the receipt. Not a free-text
    /// note — see [`AllocationBasis`].
    basis: AllocationBasis,
}

/// Why the budget is the size it is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum AllocationBasis {
    /// A fixed fraction of the step's token budget. What the code does today,
    /// and nameable so a receipt can say that is what happened.
    FixedFraction { denominator: u64 },
    /// Sized from what previous runs on this repository actually needed.
    MeasuredHistory { runs: u32 },
    /// Sized from the impact set of the declared write set.
    ImpactSet { symbols: u32 },
}

impl BudgetAllocation {
    pub fn new(tokens: u64, basis: AllocationBasis) -> Self {
        Self { tokens, basis }
    }

    pub fn tokens(&self) -> u64 {
        self.tokens
    }

    /// Goes on the receipt beside the number.
    pub fn basis(&self) -> &AllocationBasis {
        &self.basis
    }
}

/// What to do about the next allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Adjustment {
    /// The agent had to discover files it should have been handed, and what it
    /// was handed was useful. More of the same.
    Enlarge,
    /// Everything needed was there and much of the rest was never opened.
    /// Context rot, paid for twice.
    Shrink,
    /// The context missed files *and* most of what it contained went unread.
    /// The size is not the problem; the selection is. Enlarging buys more of
    /// what was already not helping.
    Reselect,
    /// Nothing to act on, or nothing measured.
    Hold,
}

/// Recall at or above this is "the agent was given what it needed".
const RECALL_SUFFICIENT: f64 = 1.0;
/// Precision below this is over-retrieval worth acting on.
const PRECISION_WASTEFUL: f64 = 0.5;

/// Read a completed run's localization scores as advice about the next budget.
///
/// Both scores are `Option` because Phase 29.5 declines to invent them: a run
/// that changed nothing has no recall, and a run with no assembled context has
/// no precision. Either being absent means this run measured nothing about
/// allocation, so it advises nothing.
pub fn suggest(recall: Option<f64>, precision: Option<f64>) -> Adjustment {
    let (Some(recall), Some(precision)) = (recall, precision) else {
        return Adjustment::Hold;
    };

    let missed_files = recall < RECALL_SUFFICIENT;
    let wasted_context = precision < PRECISION_WASTEFUL;

    match (missed_files, wasted_context) {
        (true, true) => Adjustment::Reselect,
        (true, false) => Adjustment::Enlarge,
        (false, true) => Adjustment::Shrink,
        (false, false) => Adjustment::Hold,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_budget_carries_why_it_is_that_size() {
        // The whole change from a constant. A receipt that says "40,000 tokens"
        // and a receipt that says "40,000 tokens, one twentieth of the step
        // budget" are different documents, and only the second can be argued
        // with.
        let a = BudgetAllocation::new(40_000, AllocationBasis::FixedFraction { denominator: 20 });
        assert_eq!(a.tokens(), 40_000);
        assert_eq!(
            a.basis(),
            &AllocationBasis::FixedFraction { denominator: 20 }
        );
    }

    #[test]
    fn todays_constant_is_a_nameable_basis_not_an_absence() {
        // `REPO_MAP_BUDGET_FRACTION = 20` is a real decision someone made. It
        // should appear on a receipt as itself rather than as a blank, so the
        // first repo where it is wrong is visible.
        let json =
            serde_json::to_value(AllocationBasis::FixedFraction { denominator: 20 }).unwrap();
        assert_eq!(json["kind"], "fixed_fraction");
        assert_eq!(json["denominator"], 20);
    }

    #[test]
    fn missing_files_with_a_useful_context_asks_for_more() {
        // The agent had to go looking, and what it was given was worth having.
        assert_eq!(suggest(Some(0.6), Some(0.9)), Adjustment::Enlarge);
    }

    #[test]
    fn a_complete_context_nobody_read_asks_for_less() {
        // Everything needed was there. The rest is context rot, and it is paid
        // for twice -- once in tokens and once in a worse outcome.
        assert_eq!(suggest(Some(1.0), Some(0.2)), Adjustment::Shrink);
    }

    #[test]
    fn missing_files_and_wasted_context_is_a_selection_problem() {
        // The case a naive rule gets wrong. Low recall alone reads as "too
        // small", but precision is low too, so the budget was full of the wrong
        // context rather than the wrong amount of it. Enlarging buys more of
        // what was already not helping.
        assert_eq!(suggest(Some(0.4), Some(0.3)), Adjustment::Reselect);
    }

    #[test]
    fn a_context_that_was_complete_and_used_is_left_alone() {
        assert_eq!(suggest(Some(1.0), Some(0.95)), Adjustment::Hold);
    }

    #[test]
    fn a_run_that_measured_nothing_advises_nothing() {
        // Phase 29.5 declines to invent these scores, and this declines to act
        // on their absence. A `Hold` here is "no evidence", not "looks fine".
        assert_eq!(suggest(None, Some(0.9)), Adjustment::Hold);
        assert_eq!(suggest(Some(0.9), None), Adjustment::Hold);
        assert_eq!(suggest(None, None), Adjustment::Hold);
    }

    #[test]
    fn one_file_missing_is_still_missing() {
        // Recall is graded against 1.0, not against a tolerance. A file the
        // agent had to hunt for is a file it paid to find, whether it was one
        // of two or one of twenty.
        assert_eq!(suggest(Some(0.99), Some(0.99)), Adjustment::Enlarge);
        assert_eq!(suggest(Some(1.0), Some(0.99)), Adjustment::Hold);
    }
}
