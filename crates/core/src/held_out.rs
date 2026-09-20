//! The suite is held out from everything that learns, and nothing publishes
//! without it.
//!
//! Phase 30.3. Two disciplines the plan calls easy to state and easy to violate.
//!
//! # Contamination does not degrade the metric, it inverts it
//!
//! > No suite task, tree, or outcome may enter the router's training signal, the
//! > professional grading corpus, the estimator's calibration set, or the Phase
//! > 29.4 experience layer.
//!
//! "Inverts" is exact rather than rhetorical. A contaminated suite stops
//! measuring capability and starts measuring memorisation of the suite, so a
//! *higher* score carries *less* information about how Cortex will do on work it
//! has not seen — and the more thoroughly the contamination takes hold, the more
//! confident and the more wrong the number becomes. There is no level of it that
//! merely adds noise.
//!
//! [`admissible`] is a check on the pair, so the origin has to be known at the
//! point of the write. A sink that accepted anything and filtered later would be
//! a sink someone can forget to filter.
//!
//! The **adversarial and honesty suites are rotated, not fixed**, for the same
//! reason Phase 27 exists: they are the suites optimisation pressure will find.
//!
//! # A version that regresses the suite does not publish
//!
//! Phase 12.11's immutable-versioning rule already forbids editing a published
//! artifact in place. Adding *and cannot be published without a suite result
//! attached* turns the version history from a changelog into an evidence chain.
//!
//! [`SuiteEvidence::Missing`] therefore withholds publication rather than
//! allowing it. A version nobody measured is not a safe default — it is the
//! state every unmeasured change is already in.
//!
//! It also gives the org-authored professionals of Phase 12.12 a property they
//! otherwise lack: a customer's own reviewer pack cannot silently make their
//! outcomes worse.

use serde::{Deserialize, Serialize};

/// Where a piece of data came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataOrigin {
    /// Ordinary customer work.
    Production,
    /// A held-out suite task, its tree, or its outcome.
    HeldOutSuite,
}

/// A place that learns from what it is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningSink {
    /// Phase 5's routing rewards.
    RouterTrainingSignal,
    /// Phase 12's grading corpus for professionals.
    ProfessionalGradingCorpus,
    /// Phase 6.5's forecast calibration.
    EstimatorCalibrationSet,
    /// Phase 29.4's per-repository experience.
    ExperienceLayer,
}

/// Why a write was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum ContaminationRefusal {
    #[error("held-out suite data may not enter {sink:?}")]
    SuiteDataIntoLearningSink { sink: LearningSink },
}

/// Whether data of this origin may enter this sink.
///
/// Every sink refuses suite data and accepts production data. The enum is
/// exhaustive so that adding a fifth thing that learns is a change to this file,
/// where the reasoning is, rather than a new import somewhere that quietly
/// widens the boundary.
pub fn admissible(origin: DataOrigin, sink: LearningSink) -> Result<(), ContaminationRefusal> {
    match origin {
        DataOrigin::Production => Ok(()),
        DataOrigin::HeldOutSuite => Err(ContaminationRefusal::SuiteDataIntoLearningSink { sink }),
    }
}

/// An artifact whose change can move outcomes, and which therefore runs the
/// suite before it publishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangedArtifact {
    Prompt,
    /// Phase 8.4's Engineering Method Library.
    EngineeringMethod,
    /// Phase 12's professionals and knowledge packs, including org-authored
    /// ones.
    ProfessionalPack,
    /// Phase 8.4 and 25.2's check-derivation rules.
    CheckDerivationRule,
    /// Phase 6.2's router policy.
    RouterPolicy,
    /// Phase 6.2's `ExecutionPolicy` versions.
    ExecutionPolicy,
    SandboxImage,
    RunnerImage,
    /// Phase 29's context layer.
    ContextLayer,
}

/// What the suite said about a candidate version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuiteEvidence {
    /// The suite ran and did not regress.
    Passed,
    /// The suite ran and the version regressed it.
    Regressed,
    /// No suite result is attached to this version.
    Missing,
}

/// Whether a version may publish.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Publication {
    Allowed,
    Withheld { reason: WithholdReason },
}

/// Why a version did not publish.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WithholdReason {
    /// It made the suite worse.
    Regressed,
    /// Nobody measured it.
    NoSuiteResult,
}

/// The Phase 30.3 publication gate.
///
/// `artifact` is carried for the record rather than consulted: every artifact
/// that can change behaviour runs the suite, so there is no variant to exempt
/// and no argument to have about which ones count.
pub fn may_publish(_artifact: ChangedArtifact, evidence: SuiteEvidence) -> Publication {
    match evidence {
        SuiteEvidence::Passed => Publication::Allowed,
        SuiteEvidence::Regressed => Publication::Withheld {
            reason: WithholdReason::Regressed,
        },
        SuiteEvidence::Missing => Publication::Withheld {
            reason: WithholdReason::NoSuiteResult,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SINKS: [LearningSink; 4] = [
        LearningSink::RouterTrainingSignal,
        LearningSink::ProfessionalGradingCorpus,
        LearningSink::EstimatorCalibrationSet,
        LearningSink::ExperienceLayer,
    ];

    const ARTIFACTS: [ChangedArtifact; 9] = [
        ChangedArtifact::Prompt,
        ChangedArtifact::EngineeringMethod,
        ChangedArtifact::ProfessionalPack,
        ChangedArtifact::CheckDerivationRule,
        ChangedArtifact::RouterPolicy,
        ChangedArtifact::ExecutionPolicy,
        ChangedArtifact::SandboxImage,
        ChangedArtifact::RunnerImage,
        ChangedArtifact::ContextLayer,
    ];

    #[test]
    fn suite_data_reaches_none_of_the_things_that_learn() {
        // All four, with no exception for the one that seems harmless. A suite
        // outcome in the estimator's calibration set makes the forecast look
        // good on exactly the tasks used to check the forecast.
        for sink in SINKS {
            assert_eq!(
                admissible(DataOrigin::HeldOutSuite, sink),
                Err(ContaminationRefusal::SuiteDataIntoLearningSink { sink })
            );
        }
    }

    #[test]
    fn production_work_reaches_all_of_them() {
        // The corpus is supposed to compound. The boundary is the suite, not
        // learning.
        for sink in SINKS {
            assert_eq!(admissible(DataOrigin::Production, sink), Ok(()));
        }
    }

    #[test]
    fn an_unmeasured_version_does_not_publish() {
        // The rule that turns the version history into an evidence chain. A
        // version nobody measured is not a safe default -- it is the state
        // every unmeasured change is already in.
        for artifact in ARTIFACTS {
            assert_eq!(
                may_publish(artifact, SuiteEvidence::Missing),
                Publication::Withheld {
                    reason: WithholdReason::NoSuiteResult
                },
                "{artifact:?} must not publish without a suite result"
            );
        }
    }

    #[test]
    fn a_regressing_version_does_not_publish() {
        for artifact in ARTIFACTS {
            assert_eq!(
                may_publish(artifact, SuiteEvidence::Regressed),
                Publication::Withheld {
                    reason: WithholdReason::Regressed
                },
                "{artifact:?} must not publish a regression"
            );
        }
    }

    #[test]
    fn a_measured_version_that_held_publishes() {
        for artifact in ARTIFACTS {
            assert_eq!(
                may_publish(artifact, SuiteEvidence::Passed),
                Publication::Allowed
            );
        }
    }

    #[test]
    fn a_customers_own_reviewer_pack_is_gated_like_everything_else() {
        // Phase 12.12's org-authored professionals get a property they
        // otherwise lack: a customer cannot silently make their own outcomes
        // worse, because their pack is a published artifact and this is the
        // gate every published artifact goes through.
        assert_eq!(
            may_publish(ChangedArtifact::ProfessionalPack, SuiteEvidence::Regressed),
            Publication::Withheld {
                reason: WithholdReason::Regressed
            }
        );
    }

    #[test]
    fn the_two_withholding_reasons_are_distinguishable_on_the_record() {
        // "It made things worse" and "nobody checked" call for different
        // responses from whoever is looking at the blocked version.
        let regressed = serde_json::to_value(may_publish(
            ChangedArtifact::Prompt,
            SuiteEvidence::Regressed,
        ))
        .unwrap();
        let missing =
            serde_json::to_value(may_publish(ChangedArtifact::Prompt, SuiteEvidence::Missing))
                .unwrap();
        assert_eq!(regressed["reason"], "regressed");
        assert_eq!(missing["reason"], "no_suite_result");
    }
}
