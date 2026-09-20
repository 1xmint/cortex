//! A race arm is a tuple, not a model.
//!
//! Phase 28.2c. Racing only works if the attempts fail differently, and the
//! measured news is bad: across more than 350 models
//! ([arXiv:2506.07962](https://arxiv.org/abs/2506.07962)), frontier models show
//! **high** error correlation across providers *and* across architectures, and
//! **correlation rises with capability**. Two of the best available models agree
//! on the wrong answer roughly **60%** of the time when both are wrong. The
//! mechanism Cortex depends on gets *weaker* as the models it routes to get
//! better.
//!
//! Cross-vendor diversity is still worth buying — decorrelation is real and
//! measurable — but it is a modest gain, and **model identity is the weakest of
//! the diversity axes**. So a generator arm is
//! `(model_ref, method_ref, context_rendering_ref)`:
//!
//! | Axis | What varies |
//! |---|---|
//! | `model_ref` | provider, family, size, effort |
//! | `method_ref` | the engineering approach taken |
//! | `context_rendering_ref` | what the model was shown, and how it was assembled |
//!
//! Phase 12.10's complementarity matrix is measured **over the tuple**, never
//! over `model_ref` — complementarity between two arms differing only in vendor
//! measures the one axis the correlation result says is weakest. That is what
//! [`RaceArm::complementarity_key`] is for.
//!
//! > A race that varies only `model_ref` is recorded as `single_axis_diversity`
//! > and may not claim pass@N above single-arm. It is a legal race — still
//! > better than racing one model against itself — but its receipt says what it
//! > was, and nothing downstream may price it as portfolio diversity.
//!
//! # The shared anchor, and the one sample of it we can get
//!
//! Every arm in a conventional race is anchored to the same context, so every
//! arm inherits the same framing errors, the same omissions, and the same misread
//! of the repository. That is the **shared-anchor term** in the correlation, and
//! it is the one term N samples of the same rendering can never estimate. At
//! `ultra`, at least one arm re-derives its context from the `TaskFrame` — the
//! only sample of that term Cortex will ever get, and it costs one arm.
//!
//! Context diversity is unmeasured in the literature for code: the correlation
//! work varies models, the ensemble work varies scaffolds, and nobody has
//! isolated *what the model was shown* on a code benchmark. Cortex holds the
//! `TaskFrame`, the renderings, and an executable verifier over the outcome, so
//! this is its cheapest original result — a suite configuration rather than a
//! research programme.
//!
//! # The published negative result, and why it does not bind
//!
//! Mixed-model Mixture-of-Agents underperforms self-MoA by **6.6%**
//! ([arXiv:2502.00674](https://arxiv.org/abs/2502.00674)): mixing in weaker
//! models costs more in quality than it gains in diversity. At face value that
//! refutes this whole section, and it is recorded here rather than ignored.
//!
//! It does not bind for one structural reason. Under MoA, candidates combine by
//! **synthesis** — an aggregator model reads them all and writes one answer — so
//! a weak candidate is an *input* to the answer, its quality is load-bearing, and
//! the quality–diversity tradeoff acts on the aggregate continuously. Under
//! Cortex, candidates combine by **execution**: every candidate faces the frozen
//! battery, a weak one fails its checks and is discarded, and selection is a
//! filter rather than a blend. The quality–diversity tradeoff does not bind on a
//! selector.
//!
//! This is why "aggregation is execution, not voting" is load-bearing rather
//! than stylistic. If aggregation ever drifts toward synthesis — a model
//! summarising the candidates, a model merging two diffs, a model picking the
//! best-looking one — the 6.6% result starts applying in full and this module's
//! reasoning becomes wrong.

use serde::{Deserialize, Serialize};

/// One generator in a race.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RaceArm {
    /// Provider, family, size, effort.
    pub model_ref: String,
    /// The engineering approach taken.
    pub method_ref: String,
    /// What the model was shown, and how it was assembled.
    pub context_rendering_ref: String,
    /// This arm re-derived its rendering from the `TaskFrame` instead of
    /// inheriting the one the other arms share.
    pub rederived_context: bool,
}

impl RaceArm {
    /// The key complementarity is measured over.
    ///
    /// All three axes, length-prefixed so no axis's contents can imitate the
    /// structure. The point of the method existing at all is that there is no
    /// convenient way to key a complementarity matrix on `model_ref` alone —
    /// which is the mistake Phase 12.10's machinery would otherwise inherit,
    /// and it would be measuring the weakest axis while reporting portfolio
    /// diversity.
    pub fn complementarity_key(&self) -> String {
        let parts = [
            self.model_ref.as_str(),
            self.method_ref.as_str(),
            self.context_rendering_ref.as_str(),
        ];
        let mut out = String::new();
        for p in parts {
            out.push_str(&p.len().to_string());
            out.push(':');
            out.push_str(p);
        }
        out
    }
}

/// What kind of diversity a race set actually has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiversityClass {
    /// Varies at least one axis stronger than model identity.
    Portfolio,
    /// Varies only `model_ref`. Legal, and better than nothing, but priced as
    /// what it is.
    SingleAxis,
    /// Varies nothing: one model against itself, which is the weakest possible
    /// version of the technique and what the pre-28.2c design did.
    Repetition,
}

impl DiversityClass {
    /// Whether this race may claim pass@N above a single arm.
    ///
    /// Only a portfolio may. A single-axis race is still worth running — it is
    /// better than repetition — but nothing downstream may price it as
    /// portfolio diversity, and the estimator may not size it from
    /// `1 − (1 − a)^N`.
    pub fn may_claim_pass_at_n(&self) -> bool {
        matches!(self, Self::Portfolio)
    }

    /// The value recorded on the receipt.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Portfolio => "portfolio_diversity",
            Self::SingleAxis => "single_axis_diversity",
            Self::Repetition => "repetition",
        }
    }
}

/// Classify a race set by what it actually varies.
pub fn classify(arms: &[RaceArm]) -> DiversityClass {
    let Some(first) = arms.first() else {
        return DiversityClass::Repetition;
    };
    let varies_strong_axis = arms.iter().any(|a| {
        a.method_ref != first.method_ref || a.context_rendering_ref != first.context_rendering_ref
    });
    if varies_strong_axis {
        return DiversityClass::Portfolio;
    }
    if arms.iter().any(|a| a.model_ref != first.model_ref) {
        return DiversityClass::SingleAxis;
    }
    DiversityClass::Repetition
}

/// Why a race set is not admissible at `ultra`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum UltraRefusal {
    /// Every arm inherited the shared rendering, so every arm inherits the same
    /// framing errors and the shared-anchor term goes unsampled.
    #[error("no arm re-derives its context from the TaskFrame")]
    NoRederivedArm,
    /// A single arm is not a race.
    #[error("a race needs more than one arm")]
    NotARace,
}

/// Whether a race set may run at `ultra`.
///
/// `ultra` is the tier that buys the most expensive thing Cortex sells, so it is
/// the tier where the shared-anchor sample is worth one arm. At lower tiers the
/// rule does not apply — the sample is a real cost and a cheaper tier is
/// entitled to decline it.
pub fn ultra_admissible(arms: &[RaceArm]) -> Result<(), UltraRefusal> {
    if arms.len() < 2 {
        return Err(UltraRefusal::NotARace);
    }
    if !arms.iter().any(|a| a.rederived_context) {
        return Err(UltraRefusal::NoRederivedArm);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arm(model: &str, method: &str, rendering: &str) -> RaceArm {
        RaceArm {
            model_ref: model.to_string(),
            method_ref: method.to_string(),
            context_rendering_ref: rendering.to_string(),
            rederived_context: false,
        }
    }

    fn rederiving(model: &str, method: &str, rendering: &str) -> RaceArm {
        RaceArm {
            rederived_context: true,
            ..arm(model, method, rendering)
        }
    }

    #[test]
    fn racing_two_vendors_on_one_rendering_is_single_axis() {
        // The headline correction. This looks like the diverse portfolio the
        // product is sold on, and it varies the axis a 350-model study found
        // weakest -- the two best models agree on the wrong answer ~60% of the
        // time when both are wrong.
        let arms = [
            arm("anthropic/x", "direct-patch", "repo-map-v1"),
            arm("openai/y", "direct-patch", "repo-map-v1"),
        ];
        let class = classify(&arms);
        assert_eq!(class, DiversityClass::SingleAxis);
        assert!(
            !class.may_claim_pass_at_n(),
            "a single-axis race must not be priced as portfolio diversity"
        );
        assert_eq!(class.as_str(), "single_axis_diversity");
    }

    #[test]
    fn one_model_against_itself_is_repetition() {
        // What the pre-28.2c design did: the router picks one model and Phase
        // 6.7 races it against itself.
        let arms = [
            arm("anthropic/x", "direct-patch", "repo-map-v1"),
            arm("anthropic/x", "direct-patch", "repo-map-v1"),
        ];
        assert_eq!(classify(&arms), DiversityClass::Repetition);
        assert!(!classify(&arms).may_claim_pass_at_n());
    }

    #[test]
    fn varying_the_method_is_a_portfolio_even_on_one_model() {
        // Model identity is the weakest axis, so a single model taking two
        // genuinely different approaches is *more* diverse than two vendors
        // taking the same one.
        let arms = [
            arm("anthropic/x", "direct-patch", "repo-map-v1"),
            arm("anthropic/x", "test-first", "repo-map-v1"),
        ];
        assert_eq!(classify(&arms), DiversityClass::Portfolio);
        assert!(classify(&arms).may_claim_pass_at_n());
    }

    #[test]
    fn varying_the_rendering_is_a_portfolio() {
        let arms = [
            arm("anthropic/x", "direct-patch", "repo-map-v1"),
            arm("anthropic/x", "direct-patch", "call-graph-v2"),
        ];
        assert_eq!(classify(&arms), DiversityClass::Portfolio);
    }

    #[test]
    fn an_empty_or_single_race_set_claims_nothing() {
        assert_eq!(classify(&[]), DiversityClass::Repetition);
        assert_eq!(
            classify(&[arm("anthropic/x", "direct-patch", "repo-map-v1")]),
            DiversityClass::Repetition
        );
    }

    #[test]
    fn complementarity_is_keyed_over_the_tuple_not_the_model() {
        // Phase 12.10's matrix inherits whatever key it is given. Keyed on
        // `model_ref`, two arms that differ in approach collapse into one cell
        // and the matrix measures the weakest axis while reporting portfolio
        // diversity.
        let a = arm("anthropic/x", "direct-patch", "repo-map-v1");
        let b = arm("anthropic/x", "test-first", "repo-map-v1");
        assert_eq!(a.model_ref, b.model_ref);
        assert_ne!(a.complementarity_key(), b.complementarity_key());

        let c = arm("anthropic/x", "direct-patch", "call-graph-v2");
        assert_ne!(a.complementarity_key(), c.complementarity_key());
    }

    #[test]
    fn an_axis_cannot_forge_another_arms_key() {
        // Length-prefixed, not joined. On a colon join these two are the same
        // cell, and a method name is something a plan can choose.
        let a = arm("m", "a:b", "c");
        let b = arm("m", "a", "b:c");
        assert_ne!(a.complementarity_key(), b.complementarity_key());
    }

    #[test]
    fn the_same_tuple_keys_the_same_cell() {
        let a = arm("anthropic/x", "direct-patch", "repo-map-v1");
        let b = arm("anthropic/x", "direct-patch", "repo-map-v1");
        assert_eq!(a.complementarity_key(), b.complementarity_key());
    }

    #[test]
    fn ultra_refuses_a_race_where_every_arm_inherits_the_same_context() {
        // The shared-anchor term. Every arm anchored to one rendering inherits
        // the same misread of the repository, and N samples of that rendering
        // cannot estimate the error it introduced.
        let arms = [
            arm("anthropic/x", "direct-patch", "repo-map-v1"),
            arm("openai/y", "test-first", "repo-map-v1"),
        ];
        assert_eq!(classify(&arms), DiversityClass::Portfolio);
        assert_eq!(ultra_admissible(&arms), Err(UltraRefusal::NoRederivedArm));
    }

    #[test]
    fn one_rederiving_arm_is_enough() {
        // It costs exactly one arm, and it is the only sample of the
        // shared-anchor term Cortex will ever get.
        let arms = [
            arm("anthropic/x", "direct-patch", "repo-map-v1"),
            rederiving("openai/y", "test-first", "frame-derived"),
        ];
        assert_eq!(ultra_admissible(&arms), Ok(()));
    }

    #[test]
    fn a_single_arm_is_not_a_race_at_ultra() {
        let arms = [rederiving("anthropic/x", "direct-patch", "frame-derived")];
        assert_eq!(ultra_admissible(&arms), Err(UltraRefusal::NotARace));
        assert_eq!(ultra_admissible(&[]), Err(UltraRefusal::NotARace));
    }
}
