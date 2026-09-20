//! Ask about intent, never about mechanism.
//!
//! Phase 33.1. The tempting reading of "simple for a vibe coder, complete for a
//! 200-person org" is a simple mode and an advanced mode, which is how a product
//! gets built twice and diverges. The correct structure is one object graph
//! where an organisation is a defaults-and-limits authority over it.
//!
//! But progressive disclosure alone does not solve the solo end:
//!
//! > The solo builder's problem is not that there are too many controls; it is
//! > that **they cannot evaluate a plan.** Hiding the dial does not help someone
//! > who could not have set it.
//!
//! What helps is Cortex pre-deciding more and saying what it decided in one line
//! each — the same Plan Receipt, with a different ratio of decided to asked.
//!
//! # The rule that lets one intake serve both ends
//!
//! That collides with invariant 13, which says low confidence produces a
//! question, because for a solo builder **a stream of clarifying questions is
//! itself the failure mode.** The resolution:
//!
//! > **Ask about intent, never about mechanism.** "Should this replace the
//! > existing login, or sit alongside it?" is always worth asking of anyone.
//! > "Should this use a bandit or a fixed policy?" is never worth asking of
//! > someone who did not bring it up. Mechanism questions are answered by
//! > Cortex, recorded as decisions on the receipt, and made visible for anyone
//! > who wants to change them.
//!
//! [`disposition`] is that rule. A mechanism question has no path to being asked
//! unless the customer raised the subject themselves — and when it is decided
//! instead, the decision is a required field, because a mechanism question
//! silently dropped is worse than one asked.

use serde::{Deserialize, Serialize};

/// What a question is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subject {
    /// What the customer wants. Only they can answer it.
    Intent,
    /// How Cortex will do it. Cortex can answer it, and answering it is what
    /// they are paying for.
    Mechanism,
}

/// Something the planner is uncertain about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub text: String,
    pub subject: Subject,
    /// The customer raised this themselves. A mechanism question they asked is
    /// a conversation they started, and refusing to have it would be worse than
    /// the stream of questions this rule exists to prevent.
    pub raised_by_customer: bool,
}

/// What happens to a question.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Disposition {
    /// Put to the customer.
    Ask,
    /// Answered by Cortex and recorded on the Plan Receipt, visible to anyone
    /// who wants to change it.
    ///
    /// The decision is a required field. A mechanism question that is neither
    /// asked nor recorded is a choice nobody made and nobody can find.
    DecideAndRecord { decision: String },
}

/// Apply the intake rule to one question.
///
/// `decision` is what Cortex would decide if it decides. It is taken up front so
/// that there is no way to route a question to `DecideAndRecord` without having
/// the answer in hand.
pub fn disposition(question: &Question, decision: impl Into<String>) -> Disposition {
    match question.subject {
        Subject::Intent => Disposition::Ask,
        Subject::Mechanism if question.raised_by_customer => Disposition::Ask,
        Subject::Mechanism => Disposition::DecideAndRecord {
            decision: decision.into(),
        },
    }
}

/// How much of a plan Cortex settled versus put back to the customer.
///
/// The plan calls this the difference between the two ends of the range: the
/// same Plan Receipt with a different ratio of decided to asked. `None` when
/// there was nothing to settle either way.
pub fn decided_share(dispositions: &[Disposition]) -> Option<f64> {
    if dispositions.is_empty() {
        return None;
    }
    let decided = dispositions
        .iter()
        .filter(|d| matches!(d, Disposition::DecideAndRecord { .. }))
        .count();
    Some(decided as f64 / dispositions.len() as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent(text: &str) -> Question {
        Question {
            text: text.to_string(),
            subject: Subject::Intent,
            raised_by_customer: false,
        }
    }

    fn mechanism(text: &str) -> Question {
        Question {
            text: text.to_string(),
            subject: Subject::Mechanism,
            raised_by_customer: false,
        }
    }

    #[test]
    fn an_intent_question_is_always_asked() {
        // "Should this replace the existing login, or sit alongside it?" is
        // worth asking of anyone, at any level of expertise, because only they
        // know the answer.
        let q = intent("should this replace the existing login, or sit alongside it?");
        assert_eq!(disposition(&q, "replace it"), Disposition::Ask);
    }

    #[test]
    fn a_mechanism_question_is_decided_rather_than_asked() {
        // "Should this use a bandit or a fixed policy?" is never worth asking of
        // someone who did not bring it up. For a solo builder a stream of these
        // is the failure mode, and hiding the dial does not help someone who
        // could not have set it.
        let q = mechanism("should the router use a bandit or a fixed policy?");
        assert_eq!(
            disposition(&q, "fixed policy: too few samples for a bandit"),
            Disposition::DecideAndRecord {
                decision: "fixed policy: too few samples for a bandit".to_string()
            }
        );
    }

    #[test]
    fn a_mechanism_question_the_customer_raised_is_a_conversation_they_started() {
        // Refusing to discuss something they asked about would be worse than
        // the stream of questions this rule prevents.
        let q = Question {
            raised_by_customer: true,
            ..mechanism("should this use a bandit?")
        };
        assert_eq!(disposition(&q, "fixed policy"), Disposition::Ask);
    }

    #[test]
    fn a_decided_question_always_carries_its_decision() {
        // A mechanism question that is neither asked nor recorded is a choice
        // nobody made and nobody can find. The decision is taken up front, so
        // there is no path to deciding without having the answer.
        let q = mechanism("which runner image?");
        let Disposition::DecideAndRecord { decision } = disposition(&q, "the pinned default")
        else {
            panic!("expected a recorded decision")
        };
        assert!(!decision.is_empty());
    }

    #[test]
    fn intent_is_asked_whether_or_not_the_customer_raised_it() {
        // The `raised_by_customer` flag only ever widens what gets asked. It
        // cannot suppress an intent question.
        for raised in [true, false] {
            let q = Question {
                raised_by_customer: raised,
                ..intent("replace or extend?")
            };
            assert_eq!(disposition(&q, "extend"), Disposition::Ask);
        }
    }

    #[test]
    fn a_solo_plan_settles_most_of_itself() {
        // The same Plan Receipt, with a different ratio of decided to asked.
        // Four mechanism questions and one intent question is one question in
        // front of someone who cannot evaluate a plan, rather than five.
        let questions = [
            intent("replace the existing login, or sit alongside it?"),
            mechanism("which test runner?"),
            mechanism("bandit or fixed policy?"),
            mechanism("one commit or a stack?"),
            mechanism("which effort level?"),
        ];
        let dispositions: Vec<Disposition> = questions
            .iter()
            .map(|q| disposition(q, "decided"))
            .collect();
        assert_eq!(decided_share(&dispositions), Some(0.8));
        assert_eq!(
            dispositions
                .iter()
                .filter(|d| **d == Disposition::Ask)
                .count(),
            1
        );
    }

    #[test]
    fn a_plan_of_pure_intent_questions_decides_nothing() {
        let dispositions = [Disposition::Ask, Disposition::Ask];
        assert_eq!(decided_share(&dispositions), Some(0.0));
    }

    #[test]
    fn a_plan_with_no_uncertainty_has_no_ratio() {
        // Nothing was settled and nothing was asked, so a ratio would be a
        // statement about an empty set.
        assert_eq!(decided_share(&[]), None);
    }
}
