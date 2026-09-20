use serde::{Deserialize, Serialize};

use crate::provider::{ProviderId, Tier};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    Fix,
    Add,
    Explore,
    Review,
    Think,
    Test,
    Refactor,
    Ship,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub provider: ProviderId,
    pub tier: Tier,
    pub model_id: String,
    pub rationale: Vec<RationaleCode>,
    pub score: f64,
    pub alternatives_considered: Vec<ScoredRoute>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredRoute {
    pub provider: ProviderId,
    pub tier: Tier,
    pub score: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RationaleCode {
    BestAvailableForTier,
    LowerPressure,
    RiskRequiresHigherTier,
    ProviderUnavailable,
    UserOverride,
    FailureEscalation,
    CostOptimized,
}

impl RiskLevel {
    pub fn requires_approval(&self) -> bool {
        matches!(self, Self::High | Self::Critical)
    }

    pub fn minimum_tier(&self) -> Tier {
        match self {
            Self::Low => Tier::Search,
            Self::Medium | Self::High => Tier::Execute,
            Self::Critical => Tier::Think,
        }
    }
}

impl Intent {
    pub fn default_tier(&self) -> Tier {
        match self {
            Self::Explore => Tier::Search,
            Self::Fix | Self::Add | Self::Test | Self::Refactor | Self::Ship => Tier::Execute,
            Self::Review | Self::Think => Tier::Think,
        }
    }
}
