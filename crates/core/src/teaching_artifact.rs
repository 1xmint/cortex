//! Derived from the decision record, never narrated by a model.
//!
//! Phase 35. Every phase before it optimised a delegation loop, and delegation
//! is the shape measured to degrade the developer using it: a randomised trial
//! of 52 professional engineers found the assisted arm scoring **50% against
//! 67%** on concepts they had used minutes earlier, `d = 0.738`, `p = 0.01`,
//! with debugging the widest gap. So the teaching layer is not an upsell
//! attached to Cortex — it is remediation of a harm Cortex plausibly causes.
//!
//! > The explanation is **derived from the decision record, never inferred by a
//! > model watching another model.** Cortex already holds the `TaskFrame`, the
//! > routing decision and its reasons, the plan DAG, the assumptions and the
//! > checks that settled them, why each check was derived, the verdict and its
//! > class, the battery power, and the race agreement. A second model narrating
//! > a first model's stream is a confabulation engine aimed at someone who
//! > cannot tell when it is wrong. Cortex does not have to guess, so it must
//! > not.
//!
//! # Why narration is worse than silence
//!
//! Not because models confabulate — because of **who this is for**. A narration
//! layer's errors are invisible precisely to the audience it exists to serve. A
//! user who can catch the narrator's mistake did not need the narrator; a user
//! who cannot is being taught something false with the full authority of the
//! system that just did the work, and carries it into the next task and into
//! their judgement of Cortex's later output.
//!
//! Silence leaves someone uninformed. A confident wrong explanation leaves them
//! miscalibrated, which is the failure this whole document is organised against.
//!
//! That is why [`Claim`] has no constructor that does not name a
//! [`RecordField`]. There is no narration variant to disable later.
//!
//! # No artifact above its evidence
//!
//! A claim inherits the verdict of the record it came from. An explanation built
//! on an `Unverified` step may be *reported* — "the battery could not settle
//! this" — and may not be *asserted*. And a claim not anchored to the diff in
//! front of the user does not render at all: a generic mapping card is
//! unfalsifiable against anything Cortex executed, renders identically for every
//! user, and is therefore narration with a table's formatting.
//!
//! # Why a retrieval item asks *why*
//!
//! Item-response theory: recognition-shaped items — did this pass, is this
//! correct — have near-zero discrimination, separating nobody from nobody, while
//! items requiring the causal account discriminate strongly. It is also why
//! Rustlings' make-it-compile format is the low-discrimination kind: "make the
//! error go away" is satisfiable without the causal account.
//!
//! Cortex holds the causal account, so [`RetrievalItem`] demands its three parts
//! and cannot be built without them. An item that cannot say what failed, what
//! it printed, and what fixed it is an item that was going to ask *whether*.

use serde::{Deserialize, Serialize};

use crate::verification::Verdict;

/// A field of the decision record Cortex already holds.
///
/// Closed. A teaching claim about something not in this list has nothing to cite
/// — which is the point, and the reason there is no `Other { .. }`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordField {
    TaskFrame,
    RoutingDecision,
    PlanDag,
    Assumption,
    CheckDerivation,
    CheckOutput,
    Verdict,
    BatteryPower,
    RaceAgreement,
    ResolvingChange,
}

/// How strongly a claim is put.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strength {
    /// Stated as established. Requires a verified source.
    Asserted,
    /// Stated as what the record says, including that it could not settle
    /// something. Always available.
    Reported,
}

/// One thing a teaching artifact says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    pub text: String,
    /// Where in the record this came from. Required.
    pub source: RecordField,
    /// The verdict of the step this record belongs to.
    pub inherits: Verdict,
    pub strength: Strength,
    /// The diff hunk, path, or symbol in front of the user. `None` is a generic
    /// card, which does not render.
    pub anchor: Option<String>,
}

impl Claim {
    /// Build a claim from a record field, anchored to the change in front of the
    /// user.
    pub fn derived(
        text: impl Into<String>,
        source: RecordField,
        inherits: Verdict,
        strength: Strength,
        anchor: impl Into<String>,
    ) -> Self {
        Self {
            text: text.into(),
            source,
            inherits,
            strength,
            anchor: Some(anchor.into()),
        }
    }

    /// A claim with no anchor — a generic card. Constructible so the refusal is
    /// testable, and refused by [`render`].
    pub fn generic(
        text: impl Into<String>,
        source: RecordField,
        inherits: Verdict,
        strength: Strength,
    ) -> Self {
        Self {
            text: text.into(),
            source,
            inherits,
            strength,
            anchor: None,
        }
    }
}

/// Why a claim does not render.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Withheld {
    /// Not anchored to the diff in front of the user. Unfalsifiable against
    /// anything Cortex executed.
    NotAnchoredToThisChange,
    /// Asserted as established on a step nothing established.
    AboveItsEvidence,
}

/// Whether a claim may be shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Render {
    Show,
    Withhold { because: Withheld },
}

/// Apply the two rendering rules.
///
/// Anchoring is checked first: an unanchored claim fails whatever its evidence,
/// because it is not about this change at all.
pub fn render(claim: &Claim) -> Render {
    if claim.anchor.is_none() {
        return Render::Withhold {
            because: Withheld::NotAnchoredToThisChange,
        };
    }
    if claim.strength == Strength::Asserted && claim.inherits != Verdict::Verified {
        return Render::Withhold {
            because: Withheld::AboveItsEvidence,
        };
    }
    Render::Show
}

/// A spaced-retrieval question over the user's own record.
///
/// The three fields are the causal account, and they are required. An item that
/// cannot say what failed, what it printed, and what fixed it is an item that
/// was going to ask *whether* — and recognition-shaped items separate nobody
/// from nobody.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetrievalItem {
    /// Always a "why". The shape is guaranteed by the fields, not by the text.
    pub question: String,
    /// The check that failed, from this user's own history.
    pub check_id: String,
    /// What it printed. Without this there is no account to ask for.
    pub failing_output: String,
    /// The change that resolved it — the answer to grade against.
    pub resolving_change: String,
}

impl RetrievalItem {
    /// `None` when any part of the causal account is missing.
    ///
    /// Refusing is right: an item built on two thirds of an account can only ask
    /// a recognition question, and a study surface full of those is a surface
    /// that measures nothing while feeling like progress.
    pub fn asking_why(
        question: impl Into<String>,
        check_id: impl Into<String>,
        failing_output: impl Into<String>,
        resolving_change: impl Into<String>,
    ) -> Option<Self> {
        let item = Self {
            question: question.into(),
            check_id: check_id.into(),
            failing_output: failing_output.into(),
            resolving_change: resolving_change.into(),
        };
        if item.check_id.trim().is_empty()
            || item.failing_output.trim().is_empty()
            || item.resolving_change.trim().is_empty()
        {
            return None;
        }
        Some(item)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verified_claim() -> Claim {
        Claim::derived(
            "this check was derived from Cargo.toml, so it runs on every Rust change here",
            RecordField::CheckDerivation,
            Verdict::Verified,
            Strength::Asserted,
            "crates/api/src/db/ledger.rs:68",
        )
    }

    #[test]
    fn a_claim_anchored_to_this_change_and_backed_by_a_verdict_renders() {
        assert_eq!(render(&verified_claim()), Render::Show);
    }

    #[test]
    fn a_generic_card_does_not_render() {
        // Unfalsifiable against anything Cortex executed: it derives from no
        // record field about *this* work, renders identically for every user,
        // and is therefore narration with a table's formatting.
        let generic = Claim::generic(
            "in Rust, `?` propagates errors",
            RecordField::CheckOutput,
            Verdict::Verified,
            Strength::Asserted,
        );
        assert_eq!(
            render(&generic),
            Render::Withhold {
                because: Withheld::NotAnchoredToThisChange
            }
        );
    }

    #[test]
    fn nothing_is_asserted_on_a_step_that_established_nothing() {
        // No artifact above its evidence. An explanation built on an
        // unverified step is being given the authority of a verified one.
        for weak in [Verdict::Unverified, Verdict::Inconclusive, Verdict::Failed] {
            let claim = Claim {
                inherits: weak,
                ..verified_claim()
            };
            assert_eq!(
                render(&claim),
                Render::Withhold {
                    because: Withheld::AboveItsEvidence
                },
                "{weak:?} must not carry an asserted claim"
            );
        }
    }

    #[test]
    fn the_same_claim_reported_rather_than_asserted_does_render() {
        // Silence leaves someone uninformed, which is bad. The fix is to say
        // what the record says -- including that it could not settle something
        // -- rather than to say nothing or to overstate.
        for weak in [Verdict::Unverified, Verdict::Inconclusive, Verdict::Failed] {
            let claim = Claim {
                inherits: weak,
                strength: Strength::Reported,
                ..verified_claim()
            };
            assert_eq!(
                render(&claim),
                Render::Show,
                "{weak:?} may still be reported"
            );
        }
    }

    #[test]
    fn anchoring_is_checked_before_evidence() {
        // An unanchored claim is not about this change at all, so its evidence
        // is beside the point and naming the weaker fault would send someone to
        // fix the wrong thing.
        let neither = Claim::generic(
            "generic and unsupported",
            RecordField::Verdict,
            Verdict::Inconclusive,
            Strength::Asserted,
        );
        assert_eq!(
            render(&neither),
            Render::Withhold {
                because: Withheld::NotAnchoredToThisChange
            }
        );
    }

    #[test]
    fn every_claim_names_a_field_of_the_record() {
        // The structural half of 35.3: there is no constructor that omits the
        // source, so a claim about something Cortex does not hold has nothing
        // to cite. Narration is not disabled -- it is unrepresentable.
        let claim = verified_claim();
        assert_eq!(claim.source, RecordField::CheckDerivation);
        let json = serde_json::to_value(&claim).unwrap();
        assert_eq!(json["source"], "check_derivation");
    }

    #[test]
    fn a_retrieval_item_needs_the_whole_causal_account() {
        // Recognition-shaped items have near-zero discrimination -- they
        // separate nobody from nobody. Cortex holds the causal account, so an
        // item that cannot cite all of it was going to ask "whether".
        assert!(RetrievalItem::asking_why(
            "why did the ledger test fail on your payments change?",
            "cargo:test",
            "assertion failed: pack_remaining == 50",
            "refund now mirrors the bucket split",
        )
        .is_some());

        assert!(RetrievalItem::asking_why("why?", "", "output", "change").is_none());
        assert!(RetrievalItem::asking_why("why?", "check", "", "change").is_none());
        assert!(RetrievalItem::asking_why("why?", "check", "output", "").is_none());
        assert!(RetrievalItem::asking_why("why?", "check", "output", "   ").is_none());
    }

    #[test]
    fn a_retrieval_item_carries_what_to_grade_against() {
        // The resolving change is the answer. Without it the item can be asked
        // and not marked, which is a quiz rather than a measurement.
        let item = RetrievalItem::asking_why(
            "why did this fail?",
            "cargo:clippy",
            "unused variable: `cost_estimate`",
            "renamed under a cfg_attr rather than deleted",
        )
        .expect("complete account");
        assert!(!item.resolving_change.is_empty());
        assert!(!item.failing_output.is_empty());
    }
}
