//! The router optimises two different things, and the dial says which.
//!
//! Invariant 12 stated the routing objective unconditionally:
//!
//! > The router optimises cost-to-verified-outcome, never pass rate. A route
//! > that passes at four times the price is a worse route.
//!
//! That is correct for most work, and stating it unconditionally is the defect.
//! Under an unconditional cost objective Cortex converges on the cheapest model
//! that clears the bar, so its pass-rate ceiling is *that model's* pass rate —
//! necessarily at or below the frontier. The effort dial changed fan-out shape
//! and model tier but not the objective, so even at `ultra` the optimiser was
//! still trying to spend less. The top of the dial meant more spending toward a
//! goal of less spending.
//!
//! There are two objectives:
//!
//! | | Objective | Dial position | Ceiling |
//! |---|---|---|---|
//! | **Efficiency** | minimise cost subject to passing | `xhigh` and below | the routed model's pass rate |
//! | **Capability** | maximise P(verified) under the cap | `ultra` | strictly above any single sample, iff the verifier is sound |
//!
//! > **Invariant 26.** The router's objective is set by the effort dial, not
//! > fixed. Below the top position it minimises cost subject to reaching the
//! > required confidence. At the top it **maximises P(verified) subject to the
//! > cap**, and a route that passes at four times the price is the *better*
//! > route there. The receipt records which objective was in force.
//!
//! # Why the decision carries the objective
//!
//! "The receipt records which objective was in force" is not a logging note. The
//! same route chosen under the two objectives means opposite things — under
//! efficiency it is the cheapest thing that clears the bar, under capability it
//! is the most likely thing to pass — and a dispute six months later turns on
//! which one the customer bought.
//!
//! So [`choose`] returns a [`RouteDecision`] rather than an index, and there is
//! no way to obtain the choice without also holding the objective that produced
//! it. Recording it becomes something a caller has in hand rather than something
//! a caller must remember.

use serde::{Deserialize, Serialize};

use crate::execution_job::EffortLevel;

/// What the router is trying to do on this request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouterObjective {
    /// Minimise cost subject to reaching the required confidence.
    MinimiseCost,
    /// Maximise P(verified) subject to the spend cap. Here a route that passes
    /// at four times the price is the *better* route.
    MaximiseVerified,
}

impl RouterObjective {
    /// The value written to the receipt.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MinimiseCost => "minimise_cost",
            Self::MaximiseVerified => "maximise_verified",
        }
    }
}

/// The dial position determines the objective. This is the whole of invariant 26.
pub fn objective_for(effort: EffortLevel) -> RouterObjective {
    match effort {
        EffortLevel::Ultra => RouterObjective::MaximiseVerified,
        EffortLevel::Low | EffortLevel::Medium | EffortLevel::High | EffortLevel::XHigh => {
            RouterObjective::MinimiseCost
        }
    }
}

/// One route the router could take.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RouteCandidate {
    pub label: String,
    /// Predicted probability the delivered work is verified.
    pub p_verified: f64,
    /// Predicted spend, in the same unit as the cap.
    pub cost: f64,
}

/// A chosen route and the objective that chose it.
///
/// Constructible only by [`choose`], so the objective cannot be separated from
/// the decision on the way to the receipt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RouteDecision {
    label: String,
    objective: RouterObjective,
}

impl RouteDecision {
    pub fn route(&self) -> &str {
        &self.label
    }

    /// What the router was optimising when it picked this. Goes on the receipt.
    pub fn objective_in_force(&self) -> RouterObjective {
        self.objective
    }
}

/// Pick a route under the objective the dial selects.
///
/// Returns `None` when nothing qualifies — over the cap under either objective,
/// or under the cap but short of `required_confidence` under the cost objective.
/// A confidence requirement that silently degrades to "the best available" is a
/// confidence requirement that means nothing, so no route is a real answer here
/// and the caller decides whether to raise the cap, lower the bar, or decline
/// the work.
///
/// `required_confidence` is ignored under [`RouterObjective::MaximiseVerified`]:
/// at `ultra` the instruction is to buy the most probable pass the cap allows,
/// and refusing the best available route for falling short of a threshold would
/// hand the customer nothing rather than the best thing they asked for.
pub fn choose(
    effort: EffortLevel,
    required_confidence: f64,
    cap: f64,
    candidates: &[RouteCandidate],
) -> Option<RouteDecision> {
    let objective = objective_for(effort);
    let affordable = candidates.iter().filter(|c| c.cost <= cap);

    let chosen = match objective {
        RouterObjective::MinimiseCost => affordable
            .filter(|c| c.p_verified >= required_confidence)
            // Cheapest, and on a tie the one more likely to pass.
            .min_by(|a, b| a.cost.total_cmp(&b.cost).then(b.p_verified.total_cmp(&a.p_verified))),
        RouterObjective::MaximiseVerified => affordable
            // Most likely to pass, and on a tie the cheaper one. Price is the
            // tiebreak here, never the objective.
            .max_by(|a, b| {
                a.p_verified
                    .total_cmp(&b.p_verified)
                    .then(b.cost.total_cmp(&a.cost))
            }),
    }?;

    Some(RouteDecision {
        label: chosen.label.clone(),
        objective,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(label: &str, p_verified: f64, cost: f64) -> RouteCandidate {
        RouteCandidate {
            label: label.to_string(),
            p_verified,
            cost,
        }
    }

    /// The plan's own example: a route that passes at four times the price.
    fn cheap_and_expensive() -> Vec<RouteCandidate> {
        vec![route("cheap", 0.55, 1.0), route("expensive", 0.88, 4.0)]
    }

    #[test]
    fn only_the_top_of_the_dial_changes_the_objective() {
        assert_eq!(
            objective_for(EffortLevel::Ultra),
            RouterObjective::MaximiseVerified
        );
        for below in [
            EffortLevel::Low,
            EffortLevel::Medium,
            EffortLevel::High,
            EffortLevel::XHigh,
        ] {
            assert_eq!(
                objective_for(below),
                RouterObjective::MinimiseCost,
                "{below:?} must still minimise cost"
            );
        }
    }

    #[test]
    fn four_times_the_price_is_a_worse_route_below_the_top() {
        // Invariant 12, unchanged for the work it was written for.
        let d = choose(EffortLevel::High, 0.5, 10.0, &cheap_and_expensive()).expect("a route");
        assert_eq!(d.route(), "cheap");
        assert_eq!(d.objective_in_force(), RouterObjective::MinimiseCost);
    }

    #[test]
    fn four_times_the_price_is_the_better_route_at_ultra() {
        // The correction. Same candidates, same cap, opposite answer -- and
        // this is what makes the top of the dial mean something instead of
        // spending more toward a goal of spending less.
        let d = choose(EffortLevel::Ultra, 0.5, 10.0, &cheap_and_expensive()).expect("a route");
        assert_eq!(d.route(), "expensive");
        assert_eq!(d.objective_in_force(), RouterObjective::MaximiseVerified);
    }

    #[test]
    fn the_objective_cannot_be_separated_from_the_decision() {
        // Two receipts could name the same route and mean opposite things:
        // cheapest-that-clears-the-bar, or most-likely-to-pass. A dispute turns
        // on which one the customer bought, so the choice does not exist
        // without it.
        let low = choose(EffortLevel::Low, 0.5, 10.0, &[route("only", 0.6, 1.0)]).unwrap();
        let ultra = choose(EffortLevel::Ultra, 0.5, 10.0, &[route("only", 0.6, 1.0)]).unwrap();
        assert_eq!(low.route(), ultra.route());
        assert_ne!(low.objective_in_force(), ultra.objective_in_force());
        assert_eq!(low.objective_in_force().as_str(), "minimise_cost");
        assert_eq!(ultra.objective_in_force().as_str(), "maximise_verified");
    }

    #[test]
    fn the_cap_binds_under_both_objectives() {
        // "Subject to the cap" is not advisory at `ultra`. The most likely
        // route to pass is not a route if it cannot be paid for.
        let candidates = cheap_and_expensive();
        for effort in [EffortLevel::High, EffortLevel::Ultra] {
            let d = choose(effort, 0.5, 2.0, &candidates).expect("a route under the cap");
            assert_eq!(d.route(), "cheap", "{effort:?} must respect the cap");
        }
    }

    #[test]
    fn nothing_affordable_is_no_route() {
        for effort in [EffortLevel::High, EffortLevel::Ultra] {
            assert!(choose(effort, 0.0, 0.5, &cheap_and_expensive()).is_none());
        }
    }

    #[test]
    fn the_cost_objective_will_not_quietly_lower_the_bar() {
        // Nothing clears 0.95. Returning the best available would make the
        // confidence requirement decorative, so the caller is told instead and
        // decides whether to raise the cap, lower the bar, or decline the work.
        assert!(choose(EffortLevel::High, 0.95, 100.0, &cheap_and_expensive()).is_none());
    }

    #[test]
    fn ultra_buys_the_best_available_rather_than_refusing_on_the_threshold() {
        // The mirror of the previous test, and deliberately the other way. At
        // the top of the dial the instruction is to buy the most probable pass
        // the cap allows; refusing it for falling short of a threshold hands
        // the customer nothing instead of the best thing they asked for.
        let d = choose(EffortLevel::Ultra, 0.95, 100.0, &cheap_and_expensive()).expect("a route");
        assert_eq!(d.route(), "expensive");
    }

    #[test]
    fn cost_breaks_ties_at_ultra_but_never_leads() {
        let candidates = [
            route("pricey", 0.8, 9.0),
            route("same-odds-cheaper", 0.8, 2.0),
            route("cheapest-worse-odds", 0.4, 0.5),
        ];
        let d = choose(EffortLevel::Ultra, 0.0, 10.0, &candidates).unwrap();
        assert_eq!(d.route(), "same-odds-cheaper");
    }

    #[test]
    fn confidence_breaks_ties_under_the_cost_objective() {
        let candidates = [
            route("same-price-worse-odds", 0.6, 2.0),
            route("same-price-better-odds", 0.9, 2.0),
        ];
        let d = choose(EffortLevel::High, 0.5, 10.0, &candidates).unwrap();
        assert_eq!(d.route(), "same-price-better-odds");
    }

    #[test]
    fn no_candidates_is_no_route() {
        assert!(choose(EffortLevel::Ultra, 0.0, 100.0, &[]).is_none());
    }
}
