use cortex_core::provider::ProviderId;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum RouteTemplate {
    CheapSingle,
    BestSingle,
    Cascade,
    ImplementThenReview,
    DualRace,
    VerifiedSingle,
}

#[derive(Clone, Debug)]
pub struct TemplateConfig {
    pub template: RouteTemplate,
    pub min_dial: u8,
    pub max_dial: u8,
    pub min_providers: u8,
    pub estimated_cost_multiplier: f64,
    pub description: &'static str,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum StepRole {
    Primary,
    Reviewer,
    Verifier,
    Racer,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionStep {
    pub provider: Option<ProviderId>,
    pub role: StepRole,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateError {
    NotEnoughProviders { need: u8, have: u8 },
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotEnoughProviders { need, have } => {
                write!(f, "need {need} providers but only {have} available")
            }
        }
    }
}

impl std::error::Error for TemplateError {}

impl RouteTemplate {
    pub fn config(&self) -> TemplateConfig {
        match self {
            Self::CheapSingle => TemplateConfig {
                template: *self,
                min_dial: 1,
                max_dial: 4,
                min_providers: 1,
                estimated_cost_multiplier: 1.0,
                description: "Cheapest available model, minimal verification",
            },
            Self::BestSingle => TemplateConfig {
                template: *self,
                min_dial: 3,
                max_dial: 7,
                min_providers: 1,
                estimated_cost_multiplier: 2.0,
                description: "Best model for task shape, standard verification",
            },
            Self::Cascade => TemplateConfig {
                template: *self,
                min_dial: 2,
                max_dial: 6,
                min_providers: 1,
                estimated_cost_multiplier: 1.5,
                description: "Try cheap first, escalate if quality insufficient",
            },
            Self::ImplementThenReview => TemplateConfig {
                template: *self,
                min_dial: 5,
                max_dial: 9,
                min_providers: 2,
                estimated_cost_multiplier: 3.0,
                description: "Execute with one model, review with different model",
            },
            Self::DualRace => TemplateConfig {
                template: *self,
                min_dial: 7,
                max_dial: 10,
                min_providers: 2,
                estimated_cost_multiplier: 4.0,
                description: "Two providers race, pick best result",
            },
            Self::VerifiedSingle => TemplateConfig {
                template: *self,
                min_dial: 4,
                max_dial: 8,
                min_providers: 1,
                estimated_cost_multiplier: 2.5,
                description: "One model plus independent verification step",
            },
        }
    }

    pub fn all() -> Vec<RouteTemplate> {
        vec![
            Self::CheapSingle,
            Self::BestSingle,
            Self::Cascade,
            Self::ImplementThenReview,
            Self::DualRace,
            Self::VerifiedSingle,
        ]
    }

    pub fn available_for_dial(dial: u8) -> Vec<RouteTemplate> {
        Self::all()
            .into_iter()
            .filter(|t| {
                let c = t.config();
                dial >= c.min_dial && dial <= c.max_dial
            })
            .collect()
    }

    pub fn requires_multiple_providers(&self) -> bool {
        self.config().min_providers > 1
    }
}

pub fn plan_execution(
    template: RouteTemplate,
    available_providers: &[ProviderId],
) -> Result<Vec<ExecutionStep>, TemplateError> {
    let config = template.config();
    if (available_providers.len() as u8) < config.min_providers {
        return Err(TemplateError::NotEnoughProviders {
            need: config.min_providers,
            have: available_providers.len() as u8,
        });
    }

    let first = available_providers.first().copied();
    let second = available_providers.get(1).copied();

    let steps = match template {
        RouteTemplate::CheapSingle => vec![ExecutionStep {
            provider: first,
            role: StepRole::Primary,
            description: "Execute with cheapest model".into(),
        }],
        RouteTemplate::BestSingle => vec![ExecutionStep {
            provider: first,
            role: StepRole::Primary,
            description: "Execute with best model for task shape".into(),
        }],
        RouteTemplate::Cascade => vec![
            ExecutionStep {
                provider: first,
                role: StepRole::Primary,
                description: "Try with cheap model first".into(),
            },
            ExecutionStep {
                provider: first,
                role: StepRole::Verifier,
                description: "Check quality, escalate if needed".into(),
            },
        ],
        RouteTemplate::ImplementThenReview => vec![
            ExecutionStep {
                provider: first,
                role: StepRole::Primary,
                description: "Execute implementation".into(),
            },
            ExecutionStep {
                provider: second,
                role: StepRole::Reviewer,
                description: "Independent review by different provider".into(),
            },
        ],
        RouteTemplate::DualRace => vec![
            ExecutionStep {
                provider: first,
                role: StepRole::Racer,
                description: "Provider A races".into(),
            },
            ExecutionStep {
                provider: second,
                role: StepRole::Racer,
                description: "Provider B races".into(),
            },
        ],
        RouteTemplate::VerifiedSingle => vec![
            ExecutionStep {
                provider: first,
                role: StepRole::Primary,
                description: "Execute with selected model".into(),
            },
            ExecutionStep {
                provider: first,
                role: StepRole::Verifier,
                description: "Independent verification step".into(),
            },
        ],
    };

    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cheap_single_any_dial_1_to_4() {
        for dial in 1..=4 {
            let available = RouteTemplate::available_for_dial(dial);
            assert!(
                available.contains(&RouteTemplate::CheapSingle),
                "dial {dial}"
            );
        }
        let at_5 = RouteTemplate::available_for_dial(5);
        assert!(!at_5.contains(&RouteTemplate::CheapSingle));
    }

    #[test]
    fn test_dual_race_needs_two_providers() {
        assert!(RouteTemplate::DualRace.requires_multiple_providers());
        assert!(!RouteTemplate::BestSingle.requires_multiple_providers());
    }

    #[test]
    fn test_available_for_dial_filtering() {
        let at_1 = RouteTemplate::available_for_dial(1);
        assert_eq!(at_1, vec![RouteTemplate::CheapSingle]);

        let at_10 = RouteTemplate::available_for_dial(10);
        assert_eq!(at_10, vec![RouteTemplate::DualRace]);

        let at_5 = RouteTemplate::available_for_dial(5);
        assert!(at_5.len() >= 3);
    }

    #[test]
    fn test_plan_single_provider() {
        let providers = vec![ProviderId::Claude];
        let steps = plan_execution(RouteTemplate::BestSingle, &providers).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].role, StepRole::Primary);
        assert_eq!(steps[0].provider, Some(ProviderId::Claude));
    }

    #[test]
    fn test_plan_dual_race_with_two_providers() {
        let providers = vec![ProviderId::Claude, ProviderId::Openai];
        let steps = plan_execution(RouteTemplate::DualRace, &providers).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].role, StepRole::Racer);
        assert_eq!(steps[1].role, StepRole::Racer);
        assert_eq!(steps[0].provider, Some(ProviderId::Claude));
        assert_eq!(steps[1].provider, Some(ProviderId::Openai));
    }

    #[test]
    fn test_plan_errors_without_enough_providers() {
        let providers = vec![ProviderId::Claude];
        let result = plan_execution(RouteTemplate::DualRace, &providers);
        assert_eq!(
            result,
            Err(TemplateError::NotEnoughProviders { need: 2, have: 1 })
        );
    }
}
