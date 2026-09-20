//! No claim survives a confidence interval that spans zero.
//!
//! Phase 30.4. Agent runs are nondeterministic, suites are small, and the
//! effects being measured are often a few percent. Most published agent
//! comparisons are underpowered and do not say so.
//!
//! The rules the plan sets, and where each lands here:
//!
//! - **Paired comparison on identical tasks**, never two independent samples.
//!   [`paired_interval`] takes per-task deltas, so there is no signature that
//!   accepts two unpaired score vectors.
//! - **Multiple seeds per task, reported with an interval**, because a single
//!   run per task measures sampling noise as much as capability.
//! - **Cost and latency alongside every capability number.** [`Comparison`]
//!   holds both, and has no method that answers "is this better".
//! - **No claim survives an interval spanning zero.** Internally as well as
//!   externally: a plan this large will generate many changes that feel right
//!   and measure as noise, and the discipline that catches them is arithmetic.
//!
//! # Why there is no `is_improvement`
//!
//! > A 2% capability gain at 3× spend is a regression under Phase 28's
//! > efficiency objective and an improvement under its capability objective, and
//! > the report must say which question it is answering.
//!
//! So [`Comparison`] offers [`under_cost_objective`] and
//! [`under_capability_objective`] and nothing in between. A single
//! `is_improvement` would be answered by whoever wrote it, once, for a question
//! the reader cannot see — which is how a report ends up making a capability
//! claim out of an efficiency result.
//!
//! [`under_cost_objective`]: Comparison::under_cost_objective
//! [`under_capability_objective`]: Comparison::under_capability_objective
//!
//! # Why a t interval and not 1.96
//!
//! Suites are small, and that is stated rather than incidental. Using the normal
//! critical value on ten tasks produces an interval about 14% too narrow, which
//! turns noise into findings in exactly the regime this section exists to
//! police. [`T_CRITICAL_95`] is a table, and degrees of freedom between anchors
//! round *down* to the wider interval.

use serde::{Deserialize, Serialize};

/// Two-tailed 95% critical values of Student's t, by degrees of freedom.
///
/// Anchored rather than complete: a df between two anchors takes the lower
/// anchor's value, which is the larger critical value and so the wider interval.
/// Erring wide is the only direction that fails safe here.
const T_CRITICAL_95: &[(u32, f64)] = &[
    (1, 12.706),
    (2, 4.303),
    (3, 3.182),
    (4, 2.776),
    (5, 2.571),
    (6, 2.447),
    (7, 2.365),
    (8, 2.306),
    (9, 2.262),
    (10, 2.228),
    (12, 2.179),
    (15, 2.131),
    (20, 2.086),
    (25, 2.060),
    (30, 2.042),
    (40, 2.021),
    (60, 2.000),
    (120, 1.980),
];

/// Past the last anchor the table stops rather than switching to the normal
/// critical value of 1.960. A suite of more than 121 tasks therefore gets a
/// 1% wider interval than it strictly needs, which is both negligible and the
/// safe direction. Nothing here should ever return a *narrower* interval than
/// the data supports.
fn t_critical(df: u32) -> f64 {
    let mut chosen = f64::INFINITY;
    for (anchor, value) in T_CRITICAL_95 {
        if df >= *anchor {
            chosen = *value;
        } else {
            break;
        }
    }
    chosen
}

/// A mean difference with its 95% interval.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Interval {
    pub mean: f64,
    pub low: f64,
    pub high: f64,
    /// Paired observations behind it. Reported because "underpowered and does
    /// not say so" is the failure mode, and n is what says so.
    pub n: usize,
}

impl Interval {
    /// Whether the interval contains zero, inclusive at the endpoints.
    ///
    /// An interval touching zero has not excluded it.
    pub fn spans_zero(&self) -> bool {
        self.low <= 0.0 && self.high >= 0.0
    }
}

/// What a measured difference is allowed to be called.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Measured {
    /// The interval excludes zero. A claim may be made, at this size.
    Effect(Interval),
    /// The interval spans zero. This is noise, and calling it anything else is
    /// the thing the rule forbids — including internally.
    Noise(Interval),
    /// Fewer than two paired observations. No variance estimate exists, so no
    /// interval exists, so there is nothing to claim. Distinct from `Noise`:
    /// nothing was measured rather than something measured as zero.
    NotMeasurable { n: usize },
}

/// The paired mean difference and its 95% interval.
///
/// `deltas` is one value per task — the paired difference, already averaged over
/// that task's seeds. Taking deltas rather than two score vectors is deliberate:
/// there is no way to hand this two independent samples and have it pretend they
/// were paired.
pub fn paired_interval(deltas: &[f64]) -> Measured {
    let n = deltas.len();
    if n < 2 {
        return Measured::NotMeasurable { n };
    }

    let mean = deltas.iter().sum::<f64>() / n as f64;
    let variance = deltas.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / (n as f64 - 1.0);
    let standard_error = (variance / n as f64).sqrt();
    let margin = t_critical(n as u32 - 1) * standard_error;

    let interval = Interval {
        mean,
        low: mean - margin,
        high: mean + margin,
        n,
    };

    if interval.spans_zero() {
        Measured::Noise(interval)
    } else {
        Measured::Effect(interval)
    }
}

/// A capability result and what it cost, together.
///
/// Cost is a field rather than an appendix because cost per resolved instance is
/// a first-class result. A comparison that reports capability alone cannot be
/// read under either objective.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    /// Paired difference in resolution or acceptance.
    pub capability: Measured,
    /// Paired difference in cost per resolved instance. Positive means the new
    /// arm costs more.
    pub cost: Measured,
}

/// What a comparison means under one of the two objectives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reading {
    Improvement,
    Regression,
    /// Nothing measured, under this question.
    NoFinding,
}

impl Comparison {
    /// Read under the efficiency objective: cheaper at the same capability.
    ///
    /// A capability gain that is noise, bought with a real cost increase, is a
    /// regression here — which is the reading a single `is_improvement` would
    /// have hidden.
    pub fn under_cost_objective(&self) -> Reading {
        match (effect(&self.capability), effect(&self.cost)) {
            // Capability fell for real: a regression whatever it saved.
            (Some(c), _) if c < 0.0 => Reading::Regression,
            (_, Some(spend)) if spend > 0.0 => Reading::Regression,
            (_, Some(spend)) if spend < 0.0 => Reading::Improvement,
            (Some(c), None) if c > 0.0 => Reading::Improvement,
            _ => Reading::NoFinding,
        }
    }

    /// Read under the capability objective: more likely to pass, under the cap.
    ///
    /// Cost does not make a capability result better or worse here — a route
    /// that passes at four times the price is the better route at `ultra`. It is
    /// still reported, because the reader needs it to check the cap.
    pub fn under_capability_objective(&self) -> Reading {
        match effect(&self.capability) {
            Some(c) if c > 0.0 => Reading::Improvement,
            Some(c) if c < 0.0 => Reading::Regression,
            _ => Reading::NoFinding,
        }
    }
}

/// The mean, but only when the interval excluded zero.
fn effect(measured: &Measured) -> Option<f64> {
    match measured {
        Measured::Effect(i) => Some(i.mean),
        Measured::Noise(_) | Measured::NotMeasurable { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_consistent_difference_is_an_effect() {
        let m = paired_interval(&[0.10, 0.09, 0.11, 0.10, 0.10]);
        let Measured::Effect(i) = m else {
            panic!("expected an effect, got {m:?}")
        };
        assert!((i.mean - 0.10).abs() < 1e-9);
        assert!(!i.spans_zero());
        assert_eq!(i.n, 5);
    }

    #[test]
    fn the_same_mean_becomes_noise_when_the_tasks_disagree() {
        // The substantive point. Two comparisons with means a hair apart, and
        // only one of them is a finding -- which is the arithmetic that catches
        // the many changes a plan this size will generate that feel right.
        let tight = paired_interval(&[0.02, 0.02, 0.02, 0.02, 0.02]);
        let scattered = paired_interval(&[0.30, -0.28, 0.25, -0.24, 0.07]);
        assert!(matches!(tight, Measured::Effect(_)));
        assert!(matches!(scattered, Measured::Noise(_)));
    }

    #[test]
    fn a_difference_that_averages_to_nothing_is_noise() {
        let m = paired_interval(&[0.1, -0.1, 0.1, -0.1]);
        let Measured::Noise(i) = m else {
            panic!("expected noise, got {m:?}")
        };
        assert!(i.mean.abs() < 1e-9);
        assert!(i.spans_zero());
    }

    #[test]
    fn one_task_measures_nothing() {
        // No variance estimate exists, so no interval exists. Distinct from
        // noise: nothing was measured, rather than something measured as zero.
        assert_eq!(paired_interval(&[0.9]), Measured::NotMeasurable { n: 1 });
        assert_eq!(paired_interval(&[]), Measured::NotMeasurable { n: 0 });
    }

    #[test]
    fn a_small_suite_gets_the_t_value_not_the_normal_one() {
        // Using 1.96 on a handful of tasks produces an interval too narrow and
        // turns noise into findings, in exactly the regime this exists to
        // police. At n=5 the multiplier is 2.776, not 1.96.
        assert!((t_critical(4) - 2.776).abs() < 1e-9);
        // df 29 rounds down to the df 25 anchor, which is wider than the
        // true value for 29 and therefore the safe direction.
        assert!((t_critical(29) - 2.060).abs() < 1e-9);
        // Past the table the interval stays at the df 120 value rather than
        // narrowing to 1.960. One percent wide, and wide is the safe way to
        // be wrong.
        assert!((t_critical(1000) - 1.980).abs() < 1e-9);
        assert!(t_critical(1000) > 1.960);
    }

    #[test]
    fn degrees_of_freedom_between_anchors_round_to_the_wider_interval() {
        // df 11 is not in the table. It takes df 10's larger critical value,
        // not df 12's smaller one -- erring wide is the only direction that
        // fails safe.
        assert!((t_critical(11) - 2.228).abs() < 1e-9);
        assert!(t_critical(11) > t_critical(12));
    }

    #[test]
    fn an_interval_touching_zero_has_not_excluded_it() {
        let i = Interval {
            mean: 0.5,
            low: 0.0,
            high: 1.0,
            n: 4,
        };
        assert!(i.spans_zero());
    }

    #[test]
    fn the_number_of_tasks_travels_with_the_interval() {
        // "Underpowered and does not say so" is the named failure. `n` is what
        // says so, so it is on the interval rather than beside it.
        let Measured::Effect(i) = paired_interval(&[0.1, 0.1, 0.1]) else {
            panic!("expected an effect")
        };
        assert_eq!(i.n, 3);
    }

    fn effectful(mean: f64) -> Measured {
        Measured::Effect(Interval {
            mean,
            low: mean - 0.001,
            high: mean + 0.001,
            n: 20,
        })
    }

    fn noisy() -> Measured {
        Measured::Noise(Interval {
            mean: 0.02,
            low: -0.05,
            high: 0.09,
            n: 20,
        })
    }

    #[test]
    fn the_same_result_reads_opposite_ways_under_the_two_objectives() {
        // The plan's own example: a small capability gain at much higher spend.
        // A single `is_improvement` would have answered this once, invisibly,
        // for a question the reader cannot see.
        let c = Comparison {
            capability: effectful(0.02),
            cost: effectful(2.0),
        };
        assert_eq!(c.under_cost_objective(), Reading::Regression);
        assert_eq!(c.under_capability_objective(), Reading::Improvement);
    }

    #[test]
    fn a_noisy_capability_gain_bought_with_real_spend_is_a_regression() {
        // The most common shape of a change that feels like an improvement:
        // capability moved within the noise, and the bill did not.
        let c = Comparison {
            capability: noisy(),
            cost: effectful(1.5),
        };
        assert_eq!(c.under_cost_objective(), Reading::Regression);
        assert_eq!(c.under_capability_objective(), Reading::NoFinding);
    }

    #[test]
    fn spending_less_at_unchanged_capability_is_an_efficiency_win_and_nothing_else() {
        let c = Comparison {
            capability: noisy(),
            cost: effectful(-0.4),
        };
        assert_eq!(c.under_cost_objective(), Reading::Improvement);
        assert_eq!(c.under_capability_objective(), Reading::NoFinding);
    }

    #[test]
    fn losing_capability_is_a_regression_under_both() {
        let c = Comparison {
            capability: effectful(-0.05),
            cost: effectful(-0.9),
        };
        assert_eq!(
            c.under_cost_objective(),
            Reading::Regression,
            "capability fell for real; the saving does not buy it back"
        );
        assert_eq!(c.under_capability_objective(), Reading::Regression);
    }

    #[test]
    fn nothing_measured_is_no_finding_under_either_question() {
        let c = Comparison {
            capability: Measured::NotMeasurable { n: 1 },
            cost: Measured::NotMeasurable { n: 1 },
        };
        assert_eq!(c.under_cost_objective(), Reading::NoFinding);
        assert_eq!(c.under_capability_objective(), Reading::NoFinding);
    }
}
