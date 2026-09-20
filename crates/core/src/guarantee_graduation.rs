//! The guarantee graduates, exactly the way pricing graduates.
//!
//! Phase 31.3. Outcome-based pricing succeeds overwhelmingly at vendors with
//! **years of outcome data** behind the estimate. Cortex has none. It is being
//! asked to underwrite before it can price — the position every new insurer is
//! in, and the reason none of them start by writing the riskiest book.
//!
//! Phase 6.4 already requires a measured variance threshold before a task class
//! moves to fixed pricing. The same gate applies to the refund promise: a class
//! is eligible for the full guarantee only once its measured `p`, its cost
//! distribution, and its false-accept rate clear a threshold. Before that it is
//! priced with a narrower guarantee and **labelled as such**. That is honest,
//! defensible to a customer, and it turns the cold-start problem from an
//! existential risk into a published roadmap.
//!
//! # The false-accept rate is the gross margin
//!
//! > **Cortex's own false-accept rate is its gross margin**, not a quality
//! > metric. A guarantee priced without it is priced on a number nobody has
//! > measured, and it must be measured before a guarantee is priced.
//!
//! So [`Measurement::false_accept_rate`] is an `Option`, and `None` is a
//! refusal rather than a zero — see [`Narrowing::FalseAcceptRateNeverMeasured`].
//! An unmeasured false-accept rate defaulting to zero is a full guarantee priced
//! on the assumption that Cortex is never wrong.
//!
//! # The three published priors, and why they may not be averaged
//!
//! Cortex has no history, but the industry has measured the quantity that drives
//! the loss ratio — how often a patch passes a frozen battery and is nonetheless
//! wrong:
//!
//! | Measurement | Rate |
//! |---|---|
//! | Patches passing SWE-bench Verified that were erroneous under augmented tests (UTBoost) | 15.7% |
//! | Instances retaining at least one surviving mutant after a passing patch | 77% |
//! | Test-passing pull requests rejected by maintainers (METR) | ~50% |
//!
//! **The three measure different things and should not be averaged.** The first
//! is a false-accept rate against a *strengthened* battery — the closest proxy
//! for what Cortex will refund on. The second is a statement about **battery
//! power**, and it is the more alarming: on more than three quarters of
//! instances the exam could not detect a deliberate defect. The third is Phase
//! 28.5's acceptance gap, pricing a different obligation entirely — a customer
//! who rejects a change Cortex verified is a refund conversation whatever the
//! battery said.
//!
//! [`SeedPrior`] is an enum with a rate and a subject on each variant, and there
//! is no function that combines them. Their mean, 47.6%, would be a number about
//! nothing.

use serde::{Deserialize, Serialize};

/// A published measurement that seeds a prior, and what it is a measurement
/// *of*.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedPrior {
    /// UTBoost: 15.7% of patches passing SWE-bench Verified were erroneous
    /// under augmented tests. The closest available proxy for what Cortex
    /// refunds on.
    FalseAcceptAgainstStrengthenedBattery,
    /// 77% of instances retained at least one surviving mutant after a passing
    /// patch. A statement about the *exam*, not about the patch.
    SurvivingMutantAfterPassingPatch,
    /// METR, March 2026: roughly half of test-passing pull requests were
    /// rejected by maintainers. Prices the acceptance obligation.
    MaintainerRejectionOfPassingWork,
}

impl SeedPrior {
    pub fn rate(&self) -> f64 {
        match self {
            Self::FalseAcceptAgainstStrengthenedBattery => 0.157,
            Self::SurvivingMutantAfterPassingPatch => 0.77,
            Self::MaintainerRejectionOfPassingWork => 0.50,
        }
    }

    /// What the number is about. Two priors with the same rate would still not
    /// be interchangeable, and this is why.
    pub fn subject(&self) -> &'static str {
        match self {
            Self::FalseAcceptAgainstStrengthenedBattery => "the delivered patch",
            Self::SurvivingMutantAfterPassingPatch => "the battery's power",
            Self::MaintainerRejectionOfPassingWork => "the maintainer's acceptance",
        }
    }
}

/// The prior that seeds the initial reserve.
///
/// Named as one specific measurement rather than assembled from the three,
/// because only this one measures what Cortex refunds on. It is the *prior*: it
/// sets the initial reserve and the initial graduation threshold, and it is
/// replaced by Cortex's own measurement as that accumulates.
pub const RESERVE_SEED: SeedPrior = SeedPrior::FalseAcceptAgainstStrengthenedBattery;

/// What has actually been measured about one task class.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    /// Resolved instances behind these numbers.
    pub resolved_instances: u32,
    /// Measured pass rate.
    pub pass_rate: f64,
    /// Width of the 95% interval on `pass_rate`. Phase 6.4's variance gate.
    pub pass_rate_interval_width: f64,
    /// Measured share of `verified` deliveries that were nonetheless wrong.
    ///
    /// `None` means never measured — which is not zero, and is the state every
    /// class starts in.
    pub false_accept_rate: Option<f64>,
}

/// What a class must clear before it carries the full guarantee.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Threshold {
    pub min_resolved_instances: u32,
    pub max_pass_rate_interval_width: f64,
    /// The break-even pass rate for this class, from the loss model.
    pub break_even_pass_rate: f64,
    /// The most false-accept Cortex will underwrite at full value.
    pub max_false_accept_rate: f64,
}

/// Why a class is not yet on the full guarantee.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Narrowing {
    /// Not enough resolved instances to have measured anything.
    TooLittleHistory,
    /// `p` is known too imprecisely to underwrite against.
    PassRateTooUncertain,
    /// Measured `p` is below break-even. Selling the full guarantee here is
    /// selling a known loss.
    BelowBreakEven,
    /// Nobody has measured how often this class delivers a wrong `verified`.
    /// The gross margin is unknown, so the guarantee cannot be priced.
    FalseAcceptRateNeverMeasured,
    /// Measured, and too high to underwrite at full value.
    FalseAcceptRateTooHigh,
}

/// What guarantee a class may be sold with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Guarantee {
    /// Full refund-on-failure.
    Full,
    /// Narrower, and labelled as such — with every reason, so the roadmap to
    /// full is explicit rather than a matter of asking.
    Narrow { because: Vec<Narrowing> },
}

/// Decide the guarantee level for a task class.
///
/// Every failing condition is reported, not the first. A class one measurement
/// short and a class four measurements short are different situations, and
/// "narrow" alone tells a customer neither what is missing nor when it changes.
pub fn guarantee_for(measured: &Measurement, threshold: &Threshold) -> Guarantee {
    let mut because = Vec::new();

    if measured.resolved_instances < threshold.min_resolved_instances {
        because.push(Narrowing::TooLittleHistory);
    }
    if measured.pass_rate_interval_width > threshold.max_pass_rate_interval_width {
        because.push(Narrowing::PassRateTooUncertain);
    }
    if measured.pass_rate < threshold.break_even_pass_rate {
        because.push(Narrowing::BelowBreakEven);
    }
    match measured.false_accept_rate {
        None => because.push(Narrowing::FalseAcceptRateNeverMeasured),
        Some(rate) if rate > threshold.max_false_accept_rate => {
            because.push(Narrowing::FalseAcceptRateTooHigh)
        }
        Some(_) => {}
    }

    if because.is_empty() {
        Guarantee::Full
    } else {
        Guarantee::Narrow { because }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn threshold() -> Threshold {
        Threshold {
            min_resolved_instances: 200,
            max_pass_rate_interval_width: 0.10,
            break_even_pass_rate: 0.45,
            max_false_accept_rate: 0.05,
        }
    }

    fn graduated() -> Measurement {
        Measurement {
            resolved_instances: 500,
            pass_rate: 0.82,
            pass_rate_interval_width: 0.04,
            false_accept_rate: Some(0.02),
        }
    }

    fn narrowings(g: &Guarantee) -> &[Narrowing] {
        match g {
            Guarantee::Narrow { because } => because,
            Guarantee::Full => panic!("expected a narrowed guarantee, got Full"),
        }
    }

    #[test]
    fn a_brand_new_class_carries_a_narrower_guarantee() {
        // The cold start. Cortex is being asked to underwrite before it can
        // price, which is where every new insurer starts -- and none of them
        // start by writing the riskiest book.
        let fresh = Measurement {
            resolved_instances: 0,
            pass_rate: 0.0,
            pass_rate_interval_width: 1.0,
            false_accept_rate: None,
        };
        let g = guarantee_for(&fresh, &threshold());
        assert!(narrowings(&g).contains(&Narrowing::TooLittleHistory));
        assert!(narrowings(&g).contains(&Narrowing::FalseAcceptRateNeverMeasured));
    }

    #[test]
    fn an_unmeasured_false_accept_rate_is_not_a_zero() {
        // The rule this module exists for. Everything else about this class is
        // excellent, and nobody has measured how often it delivers a wrong
        // `verified` -- so the gross margin is unknown and the guarantee cannot
        // be priced. Defaulting it to zero would sell a full guarantee on the
        // assumption that Cortex is never wrong.
        let unmeasured = Measurement {
            false_accept_rate: None,
            ..graduated()
        };
        assert_eq!(
            narrowings(&guarantee_for(&unmeasured, &threshold())),
            &[Narrowing::FalseAcceptRateNeverMeasured]
        );
    }

    #[test]
    fn a_measured_class_that_clears_every_gate_graduates() {
        assert_eq!(guarantee_for(&graduated(), &threshold()), Guarantee::Full);
    }

    #[test]
    fn a_lot_of_history_does_not_substitute_for_precision() {
        // Phase 6.4's variance gate. Thousands of instances with a `p` known
        // only to +/- 15 points is not something to underwrite against.
        let vague = Measurement {
            resolved_instances: 5_000,
            pass_rate_interval_width: 0.30,
            ..graduated()
        };
        assert_eq!(
            narrowings(&guarantee_for(&vague, &threshold())),
            &[Narrowing::PassRateTooUncertain]
        );
    }

    #[test]
    fn a_class_below_break_even_does_not_get_the_full_guarantee() {
        // Selling it would be selling a known loss.
        let losing = Measurement {
            pass_rate: 0.30,
            ..graduated()
        };
        assert_eq!(
            narrowings(&guarantee_for(&losing, &threshold())),
            &[Narrowing::BelowBreakEven]
        );
    }

    #[test]
    fn a_measured_but_high_false_accept_rate_reads_differently_from_an_absent_one() {
        // "We measured it and it is 12%" and "nobody looked" call for different
        // responses -- the first is an engineering problem with a known size,
        // the second is not knowing the size of anything.
        let high = Measurement {
            false_accept_rate: Some(0.12),
            ..graduated()
        };
        assert_eq!(
            narrowings(&guarantee_for(&high, &threshold())),
            &[Narrowing::FalseAcceptRateTooHigh]
        );
    }

    #[test]
    fn every_failing_gate_is_reported_so_the_roadmap_is_explicit() {
        // A class one measurement short and a class four short are different
        // situations, and "narrow" alone tells a customer neither what is
        // missing nor when it changes.
        let bad = Measurement {
            resolved_instances: 10,
            pass_rate: 0.2,
            pass_rate_interval_width: 0.4,
            false_accept_rate: None,
        };
        let g = guarantee_for(&bad, &threshold());
        assert_eq!(narrowings(&g).len(), 4);
    }

    #[test]
    fn the_three_seed_priors_measure_three_different_things() {
        // They must not be averaged. Their mean is 47.6%, which is a number
        // about nothing -- and there is no function here that computes it.
        let priors = [
            SeedPrior::FalseAcceptAgainstStrengthenedBattery,
            SeedPrior::SurvivingMutantAfterPassingPatch,
            SeedPrior::MaintainerRejectionOfPassingWork,
        ];
        let subjects: Vec<&str> = priors.iter().map(|p| p.subject()).collect();
        assert_eq!(subjects.len(), 3);
        assert_eq!(
            subjects
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3,
            "each prior is about a different thing"
        );
    }

    #[test]
    fn the_reserve_is_seeded_from_the_one_that_measures_refunds() {
        // The false-accept rate against a strengthened battery is the closest
        // proxy for what Cortex refunds on. The surviving-mutant rate is far
        // larger and is about the exam, and seeding from it would reserve
        // against the wrong quantity.
        assert_eq!(
            RESERVE_SEED,
            SeedPrior::FalseAcceptAgainstStrengthenedBattery
        );
        assert!((RESERVE_SEED.rate() - 0.157).abs() < 1e-9);
        assert!(SeedPrior::SurvivingMutantAfterPassingPatch.rate() > RESERVE_SEED.rate());
    }
}
