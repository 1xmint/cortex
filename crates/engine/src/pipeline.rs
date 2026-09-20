use cortex_core::autonomy::{decide_autonomy, AutonomyDecision};
use cortex_core::provider::ProviderId;
use cortex_core::routing::{Intent, RiskLevel};

use crate::bandit::{ArmKey, TaskFamily, UcbScorer};
use crate::risk::classify_risk;
use crate::templates::{plan_execution, ExecutionStep, RouteTemplate};

#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub dial: u8,
    pub available_providers: Vec<ProviderId>,
    pub is_first_observation: bool,
}

#[derive(Debug, Clone)]
pub struct RoutingPlan {
    pub intent: Intent,
    pub risk_level: RiskLevel,
    pub template: RouteTemplate,
    pub provider: ProviderId,
    pub autonomy: AutonomyDecision,
    pub steps: Vec<ExecutionStep>,
    pub confidence: f64,
    pub explanation: String,
}

#[derive(Debug, Clone)]
pub enum PipelineResult {
    Planned(RoutingPlan),
    Blocked {
        reason: String,
        missing_evidence: Vec<String>,
    },
    NoProviders,
}

pub fn plan_route(
    goal: &str,
    files: &[&str],
    config: &PipelineConfig,
    scorer: &UcbScorer,
) -> PipelineResult {
    if config.available_providers.is_empty() {
        return PipelineResult::NoProviders;
    }

    let intent = parse_intent(goal);
    let risk_level = classify_risk(files);
    let task_family = TaskFamily::from_intent(&intent);

    let template = match select_template(config.dial, &config.available_providers) {
        Some(t) => t,
        None => {
            return PipelineResult::Blocked {
                reason: "no route template satisfied by available providers at this dial".into(),
                missing_evidence: vec!["additional provider capacity".into()],
            };
        }
    };

    let provider = match scorer.best_arm(task_family, risk_level, &config.available_providers) {
        Some(p) => p,
        None => return PipelineResult::NoProviders,
    };

    let arm_key = ArmKey {
        task_family,
        risk_level,
        provider,
    };
    let confidence = scorer
        .arms
        .get(&arm_key)
        .map(|s| s.mean_reward())
        .unwrap_or(0.5);

    let autonomy = decide_autonomy(confidence, risk_level, config.is_first_observation);

    let providers_for_plan = if template.requires_multiple_providers() {
        config.available_providers.clone()
    } else {
        vec![provider]
    };

    let steps = match plan_execution(template, &providers_for_plan) {
        Ok(s) => s,
        Err(e) => {
            return PipelineResult::Blocked {
                reason: e.to_string(),
                missing_evidence: vec![],
            };
        }
    };

    let explanation = format!(
        "Intent: {intent:?} | Risk: {risk_level:?} | Template: {template:?} | Provider: {provider:?} | Confidence: {confidence:.2} | Autonomy: {autonomy:?}",
    );

    PipelineResult::Planned(RoutingPlan {
        intent,
        risk_level,
        template,
        provider,
        autonomy,
        steps,
        confidence,
        explanation,
    })
}

pub fn record_outcome(
    scorer: &mut UcbScorer,
    plan: &RoutingPlan,
    success: bool,
    contamination: f64,
) {
    let arm = ArmKey {
        task_family: TaskFamily::from_intent(&plan.intent),
        risk_level: plan.risk_level,
        provider: plan.provider,
    };

    let reward = if success { 1.0 } else { 0.0 };
    scorer.update(arm, reward, contamination);
}

fn parse_intent(goal: &str) -> Intent {
    let g = goal.to_ascii_lowercase();

    if contains_any(&g, &["fix", "bug", "repair"]) {
        Intent::Fix
    } else if contains_any(&g, &["add", "create", "implement", "build"]) {
        Intent::Add
    } else if contains_any(&g, &["explore", "find", "search", "where"]) {
        Intent::Explore
    } else if contains_any(&g, &["review", "check"]) {
        Intent::Review
    } else if g.contains("test") {
        Intent::Test
    } else if contains_any(&g, &["refactor", "clean"]) {
        Intent::Refactor
    } else if contains_any(&g, &["think", "plan", "design"]) {
        Intent::Think
    } else if contains_any(&g, &["deploy", "ship", "release"]) {
        Intent::Ship
    } else {
        Intent::Fix
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

fn select_template(dial: u8, providers: &[ProviderId]) -> Option<RouteTemplate> {
    let available = RouteTemplate::available_for_dial(dial.clamp(1, 10));
    available.into_iter().find(|t| {
        let cfg = t.config();
        (providers.len() as u8) >= cfg.min_providers
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config(dial: u8) -> PipelineConfig {
        PipelineConfig {
            dial,
            available_providers: vec![ProviderId::Claude],
            is_first_observation: false,
        }
    }

    #[test]
    fn test_plan_simple_fix() {
        let scorer = UcbScorer::new(1.0);
        let result = plan_route(
            "fix the login bug",
            &["src/auth/login.ts"],
            &default_config(5),
            &scorer,
        );

        match result {
            PipelineResult::Planned(plan) => {
                assert_eq!(plan.intent, Intent::Fix);
                assert_eq!(plan.provider, ProviderId::Claude);
                assert!(!plan.steps.is_empty());
            }
            other => panic!("expected Planned, got {other:?}"),
        }
    }

    #[test]
    fn test_plan_explore_task() {
        let scorer = UcbScorer::new(1.0);
        let result = plan_route(
            "find where billing is calculated",
            &["src/billing/credits.ts"],
            &default_config(4),
            &scorer,
        );

        match result {
            PipelineResult::Planned(plan) => {
                assert_eq!(plan.intent, Intent::Explore);
            }
            other => panic!("expected Planned, got {other:?}"),
        }
    }

    #[test]
    fn test_no_providers_returns_no_providers() {
        let scorer = UcbScorer::new(1.0);
        let config = PipelineConfig {
            dial: 5,
            available_providers: vec![],
            is_first_observation: false,
        };

        let result = plan_route("fix bug", &["src/main.ts"], &config, &scorer);
        assert!(matches!(result, PipelineResult::NoProviders));
    }

    #[test]
    fn test_first_observation_asks() {
        let scorer = UcbScorer::new(1.0);
        let config = PipelineConfig {
            dial: 3,
            available_providers: vec![ProviderId::Claude],
            is_first_observation: true,
        };

        let result = plan_route("fix a typo", &["README.md"], &config, &scorer);
        match result {
            PipelineResult::Planned(plan) => {
                assert_eq!(plan.autonomy, AutonomyDecision::AskBeforeProceeding);
            }
            other => panic!("expected Planned, got {other:?}"),
        }
    }

    #[test]
    fn test_record_outcome_updates_scorer() {
        let mut scorer = UcbScorer::new(1.0);
        let result = plan_route(
            "fix the login bug",
            &["src/auth/login.ts"],
            &default_config(5),
            &scorer,
        );

        let plan = match result {
            PipelineResult::Planned(p) => p,
            other => panic!("expected Planned, got {other:?}"),
        };

        record_outcome(&mut scorer, &plan, true, 0.0);

        let arm = ArmKey {
            task_family: TaskFamily::from_intent(&plan.intent),
            risk_level: plan.risk_level,
            provider: plan.provider,
        };
        let stats = scorer.arms.get(&arm).expect("arm should exist");
        assert_eq!(stats.trials, 1);
        assert!((stats.total_reward - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_high_dial_selects_expensive_template() {
        let scorer = UcbScorer::new(1.0);
        let config = PipelineConfig {
            dial: 10,
            available_providers: vec![ProviderId::Claude, ProviderId::Openai],
            is_first_observation: false,
        };

        let result = plan_route(
            "implement payments",
            &["src/billing/pay.ts"],
            &config,
            &scorer,
        );
        match result {
            PipelineResult::Planned(plan) => {
                assert!(plan.template.config().estimated_cost_multiplier >= 3.0);
            }
            other => panic!("expected Planned, got {other:?}"),
        }
    }
}
