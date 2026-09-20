//! What raising N actually buys, stated as a bound rather than a promise.
//!
//! Phase 28. The product thesis is *better than any single model*, and the
//! mechanism with a hard guarantee is repeated sampling against a sound
//! verifier: if one attempt is correct with probability `a`, at least one of N
//! is correct with probability `1 − (1 − a)^N`. That is the generation–
//! verification gap, and it is the whole reason a verifier-owning harness can
//! beat the model it calls. The model can already produce the right answer; the
//! missing capability is *identifying* it, and Cortex owns an executable
//! identifier. Nobody selling tokens does.
//!
//! # The two things that must travel with the number
//!
//! **Precision multiplies it.** The delivered-correct rate is the sampling gain
//! times verifier precision, not the sampling gain alone:
//!
//! ```text
//! P(delivered correct) ≈ [1 − (1 − a)^N] × precision
//! ```
//!
//! Raising N without raising precision is the Phase 27.4 failure — more
//! delivery, more of it wrong, concentrated in the hard tail. Precision is the
//! binding constraint on the capability ceiling, which is why Phase 27 comes
//! before this one.
//!
//! **The attempts are not independent.** `1 − (1 − a)^N` assumes they are, and
//! N samples from one model at temperature fail the same way, on the same
//! reasoning, for the same reason. So the formula is an **upper bound Cortex
//! will not reach**, and the plan requires it to be written down as one
//! everywhere it appears — the Plan Receipt, the estimator, anything a customer
//! sees.
//!
//! That requirement is why [`DeliveredCorrectBound`] is a type rather than an
//! `f64`. A bare float gets rendered as a forecast by whoever picks it up next;
//! a value you have to unwrap through [`as_upper_bound`] does not.
//!
//! [`as_upper_bound`]: DeliveredCorrectBound::as_upper_bound
//!
//! # The prize, for scale
//!
//! CodeMonkeys pooled candidate edits across five agent systems: ensemble
//! selection resolved 66.2% against 62.8% for the best individual member — the
//! Phase 28 claim demonstrated rather than asserted. Pooled *coverage* was
//! 80.8%. The correct answer was generated, sat in the pool, and was thrown
//! away for ~14 points of resolvable instances, because the selector could not
//! tell which candidate it was.
//!
//! The pool is cheap; the selector is the moat.

use serde::{Deserialize, Serialize};

/// An upper bound on the delivered-correct rate.
///
/// Deliberately not an `f64`. The independence assumption behind it does not
/// hold, so the true rate is lower by an amount nothing here measures — and a
/// bare number gets read as a forecast by the next person to touch it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeliveredCorrectBound {
    value: f64,
}

impl DeliveredCorrectBound {
    /// The bound, named so a caller cannot forget what it is.
    ///
    /// Anything rendering this to a human or writing it to a receipt has to
    /// spell `as_upper_bound` to get at it, which is the point.
    pub fn as_upper_bound(&self) -> f64 {
        self.value
    }
}

/// `1 − (1 − a)^N`: the chance at least one of N attempts is correct.
///
/// The optimistic half, and true only under independence. See
/// [`delivered_correct_upper_bound`] for the number anyone should actually
/// quote.
pub fn sampling_gain(single_attempt_success: f64, attempts: u8) -> f64 {
    let a = single_attempt_success.clamp(0.0, 1.0);
    if attempts == 0 {
        return 0.0;
    }
    1.0 - (1.0 - a).powi(attempts as i32)
}

/// The sampling gain multiplied by verifier precision.
///
/// Both terms are real and the product is what a customer receives. Quoting the
/// first without the second is the Phase 27.4 failure written as a forecast.
pub fn delivered_correct_upper_bound(
    single_attempt_success: f64,
    attempts: u8,
    verifier_precision: f64,
) -> DeliveredCorrectBound {
    let precision = verifier_precision.clamp(0.0, 1.0);
    DeliveredCorrectBound {
        value: sampling_gain(single_attempt_success, attempts) * precision,
    }
}

/// Whether decomposing into these leaves beats attempting the whole thing.
///
/// A model's success probability falls as scope grows, so decomposition helps —
/// but `Π aᵢ` against `a_whole` is **not** automatically a win, and it fails
/// exactly when a plan over-decomposes itself. Twelve leaves at 0.95 is 0.54,
/// which loses to one shot at 0.7.
///
/// `integration_success` is not 1 and must be supplied. Assuming it away is how
/// a plan talks itself past its own crossover.
pub fn decomposition_beats_single_shot(
    leaf_successes: &[f64],
    whole_success: f64,
    integration_success: f64,
) -> bool {
    if leaf_successes.is_empty() {
        return false;
    }
    let decomposed: f64 = leaf_successes
        .iter()
        .map(|a| a.clamp(0.0, 1.0))
        .product::<f64>()
        * integration_success.clamp(0.0, 1.0);
    decomposed > whole_success.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn precision_is_the_binding_constraint_not_the_sample_count() {
        // The Phase 27.4 failure, as arithmetic. Racing from 1 to 10 attempts
        // barely moves delivery when the verifier cannot tell right from
        // wrong -- it moves *coverage*, and only precision converts coverage
        // into delivery.
        let wide_blind = delivered_correct_upper_bound(0.3, 10, 0.5);
        let narrow_sharp = delivered_correct_upper_bound(0.3, 2, 1.0);
        assert!(
            narrow_sharp.as_upper_bound() > wide_blind.as_upper_bound(),
            "two attempts with a sound verifier must beat ten with a coin-flip one"
        );
    }

    #[test]
    fn a_perfect_verifier_still_cannot_exceed_the_sampling_gain() {
        let gain = sampling_gain(0.4, 3);
        let bound = delivered_correct_upper_bound(0.4, 3, 1.0);
        assert!(close(bound.as_upper_bound(), gain));
    }

    #[test]
    fn a_verifier_that_cannot_tell_delivers_nothing_correct_by_this_measure() {
        // precision 0 means every pass is a false accept. No N rescues that.
        for n in [1, 5, 50] {
            assert!(close(
                delivered_correct_upper_bound(0.9, n, 0.0).as_upper_bound(),
                0.0
            ));
        }
    }

    #[test]
    fn zero_attempts_deliver_nothing() {
        assert!(close(sampling_gain(0.9, 0), 0.0));
    }

    #[test]
    fn the_gain_rises_with_n_and_never_exceeds_one() {
        let mut prev = 0.0;
        for n in 1..=20u8 {
            let g = sampling_gain(0.25, n);
            assert!(g >= prev, "the gain must be monotone in N");
            assert!(g <= 1.0, "a probability cannot exceed 1");
            prev = g;
        }
    }

    #[test]
    fn over_decomposition_loses_to_one_shot() {
        // The number the plan quotes: twelve leaves at 0.95 is 0.54, which
        // loses to a single attempt at 0.7. Decomposition is a measurable
        // decision with an optimum, not a matter of taste.
        let twelve = vec![0.95_f64; 12];
        assert!(
            !decomposition_beats_single_shot(&twelve, 0.7, 1.0),
            "twelve leaves at 0.95 must not be claimed to beat one shot at 0.7"
        );
    }

    #[test]
    fn a_short_decomposition_of_easy_leaves_wins() {
        assert!(decomposition_beats_single_shot(&[0.95, 0.95], 0.7, 0.98));
    }

    #[test]
    fn integration_success_is_not_assumed_away() {
        // The same leaves, the same whole, and a realistic integration step
        // flips the answer. A caller that passes 1.0 here is asserting
        // integration never fails, which is how a plan talks itself past its
        // own crossover.
        let leaves = [0.9, 0.9, 0.9];
        assert!(decomposition_beats_single_shot(&leaves, 0.72, 1.0));
        assert!(!decomposition_beats_single_shot(&leaves, 0.72, 0.9));
    }

    #[test]
    fn nothing_decomposed_is_not_a_decomposition_win() {
        assert!(!decomposition_beats_single_shot(&[], 0.1, 1.0));
    }
}
