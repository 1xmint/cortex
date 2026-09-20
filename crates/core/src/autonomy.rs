use crate::routing::RiskLevel;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConfidenceBand {
    Low,
    Medium,
    High,
    Certain,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum AutonomyDecision {
    ProceedAutonomously,
    ProceedAndFlag,
    AskBeforeProceeding,
    RequireExplicitApproval,
    Forbidden,
}

pub fn confidence_band(score: f64) -> ConfidenceBand {
    if score < 0.3 {
        ConfidenceBand::Low
    } else if score < 0.6 {
        ConfidenceBand::Medium
    } else if score <= 0.85 {
        ConfidenceBand::High
    } else {
        ConfidenceBand::Certain
    }
}

pub fn decide_autonomy(
    confidence: f64,
    risk: RiskLevel,
    is_first_observation: bool,
) -> AutonomyDecision {
    if is_first_observation {
        return AutonomyDecision::AskBeforeProceeding;
    }

    if risk == RiskLevel::Critical {
        return AutonomyDecision::RequireExplicitApproval;
    }

    match (risk, confidence_band(confidence)) {
        (RiskLevel::High, ConfidenceBand::Low | ConfidenceBand::Medium) => {
            AutonomyDecision::AskBeforeProceeding
        }
        (RiskLevel::High, ConfidenceBand::High | ConfidenceBand::Certain) => {
            AutonomyDecision::ProceedAndFlag
        }
        (RiskLevel::Medium, ConfidenceBand::Low) => AutonomyDecision::AskBeforeProceeding,
        (RiskLevel::Medium, ConfidenceBand::Medium | ConfidenceBand::High) => {
            AutonomyDecision::ProceedAndFlag
        }
        (RiskLevel::Medium, ConfidenceBand::Certain) => AutonomyDecision::ProceedAutonomously,
        (RiskLevel::Low, ConfidenceBand::Low) => AutonomyDecision::ProceedAndFlag,
        (
            RiskLevel::Low,
            ConfidenceBand::Medium | ConfidenceBand::High | ConfidenceBand::Certain,
        ) => AutonomyDecision::ProceedAutonomously,
        _ => AutonomyDecision::Forbidden,
    }
}

impl AutonomyDecision {
    pub fn requires_user_input(&self) -> bool {
        matches!(
            self,
            Self::AskBeforeProceeding | Self::RequireExplicitApproval | Self::Forbidden
        )
    }

    pub fn description(&self) -> &str {
        match self {
            Self::ProceedAutonomously => "Proceed autonomously.",
            Self::ProceedAndFlag => "Proceed and flag for review.",
            Self::AskBeforeProceeding => "Ask before proceeding.",
            Self::RequireExplicitApproval => "Require explicit approval before proceeding.",
            Self::Forbidden => "Action is forbidden.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_observation_always_asks() {
        assert_eq!(
            decide_autonomy(0.95, RiskLevel::Low, true),
            AutonomyDecision::AskBeforeProceeding
        );
        assert_eq!(
            decide_autonomy(0.1, RiskLevel::Critical, true),
            AutonomyDecision::AskBeforeProceeding
        );
    }

    #[test]
    fn test_critical_always_requires_approval() {
        assert_eq!(
            decide_autonomy(0.1, RiskLevel::Critical, false),
            AutonomyDecision::RequireExplicitApproval
        );
        assert_eq!(
            decide_autonomy(0.99, RiskLevel::Critical, false),
            AutonomyDecision::RequireExplicitApproval
        );
    }

    #[test]
    fn test_high_risk_never_autonomous() {
        assert_eq!(
            decide_autonomy(0.2, RiskLevel::High, false),
            AutonomyDecision::AskBeforeProceeding
        );
        assert_eq!(
            decide_autonomy(0.75, RiskLevel::High, false),
            AutonomyDecision::ProceedAndFlag
        );
        assert_eq!(
            decide_autonomy(0.95, RiskLevel::High, false),
            AutonomyDecision::ProceedAndFlag
        );
    }

    #[test]
    fn test_low_risk_high_confidence_autonomous() {
        assert_eq!(
            decide_autonomy(0.7, RiskLevel::Low, false),
            AutonomyDecision::ProceedAutonomously
        );
        assert_eq!(
            decide_autonomy(0.95, RiskLevel::Low, false),
            AutonomyDecision::ProceedAutonomously
        );
    }

    #[test]
    fn test_medium_risk_certain_autonomous() {
        assert_eq!(
            decide_autonomy(0.9, RiskLevel::Medium, false),
            AutonomyDecision::ProceedAutonomously
        );
    }

    #[test]
    fn test_confidence_bands() {
        assert_eq!(confidence_band(0.0), ConfidenceBand::Low);
        assert_eq!(confidence_band(0.2999), ConfidenceBand::Low);
        assert_eq!(confidence_band(0.3), ConfidenceBand::Medium);
        assert_eq!(confidence_band(0.5999), ConfidenceBand::Medium);
        assert_eq!(confidence_band(0.6), ConfidenceBand::High);
        assert_eq!(confidence_band(0.85), ConfidenceBand::High);
        assert_eq!(confidence_band(0.8501), ConfidenceBand::Certain);
    }
}
