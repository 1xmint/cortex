//! What a task *is*, for pricing and for measurement.
//!
//! # Why this is its own type
//!
//! Invariant 11 says one fact, one table, and no pricing code carrying a
//! hardcoded model name. A price list therefore cannot be keyed by model — it
//! has to be keyed by something about the *work*, so that changing which model
//! runs a class does not change what the class costs a customer. That is also
//! the only version of outcome pricing that means anything: a customer buys a
//! verified outcome, not a token count.
//!
//! Phase 31.3's graduation gate needs the same key. A class moves from
//! `provisional` to `committed` once its measured pass rate and cost
//! distribution clear a threshold, and "its" is only well defined if the class
//! is a stable identifier rather than a description.
//!
//! # Why these three axes and not more
//!
//! `work_kind` is what is being done, `risk` is how much is at stake, and
//! `verifiable` is whether an executable ground truth exists. Every one of them
//! changes the expected cost *and* the expected pass rate, which is what a price
//! has to be a function of.
//!
//! Tier is deliberately **not** an axis, even though it is available. Tier is a
//! routing decision — which class of model runs this — and folding it into the
//! price key would make the price depend on Cortex's own choice of model. That
//! is the failure invariant 11 exists to prevent, one level up: a router that
//! can change the price by changing the route can raise the price by routing
//! badly.
//!
//! More axes are not free either. Each one multiplies the number of classes and
//! divides the evidence available per class, and a class with three samples
//! cannot graduate. Three axes is 88 classes; every additional one is a reason
//! more of them stay `provisional` forever.

use serde::{Deserialize, Serialize};

use crate::routing::RiskLevel;
use crate::task::WorkKind;

/// A stable pricing and measurement key for a unit of work.
///
/// Ordered fields, stable rendering, and no free text: this string ends up in a
/// price list, on a receipt, and in a graduation decision, and all three have to
/// agree about what they are naming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TaskClass {
    pub work_kind: WorkKind,
    pub risk: RiskLevel,
    /// Whether an executable ground truth exists for this work.
    ///
    /// Not a detail. Invariant 22 says work with no executable ground truth is
    /// labelled `UNVERIFIED` and *never priced as though it had been proven*, so
    /// unverifiable work is a different class rather than the same class with a
    /// caveat. Keeping it in the key is what makes that invariant expressible as
    /// a price rather than as a footnote.
    pub verifiable: bool,
}

impl TaskClass {
    pub fn new(work_kind: WorkKind, risk: RiskLevel, verifiable: bool) -> Self {
        Self {
            work_kind,
            risk,
            verifiable,
        }
    }

    /// The canonical key. Lowercase, colon-separated, no spaces.
    ///
    /// This is what a `price_list_task_classes` row is keyed by and what a
    /// receipt names, so it is a wire format: changing it invalidates published
    /// price lists, which under invariant 23 cannot be edited to match.
    pub fn key(&self) -> String {
        format!(
            "{}:{}:{}",
            work_kind_key(self.work_kind),
            risk_key(self.risk),
            if self.verifiable {
                "verifiable"
            } else {
                "unverifiable"
            }
        )
    }

    /// Every class that exists, in a stable order.
    ///
    /// Used to seed a price list: a list that covers some classes and not others
    /// produces a quote for some work and silence for the rest, and silence
    /// here is indistinguishable from "we chose not to price this".
    pub fn all() -> Vec<TaskClass> {
        let mut out = Vec::new();
        for work_kind in WORK_KINDS {
            for risk in RISKS {
                for verifiable in [true, false] {
                    out.push(TaskClass::new(*work_kind, *risk, verifiable));
                }
            }
        }
        out
    }
}

impl std::fmt::Display for TaskClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.key())
    }
}

const WORK_KINDS: &[WorkKind] = &[
    WorkKind::Explore,
    WorkKind::Modify,
    WorkKind::Add,
    WorkKind::Refactor,
    WorkKind::Test,
    WorkKind::Build,
    WorkKind::Lint,
    WorkKind::Review,
    WorkKind::Ship,
    WorkKind::Heal,
    WorkKind::Gate,
];

const RISKS: &[RiskLevel] = &[
    RiskLevel::Low,
    RiskLevel::Medium,
    RiskLevel::High,
    RiskLevel::Critical,
];

/// Spelled out rather than derived from `Debug`.
///
/// `Debug` output is not a stable format — it changes when somebody renames a
/// variant, and a rename would silently repoint every published price list at
/// classes that no longer exist. `WorkKind::as_str` is already an explicit
/// match and is reused rather than restated, so the wire name has one
/// definition; this function exists only to give the risk half a matching one.
fn work_kind_key(kind: WorkKind) -> &'static str {
    kind.as_str()
}

fn risk_key(risk: RiskLevel) -> &'static str {
    match risk {
        RiskLevel::Low => "low",
        RiskLevel::Medium => "medium",
        RiskLevel::High => "high",
        RiskLevel::Critical => "critical",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_is_stable_and_readable() {
        let class = TaskClass::new(WorkKind::Modify, RiskLevel::High, true);
        assert_eq!(class.key(), "modify:high:verifiable");
        assert_eq!(class.to_string(), "modify:high:verifiable");
    }

    #[test]
    fn verifiable_and_unverifiable_are_different_classes() {
        // Invariant 22: work with no executable ground truth is never priced as
        // though it had been proven. That is only enforceable if the two are
        // separately priceable, which means separately keyed.
        let verifiable = TaskClass::new(WorkKind::Modify, RiskLevel::Low, true);
        let not = TaskClass::new(WorkKind::Modify, RiskLevel::Low, false);
        assert_ne!(verifiable.key(), not.key());
    }

    #[test]
    fn every_class_has_a_distinct_key() {
        // A collision would mean two kinds of work sharing a price and a
        // graduation decision, and the second one to be measured would silently
        // move the first one's status.
        let all = TaskClass::all();
        let mut keys: Vec<String> = all.iter().map(|c| c.key()).collect();
        let total = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), total, "two classes share a key");
        assert_eq!(total, 11 * 4 * 2, "the class space changed size");
    }

    #[test]
    fn the_route_cannot_change_the_class() {
        // Tier is not an axis, deliberately. If it were, a router that chose a
        // more expensive model would raise the customer's price by doing so —
        // which is invariant 11's failure one level up, and invariant 12's
        // (optimise cost-to-verified-outcome) made unenforceable.
        //
        // Expressed as a property of the constructor's signature: there is no
        // way to pass a tier or a model in.
        let class = TaskClass::new(WorkKind::Modify, RiskLevel::Medium, true);
        assert!(
            !class.key().contains("sonnet")
                && !class.key().contains("opus")
                && !class.key().contains("execute"),
            "the class key names a routing decision: {}",
            class.key()
        );
    }
}
