//! When racing is allowed, and how wide.
//!
//! Phase 27.4, which exists to correct one sentence. Phase 6.7 used to say:
//!
//! > *"Racing before width, always: width is bounded by the dependency graph
//! > and carries integration risk, while racing is bounded only by budget and
//! > carries none."*
//!
//! The plan now calls that **the most consequential wrong sentence in the
//! document**, because it was phrased as the safe default and it sits on the
//! speed dial, where a customer in a hurry reaches for it.
//!
//! # Why racing is not free
//!
//! Take one attempt at a leaf: correct-and-passing with probability `a`,
//! wrong-but-passing with probability `b`, failing otherwise. Race N and keep
//! the first passer.
//!
//! On an easy leaf nothing changes — among passers the false-accept share is
//! `b / (a + b)` whether N is 1 or 10. That is the reassuring half, and
//! presumably the intuition behind the old sentence.
//!
//! The hard tail is where it breaks. Where honest success `a` is near zero and
//! `b` is not — a genuinely difficult change over a battery with a shortcut in
//! it — a single attempt mostly fails, and **failing is safe**: it refunds, it
//! tells the truth, it routes to a human. Racing drives
//! `P(deliver) → 1 − (1 − a − b)^N`, and on that leaf nearly all the newly
//! delivered mass is `b`.
//!
//! So: racing does not change the error rate on work that was going to succeed.
//! It converts work that was going to **honestly fail** into work that passes by
//! shortcut. A false-accept generator aimed precisely at the tail.
//!
//! Three things make that worse than it sounds. The tail is not a rare corner —
//! it is the work a customer sends to a refund-backed vendor. Racing sits on the
//! *speed* dial, so it is reached for under time pressure, when a human is least
//! likely to read the diff. And `b` is not a constant: it rises as models get
//! better at finding shortcuts and falls as batteries get stronger, and Cortex
//! measures neither today.
//!
//! # What is here, and what is not
//!
//! Here: the two rules that are decidable from a plan alone.
//!
//! **Not here: `race_agreement`.** The plan's third correction is that the N−1
//! losing attempts are the cheapest precision estimator in the system — five
//! attempts converging is corroboration; five semantically different diffs, one
//! of which passed, is a warning that the *battery* was solved rather than the
//! code. That inverts the value of racing from the winner to the distribution.
//! It needs the race machinery, which does not exist yet, so it is not here and
//! is not pretended to be.

use serde::{Deserialize, Serialize};

use crate::battery_power::BatteryPower;
use crate::diff_surface::VerdictClass;

/// Why racing was refused for this plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RaceRefusal {
    /// The contract is `authored`, so the task may write its own exam.
    ///
    /// Racing that races *the authorship of the exam*: it selects, among N
    /// attempts, the one whose self-written test suite was most permissive.
    /// Prohibited outright rather than capped, because no width makes it safe.
    AuthoredContract,
    /// The battery cannot distinguish correct from incorrect code here, so
    /// racing selects among N attempts on a signal that carries nothing.
    NoBatteryPower,
}

/// Whether a plan may race, and how wide.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RaceDecision {
    Allowed { max_width: u8 },
    Refused(RaceRefusal),
}

/// The widest race a `weak` battery may support.
///
/// Two, not "some fraction of the requested width". A weak battery catches some
/// mutants and misses others, so each extra attempt is another draw against the
/// misses; the cap is small on purpose.
pub const WEAK_BATTERY_MAX_WIDTH: u8 = 2;

/// Decide racing for a plan, from the two things a plan already declares.
///
/// Refusing is a real answer and the dial must say so. Invariant 9 applied
/// honestly: a dial that silently does nothing is worse than one that refuses,
/// because the customer believes they bought something.
pub fn race_policy(class: VerdictClass, power: BatteryPower, requested_width: u8) -> RaceDecision {
    if class != VerdictClass::Strong {
        return RaceDecision::Refused(RaceRefusal::AuthoredContract);
    }
    match power {
        BatteryPower::None => RaceDecision::Refused(RaceRefusal::NoBatteryPower),
        BatteryPower::Weak => RaceDecision::Allowed {
            max_width: requested_width.min(WEAK_BATTERY_MAX_WIDTH),
        },
        BatteryPower::Strong => RaceDecision::Allowed {
            max_width: requested_width,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn racing_an_authored_contract_is_refused_at_any_width() {
        // Racing an authored contract races the authorship of the exam --
        // picking, among N, the attempt whose self-written suite was most
        // permissive. No width makes that safe, so it is refused outright
        // rather than capped.
        for power in [BatteryPower::Strong, BatteryPower::Weak, BatteryPower::None] {
            for width in [1, 2, 5, 50] {
                assert_eq!(
                    race_policy(VerdictClass::Authored, power, width),
                    RaceDecision::Refused(RaceRefusal::AuthoredContract),
                    "authored must not race at width {width} over {power:?}"
                );
            }
        }
    }

    #[test]
    fn a_battery_that_cannot_fail_cannot_be_raced_against() {
        // Racing selects the first passer. Where nothing can fail, the first
        // passer is the first attempt, and the other N-1 bought nothing.
        assert_eq!(
            race_policy(VerdictClass::Strong, BatteryPower::None, 10),
            RaceDecision::Refused(RaceRefusal::NoBatteryPower)
        );
    }

    #[test]
    fn a_weak_battery_caps_the_width_rather_than_honouring_the_dial() {
        // The dangerous case is the hard tail, where extra attempts convert
        // honest failures into shortcut passes. A weak battery misses some
        // mutants, so each extra draw is another chance at a miss.
        assert_eq!(
            race_policy(VerdictClass::Strong, BatteryPower::Weak, 10),
            RaceDecision::Allowed { max_width: 2 }
        );
        // And it caps rather than raises: asking for one still gets one.
        assert_eq!(
            race_policy(VerdictClass::Strong, BatteryPower::Weak, 1),
            RaceDecision::Allowed { max_width: 1 }
        );
    }

    #[test]
    fn a_strong_battery_over_a_strong_claim_races_at_full_width() {
        // The only configuration where the old sentence was right.
        assert_eq!(
            race_policy(VerdictClass::Strong, BatteryPower::Strong, 8),
            RaceDecision::Allowed { max_width: 8 }
        );
    }

    #[test]
    fn a_refusal_is_an_answer_rather_than_a_silent_zero() {
        // Invariant 9, applied honestly. A dial that silently does nothing is
        // worse than one that refuses: the customer believes they bought
        // something. So the refused cases are a distinct variant carrying a
        // reason, not `Allowed { max_width: 0 }`.
        let refused = race_policy(VerdictClass::Strong, BatteryPower::None, 4);
        assert!(
            !matches!(refused, RaceDecision::Allowed { .. }),
            "a refusal must not be expressible as a zero-width allowance"
        );
    }
}
