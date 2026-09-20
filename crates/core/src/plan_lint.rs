//! What may fail a plan, and what may only annotate it.
//!
//! The plan says both of these, three thousand lines apart:
//!
//! > Deterministic structural rules first […] then one fresh-context advisory
//! > pass. Annotations on the Plan Receipt, **never a block** — "models propose,
//! > never grade" holds at every effort level.
//!
//! > Record the predicted `Π aᵢ` and `a_whole` on the Plan Receipt at `ultra`,
//! > and let the plan lint **fail** a plan that decomposed itself past its own
//! > crossover.
//!
//! Read as one rule about "the plan lint" those contradict. Read as two rules
//! about two different kinds of finding they do not, and the boundary is the
//! same one invariant 27 draws everywhere else: **a deterministic rule may
//! block; a model's opinion may not.** Over-decomposition is arithmetic over
//! priors the plan already declared — no model is consulted and re-running it
//! gives the same answer — so it can fail a plan without a model grading
//! anything. The fresh-context pass is a model reading a plan and having a view,
//! so it annotates and stops there.
//!
//! # Why this is a type and not a convention
//!
//! The advisory pass is the natural place for scope creep. It is the rule that
//! catches the most interesting problems, its findings are the ones a reviewer
//! most wants to act on, and promoting "just the serious ones" to blocking is a
//! one-line change that reads as an improvement. It would also hand a model a
//! veto over the plan.
//!
//! So [`Finding::from_model`] produces [`Severity::Advisory`] and there is no
//! constructor taking a severity. Making a model finding block requires editing
//! this module, which is where someone will read the paragraph above.
//!
//! # What is not here yet
//!
//! The other three deterministic rules the plan names — a leaf with an empty
//! check union not labelled `UNVERIFIED`, a leaf whose impact set exceeds its
//! lease, a DAG edge with no data dependency — need plan structure that does not
//! exist in this codebase yet. They land as `Finding::structural` beside
//! [`over_decomposition`] when it does.

use serde::{Deserialize, Serialize};

/// Whether a finding can fail a plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// A deterministic structural rule. Fails the plan.
    Blocking,
    /// An observation. Goes on the Plan Receipt and changes nothing.
    Advisory,
}

/// One thing the lint noticed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    rule: String,
    severity: Severity,
    detail: String,
}

impl Finding {
    /// A deterministic rule fired. Same plan, same answer, no model consulted.
    pub fn structural(rule: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            rule: rule.into(),
            severity: Severity::Blocking,
            detail: detail.into(),
        }
    }

    /// The fresh-context pass had a view.
    ///
    /// Always advisory. There is no constructor that takes a severity, so
    /// nothing a model says can be routed to `Blocking` without editing this
    /// file.
    pub fn from_model(rule: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            rule: rule.into(),
            severity: Severity::Advisory,
            detail: detail.into(),
        }
    }

    pub fn rule(&self) -> &str {
        &self.rule
    }

    pub fn severity(&self) -> Severity {
        self.severity
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

/// Whether these findings fail the plan.
///
/// Advisory findings are counted, rendered, and disregarded here — which is the
/// entire behaviour the "never a block" rule asks for.
pub fn plan_fails(findings: &[Finding]) -> bool {
    findings.iter().any(|f| f.severity == Severity::Blocking)
}

/// The name recorded on the Plan Receipt for the over-decomposition rule.
pub const OVER_DECOMPOSITION: &str = "over_decomposition";

/// Fail a plan that decomposed itself past its own crossover.
///
/// `beats_single_shot` is the caller's `Π aᵢ × integration > a_whole`
/// comparison — `cortex_core::capability::decomposition_beats_single_shot`
/// computes it. It is passed in rather than recomputed so this module holds the
/// blocking policy and nothing else; the arithmetic and its documented caveats
/// live in one place.
///
/// A single-leaf plan is not decomposed and is never failed by this rule: the
/// comparison would be leaf-against-whole for the same work, and failing it
/// would refuse the plan for being exactly as ambitious as the task.
pub fn over_decomposition(beats_single_shot: bool, leaf_count: usize) -> Option<Finding> {
    if leaf_count < 2 || beats_single_shot {
        return None;
    }
    Some(Finding::structural(
        OVER_DECOMPOSITION,
        format!(
            "{leaf_count} leaves: the product of the per-leaf priors, times \
             integration success, is below the single-shot prior for the whole \
             task. Decomposing this far makes the plan less likely to succeed, \
             not more."
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_finding_is_never_blocking() {
        // The rule the advisory pass exists under. `from_model` is the only
        // door a fresh-context pass has, and it opens onto `Advisory`.
        let f = Finding::from_model("fresh_context", "step 3 looks under-specified");
        assert_eq!(f.severity(), Severity::Advisory);
        assert!(!plan_fails(std::slice::from_ref(&f)));
    }

    #[test]
    fn a_plan_of_nothing_but_model_findings_still_passes() {
        // However many, however serious-sounding. Aggregating opinions into a
        // block is the veto by another name.
        let findings: Vec<Finding> = (0..20)
            .map(|i| Finding::from_model("fresh_context", format!("concern {i}")))
            .collect();
        assert!(!plan_fails(&findings));
    }

    #[test]
    fn a_structural_finding_fails_the_plan() {
        let f = Finding::structural(OVER_DECOMPOSITION, "twelve leaves at 0.95");
        assert_eq!(f.severity(), Severity::Blocking);
        assert!(plan_fails(std::slice::from_ref(&f)));
    }

    #[test]
    fn one_structural_finding_among_advisories_still_fails() {
        let findings = [
            Finding::from_model("fresh_context", "a"),
            Finding::structural("empty_check_union", "leaf 4 declares no checks"),
            Finding::from_model("fresh_context", "b"),
        ];
        assert!(plan_fails(&findings));
    }

    #[test]
    fn a_plan_past_its_crossover_is_failed() {
        // Twelve leaves at 0.95 is 0.54, which loses to one shot at 0.7. The
        // caller does that arithmetic; this rule turns it into a block.
        let f = over_decomposition(false, 12).expect("the rule must fire");
        assert_eq!(f.rule(), OVER_DECOMPOSITION);
        assert_eq!(f.severity(), Severity::Blocking);
        assert!(f.detail().contains("12 leaves"));
    }

    #[test]
    fn a_plan_on_the_winning_side_of_its_crossover_is_left_alone() {
        assert!(over_decomposition(true, 12).is_none());
        assert!(over_decomposition(true, 2).is_none());
    }

    #[test]
    fn a_single_leaf_plan_is_not_over_decomposed() {
        // Not decomposed at all. Comparing one leaf against the whole is
        // comparing the task to itself, and failing on it would refuse a plan
        // for being exactly as ambitious as the work.
        assert!(over_decomposition(false, 1).is_none());
        assert!(over_decomposition(false, 0).is_none());
    }

    #[test]
    fn no_findings_is_a_passing_plan() {
        assert!(!plan_fails(&[]));
    }
}
