use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::provider::{ProviderId, Tier};
use crate::routing::{RationaleCode, RiskLevel, ScoredRoute};
use crate::task::TaskStatus;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub event: LedgerEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LedgerEvent {
    RoutingDecision {
        task_id: Uuid,
        provider: ProviderId,
        tier: Tier,
        risk: RiskLevel,
        rationale: Vec<RationaleCode>,
        score: f64,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        alternatives_considered: Vec<ScoredRoute>,
    },
    TaskOutcome {
        task_id: Uuid,
        provider: ProviderId,
        status: TaskStatus,
        duration_ms: u64,
        files_changed: u32,
        tests_passed: Option<bool>,
    },
    UserOverride {
        task_id: Uuid,
        from_provider: ProviderId,
        to_provider: ProviderId,
        reason: Option<String>,
    },
    ProviderStatus {
        provider: ProviderId,
        authenticated: bool,
        pressure: f64,
    },
}

impl LedgerEntry {
    pub fn new(event: LedgerEvent) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            event,
        }
    }
}
