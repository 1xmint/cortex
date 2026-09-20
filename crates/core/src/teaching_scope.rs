//! What a teaching artifact may say when the record is thin.
//!
//! Phase 35.8. The rule from 35.3 — derived from the record, never narrated —
//! is easy to follow where the record is rich.
//!
//! > **The record is thin in exactly the cases a teaching layer would most want
//! > to talk about, and those cases are common rather than exceptional.**
//!
//! | State | What the teaching layer must not do |
//! |---|---|
//! | `Unverified` verdict | explain *why the code is correct* — nothing established that it is |
//! | `none` or weak battery power | present the passing checks as evidence of correctness — they discriminate nothing |
//! | degraded or absent comprehension | explain the subsystem's structure — Cortex did not have the map either |
//! | `Inconclusive` on an exam-touching diff | teach the change as a worked example at all — the example is contaminated |
//!
//! Each row is a different refusal with a different reason, which is why this is
//! a table rather than one confidence score. "We are less sure" is not the same
//! statement as "the checks that passed cannot tell right from wrong", and
//! collapsing them would lose the part a reader needs.
//!
//! # The one thing always permitted
//!
//! [`Intent::ReportWhatTheRecordSays`] is never refused. The failure this guards
//! against is a confident wrong explanation, not an honest thin one — silence
//! leaves someone uninformed, and saying "the battery could not settle this"
//! leaves them correctly informed about a thin record. A gate that refused
//! everything would push the surface toward saying nothing, which is the
//! outcome nobody wanted.
//!
//! # Why a failed change may still be taught
//!
//! Deliberately not in the table. A worked example of a change that *failed* is
//! good teaching material — what went wrong, and why — and it is exactly the
//! shape a retrieval item takes. Only contamination bars the worked example,
//! because there the example itself cannot be trusted.

use serde::{Deserialize, Serialize};

use crate::battery_power::BatteryPower;
use crate::verification::Verdict;

/// What the comprehension layer managed for this step.
///
/// Local rather than imported so this module lands independently of the
/// comprehension work; the three states line up with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapState {
    Structural,
    Degraded,
    Absent,
}

/// What Cortex actually established about the step being explained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub verdict: Verdict,
    pub battery_power: BatteryPower,
    pub map: MapState,
    /// The delivered diff touched the exam surface.
    pub exam_touched: bool,
}

/// Something a teaching artifact might try to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    /// "This is correct, and here is why."
    ExplainWhyTheCodeIsCorrect,
    /// "These checks passed, so the change is right."
    PresentPassingChecksAsEvidence,
    /// "Here is how this subsystem is put together."
    ExplainTheSubsystemStructure,
    /// "Here is a change to learn from."
    TeachTheChangeAsAWorkedExample,
    /// "Here is what the record says, including what it could not settle."
    ReportWhatTheRecordSays,
}

/// Why an intent is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// Invariant 22. The work was sold without a verification promise.
    NothingEstablishedCorrectness,
    /// Invariant 25. A battery that cannot distinguish correct from incorrect
    /// code is not evidence of either.
    ThoseChecksDiscriminateNothing,
    /// Invariant 28. Cortex worked here without a map, so it has no more
    /// standing to describe the structure than the reader does.
    CortexDidNotHaveTheMapEither,
    /// Invariant 25. A change that rewrote its own exam is not a worked
    /// example; it is a worked example of the thing being guarded against.
    TheWorkedExampleIsContaminated,
}

/// Whether an intent may be carried out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Permission {
    Permitted,
    Refused { because: Refusal },
}

/// Apply the Phase 35.8 table.
pub fn may(intent: Intent, evidence: &Evidence) -> Permission {
    let refuse = |because| Permission::Refused { because };

    match intent {
        Intent::ReportWhatTheRecordSays => Permission::Permitted,

        Intent::ExplainWhyTheCodeIsCorrect => {
            if evidence.verdict == Verdict::Verified {
                Permission::Permitted
            } else {
                refuse(Refusal::NothingEstablishedCorrectness)
            }
        }

        Intent::PresentPassingChecksAsEvidence => match evidence.battery_power {
            // Strong kills its mutants, so a pass means something.
            BatteryPower::Strong => Permission::Permitted,
            // Weak catches some and misses others: a pass is evidence the
            // change did not break what the battery covers, which is a
            // different claim from correctness and must not be dressed as one.
            BatteryPower::Weak | BatteryPower::None => {
                refuse(Refusal::ThoseChecksDiscriminateNothing)
            }
        },

        Intent::ExplainTheSubsystemStructure => match evidence.map {
            MapState::Structural => Permission::Permitted,
            MapState::Degraded | MapState::Absent => refuse(Refusal::CortexDidNotHaveTheMapEither),
        },

        Intent::TeachTheChangeAsAWorkedExample => {
            if evidence.verdict == Verdict::Inconclusive && evidence.exam_touched {
                refuse(Refusal::TheWorkedExampleIsContaminated)
            } else {
                Permission::Permitted
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rich() -> Evidence {
        Evidence {
            verdict: Verdict::Verified,
            battery_power: BatteryPower::Strong,
            map: MapState::Structural,
            exam_touched: false,
        }
    }

    const EVERY_INTENT: [Intent; 5] = [
        Intent::ExplainWhyTheCodeIsCorrect,
        Intent::PresentPassingChecksAsEvidence,
        Intent::ExplainTheSubsystemStructure,
        Intent::TeachTheChangeAsAWorkedExample,
        Intent::ReportWhatTheRecordSays,
    ];

    #[test]
    fn a_rich_record_permits_everything() {
        for intent in EVERY_INTENT {
            assert_eq!(may(intent, &rich()), Permission::Permitted, "{intent:?}");
        }
    }

    #[test]
    fn unverified_work_gets_no_explanation_of_why_it_is_correct() {
        // Invariant 22: the work was sold without a verification promise.
        // Explaining why it is correct would supply, in prose, the assurance
        // the price explicitly did not include.
        for unproven in [Verdict::Unverified, Verdict::Failed, Verdict::Inconclusive] {
            let evidence = Evidence {
                verdict: unproven,
                ..rich()
            };
            assert_eq!(
                may(Intent::ExplainWhyTheCodeIsCorrect, &evidence),
                Permission::Refused {
                    because: Refusal::NothingEstablishedCorrectness
                },
                "{unproven:?}"
            );
        }
    }

    #[test]
    fn a_battery_that_cannot_discriminate_is_not_offered_as_evidence() {
        // Invariant 25. The passing checks are true and they are not about
        // correctness, and a teaching artifact presenting them as such teaches
        // the reader to trust a green tick that means nothing.
        for powerless in [BatteryPower::None, BatteryPower::Weak] {
            let evidence = Evidence {
                battery_power: powerless,
                ..rich()
            };
            assert_eq!(
                may(Intent::PresentPassingChecksAsEvidence, &evidence),
                Permission::Refused {
                    because: Refusal::ThoseChecksDiscriminateNothing
                },
                "{powerless:?}"
            );
        }
    }

    #[test]
    fn a_blind_run_does_not_explain_the_subsystem() {
        // Invariant 28. Cortex worked here without a map, so it has no more
        // standing to describe the structure than the person reading does --
        // and it would be describing it with the authority of a system that
        // just delivered a change.
        for blind in [MapState::Degraded, MapState::Absent] {
            let evidence = Evidence {
                map: blind,
                ..rich()
            };
            assert_eq!(
                may(Intent::ExplainTheSubsystemStructure, &evidence),
                Permission::Refused {
                    because: Refusal::CortexDidNotHaveTheMapEither
                },
                "{blind:?}"
            );
        }
    }

    #[test]
    fn a_contaminated_change_is_not_a_worked_example() {
        // An inconclusive verdict on a diff that touched the exam. The change
        // rewrote the thing that was supposed to grade it, so teaching from it
        // teaches the failure mode.
        let evidence = Evidence {
            verdict: Verdict::Inconclusive,
            exam_touched: true,
            ..rich()
        };
        assert_eq!(
            may(Intent::TeachTheChangeAsAWorkedExample, &evidence),
            Permission::Refused {
                because: Refusal::TheWorkedExampleIsContaminated
            }
        );
    }

    #[test]
    fn a_failed_change_is_still_worth_teaching_from() {
        // Deliberately not in the table. What went wrong and why is good
        // material, and it is exactly the shape a retrieval item takes. Only
        // contamination bars the worked example, because only there is the
        // example itself untrustworthy.
        let evidence = Evidence {
            verdict: Verdict::Failed,
            ..rich()
        };
        assert_eq!(
            may(Intent::TeachTheChangeAsAWorkedExample, &evidence),
            Permission::Permitted
        );
    }

    #[test]
    fn an_inconclusive_verdict_that_left_the_exam_alone_may_still_be_taught() {
        // Both halves of the row are required. An inconclusive run that never
        // touched the exam is a run that could not execute something, not a run
        // whose example is contaminated.
        let evidence = Evidence {
            verdict: Verdict::Inconclusive,
            exam_touched: false,
            ..rich()
        };
        assert_eq!(
            may(Intent::TeachTheChangeAsAWorkedExample, &evidence),
            Permission::Permitted
        );
    }

    #[test]
    fn reporting_the_record_survives_every_thin_state() {
        // The escape valve, and the reason this is a scope rule rather than a
        // mute button. Silence leaves someone uninformed; "the battery could
        // not settle this" leaves them correctly informed about a thin record.
        let worst = Evidence {
            verdict: Verdict::Inconclusive,
            battery_power: BatteryPower::None,
            map: MapState::Absent,
            exam_touched: true,
        };
        assert_eq!(
            may(Intent::ReportWhatTheRecordSays, &worst),
            Permission::Permitted
        );
    }

    #[test]
    fn the_worst_record_refuses_every_assertion_and_each_for_its_own_reason() {
        // Four different refusals rather than one confidence score. "We are
        // less sure" is not the same statement as "the checks that passed
        // cannot tell right from wrong", and a reader needs to know which.
        let worst = Evidence {
            verdict: Verdict::Inconclusive,
            battery_power: BatteryPower::None,
            map: MapState::Absent,
            exam_touched: true,
        };
        let reasons: Vec<Permission> = EVERY_INTENT.iter().map(|i| may(*i, &worst)).collect();
        assert_eq!(
            reasons[0],
            Permission::Refused {
                because: Refusal::NothingEstablishedCorrectness
            }
        );
        assert_eq!(
            reasons[1],
            Permission::Refused {
                because: Refusal::ThoseChecksDiscriminateNothing
            }
        );
        assert_eq!(
            reasons[2],
            Permission::Refused {
                because: Refusal::CortexDidNotHaveTheMapEither
            }
        );
        assert_eq!(
            reasons[3],
            Permission::Refused {
                because: Refusal::TheWorkedExampleIsContaminated
            }
        );
        assert_eq!(reasons[4], Permission::Permitted);
    }
}
