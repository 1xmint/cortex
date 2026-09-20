//! Cortex has taken the other side of a bet on every task, and nowhere prices
//! it.
//!
//! Phase 31.1. Refund-on-failure is the moat — an incumbent selling tokens
//! cannot cannibalise its own meter — and the part the plan skips is what
//! follows from it. For one task with revenue `R` charged only on success and
//! pass probability `p`:
//!
//! ```text
//! E[margin] = p·R − E[C]
//! ```
//!
//! # The term that makes this harder than it looks
//!
//! **`E[C | fail] > E[C | pass]`.** A failing task does not fail cheaply. It
//! exhausts the retry budget, climbs the escalation ladder to more expensive
//! models, re-runs the check battery each time, and stops only at the cap. **The
//! runs Cortex is not paid for are systematically its most expensive runs** —
//! the exact inverse of the intuition that failures are cheap because nothing
//! shipped.
//!
//! That inverts the naive break-even too. `p ≥ E[C]/R` is only correct when
//! `E[C]` is measured at the pass rate you are solving for, and the honest form
//! separates the two costs:
//!
//! ```text
//! p·R = p·C_pass + (1 − p)·C_fail
//! p   = C_fail / (R − C_pass + C_fail)
//! ```
//!
//! Substituting a single average cost measured on a *healthier* mix understates
//! the pass rate needed, and understates it worst exactly when failures are
//! dear — see `a_single_average_cost_understates_the_pass_rate_needed`.
//!
//! # Which compounds with adverse selection
//!
//! Phase 31.2: a rational customer sends the hard, underspecified, risky items
//! to the vendor that does not charge on failure, and the routine items to the
//! flat-rate tool they already pay for. Nobody is cheating — that is what a
//! guarantee is *for*. So **realised `p` is systematically below the `p` measured
//! on a representative sample**, and a benchmark drawn from average work
//! overstates margin at launch.
//!
//! The two findings multiply: the selected distribution is the hard tail, the
//! hard tail is where failures are dearest, and Phase 27.4 says the hard tail is
//! also where racing converts honest failures into shortcut passes.
//!
//! # Why the tail, not the mean
//!
//! Agent cost variance on identical tasks is measured at **3×–30×**. A
//! distribution with that spread is dominated by its tail, so a mean-based
//! margin calculation is wrong in the direction that hurts, every time.
//! [`break_even_pass_rate`] takes a [`CostBasis`] so the caller has to say which
//! question they are answering, and [`CostBasis::TailOfFailures`] is the one to
//! plan with.

use serde::{Deserialize, Serialize};

/// What a task costs, conditioned on how it ends.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CostDistribution {
    /// Mean cost of a task that passes.
    pub mean_given_pass: f64,
    /// Mean cost of a task that fails. Structurally the larger of the two.
    pub mean_given_fail: f64,
    /// 90th-percentile cost of a task that fails. The number to plan with,
    /// because a 3×–30× spread is dominated by its tail.
    pub p90_given_fail: f64,
}

/// Which cost figure a calculation is using.
///
/// An argument rather than a default, so a margin number always says which
/// question it answered. "Break-even" computed on means and "break-even"
/// computed on the tail are different numbers and only one of them is safe to
/// plan with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostBasis {
    /// Mean failure cost. Describes the past.
    MeanOfFailures,
    /// 90th-percentile failure cost. Plans for the distribution actually seen.
    TailOfFailures,
}

impl CostDistribution {
    fn failure_cost(&self, basis: CostBasis) -> f64 {
        match basis {
            CostBasis::MeanOfFailures => self.mean_given_fail,
            CostBasis::TailOfFailures => self.p90_given_fail,
        }
    }

    /// Whether failures really are dearer than passes here.
    ///
    /// False is a signal that something is mismeasured — a failing task
    /// exhausts retries and climbs the ladder, so it should not be cheaper than
    /// one that passed first time.
    pub fn failures_cost_more(&self) -> bool {
        self.mean_given_fail > self.mean_given_pass
    }
}

/// `E[C]` at this pass rate.
pub fn expected_cost(pass_rate: f64, costs: &CostDistribution, basis: CostBasis) -> f64 {
    let p = pass_rate.clamp(0.0, 1.0);
    p * costs.mean_given_pass + (1.0 - p) * costs.failure_cost(basis)
}

/// `E[margin] = p·R − E[C]`.
pub fn expected_margin(
    pass_rate: f64,
    revenue: f64,
    costs: &CostDistribution,
    basis: CostBasis,
) -> f64 {
    let p = pass_rate.clamp(0.0, 1.0);
    p * revenue - expected_cost(p, costs, basis)
}

/// The pass rate at which this task class stops losing money.
///
/// `C_fail / (R − C_pass + C_fail)`, which reduces to the familiar `C/R` only
/// when the two costs are equal. `None` when the denominator is not positive —
/// no pass rate makes the class profitable, which is a real answer and not an
/// error to paper over.
pub fn break_even_pass_rate(
    revenue: f64,
    costs: &CostDistribution,
    basis: CostBasis,
) -> Option<f64> {
    let c_fail = costs.failure_cost(basis);
    let denominator = revenue - costs.mean_given_pass + c_fail;
    if denominator <= 0.0 {
        return None;
    }
    let p = c_fail / denominator;
    if p > 1.0 {
        None
    } else {
        Some(p)
    }
}

/// What to do about a quote.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Quote {
    /// Predicted `p` clears break-even. Sell it at the standard rate.
    StandardRate,
    /// Predicted `p` is below break-even. Reprice, narrow the guarantee, or
    /// decline by odds — never sell this silently at the standard rate.
    RepriceOrNarrow { predicted: f64, break_even: f64 },
    /// No pass rate makes this class profitable at this revenue.
    NotSellableAtThisPrice,
}

/// Price the odds, not just the work.
///
/// Phase 31.2(a). The estimator already produces predicted `p` with an interval;
/// this turns it into the one decision that must not be made by default. There
/// is no path that returns [`Quote::StandardRate`] for a predicted `p` below
/// break-even, which is the whole point — a silently loss-making sale at the
/// standard rate is what happens when nobody joins these two numbers up.
///
/// Declining is not a product failure. It is the most credible thing a vendor
/// selling verified outcomes can do, and a decline that names what would make
/// the task acceptable routes into Phase 24.2's priced on-ramp: trust plus
/// revenue rather than a missed sale.
pub fn quote(predicted_pass_rate: f64, revenue: f64, costs: &CostDistribution) -> Quote {
    // Planning uses the tail. A quote sized on mean failure cost is a quote
    // that will be wrong in the direction that hurts.
    let Some(break_even) = break_even_pass_rate(revenue, costs, CostBasis::TailOfFailures) else {
        return Quote::NotSellableAtThisPrice;
    };
    let predicted = predicted_pass_rate.clamp(0.0, 1.0);
    if predicted >= break_even {
        Quote::StandardRate
    } else {
        Quote::RepriceOrNarrow {
            predicted,
            break_even,
        }
    }
}

/// How far realised `p` fell below forecast, per organisation or repository.
///
/// Phase 31.2(c). Positive means Cortex did worse than it predicted. A
/// persistent gap is information — an adverse selector, an unverifiable
/// repository, or a badly calibrated estimator — and each has a different
/// response. What it must never be is a silent margin leak discovered in
/// aggregate a quarter later.
pub fn selection_gap(forecast_pass_rate: f64, realised_pass_rate: f64) -> f64 {
    forecast_pass_rate.clamp(0.0, 1.0) - realised_pass_rate.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Revenue 100, a cheap pass, a dear failure, a dearer tail.
    fn costs() -> CostDistribution {
        CostDistribution {
            mean_given_pass: 20.0,
            mean_given_fail: 60.0,
            p90_given_fail: 140.0,
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn the_break_even_reduces_to_the_familiar_form_when_costs_are_equal() {
        // `p >= C/R` is the plan's simple statement, and it is the special case
        // where a failure costs what a pass costs. Keeping it exact is how you
        // know the general form is right.
        let flat = CostDistribution {
            mean_given_pass: 25.0,
            mean_given_fail: 25.0,
            p90_given_fail: 25.0,
        };
        let p = break_even_pass_rate(100.0, &flat, CostBasis::MeanOfFailures).unwrap();
        assert!(close(p, 0.25));
    }

    #[test]
    fn a_single_average_cost_understates_the_pass_rate_needed() {
        // The failure this module exists to prevent, in numbers.
        //
        // Cost measured on a healthy mix at p = 0.8: 0.8*20 + 0.2*60 = 28.
        // The naive `E[C]/R` then says break-even is 0.28.
        //
        // The honest form says 0.4286 -- more than half again as high. A
        // business planning to 0.28 is loss-making across a wide band it
        // believes is profitable, and adverse selection pushes it into exactly
        // that band.
        let naive = (0.8 * 20.0 + 0.2 * 60.0) / 100.0;
        let honest = break_even_pass_rate(100.0, &costs(), CostBasis::MeanOfFailures).unwrap();
        assert!(close(naive, 0.28));
        assert!(close(honest, 60.0 / 140.0));
        assert!(honest > naive * 1.5);
    }

    #[test]
    fn planning_on_the_tail_demands_a_higher_pass_rate_than_planning_on_the_mean() {
        // 3x-30x cost variance means the tail is the distribution. Both numbers
        // are computable and only one is safe to plan with, which is why the
        // basis is an argument rather than a default.
        let mean = break_even_pass_rate(100.0, &costs(), CostBasis::MeanOfFailures).unwrap();
        let tail = break_even_pass_rate(100.0, &costs(), CostBasis::TailOfFailures).unwrap();
        assert!(tail > mean);
        assert!(close(tail, 140.0 / 220.0));
    }

    #[test]
    fn margin_is_negative_below_break_even_and_positive_above_it() {
        let b = break_even_pass_rate(100.0, &costs(), CostBasis::MeanOfFailures).unwrap();
        assert!(expected_margin(b - 0.05, 100.0, &costs(), CostBasis::MeanOfFailures) < 0.0);
        assert!(close(
            expected_margin(b, 100.0, &costs(), CostBasis::MeanOfFailures),
            0.0
        ));
        assert!(expected_margin(b + 0.05, 100.0, &costs(), CostBasis::MeanOfFailures) > 0.0);
    }

    #[test]
    fn expected_cost_rises_as_the_pass_rate_falls() {
        // The inverse of the intuition that failures are cheap because nothing
        // shipped. A worse pass rate is a bigger bill as well as less revenue,
        // and both move at once.
        let high = expected_cost(0.9, &costs(), CostBasis::MeanOfFailures);
        let low = expected_cost(0.3, &costs(), CostBasis::MeanOfFailures);
        assert!(low > high);
    }

    #[test]
    fn a_quote_below_break_even_never_comes_back_as_the_standard_rate() {
        // "Never a silently loss-making one at the standard rate" is the rule,
        // and there is no path through `quote` that produces one.
        let tail_break_even = break_even_pass_rate(100.0, &costs(), CostBasis::TailOfFailures)
            .expect("sellable at some rate");
        for predicted in [0.0, 0.2, 0.5, tail_break_even - 0.01] {
            assert_eq!(
                quote(predicted, 100.0, &costs()),
                Quote::RepriceOrNarrow {
                    predicted,
                    break_even: tail_break_even
                },
                "predicted {predicted} must not be sold at the standard rate"
            );
        }
    }

    #[test]
    fn a_quote_is_sized_on_the_tail_not_the_mean() {
        // A predicted `p` that clears the mean-based break-even and not the
        // tail-based one is exactly the sale that looks fine and is not.
        let mean = break_even_pass_rate(100.0, &costs(), CostBasis::MeanOfFailures).unwrap();
        let tail = break_even_pass_rate(100.0, &costs(), CostBasis::TailOfFailures).unwrap();
        let between = (mean + tail) / 2.0;
        assert!(between > mean && between < tail);
        assert!(matches!(
            quote(between, 100.0, &costs()),
            Quote::RepriceOrNarrow { .. }
        ));
    }

    #[test]
    fn a_good_bet_sells_at_the_standard_rate() {
        assert_eq!(quote(0.95, 100.0, &costs()), Quote::StandardRate);
    }

    #[test]
    fn a_class_that_cannot_be_profitable_says_so_rather_than_quoting() {
        // Revenue below the cost of passing. No pass rate rescues it, and
        // returning a break-even above 1 would let a caller compare against it
        // and conclude the task is merely difficult.
        let dear = CostDistribution {
            mean_given_pass: 200.0,
            mean_given_fail: 400.0,
            p90_given_fail: 900.0,
        };
        assert_eq!(
            quote(0.99, 100.0, &dear),
            Quote::NotSellableAtThisPrice,
            "a class whose passes cost more than they earn is not a pricing problem"
        );
    }

    #[test]
    fn failures_costing_less_than_passes_is_a_measurement_smell() {
        // A failing task exhausts retries and climbs the ladder, so it should
        // not be cheaper than one that passed first time. This is a check on
        // the inputs, not on the business.
        assert!(costs().failures_cost_more());
        let suspicious = CostDistribution {
            mean_given_pass: 60.0,
            mean_given_fail: 20.0,
            p90_given_fail: 30.0,
        };
        assert!(!suspicious.failures_cost_more());
    }

    #[test]
    fn the_selection_gap_is_positive_when_reality_underperforms_the_forecast() {
        // Phase 31.2(c). Nobody is cheating -- sending the hard, risky work to
        // the vendor that does not charge on failure is what a guarantee is
        // for. The gap is information, and its sign says which way.
        assert!(close(selection_gap(0.80, 0.62), 0.18));
        assert!(selection_gap(0.62, 0.80) < 0.0);
        assert!(close(selection_gap(0.7, 0.7), 0.0));
    }
}
