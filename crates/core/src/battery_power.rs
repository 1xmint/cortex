//! How much the battery could have caught.
//!
//! Phase 27.3. Two quantities govern the whole product and neither is measured:
//! `p_fa` — the battery passed and the work is wrong — and `p_fr`, its mirror.
//! `p_fr` costs Cortex money directly under refund-on-failure, which at least
//! makes it self-correcting. `p_fa` is not: every incentive in the system
//! points away from discovering it. The router learns a false accept as
//! success, the professional record scores it as a hit, and the customer finds
//! it in production.
//!
//! [`BatteryPower`] is the first of the three estimators the phase names, and
//! the only *executable* measure of check power that exists: inject mutants
//! into the subject surface, re-run the frozen battery, and see whether it
//! notices. A battery that survives its mutants proves nothing about the code
//! it graded, however green it is.
//!
//! # What is here, and what is not
//!
//! Here: the class, and the rule that pairs it with a [`VerdictClass`].
//!
//! **Not here: the measurement.** Nothing computes a mutation score yet, so
//! nothing produces a [`BatteryPower`] except a caller declaring one. Running
//! mutants needs a per-ecosystem tool and a plan-time hook, and is scoped in
//! the phase to the change's blast radius rather than the repository — cheap
//! because it runs against the *base* tree at plan time, off the critical path,
//! where the answer is a property of the repo rather than of the change.
//!
//! Until that lands, a declared `Strong` battery is a claim nobody checked. The
//! pairing rule below is still worth having: it is what stops the two claims
//! being combined into the one shape this phase exists to prevent.

use serde::{Deserialize, Serialize};

use crate::diff_surface::VerdictClass;
use crate::verification::Verdict;

/// What the frozen battery could distinguish in the region being changed.
///
/// A class rather than a score, because the number is only meaningful against
/// per-ecosystem thresholds and a receipt that says `0.62` invites arguments
/// about the third decimal rather than about whether the checks work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatteryPower {
    /// The battery kills its mutants. Passing it means something.
    Strong,
    /// The battery catches some mutants and misses others. Passing it is
    /// evidence, not proof.
    Weak,
    /// The frozen checks cannot distinguish correct from incorrect code in the
    /// region being changed.
    ///
    /// The work may still proceed — this is not a gate. What it may not do is
    /// be sold as proven, and this is the moment Phase 24.2's characterization
    /// on-ramp sells itself: the customer is shown "the checks in this region
    /// cannot fail" and offered the task that fixes it.
    None,
}

impl BatteryPower {
    /// Whether a green battery of this power is worth calling proof.
    pub fn can_support_a_verified_claim(&self) -> bool {
        !matches!(self, Self::None)
    }
}

/// Why a declared pairing was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PairingRefusal {
    /// `strong` verdict over a `none` battery.
    ///
    /// The plan names this exactly: *"a maximally confident claim resting on
    /// checks that cannot fail"*. It is the shape Phase 27 exists to prevent,
    /// and it is refused at declaration rather than corrected at delivery,
    /// because by delivery the work is done and someone is owed an answer.
    StrongClaimOnNoBattery,
}

/// Whether a plan may declare this pairing at all.
///
/// Called at plan time. A refusal here is cheap; the same refusal after the
/// work is done is a customer holding a bill and an argument.
pub fn permitted_pairing(class: VerdictClass, power: BatteryPower) -> Result<(), PairingRefusal> {
    match (class, power) {
        (VerdictClass::Strong, BatteryPower::None) => Err(PairingRefusal::StrongClaimOnNoBattery),
        _ => Ok(()),
    }
}

/// The verdict a green battery of this power may produce.
///
/// `Verified` carries the badge and the refund promise. A `none` battery cannot
/// earn either: every check passed and none of them could have failed, so
/// `Unverified` is the honest answer — charged at task rate, sold without the
/// badge, which is exactly what invariant 22 already says that state is for.
pub fn verdict_for_passing_battery(power: BatteryPower) -> Verdict {
    if power.can_support_a_verified_claim() {
        Verdict::Verified
    } else {
        Verdict::Unverified
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_maximally_confident_claim_may_not_rest_on_checks_that_cannot_fail() {
        // The pairing the whole phase exists to prevent, refused where it is
        // still cheap to refuse it -- at plan time, before anyone has done
        // work they expect to be paid for.
        assert_eq!(
            permitted_pairing(VerdictClass::Strong, BatteryPower::None),
            Err(PairingRefusal::StrongClaimOnNoBattery)
        );
    }

    #[test]
    fn every_other_pairing_is_allowed() {
        // `none` is not a gate. Weak checks over authored work is most of
        // brownfield, and refusing it would refuse the customers Phase 24.2
        // exists to reach.
        for (class, power) in [
            (VerdictClass::Strong, BatteryPower::Strong),
            (VerdictClass::Strong, BatteryPower::Weak),
            (VerdictClass::Authored, BatteryPower::Strong),
            (VerdictClass::Authored, BatteryPower::Weak),
            (VerdictClass::Authored, BatteryPower::None),
        ] {
            assert_eq!(
                permitted_pairing(class, power),
                Ok(()),
                "{class:?} over {power:?} must be allowed"
            );
        }
    }

    #[test]
    fn a_battery_that_cannot_fail_does_not_earn_the_badge() {
        // Every check passed and none of them could have failed. That is not
        // proof, so it is not `Verified` -- it is charged at task rate without
        // the badge and without the refund promise.
        assert_eq!(
            verdict_for_passing_battery(BatteryPower::None),
            Verdict::Unverified
        );
        assert_eq!(
            verdict_for_passing_battery(BatteryPower::Weak),
            Verdict::Verified
        );
        assert_eq!(
            verdict_for_passing_battery(BatteryPower::Strong),
            Verdict::Verified
        );
    }

    #[test]
    fn unverified_still_bills_and_that_is_the_point() {
        // A `none` battery does not make the work free. The customer asked for
        // it and got it; what they did not get is the badge. Reading this as
        // "no charge" would make honesty about check power expensive, and the
        // thing you make expensive is the thing that stops happening.
        assert!(Verdict::Unverified.has_billing_effect());
        assert!(!Verdict::Inconclusive.has_billing_effect());
    }
}
