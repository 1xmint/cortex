//! Every mechanism declares who has to read it.
//!
//! Phase 33.5, which turns the plan's own discipline on itself. This round added
//! verdict classes, battery power, race agreement, comprehension declarations,
//! loss ratios, and recall procedures. Every one is justified. **Every one is
//! also vocabulary, and vocabulary is exactly what leaks into a surface a solo
//! builder was supposed to find simple.**
//!
//! | Tier | Who sees it |
//! |---|---|
//! | Always | everyone, in plain language |
//! | On request | anyone who opens the detail |
//! | Operator | Cortex operations only |
//! | Org admin | whoever sets policy |
//!
//! Two rules, and they are the whole module.
//!
//! **An undeclared mechanism defaults to `Operator`** — see [`Tier::DEFAULT`].
//! Defaulting to `Always` would let every new mechanism arrive on the customer's
//! screen by omission, which is precisely the failure this catches: shipping a
//! correct product that a solo builder finds unreadable.
//!
//! **Nothing above `Always` may be required reading to complete a task.** A
//! customer who has to understand `battery power` before they can finish is a
//! customer for whom the detail tier has stopped being optional.
//! [`required_reading_violations`] is that check, and it is meant to be run over
//! whatever a real completion path actually demands.

use serde::{Deserialize, Serialize};

/// Who a mechanism is written for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Everyone, in plain language. "Cortex could not verify this."
    Always,
    /// Anyone who opens the detail.
    OnRequest,
    /// Whoever sets policy.
    OrgAdmin,
    /// Cortex operations only.
    Operator,
}

impl Tier {
    /// What a mechanism with no declared tier gets.
    ///
    /// `Operator`, deliberately. A default of `Always` would put every new
    /// mechanism in front of a customer by omission rather than by decision,
    /// and the omission is much more likely than the decision.
    pub const DEFAULT: Tier = Tier::Operator;

    /// Whether understanding this may be required to complete a task.
    pub fn may_be_required_reading(&self) -> bool {
        matches!(self, Self::Always)
    }
}

/// A named mechanism and the tier it declared.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mechanism {
    pub name: String,
    tier: Tier,
}

impl Mechanism {
    /// Declare a mechanism's tier.
    pub fn declared(name: impl Into<String>, tier: Tier) -> Self {
        Self {
            name: name.into(),
            tier,
        }
    }

    /// A mechanism that declared nothing.
    ///
    /// Exists so the default is reachable and testable rather than implicit in
    /// a deserialiser somewhere.
    pub fn undeclared(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tier: Tier::DEFAULT,
        }
    }

    pub fn tier(&self) -> Tier {
        self.tier
    }
}

/// Mechanisms a task cannot be completed without understanding, that are not
/// `Always`.
///
/// Invariant 31: nothing above the `always` tier may be required reading to
/// complete a task. Anything this returns is a solo builder stuck on vocabulary.
pub fn required_reading_violations(required_to_complete: &[Mechanism]) -> Vec<&Mechanism> {
    required_to_complete
        .iter()
        .filter(|m| !m.tier().may_be_required_reading())
        .collect()
}

/// The tiers this round's mechanisms declare.
///
/// Here rather than scattered across modules so the reflexive check is a thing
/// somebody can read in one place — the plan's requirement is that a tier is
/// declared *in the same commit that introduces the mechanism*, and a list
/// nobody can see is not a declaration.
pub fn declared_mechanisms() -> Vec<Mechanism> {
    vec![
        // Always: plain language, no vocabulary.
        Mechanism::declared("could not verify this work", Tier::Always),
        Mechanism::declared("these checks cannot fail", Tier::Always),
        Mechanism::declared("working without a symbol map here", Tier::Always),
        // On request: the detail behind a verdict.
        Mechanism::declared("verdict class", Tier::OnRequest),
        Mechanism::declared("battery power", Tier::OnRequest),
        Mechanism::declared("race diversity class", Tier::OnRequest),
        Mechanism::declared("comprehension state", Tier::OnRequest),
        Mechanism::declared("context recall and precision", Tier::OnRequest),
        Mechanism::declared("dependency gate findings", Tier::OnRequest),
        // Operator: Cortex's own book and instruments.
        Mechanism::declared("loss ratio", Tier::Operator),
        Mechanism::declared("false-accept rate", Tier::Operator),
        Mechanism::declared("breaker thresholds", Tier::Operator),
        Mechanism::declared("recall queries", Tier::Operator),
        Mechanism::declared("check-result cache hit rate", Tier::Operator),
        Mechanism::declared("reviewer trust calibration", Tier::Operator),
        // Org admin: whoever sets policy.
        Mechanism::declared("effort ceiling", Tier::OrgAdmin),
        Mechanism::declared("work-in-progress limit", Tier::OrgAdmin),
        Mechanism::declared("dependency policy", Tier::OrgAdmin),
        Mechanism::declared("review policy", Tier::OrgAdmin),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_undeclared_mechanism_is_not_shown_to_the_customer() {
        // A default of `Always` would put every new mechanism in front of a
        // customer by omission rather than by decision -- and omission is much
        // more likely than decision.
        let m = Mechanism::undeclared("some new score");
        assert_eq!(m.tier(), Tier::Operator);
        assert_eq!(Tier::DEFAULT, Tier::Operator);
        assert!(!m.tier().may_be_required_reading());
    }

    #[test]
    fn only_the_always_tier_may_be_required_reading() {
        // Invariant 31. A customer who must understand "battery power" before
        // they can finish is a customer for whom the detail tier has stopped
        // being optional.
        assert!(Tier::Always.may_be_required_reading());
        for hidden in [Tier::OnRequest, Tier::OrgAdmin, Tier::Operator] {
            assert!(
                !hidden.may_be_required_reading(),
                "{hidden:?} must not be required to finish a task"
            );
        }
    }

    #[test]
    fn a_completion_path_demanding_vocabulary_is_a_violation() {
        let path = [
            Mechanism::declared("could not verify this work", Tier::Always),
            Mechanism::declared("battery power", Tier::OnRequest),
            Mechanism::declared("loss ratio", Tier::Operator),
        ];
        let violations = required_reading_violations(&path);
        assert_eq!(violations.len(), 2);
        assert_eq!(violations[0].name, "battery power");
        assert_eq!(violations[1].name, "loss ratio");
    }

    #[test]
    fn a_plain_language_completion_path_is_clean() {
        // The Phase 33 exit gate: a solo builder completes a task end to end
        // without encountering a control they must understand.
        let path = [
            Mechanism::declared("could not verify this work", Tier::Always),
            Mechanism::declared("these checks cannot fail", Tier::Always),
        ];
        assert!(required_reading_violations(&path).is_empty());
    }

    #[test]
    fn every_mechanism_this_round_added_declares_a_tier() {
        // The reflexive requirement: a tier is declared in the same commit that
        // introduces the mechanism, and a list nobody can see is not a
        // declaration.
        let declared = declared_mechanisms();
        assert!(declared.len() >= 15);
        for m in &declared {
            assert!(!m.name.is_empty());
        }
    }

    #[test]
    fn the_operator_instruments_are_not_customer_vocabulary() {
        // Loss ratio, false-accept rate, breaker thresholds and recall queries
        // are Cortex's own book. A customer reading them is reading our
        // problems, not theirs.
        let declared = declared_mechanisms();
        for name in [
            "loss ratio",
            "false-accept rate",
            "breaker thresholds",
            "recall queries",
        ] {
            let m = declared
                .iter()
                .find(|m| m.name == name)
                .unwrap_or_else(|| panic!("{name} must declare a tier"));
            assert_eq!(m.tier(), Tier::Operator, "{name}");
        }
    }

    #[test]
    fn the_verdict_detail_is_available_without_being_imposed() {
        // On request is the tier that lets a verdict be explainable without
        // making the explanation a prerequisite.
        let declared = declared_mechanisms();
        for name in ["verdict class", "battery power", "comprehension state"] {
            let m = declared.iter().find(|m| m.name == name).unwrap();
            assert_eq!(m.tier(), Tier::OnRequest, "{name}");
            assert!(!m.tier().may_be_required_reading());
        }
    }

    #[test]
    fn what_a_customer_always_sees_is_written_in_plain_language() {
        // No vocabulary from this round appears at the `Always` tier -- these
        // are sentences, not terms.
        let declared = declared_mechanisms();
        let always: Vec<&Mechanism> = declared
            .iter()
            .filter(|m| m.tier() == Tier::Always)
            .collect();
        assert!(!always.is_empty());
        for m in always {
            assert!(
                m.name.contains(' '),
                "{} reads as a term rather than a sentence",
                m.name
            );
        }
    }

    #[test]
    fn nothing_required_is_no_violation() {
        assert!(required_reading_violations(&[]).is_empty());
    }
}
