use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::provider::{ProviderId, Tier};
use crate::routing::RiskLevel;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskContract {
    pub id: Uuid,
    pub objective: String,
    pub tier: Tier,
    pub risk: RiskLevel,
    pub allowed_operations: Vec<Operation>,
    pub acceptance_criteria: Vec<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub project_name: Option<String>,
    #[serde(default)]
    pub repo_path: Option<String>,
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    #[serde(default)]
    pub forbidden_paths: Vec<String>,
    #[serde(default)]
    pub expected_base_commit: Option<String>,
    /// What kind of claim a verdict over this task may be, declared here --
    /// at plan time, before approval -- so that delivery cannot choose it
    /// after the fact.
    ///
    /// Defaults to `Authored`, the weaker claim. A contract written before
    /// this field existed promised nothing about the exam, and defaulting to
    /// `Strong` would retroactively put words in its mouth.
    #[serde(default)]
    pub verdict_class: crate::diff_surface::VerdictClass,
    #[serde(default)]
    pub autonomy: Option<String>,
    #[serde(default)]
    pub approval: Option<String>,
    #[serde(default)]
    pub budget_limit: Option<f64>,
    #[serde(default)]
    pub rollback_notes: Option<String>,
    #[serde(default)]
    pub required_checks: Vec<RequiredCheck>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_recipe: Option<WorkRecipe>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequiredCheck {
    pub name: String,
    pub command: String,
    #[serde(default = "default_required_check")]
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkRecipe {
    pub version: u32,
    pub kind: WorkKind,
    pub objective: String,
    #[serde(default)]
    pub target_paths: Vec<String>,
    #[serde(default)]
    pub required_checks: Vec<RequiredCheck>,
    #[serde(default)]
    pub acceptance: Vec<AcceptanceCriterion>,
    #[serde(default)]
    pub constraints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkRecipeSeed {
    #[serde(default)]
    pub target_paths: Vec<String>,
    #[serde(default)]
    pub acceptance: Vec<AcceptanceCriterion>,
    #[serde(default)]
    pub constraints: Vec<String>,
}

// `Ord` and `Hash` are here so `TaskClass` can be a map key and sort into a
// stable order — a price list rendered in a different order every time is a
// price list nobody can diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkKind {
    Explore,
    Modify,
    Add,
    Refactor,
    Test,
    Build,
    Lint,
    Review,
    Ship,
    Heal,
    Gate,
}

impl WorkKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Explore => "explore",
            Self::Modify => "modify",
            Self::Add => "add",
            Self::Refactor => "refactor",
            Self::Test => "test",
            Self::Build => "build",
            Self::Lint => "lint",
            Self::Review => "review",
            Self::Ship => "ship",
            Self::Heal => "heal",
            Self::Gate => "gate",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "explore" => Some(Self::Explore),
            "modify" => Some(Self::Modify),
            "add" => Some(Self::Add),
            "refactor" => Some(Self::Refactor),
            "test" => Some(Self::Test),
            "build" => Some(Self::Build),
            "lint" => Some(Self::Lint),
            "review" => Some(Self::Review),
            "ship" => Some(Self::Ship),
            "heal" => Some(Self::Heal),
            "gate" => Some(Self::Gate),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptanceCriterion {
    pub id: String,
    pub text: String,
    pub verification: AcceptanceVerification,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceVerification {
    Manual,
    RequiredCheck { check_name: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Read,
    Edit,
    Create,
    Delete,
    Test,
    GitCommit,
    GitPush,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub task_id: Uuid,
    pub status: TaskStatus,
    pub provider: ProviderId,
    pub files_changed: Vec<String>,
    pub tests_run: Option<TestOutcome>,
    pub duration_ms: u64,
    pub output_summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    AwaitingApproval,
    Approved,
    Rejected,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestOutcome {
    pub total: u32,
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
}

impl TaskContract {
    pub fn new(objective: String, tier: Tier, risk: RiskLevel) -> Self {
        Self {
            id: Uuid::new_v4(),
            objective,
            tier,
            risk,
            allowed_operations: default_operations_for_tier(tier),
            acceptance_criteria: Vec::new(),
            created_at: Utc::now(),
            project_id: None,
            project_name: None,
            repo_path: None,
            allowed_paths: Vec::new(),
            forbidden_paths: Vec::new(),
            expected_base_commit: None,
            // The weaker claim until a planner declares otherwise: a task
            // constructed here has promised nothing about the exam.
            verdict_class: crate::diff_surface::VerdictClass::Authored,
            autonomy: None,
            approval: None,
            budget_limit: None,
            rollback_notes: None,
            required_checks: Vec::new(),
            work_recipe: None,
        }
    }

    pub fn with_allowed_paths(mut self, allowed_paths: Vec<String>) -> Self {
        self.allowed_paths = allowed_paths;
        self
    }

    pub fn with_expected_base_commit(mut self, expected_base_commit: Option<String>) -> Self {
        self.expected_base_commit = expected_base_commit;
        self
    }

    pub fn with_dispatch_contract(
        self,
        allowed_paths: Vec<String>,
        expected_base_commit: Option<String>,
    ) -> Self {
        self.with_allowed_paths(allowed_paths)
            .with_expected_base_commit(expected_base_commit)
    }

    pub fn with_work_recipe(mut self, work_recipe: WorkRecipe) -> Self {
        self.work_recipe = Some(work_recipe);
        self
    }
}

fn default_operations_for_tier(tier: Tier) -> Vec<Operation> {
    match tier {
        Tier::Search => vec![Operation::Read],
        Tier::Execute => vec![
            Operation::Read,
            Operation::Edit,
            Operation::Create,
            Operation::Test,
        ],
        Tier::Think => vec![Operation::Read],
    }
}

fn default_required_check() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_contract_deserializes_without_work_recipe() {
        let raw = r#"{
            "id": "00000000-0000-0000-0000-000000000000",
            "objective": "change the thing",
            "tier": "execute",
            "risk": "medium",
            "allowed_operations": ["read", "edit"],
            "acceptance_criteria": [],
            "created_at": "2026-05-23T00:00:00Z"
        }"#;

        let task: TaskContract = serde_json::from_str(raw).unwrap();

        assert!(task.work_recipe.is_none());
    }

    #[test]
    fn task_contract_serializes_work_recipe_when_present() {
        let recipe = WorkRecipe {
            version: 1,
            kind: WorkKind::Test,
            objective: "run checks".to_string(),
            target_paths: vec!["crates/api/src/lib.rs".to_string()],
            required_checks: vec![RequiredCheck {
                name: "cargo:test".to_string(),
                command: "cargo test -p cortex-api".to_string(),
                required: true,
            }],
            acceptance: vec![AcceptanceCriterion {
                id: "required-check-cargo:test".to_string(),
                text: "Required check `cargo:test` passes".to_string(),
                verification: AcceptanceVerification::RequiredCheck {
                    check_name: "cargo:test".to_string(),
                },
            }],
            constraints: vec!["Stay within allowed paths".to_string()],
        };

        let task = TaskContract::new("run checks".to_string(), Tier::Execute, RiskLevel::Medium)
            .with_work_recipe(recipe);
        let value = serde_json::to_value(task).unwrap();

        assert_eq!(value["work_recipe"]["kind"], "test");
        assert_eq!(value["work_recipe"]["version"], 1);
    }
}
