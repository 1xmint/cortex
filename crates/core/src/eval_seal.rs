//! Invariant 34: a sealed run and an unsealed one measure different quantities.
//!
//! Phase 30.2a. The benchmark Cortex can build that nobody else can is its
//! customers' own merged pull requests: the description is the request, the
//! merge base is the starting tree, the tests at merge time are the exam, and
//! the merged diff is a reference solution nobody had to write.
//!
//! Restricting to PRs merged after the model's training cutoff closes **training**
//! contamination. It does nothing about **retrieval** contamination, and this
//! construction makes that worse than for any public suite: the tasks *are*
//! merged pull requests, so the reference solution is sitting in the
//! repository's own git history, one `git log` away from the agent being
//! evaluated. A harness with a shell and a network does not need the answer in
//! its weights.
//!
//! # This is measured, not hypothetical
//!
//! Cursor's June 2026 sealed-evaluation report, on sealing an environment that
//! had previously been open:
//!
//! | | Unsealed | Sealed |
//! |---|---|---|
//! | Suite A | 87.1% | 73.0% |
//! | Suite B | 74.7% | 54.0% |
//!
//! with **57%** of runs performing an upstream lookup, **9%** mining git history
//! directly, and **63% of resolutions retrieved rather than derived**. A
//! twenty-point swing is larger than every capability effect this plan proposes
//! to measure. An unsealed evaluation is not a noisy capability measurement — it
//! is a measurement of a different quantity.
//!
//! > **Invariant 34.** An evaluation environment is sealed, and the seal is
//! > recorded with the result. No network egress beyond the provider allowlist;
//! > the evaluation checkout is truncated at the merge base so the reference
//! > solution is unreachable from git history; and every suite run reports a
//! > retrieval-leak probe. **An unsealed run may not be compared to a sealed
//! > one**, in either direction, internally or externally.
//!
//! # The declared seal is a claim; the probe is the evidence
//!
//! [`Seal::Sealed`] carries its [`LeakProbe`], and [`Seal::holds`] is false when
//! that probe found a breach. So a run that *declared* a seal and then read git
//! objects outside the truncated range is not sealed, and [`comparable`] refuses
//! it against a real one. Nothing about believing the environment was sealed
//! survives evidence that it was not.
//!
//! Truncation is the part that is easy to get wrong. Removing the merge commit
//! is not enough: the solution survives in reflogs, in remote refs, in packed
//! objects, in tags, and in any sibling branch. The checkout handed to the
//! harness is constructed *at* the merge base with no path to anything after it,
//! and `git_objects_outside_range` is what catches the attempt when that
//! construction is imperfect.
//!
//! # What Cortex already owns
//!
//! PR C2's egress allowlist is derived at plan time and enforced at the sandbox
//! boundary — which is exactly the web-lookup channel, the 57% row, closed
//! already in shipped code. It was built for tenancy and safety and is an
//! existing, unclaimed evaluation asset. The sealing work remaining is git
//! truncation and the probe, not network isolation.

use serde::{Deserialize, Serialize};

/// What one suite run's leak probe found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LeakProbe {
    /// The run attempted network egress outside the provider allowlist. The
    /// 57% channel.
    pub egress_outside_allowlist: bool,
    /// The run read git objects outside the truncated range. The 9% channel,
    /// and the one truncation is supposed to make impossible.
    pub git_objects_outside_range: bool,
    /// How close the delivered diff is to the reference solution.
    ///
    /// **A signal, not a verdict.** It feeds Phase 27.2's disclosure list, where
    /// a model-produced signal is allowed to live. High similarity is what a
    /// correct answer looks like as often as it is what a copied one looks like,
    /// and treating it as proof would fail the runs that got it right.
    pub reference_similarity: Option<f64>,
}

impl LeakProbe {
    /// Whether the probe caught the run reaching outside its environment.
    ///
    /// Deliberately excludes `reference_similarity`: that is an observation
    /// about the *answer*, not about the environment, and only the environment
    /// determines what the number measures.
    pub fn found_a_breach(&self) -> bool {
        self.egress_outside_allowlist || self.git_objects_outside_range
    }
}

/// The environment a suite run happened in.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Seal {
    /// Egress restricted to the provider allowlist and the checkout truncated
    /// at the merge base — as claimed, with the probe that checked.
    Sealed { probe: LeakProbe },
    /// Not sealed. A number from here measures retrieval as much as capability.
    Unsealed,
}

impl Seal {
    /// Whether this run's seal actually held.
    ///
    /// A declared seal with a breached probe is not a seal. This is the method
    /// every comparison goes through, so believing an environment was sealed
    /// cannot outlive evidence that it was not.
    pub fn holds(&self) -> bool {
        match self {
            Self::Sealed { probe } => !probe.found_a_breach(),
            Self::Unsealed => false,
        }
    }

    /// The value recorded with the result.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sealed { probe } if probe.found_a_breach() => "sealed_but_breached",
            Self::Sealed { .. } => "sealed",
            Self::Unsealed => "unsealed",
        }
    }
}

/// Why two runs may not be compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum ComparabilityRefusal {
    /// One run's seal held and the other's did not. They measure different
    /// quantities, and the difference between them is not an effect.
    #[error("a sealed run may not be compared to an unsealed one, in either direction")]
    MixedSeals,
}

/// Whether two suite runs may be compared.
///
/// Both seals holding is comparable. Neither holding is also comparable — two
/// unsealed runs measure the same different quantity, so the difference between
/// them is meaningful about *that* quantity, and the result must be labelled
/// unsealed. It is the mixture that is forbidden, because a twenty-point sealing
/// swing dwarfs every effect being looked for.
pub fn comparable(a: &Seal, b: &Seal) -> Result<(), ComparabilityRefusal> {
    if a.holds() == b.holds() {
        Ok(())
    } else {
        Err(ComparabilityRefusal::MixedSeals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean() -> Seal {
        Seal::Sealed {
            probe: LeakProbe::default(),
        }
    }

    #[test]
    fn a_sealed_run_may_not_be_compared_to_an_unsealed_one() {
        // The invariant, in both directions, which is how it is written.
        assert_eq!(
            comparable(&clean(), &Seal::Unsealed),
            Err(ComparabilityRefusal::MixedSeals)
        );
        assert_eq!(
            comparable(&Seal::Unsealed, &clean()),
            Err(ComparabilityRefusal::MixedSeals)
        );
    }

    #[test]
    fn a_declared_seal_that_the_probe_contradicts_is_not_a_seal() {
        // The subtlety worth the type. This run believed it was sealed and then
        // mined git history -- the 9% channel. Comparing it to a real sealed run
        // would put a twenty-point retrieval advantage inside a measurement of a
        // few percent.
        let breached = Seal::Sealed {
            probe: LeakProbe {
                git_objects_outside_range: true,
                ..LeakProbe::default()
            },
        };
        assert!(!breached.holds());
        assert_eq!(breached.as_str(), "sealed_but_breached");
        assert_eq!(
            comparable(&breached, &clean()),
            Err(ComparabilityRefusal::MixedSeals)
        );
    }

    #[test]
    fn an_egress_breach_also_breaks_the_seal() {
        // The 57% channel, and the one Cortex's plan-time egress allowlist
        // already closes at the sandbox boundary.
        let breached = Seal::Sealed {
            probe: LeakProbe {
                egress_outside_allowlist: true,
                ..LeakProbe::default()
            },
        };
        assert!(!breached.holds());
    }

    #[test]
    fn similarity_to_the_reference_is_a_signal_and_never_breaks_the_seal() {
        // High similarity is what a correct answer looks like as often as it is
        // what a copied one looks like. Treating it as proof would fail the
        // runs that got it right, so it goes to Phase 27.2's disclosure list --
        // where a model-produced signal is allowed to live -- and leaves the
        // environment's status alone.
        let suspicious = Seal::Sealed {
            probe: LeakProbe {
                reference_similarity: Some(0.99),
                ..LeakProbe::default()
            },
        };
        assert!(suspicious.holds());
        assert_eq!(suspicious.as_str(), "sealed");
        assert_eq!(comparable(&suspicious, &clean()), Ok(()));
    }

    #[test]
    fn two_sealed_runs_are_comparable() {
        assert_eq!(comparable(&clean(), &clean()), Ok(()));
    }

    #[test]
    fn two_unsealed_runs_are_comparable_to_each_other() {
        // They measure the same different quantity, so their difference is
        // meaningful about that quantity. It is the mixture that is forbidden.
        assert_eq!(comparable(&Seal::Unsealed, &Seal::Unsealed), Ok(()));
    }

    #[test]
    fn two_breached_runs_are_comparable_to_each_other_and_to_an_unsealed_one() {
        // Consistent with the rule above: a breached seal is an unsealed run
        // with an inaccurate label, so it belongs with the unsealed ones.
        let breached = Seal::Sealed {
            probe: LeakProbe {
                egress_outside_allowlist: true,
                ..LeakProbe::default()
            },
        };
        assert_eq!(comparable(&breached, &breached), Ok(()));
        assert_eq!(comparable(&breached, &Seal::Unsealed), Ok(()));
    }

    #[test]
    fn the_seal_is_recorded_with_the_result() {
        // "The seal is recorded with the result" is half the invariant. Three
        // distinguishable values, because "sealed but breached" is a different
        // fact from either of the others and a reader needs to see which.
        assert_eq!(clean().as_str(), "sealed");
        assert_eq!(Seal::Unsealed.as_str(), "unsealed");
        let json = serde_json::to_value(clean()).unwrap();
        assert_eq!(json["kind"], "sealed");
    }

    #[test]
    fn a_probe_that_found_nothing_is_not_a_breach() {
        assert!(!LeakProbe::default().found_a_breach());
        assert!(LeakProbe {
            egress_outside_allowlist: true,
            ..LeakProbe::default()
        }
        .found_a_breach());
        assert!(LeakProbe {
            git_objects_outside_range: true,
            ..LeakProbe::default()
        }
        .found_a_breach());
    }
}
