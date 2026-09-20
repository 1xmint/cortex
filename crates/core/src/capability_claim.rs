//! The falsification test, written before the claim it would falsify.
//!
//! Phase 28.5 states the ceiling plainly so it can be checked rather than
//! believed: Cortex's capability ceiling is the routed portfolio's pass@N times
//! verifier precision, minus integration loss. It exceeds the best single model
//! when — and only when — `N > 1` over genuinely diverse generators, verifier
//! precision is high and *measured*, and decomposition stays on the winning side
//! of its crossover.
//!
//! Fail any one and Cortex is a governance layer over a frontier model:
//! valuable, honest, cheaper, and **not better**. That is a perfectly good
//! product. It is not the stated vision, and the difference has to be measured
//! rather than asserted.
//!
//! > If Cortex at `ultra` does not beat the best single model run at maximum
//! > effort on the held-out suite, at any price, the capability claim is false
//! > and must not be made.
//!
//! # Why the comparison is the hard part
//!
//! The naive version — Cortex against a vendor's published score — proves
//! nothing, and each condition below exists because a specific, measured effect
//! is large enough to manufacture the result on its own.
//!
//! **The baseline is each single model dropped into Cortex's own scaffold.** The
//! same model family shows a roughly **17-point spread** between its vendor's
//! bespoke scaffold and a standardised harness — larger than any effect Cortex
//! is trying to demonstrate. A published number measures a model *and* the
//! scaffold its vendor built around it. Holding the scaffold fixed and varying
//! only the model is the only comparison that isolates the claim.
//!
//! **Cost-matched.** Cortex at `ultra` against a single call is a spend
//! comparison wearing a capability label.
//!
//! **Sealed and post-cutoff.** An unsealed baseline is a retrieval score.
//! Post-cutoff closes training contamination and nothing else.
//!
//! # The headline number is acceptance, not resolution
//!
//! METR (March 2026) found automated grading overstates merge-worthiness by
//! **24.2 points**: roughly half of test-passing pull requests were rejected by
//! maintainers. A capability result reported only against a battery therefore
//! overstates the deliverable by a known, large, measured margin.
//!
//! So both arms carry a maintainer-acceptance rate, and where the two numbers
//! diverge, **the acceptance number is the one that goes in front of a
//! customer.** [`PermittedClaim::headline`] returns acceptance and there is no
//! accessor for the resolution rate, which is the whole reason this is a type
//! and not a comparison written at the call site.

use serde::{Deserialize, Serialize};

/// The conditions that make a measured difference mean anything.
///
/// All four default to `false`. A `..Default::default()` construction therefore
/// produces a design that is refused, rather than one that silently claims
/// properties nobody established.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComparisonDesign {
    /// Each baseline model ran inside Cortex's own scaffold, not its vendor's.
    pub baseline_in_our_scaffold: bool,
    /// Equal budget, both arms.
    pub cost_matched: bool,
    /// Both arms sealed, with the seal recorded.
    pub sealed: bool,
    /// The suite is post-cutoff for every model in the comparison.
    pub post_cutoff: bool,
}

/// One arm of the comparison.
///
/// `acceptance` is `Option` because a run genuinely may not have a human arm —
/// and a missing one is refused rather than defaulted, since defaulting it to
/// the resolution rate is exactly the 24.2-point overstatement.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Arm {
    pub label: String,
    /// Fraction of the held-out suite resolved, as graded by the battery.
    pub resolution: f64,
    /// Fraction a maintainer would actually merge.
    pub acceptance: Option<f64>,
}

/// A maintainer-acceptance rate, and the only number this module will hand out.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AcceptanceRate {
    value: f64,
}

impl AcceptanceRate {
    pub fn as_fraction(&self) -> f64 {
        self.value
    }
}

/// A capability claim that has passed its own falsification test.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PermittedClaim {
    headline: AcceptanceRate,
    beaten: String,
    margin: f64,
}

impl PermittedClaim {
    /// The number that goes in front of a customer.
    ///
    /// There is deliberately no accessor for the resolution rate. Where the two
    /// diverge the resolution rate is the flattering one, and a struct that
    /// offered both would be a struct whose more flattering field gets quoted.
    pub fn headline(&self) -> AcceptanceRate {
        self.headline
    }

    /// The single model this beat, named so the claim can be checked.
    pub fn beaten(&self) -> &str {
        &self.beaten
    }

    /// Acceptance-rate points over the best baseline.
    pub fn margin(&self) -> f64 {
        self.margin
    }
}

/// Why a capability claim may not be made.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimRefusal {
    /// A vendor's published number measures their scaffold as much as their
    /// model, and that gap is bigger than the effect being claimed.
    BaselineNotInOurScaffold,
    /// More spend is not more capability.
    NotCostMatched,
    /// An unsealed baseline is a retrieval score.
    NotSealed,
    /// The suite is inside some model's training window.
    NotPostCutoff,
    /// No maintainer-acceptance arm on the named run. Automated grading
    /// overstates merge-worthiness by a measured 24.2 points, so a
    /// battery-only result is not the deliverable.
    NoAcceptanceArm { arm: String },
    /// Nothing was compared against.
    NoBaseline,
    /// Cortex did not beat the best single model. A tie is a loss: the claim is
    /// *better*, and equal is not better.
    DidNotBeatBestSingleModel,
}

/// The outcome of the falsification test.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClaimVerdict {
    Permitted(PermittedClaim),
    /// Every reason at once, so a reviewer fixes the comparison in one pass
    /// instead of rediscovering the next problem after each repair.
    Refused(Vec<ClaimRefusal>),
}

/// Run the Phase 28.5 falsification test.
///
/// Design failures and outcome failures are reported together. A design failure
/// does not merely weaken the result — it means no result was measured — but
/// listing them alongside the outcome is still right, because the alternative is
/// a reviewer who fixes the seal, re-runs, and only then learns the arm was
/// missing.
pub fn evaluate(design: &ComparisonDesign, cortex: &Arm, baselines: &[Arm]) -> ClaimVerdict {
    let mut refusals = Vec::new();

    if !design.baseline_in_our_scaffold {
        refusals.push(ClaimRefusal::BaselineNotInOurScaffold);
    }
    if !design.cost_matched {
        refusals.push(ClaimRefusal::NotCostMatched);
    }
    if !design.sealed {
        refusals.push(ClaimRefusal::NotSealed);
    }
    if !design.post_cutoff {
        refusals.push(ClaimRefusal::NotPostCutoff);
    }

    if baselines.is_empty() {
        refusals.push(ClaimRefusal::NoBaseline);
    }
    for arm in std::iter::once(cortex).chain(baselines) {
        if arm.acceptance.is_none() {
            refusals.push(ClaimRefusal::NoAcceptanceArm {
                arm: arm.label.clone(),
            });
        }
    }

    // The comparison itself, on acceptance. Only attempted when every arm has
    // the number; otherwise the missing-arm refusals above are the whole story
    // and inventing a comparison here would be the overstatement in code.
    let best = baselines
        .iter()
        .filter_map(|b| b.acceptance.map(|a| (b, a)))
        .max_by(|(_, x), (_, y)| x.total_cmp(y));

    match (cortex.acceptance, best) {
        (Some(ours), Some((beaten, theirs))) if refusals.is_empty() => {
            if ours > theirs {
                return ClaimVerdict::Permitted(PermittedClaim {
                    headline: AcceptanceRate { value: ours },
                    beaten: beaten.label.clone(),
                    margin: ours - theirs,
                });
            }
            refusals.push(ClaimRefusal::DidNotBeatBestSingleModel);
        }
        (Some(ours), Some((_, theirs))) if ours <= theirs => {
            refusals.push(ClaimRefusal::DidNotBeatBestSingleModel);
        }
        _ => {}
    }

    ClaimVerdict::Refused(refusals)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sound_design() -> ComparisonDesign {
        ComparisonDesign {
            baseline_in_our_scaffold: true,
            cost_matched: true,
            sealed: true,
            post_cutoff: true,
        }
    }

    fn arm(label: &str, resolution: f64, acceptance: f64) -> Arm {
        Arm {
            label: label.to_string(),
            resolution,
            acceptance: Some(acceptance),
        }
    }

    fn refusals(v: &ClaimVerdict) -> &[ClaimRefusal] {
        match v {
            ClaimVerdict::Refused(r) => r,
            ClaimVerdict::Permitted(_) => panic!("expected a refusal, got a permitted claim"),
        }
    }

    #[test]
    fn a_sound_comparison_that_wins_permits_the_claim() {
        let verdict = evaluate(
            &sound_design(),
            &arm("cortex-ultra", 0.71, 0.48),
            &[arm("model-a", 0.66, 0.41), arm("model-b", 0.68, 0.44)],
        );
        match verdict {
            ClaimVerdict::Permitted(c) => {
                assert_eq!(c.beaten(), "model-b", "the *best* baseline, not the first");
                assert!((c.headline().as_fraction() - 0.48).abs() < 1e-9);
                assert!((c.margin() - 0.04).abs() < 1e-9);
            }
            ClaimVerdict::Refused(r) => panic!("expected a permitted claim, got {r:?}"),
        }
    }

    #[test]
    fn the_headline_is_acceptance_even_when_resolution_is_far_higher() {
        // The METR point, as an API shape. Resolution 0.71 against acceptance
        // 0.48 is roughly the 24.2-point overstatement, and the only number
        // this type will hand out is the smaller, true one. There is no
        // accessor for the other.
        let verdict = evaluate(
            &sound_design(),
            &arm("cortex-ultra", 0.71, 0.48),
            &[arm("model-a", 0.66, 0.41)],
        );
        let ClaimVerdict::Permitted(c) = verdict else {
            panic!("expected permitted")
        };
        assert!(c.headline().as_fraction() < 0.5);
    }

    #[test]
    fn winning_the_battery_while_losing_the_maintainers_is_a_refusal() {
        // Cortex resolves more of the suite and gets merged less. Reporting
        // that as a capability win is exactly what 28.5 forbids.
        let verdict = evaluate(
            &sound_design(),
            &arm("cortex-ultra", 0.80, 0.39),
            &[arm("model-a", 0.62, 0.44)],
        );
        assert_eq!(
            refusals(&verdict),
            &[ClaimRefusal::DidNotBeatBestSingleModel]
        );
    }

    #[test]
    fn a_tie_is_a_loss() {
        // The claim is *better*. Equal is not better, and a comparison that
        // ties has falsified the claim just as surely as one that loses.
        let verdict = evaluate(
            &sound_design(),
            &arm("cortex-ultra", 0.9, 0.44),
            &[arm("model-a", 0.5, 0.44)],
        );
        assert_eq!(
            refusals(&verdict),
            &[ClaimRefusal::DidNotBeatBestSingleModel]
        );
    }

    #[test]
    fn a_missing_acceptance_arm_is_refused_and_not_defaulted() {
        // Filling this in from the resolution rate is the 24.2-point
        // overstatement written as a convenience.
        let cortex = Arm {
            label: "cortex-ultra".to_string(),
            resolution: 0.9,
            acceptance: None,
        };
        let verdict = evaluate(&sound_design(), &cortex, &[arm("model-a", 0.4, 0.3)]);
        assert_eq!(
            refusals(&verdict),
            &[ClaimRefusal::NoAcceptanceArm {
                arm: "cortex-ultra".to_string()
            }]
        );
    }

    #[test]
    fn a_baseline_without_an_acceptance_arm_is_refused_too() {
        let baseline = Arm {
            label: "model-a".to_string(),
            resolution: 0.4,
            acceptance: None,
        };
        let verdict = evaluate(&sound_design(), &arm("cortex-ultra", 0.9, 0.5), &[baseline]);
        assert_eq!(
            refusals(&verdict),
            &[ClaimRefusal::NoAcceptanceArm {
                arm: "model-a".to_string()
            }]
        );
    }

    #[test]
    fn the_default_design_claims_nothing() {
        // `ComparisonDesign::default()` asserts no property. A design struct
        // whose fields defaulted to true would let an incomplete
        // initialisation claim a sealed, cost-matched comparison nobody ran.
        let verdict = evaluate(
            &ComparisonDesign::default(),
            &arm("cortex-ultra", 0.9, 0.9),
            &[arm("model-a", 0.1, 0.1)],
        );
        assert_eq!(
            refusals(&verdict),
            &[
                ClaimRefusal::BaselineNotInOurScaffold,
                ClaimRefusal::NotCostMatched,
                ClaimRefusal::NotSealed,
                ClaimRefusal::NotPostCutoff,
            ]
        );
    }

    #[test]
    fn each_design_condition_refuses_on_its_own() {
        let cortex = arm("cortex-ultra", 0.9, 0.9);
        let baselines = [arm("model-a", 0.1, 0.1)];
        let cases = [
            (
                ComparisonDesign {
                    baseline_in_our_scaffold: false,
                    ..sound_design()
                },
                ClaimRefusal::BaselineNotInOurScaffold,
            ),
            (
                ComparisonDesign {
                    cost_matched: false,
                    ..sound_design()
                },
                ClaimRefusal::NotCostMatched,
            ),
            (
                ComparisonDesign {
                    sealed: false,
                    ..sound_design()
                },
                ClaimRefusal::NotSealed,
            ),
            (
                ComparisonDesign {
                    post_cutoff: false,
                    ..sound_design()
                },
                ClaimRefusal::NotPostCutoff,
            ),
        ];
        for (design, expected) in cases {
            let verdict = evaluate(&design, &cortex, &baselines);
            assert_eq!(
                refusals(&verdict),
                std::slice::from_ref(&expected),
                "a landslide win must not survive {expected:?}"
            );
        }
    }

    #[test]
    fn beating_nobody_is_not_a_win() {
        let verdict = evaluate(&sound_design(), &arm("cortex-ultra", 0.9, 0.9), &[]);
        assert_eq!(refusals(&verdict), &[ClaimRefusal::NoBaseline]);
    }

    #[test]
    fn every_reason_is_reported_at_once() {
        // A reviewer should not have to fix the seal, re-run the suite, and
        // only then discover the human arm was never collected.
        let cortex = Arm {
            label: "cortex-ultra".to_string(),
            resolution: 0.9,
            acceptance: None,
        };
        let verdict = evaluate(&ComparisonDesign::default(), &cortex, &[]);
        let r = refusals(&verdict);
        assert!(r.len() >= 6, "expected the full list, got {r:?}");
        assert!(r.contains(&ClaimRefusal::NoBaseline));
        assert!(r.contains(&ClaimRefusal::NotSealed));
        assert!(r.contains(&ClaimRefusal::NoAcceptanceArm {
            arm: "cortex-ultra".to_string()
        }));
    }
}
