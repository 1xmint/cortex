//! What a teaching artifact may assert, and the five sentences it never may.
//!
//! Phase 35.4. The plan gives two tables and then says which one matters:
//!
//! > The second table matters more than the first. **Almost every failure of a
//! > teaching feature is a sentence from the right-hand column delivered in the
//! > voice of the left.**
//!
//! The right-hand column is the forbidden list, and the "voice of the left" is
//! the calm, sourced, record-backed tone the permitted assertions have earned.
//! That combination — an unknowable claim in a trustworthy voice — is the whole
//! failure mode, and it is why this is a closed enum rather than a style guide.
//!
//! # The rule underneath both tables
//!
//! An assertion is permitted **exactly when the record holds a field that
//! supports it.** [`Assertion::source`] returns the field for a permitted
//! assertion and `None` for a forbidden one, and the forbidden ones are
//! forbidden *because* there is nothing to return — not by a separate rule that
//! could drift out of agreement with the first.
//!
//! # The most tempting sentence in the product
//!
//! "Why the *model* did something." It is not recorded, not knowable, and the
//! thing every reader wants. Chain-of-thought faithfulness work is the
//! measurement of what happens to a system that answers anyway: the stated
//! reason and the operative cause come apart, and nothing in the output marks
//! which one you are reading.
//!
//! Rudin (*Nature Machine Intelligence*, 2019) is the structural version of the
//! same point: a post-hoc explanation of a black box cannot have perfect
//! fidelity to it, because if it did it would *be* the model and there would be
//! no black box left to explain. Any gap between narration and computation is
//! not a defect in the narrator — it is the definition of one. The prescription
//! this phase adopts is Rudin's: build the interpretable object rather than an
//! explainer bolted onto an opaque one. The record *is* the interpretable
//! object, and these eight assertions are what reading it out loud sounds like.

use serde::{Deserialize, Serialize};

/// Something a teaching artifact might say.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Assertion {
    // --- Backed by a field of the decision record ---
    /// What was decided, and what the alternatives were.
    WhatWasDecided,
    /// What was assumed, and how the assumption was settled.
    WhatWasAssumed,
    /// Why each check was in the exam.
    WhyEachCheckWasDerived,
    /// What the verdict means and does not mean.
    WhatTheVerdictMeans,
    /// How much the exam could have caught.
    HowMuchTheExamCouldCatch,
    /// What the step was allowed to reach, and why.
    WhatTheStepCouldReach,
    /// What it cost, against what estimate.
    WhatItCost,
    /// What Cortex looked at and found irrelevant.
    WhatWasFoundIrrelevant,

    // --- Nothing in the record supports these ---
    /// Why the *model* did something.
    WhyTheModelDidSomething,
    /// That the work is correct.
    ThatTheWorkIsCorrect,
    /// A general lesson about the user's codebase.
    AGeneralLessonAboutThisCodebase,
    /// What would have happened under a different decision.
    WhatWouldHaveHappenedOtherwise,
    /// Anything phrased as the model's intent or reasoning.
    TheModelsIntentOrReasoning,
}

/// Why an assertion has no record behind it.
///
/// Kept as text because these are arguments rather than codes: whoever reaches
/// for one of these sentences needs the reason, not an identifier to look up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoRecord {
    pub because: &'static str,
}

impl Assertion {
    /// The record field this assertion reads out, if there is one.
    ///
    /// `None` is the definition of forbidden here. An assertion with no source
    /// is one the record cannot support, and the two facts are the same fact
    /// rather than two rules that could disagree.
    pub fn source(&self) -> Option<&'static str> {
        match self {
            Self::WhatWasDecided => Some("RoutingDecision and its reason set"),
            Self::WhatWasAssumed => {
                Some("the assumption record and the mechanical check that settled it")
            }
            Self::WhyEachCheckWasDerived => Some("CheckSpec::source"),
            Self::WhatTheVerdictMeans => Some("Verdict and VerdictReport counts"),
            Self::HowMuchTheExamCouldCatch => Some("battery power"),
            Self::WhatTheStepCouldReach => Some("CapabilityGrant and its derivation"),
            Self::WhatItCost => Some("the quote and the measured spend"),
            Self::WhatWasFoundIrrelevant => Some("dead_ends"),
            Self::WhyTheModelDidSomething
            | Self::ThatTheWorkIsCorrect
            | Self::AGeneralLessonAboutThisCodebase
            | Self::WhatWouldHaveHappenedOtherwise
            | Self::TheModelsIntentOrReasoning => None,
        }
    }

    /// Why this one can never be said.
    ///
    /// `None` for the permitted assertions, which have a source instead.
    pub fn refusal(&self) -> Option<NoRecord> {
        let because = match self {
            Self::WhyTheModelDidSomething => {
                "not recorded, not knowable, and the most tempting sentence in the product"
            }
            Self::ThatTheWorkIsCorrect => "only that the stated checks passed — invariant 12",
            Self::AGeneralLessonAboutThisCodebase => {
                "one run is not evidence of a convention; convention findings are, \
                 and they carry their own evidence"
            }
            Self::WhatWouldHaveHappenedOtherwise => {
                "counterfactual, unrun, and indistinguishable in tone from the rest"
            }
            Self::TheModelsIntentOrReasoning => "the record holds decisions, not motives",
            _ => return None,
        };
        Some(NoRecord { because })
    }

    pub fn is_permitted(&self) -> bool {
        self.source().is_some()
    }
}

/// Every assertion this module knows about, permitted first.
pub const ALL: &[Assertion] = &[
    Assertion::WhatWasDecided,
    Assertion::WhatWasAssumed,
    Assertion::WhyEachCheckWasDerived,
    Assertion::WhatTheVerdictMeans,
    Assertion::HowMuchTheExamCouldCatch,
    Assertion::WhatTheStepCouldReach,
    Assertion::WhatItCost,
    Assertion::WhatWasFoundIrrelevant,
    Assertion::WhyTheModelDidSomething,
    Assertion::ThatTheWorkIsCorrect,
    Assertion::AGeneralLessonAboutThisCodebase,
    Assertion::WhatWouldHaveHappenedOtherwise,
    Assertion::TheModelsIntentOrReasoning,
];

#[cfg(test)]
mod tests {
    use super::*;

    const FORBIDDEN: [Assertion; 5] = [
        Assertion::WhyTheModelDidSomething,
        Assertion::ThatTheWorkIsCorrect,
        Assertion::AGeneralLessonAboutThisCodebase,
        Assertion::WhatWouldHaveHappenedOtherwise,
        Assertion::TheModelsIntentOrReasoning,
    ];

    #[test]
    fn permitted_and_sourced_are_the_same_property() {
        // The rule underneath both tables. An assertion is permitted exactly
        // when the record holds a field supporting it, so the two facts cannot
        // drift apart into two rules that disagree.
        for assertion in ALL {
            assert_eq!(
                assertion.is_permitted(),
                assertion.source().is_some(),
                "{assertion:?}"
            );
            assert_eq!(
                assertion.is_permitted(),
                assertion.refusal().is_none(),
                "{assertion:?} must have exactly one of a source and a refusal"
            );
        }
    }

    #[test]
    fn the_five_forbidden_sentences_have_nothing_to_cite() {
        // "Almost every failure of a teaching feature is a sentence from the
        // right-hand column delivered in the voice of the left." These are that
        // column.
        for assertion in FORBIDDEN {
            assert!(!assertion.is_permitted(), "{assertion:?}");
            assert!(assertion.source().is_none());
            assert!(assertion.refusal().is_some());
        }
        assert_eq!(
            ALL.iter().filter(|a| !a.is_permitted()).count(),
            FORBIDDEN.len()
        );
    }

    #[test]
    fn the_models_reasoning_is_refused_because_the_record_holds_decisions_not_motives() {
        // The most tempting sentence in the product, and the one a narration
        // layer exists to produce. The record holds what was decided; why a
        // model decided it is not in there and is not knowable from outside.
        let r = Assertion::WhyTheModelDidSomething
            .refusal()
            .expect("refused");
        assert!(r.because.contains("not knowable"));

        let intent = Assertion::TheModelsIntentOrReasoning
            .refusal()
            .expect("refused");
        assert!(intent.because.contains("decisions, not motives"));
    }

    #[test]
    fn correctness_is_never_asserted_only_that_checks_passed() {
        // Invariant 12 in prose form. The gap between "these checks passed" and
        // "this is correct" is the entire product, and a teaching artifact
        // closing it in a sentence gives away what the verdict class exists to
        // protect.
        assert!(!Assertion::ThatTheWorkIsCorrect.is_permitted());
        assert!(Assertion::WhatTheVerdictMeans.is_permitted());
    }

    #[test]
    fn one_run_is_not_a_convention() {
        // The tempting generalisation. A single change is not evidence about
        // how a codebase does things -- 29.3's convention findings are, and
        // they carry their own evidence, which is exactly what makes them
        // different from a sentence written after one run.
        let r = Assertion::AGeneralLessonAboutThisCodebase
            .refusal()
            .expect("refused");
        assert!(r.because.contains("one run is not evidence"));
    }

    #[test]
    fn a_counterfactual_is_refused_for_its_tone_as_much_as_its_truth() {
        // "It would have been faster with X" is unrun, and reads identically to
        // the sentences around it that were derived from something executed.
        // Indistinguishability is the harm, not the speculation.
        let r = Assertion::WhatWouldHaveHappenedOtherwise
            .refusal()
            .expect("refused");
        assert!(r.because.contains("indistinguishable in tone"));
    }

    #[test]
    fn every_permitted_assertion_names_the_field_it_reads() {
        // Not "it comes from the record" -- which field. A source a reader
        // cannot go and look at is a citation in the shape of a claim.
        for assertion in ALL.iter().filter(|a| a.is_permitted()) {
            let source = assertion.source().expect("permitted");
            assert!(!source.is_empty(), "{assertion:?}");
        }
        assert_eq!(
            Assertion::WhyEachCheckWasDerived.source(),
            Some("CheckSpec::source")
        );
    }

    #[test]
    fn the_list_is_complete_and_has_no_duplicates() {
        // `ALL` is what a renderer iterates. An assertion missing from it is
        // one that silently never gets checked.
        let unique: std::collections::BTreeSet<_> = ALL.iter().map(|a| format!("{a:?}")).collect();
        assert_eq!(unique.len(), ALL.len());
        assert_eq!(ALL.len(), 13);
    }
}
