use std::collections::{HashMap, HashSet, VecDeque};

use cortex_core::task::{WorkKind, WorkRecipeSeed};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// --- Run & Step types ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Pending,
    Planning,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl RunStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Planning => "planning",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "planning" => Some(Self::Planning),
            "running" => Some(Self::Running),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// Where a step is in its life.
///
/// The rule this enum exists to enforce: **a worker's report of success is a
/// diagnostic, not a transition.** A worker delivering a commit produces
/// `Delivered`. Only Cortex's own runner, executing the checks frozen at
/// dispatch, produces `Verified`. `Succeeded` is deliberately absent — it used
/// to mean both of those at once, which is how the worker's opinion became the
/// product's truth. See `docs/adr/ADR-0001-step-truth-model.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Ready,
    Leased,
    Running,
    /// The sandbox produced a commit. Nothing has been checked yet.
    Delivered,
    /// Our runner is executing the frozen checks against the delivered commit.
    Verifying,
    /// We executed the required checks and they passed. The only success.
    Verified,
    /// We executed the required checks and a required one did not pass.
    Failed,
    /// We could not obtain a verdict. Our failure, not the customer's, and
    /// never silently resolved into success or failure.
    Inconclusive,
    /// The delivery never happened: the sandbox or the harness failed before
    /// the step could produce a commit. Distinct from `Failed`, which is a
    /// judgement about work that exists.
    ExecutionFailed,
    /// A human decided to accept the step without a passing verdict. A real
    /// fact, and not the same fact as `Verified`.
    ManualOverride,
    Recovered,
    Cancelled,
    Orphaned,
    Skipped,
}

impl StepStatus {
    /// Terminal means "this step will not move again on its own".
    ///
    /// `Delivered` and `Verifying` are explicitly not terminal: work that has
    /// been handed over but not checked is not finished. `Inconclusive` is not
    /// terminal either — it is waiting for an operator or for a retry policy,
    /// and letting it settle would make an unanswered question look answered.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Verified
                | Self::Failed
                | Self::ExecutionFailed
                | Self::ManualOverride
                | Self::Recovered
                | Self::Cancelled
                | Self::Skipped
        )
    }

    /// Whether the step reached an outcome that downstream work may build on.
    ///
    /// `ManualOverride` counts because a human took responsibility for it.
    /// Nothing else does.
    pub fn is_accepted(&self) -> bool {
        matches!(self, Self::Verified | Self::ManualOverride)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Leased => "leased",
            Self::Running => "running",
            Self::Delivered => "delivered",
            Self::Verifying => "verifying",
            Self::Verified => "verified",
            Self::Failed => "failed",
            Self::Inconclusive => "inconclusive",
            Self::ExecutionFailed => "execution_failed",
            Self::ManualOverride => "manual_override",
            Self::Recovered => "recovered",
            Self::Cancelled => "cancelled",
            Self::Orphaned => "orphaned",
            Self::Skipped => "skipped",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "ready" => Some(Self::Ready),
            "leased" => Some(Self::Leased),
            "running" => Some(Self::Running),
            "delivered" => Some(Self::Delivered),
            "verifying" => Some(Self::Verifying),
            "verified" => Some(Self::Verified),
            "failed" => Some(Self::Failed),
            "inconclusive" => Some(Self::Inconclusive),
            "execution_failed" => Some(Self::ExecutionFailed),
            "manual_override" => Some(Self::ManualOverride),
            "recovered" => Some(Self::Recovered),
            "cancelled" => Some(Self::Cancelled),
            "orphaned" => Some(Self::Orphaned),
            "skipped" => Some(Self::Skipped),
            // Deliberately not accepted. Rows carrying it are rewritten to
            // `delivered` by the truth-model migration, and a live producer of
            // it is a bug that should surface as an unparseable status rather
            // than as a silent success.
            "succeeded" => None,
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Search,
    Execute,
    Think,
    Test,
    Build,
    Lint,
    Heal,
    Review,
    Gate,
}

impl StepKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Execute => "execute",
            Self::Think => "think",
            Self::Test => "test",
            Self::Build => "build",
            Self::Lint => "lint",
            Self::Heal => "heal",
            Self::Review => "review",
            Self::Gate => "gate",
        }
    }

    pub fn lease_duration_ms(&self) -> i64 {
        match self {
            Self::Search => 2 * 60 * 1000,
            Self::Execute => 10 * 60 * 1000,
            Self::Think | Self::Review => 15 * 60 * 1000,
            Self::Test | Self::Build | Self::Lint => 10 * 60 * 1000,
            Self::Heal => 10 * 60 * 1000,
            Self::Gate => 5 * 60 * 1000,
        }
    }

    pub fn is_healable(&self) -> bool {
        matches!(self, Self::Execute | Self::Test | Self::Build | Self::Lint)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeType {
    SuccessRequired,
    CompletionRequired,
}

impl EdgeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SuccessRequired => "success_required",
            Self::CompletionRequired => "completion_required",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "success_required" => Some(Self::SuccessRequired),
            "completion_required" => Some(Self::CompletionRequired),
            _ => None,
        }
    }

    /// A `success_required` edge is satisfied only by an *accepted* dependency
    /// — one we verified, or one a human explicitly took responsibility for.
    ///
    /// `Delivered` does not satisfy it, even for a dependent that only reads.
    /// The scheduler cannot currently establish that a dependent does not
    /// write, and unknown scope is never optimistically unblocked.
    pub fn is_satisfied(&self, dep_status: StepStatus) -> bool {
        match self {
            Self::SuccessRequired => dep_status.is_accepted(),
            Self::CompletionRequired => dep_status.is_terminal(),
        }
    }
}

// --- Scheduler events ---

#[derive(Debug, Clone)]
pub enum SchedulerEvent {
    RunCreated {
        run_id: String,
    },
    /// A worker handed back a commit. **Not** a success: the step is
    /// `delivered` and nothing has been checked yet.
    StepDelivered {
        run_id: String,
        step_id: String,
        cost_estimate: Option<f64>,
    },
    StepFailed {
        run_id: String,
        step_id: String,
    },
    WorkerConnected {
        worker_id: String,
    },
    WorkerDisconnected {
        worker_id: String,
    },
    ProviderAuthExpired {
        worker_id: String,
        provider: String,
        user_id: String,
    },
    Reconcile,
}

// --- Scheduler state ---

#[derive(Debug)]
pub struct UserQueue {
    pub ready_steps: VecDeque<StepRef>,
    pub running_count: usize,
    pub max_concurrent: usize,
}

impl Default for UserQueue {
    fn default() -> Self {
        Self {
            ready_steps: VecDeque::new(),
            running_count: 0,
            max_concurrent: 5,
        }
    }
}

impl UserQueue {
    pub fn has_capacity(&self) -> bool {
        self.running_count < self.max_concurrent
    }
}

#[derive(Debug, Clone)]
pub struct StepRef {
    pub step_id: String,
    pub run_id: String,
    pub user_id: String,
    pub kind: StepKind,
    pub work_kind: Option<WorkKind>,
    pub tier: String,
    pub risk: String,
    pub objective: String,
}

pub struct SchedulerState {
    pub user_queues: HashMap<String, UserQueue>,
    pub schedulable_users: VecDeque<String>,
    enqueued_step_ids: HashSet<String>,
}

impl Default for SchedulerState {
    fn default() -> Self {
        Self::new()
    }
}

impl SchedulerState {
    pub fn new() -> Self {
        Self {
            user_queues: HashMap::new(),
            schedulable_users: VecDeque::new(),
            enqueued_step_ids: HashSet::new(),
        }
    }

    pub fn enqueue_ready_step(&mut self, step: StepRef) {
        // Deduplicate: skip if this step is already enqueued
        if !self.enqueued_step_ids.insert(step.step_id.clone()) {
            return;
        }

        let user_id = step.user_id.clone();
        let queue = self.user_queues.entry(user_id.clone()).or_default();
        queue.ready_steps.push_back(step);

        if !self.schedulable_users.contains(&user_id) {
            self.schedulable_users.push_back(user_id);
        }
    }

    pub fn next_assignable(&mut self) -> Option<StepRef> {
        let mut attempts = self.schedulable_users.len();
        while attempts > 0 {
            if let Some(user_id) = self.schedulable_users.pop_front() {
                let queue = self.user_queues.get_mut(&user_id);
                if let Some(q) = queue {
                    if q.has_capacity() {
                        if let Some(step) = q.ready_steps.pop_front() {
                            self.enqueued_step_ids.remove(&step.step_id);
                            q.running_count += 1;
                            if !q.ready_steps.is_empty() {
                                self.schedulable_users.push_back(user_id);
                            }
                            return Some(step);
                        }
                    } else {
                        self.schedulable_users.push_back(user_id);
                    }
                }
            }
            attempts -= 1;
        }
        None
    }

    pub fn mark_step_done(&mut self, user_id: &str) {
        if let Some(q) = self.user_queues.get_mut(user_id) {
            q.running_count = q.running_count.saturating_sub(1);

            if !q.ready_steps.is_empty()
                && q.has_capacity()
                && !self.schedulable_users.contains(&user_id.to_string())
            {
                self.schedulable_users.push_back(user_id.to_string());
            }
        }
    }
}

// --- Run builder (creates DAG from decomposed steps) ---

pub struct RunBuilder {
    pub run_id: String,
    pub user_id: String,
    pub goal: String,
    pub profile: String,
    steps: Vec<StepDef>,
    edges: Vec<(usize, usize, EdgeType)>,
}

pub struct StepDef {
    pub id: String,
    pub kind: StepKind,
    pub work_kind: WorkKind,
    pub tier: String,
    pub risk: String,
    pub objective: String,
    pub recipe_seed: Option<WorkRecipeSeed>,
    pub required_provider: Option<String>,
    pub max_attempts: u32,
}

impl RunBuilder {
    pub fn new(user_id: String, goal: String, profile: String) -> Self {
        Self {
            run_id: Uuid::new_v4().to_string(),
            user_id,
            goal,
            profile,
            steps: Vec::new(),
            edges: Vec::new(),
        }
    }

    pub fn add_step(&mut self, kind: StepKind, tier: &str, risk: &str, objective: &str) -> usize {
        self.add_step_with_work_kind(
            kind,
            default_work_kind_for_step(kind),
            tier,
            risk,
            objective,
        )
    }

    pub fn add_step_with_work_kind(
        &mut self,
        kind: StepKind,
        work_kind: WorkKind,
        tier: &str,
        risk: &str,
        objective: &str,
    ) -> usize {
        let idx = self.steps.len();
        self.steps.push(StepDef {
            id: Uuid::new_v4().to_string(),
            kind,
            work_kind,
            tier: tier.to_string(),
            risk: risk.to_string(),
            objective: objective.to_string(),
            recipe_seed: None,
            required_provider: None,
            max_attempts: 3,
        });
        idx
    }

    pub fn set_step_recipe_seed(&mut self, idx: usize, recipe_seed: WorkRecipeSeed) {
        if let Some(step) = self.steps.get_mut(idx) {
            step.recipe_seed = Some(recipe_seed);
        }
    }

    pub fn add_edge(&mut self, from: usize, to: usize, edge_type: EdgeType) {
        self.edges.push((from, to, edge_type));
    }

    pub fn add_sequential(&mut self, from: usize, to: usize) {
        self.add_edge(from, to, EdgeType::SuccessRequired);
    }

    pub fn steps(&self) -> &[StepDef] {
        &self.steps
    }

    pub fn edges(&self) -> &[(usize, usize, EdgeType)] {
        &self.edges
    }

    pub fn step_id(&self, idx: usize) -> &str {
        &self.steps[idx].id
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.steps.is_empty() {
            return Err("run has no steps".into());
        }

        for &(from, to, _) in &self.edges {
            if from >= self.steps.len() || to >= self.steps.len() {
                return Err(format!(
                    "edge references invalid step index: {from} -> {to}"
                ));
            }
            if from == to {
                return Err(format!("self-referencing edge at step {from}"));
            }
        }

        if self.has_cycle() {
            return Err("dependency cycle detected".into());
        }

        Ok(())
    }

    fn has_cycle(&self) -> bool {
        let n = self.steps.len();
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut in_degree = vec![0usize; n];

        for &(from, to, _) in &self.edges {
            adj[from].push(to);
            in_degree[to] += 1;
        }

        let mut queue: VecDeque<usize> = in_degree
            .iter()
            .enumerate()
            .filter(|&(_, d)| *d == 0)
            .map(|(i, _)| i)
            .collect();

        let mut visited = 0;
        while let Some(node) = queue.pop_front() {
            visited += 1;
            for &next in &adj[node] {
                in_degree[next] -= 1;
                if in_degree[next] == 0 {
                    queue.push_back(next);
                }
            }
        }

        visited != n
    }
}

fn default_work_kind_for_step(kind: StepKind) -> WorkKind {
    match kind {
        StepKind::Search | StepKind::Think => WorkKind::Explore,
        StepKind::Execute => WorkKind::Modify,
        StepKind::Test => WorkKind::Test,
        StepKind::Build => WorkKind::Build,
        StepKind::Lint => WorkKind::Lint,
        StepKind::Heal => WorkKind::Heal,
        StepKind::Review => WorkKind::Review,
        StepKind::Gate => WorkKind::Gate,
    }
}

// --- Heal insertion ---

pub struct HealPlan {
    pub heal_step_id: String,
    pub retry_step_id: String,
    pub heal_objective: String,
    pub retry_objective: String,
    pub failed_step_id: String,
    pub original_kind: StepKind,
    pub tier: String,
    pub risk: String,
}

pub fn plan_heal(
    failed_step_id: &str,
    original_kind: StepKind,
    original_objective: &str,
    tier: &str,
    risk: &str,
    last_error: Option<&str>,
    heal_attempt: u32,
) -> HealPlan {
    let error_context = last_error
        .map(|e| format!(" Error: {e}"))
        .unwrap_or_default();

    HealPlan {
        heal_step_id: Uuid::new_v4().to_string(),
        retry_step_id: Uuid::new_v4().to_string(),
        heal_objective: format!(
            "Fix the failure in step {failed_step_id} (attempt {heal_attempt}).{error_context}"
        ),
        retry_objective: format!("Retry: {original_objective}"),
        failed_step_id: failed_step_id.to_string(),
        original_kind,
        tier: tier.to_string(),
        risk: risk.to_string(),
    }
}

// --- Run completion check ---

/// Decide whether a run has finished, and how.
///
/// A run cannot report success on delivered-but-unverified work: `Delivered`,
/// `Verifying`, and `Inconclusive` are not terminal, so an unverified run is
/// simply not done yet. That is the point — the old model had no way to
/// express "every step delivered, none of them checked".
pub fn check_run_completion(step_statuses: &[(String, StepStatus)]) -> Option<RunStatus> {
    if step_statuses.is_empty() {
        return None;
    }

    let all_terminal = step_statuses.iter().all(|(_, s)| s.is_terminal());
    if !all_terminal {
        return None;
    }

    let any_failed = step_statuses.iter().any(|(_, s)| {
        matches!(
            s,
            StepStatus::Failed | StepStatus::ExecutionFailed | StepStatus::Skipped
        )
    });
    let any_cancelled = step_statuses
        .iter()
        .any(|(_, s)| *s == StepStatus::Cancelled);

    if any_failed {
        Some(RunStatus::Failed)
    } else if any_cancelled {
        Some(RunStatus::Cancelled)
    } else {
        Some(RunStatus::Succeeded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_type_satisfaction() {
        assert!(EdgeType::SuccessRequired.is_satisfied(StepStatus::Verified));
        assert!(!EdgeType::SuccessRequired.is_satisfied(StepStatus::Failed));
        assert!(!EdgeType::SuccessRequired.is_satisfied(StepStatus::Running));

        assert!(EdgeType::CompletionRequired.is_satisfied(StepStatus::Verified));
        assert!(EdgeType::CompletionRequired.is_satisfied(StepStatus::Failed));
        assert!(EdgeType::CompletionRequired.is_satisfied(StepStatus::Cancelled));
        assert!(!EdgeType::CompletionRequired.is_satisfied(StepStatus::Running));
        assert!(!EdgeType::CompletionRequired.is_satisfied(StepStatus::Pending));
    }

    #[test]
    fn truth_delivered_work_satisfies_no_dependency_edge() {
        // The regression this whole state machine exists to prevent: a worker
        // says it is done, and the next step starts writing on top of a tree
        // nobody checked.
        for status in [StepStatus::Delivered, StepStatus::Verifying] {
            assert!(
                !EdgeType::SuccessRequired.is_satisfied(status),
                "{} must not unblock a dependent",
                status.as_str()
            );
            assert!(
                !EdgeType::CompletionRequired.is_satisfied(status),
                "{} is not a completed step",
                status.as_str()
            );
            assert!(!status.is_terminal(), "{} is not terminal", status.as_str());
            assert!(
                !status.is_accepted(),
                "{} is not an accepted outcome",
                status.as_str()
            );
        }
    }

    #[test]
    fn truth_inconclusive_is_neither_done_nor_accepted() {
        // Inconclusive means we failed to obtain a verdict. Settling it either
        // way charges the customer for our outage or blames them for it.
        assert!(!StepStatus::Inconclusive.is_terminal());
        assert!(!StepStatus::Inconclusive.is_accepted());
        assert!(!EdgeType::SuccessRequired.is_satisfied(StepStatus::Inconclusive));
        assert!(!EdgeType::CompletionRequired.is_satisfied(StepStatus::Inconclusive));
    }

    #[test]
    fn truth_manual_override_is_accepted_but_is_not_verified() {
        // A human taking responsibility unblocks work. It is still a different
        // fact from a passing verdict, and must never render as one.
        assert!(StepStatus::ManualOverride.is_accepted());
        assert!(StepStatus::ManualOverride.is_terminal());
        assert_ne!(
            StepStatus::ManualOverride.as_str(),
            StepStatus::Verified.as_str()
        );
    }

    #[test]
    fn truth_succeeded_no_longer_parses() {
        // `succeeded` meant "delivered" and "verified" at once, which is how a
        // worker's opinion became the product's truth. A live producer of it
        // must surface as an unparseable status, not as a silent success.
        assert_eq!(StepStatus::from_str("succeeded"), None);
        assert_eq!(
            StepStatus::from_str("delivered"),
            Some(StepStatus::Delivered)
        );
        assert_eq!(StepStatus::from_str("verified"), Some(StepStatus::Verified));
    }

    #[test]
    fn truth_every_status_round_trips_through_its_string() {
        for status in [
            StepStatus::Pending,
            StepStatus::Ready,
            StepStatus::Leased,
            StepStatus::Running,
            StepStatus::Delivered,
            StepStatus::Verifying,
            StepStatus::Verified,
            StepStatus::Failed,
            StepStatus::Inconclusive,
            StepStatus::ExecutionFailed,
            StepStatus::ManualOverride,
            StepStatus::Recovered,
            StepStatus::Cancelled,
            StepStatus::Orphaned,
            StepStatus::Skipped,
        ] {
            assert_eq!(
                StepStatus::from_str(status.as_str()),
                Some(status),
                "{} does not survive a round trip through the database",
                status.as_str()
            );
        }
    }

    #[test]
    fn truth_a_run_of_delivered_steps_is_not_a_succeeded_run() {
        let delivered = vec![
            ("s1".to_string(), StepStatus::Delivered),
            ("s2".to_string(), StepStatus::Verified),
        ];
        assert_eq!(
            check_run_completion(&delivered),
            None,
            "a run with an unverified step has not finished"
        );

        let verifying = vec![("s1".to_string(), StepStatus::Verifying)];
        assert_eq!(check_run_completion(&verifying), None);

        let inconclusive = vec![("s1".to_string(), StepStatus::Inconclusive)];
        assert_eq!(
            check_run_completion(&inconclusive),
            None,
            "an unanswered question must not settle as a finished run"
        );

        let verified = vec![
            ("s1".to_string(), StepStatus::Verified),
            ("s2".to_string(), StepStatus::Verified),
        ];
        assert_eq!(check_run_completion(&verified), Some(RunStatus::Succeeded));
    }

    #[test]
    fn truth_execution_failure_fails_the_run() {
        // The sandbox never produced a tree. That is a failed run, not a
        // succeeded one with a missing step.
        let statuses = vec![
            ("s1".to_string(), StepStatus::Verified),
            ("s2".to_string(), StepStatus::ExecutionFailed),
        ];
        assert_eq!(check_run_completion(&statuses), Some(RunStatus::Failed));
    }

    #[test]
    fn run_builder_linear_dag() {
        let mut builder = RunBuilder::new("u1".into(), "fix and test".into(), "auto".into());

        let edit = builder.add_step(StepKind::Execute, "execute", "medium", "fix the bug");
        let test = builder.add_step(StepKind::Test, "execute", "medium", "run tests");
        builder.add_sequential(edit, test);

        assert!(builder.validate().is_ok());
        assert_eq!(builder.steps().len(), 2);
        assert_eq!(builder.edges().len(), 1);
    }

    #[test]
    fn run_builder_diamond_dag() {
        let mut builder = RunBuilder::new("u1".into(), "parallel work".into(), "auto".into());

        let start = builder.add_step(StepKind::Search, "search", "low", "explore codebase");
        let fix_a = builder.add_step(StepKind::Execute, "execute", "medium", "fix module A");
        let fix_b = builder.add_step(StepKind::Execute, "execute", "medium", "fix module B");
        let test = builder.add_step(StepKind::Test, "execute", "medium", "run all tests");

        builder.add_sequential(start, fix_a);
        builder.add_sequential(start, fix_b);
        builder.add_sequential(fix_a, test);
        builder.add_sequential(fix_b, test);

        assert!(builder.validate().is_ok());
    }

    #[test]
    fn run_builder_detects_cycle() {
        let mut builder = RunBuilder::new("u1".into(), "bad dag".into(), "auto".into());

        let a = builder.add_step(StepKind::Execute, "execute", "low", "step a");
        let b = builder.add_step(StepKind::Execute, "execute", "low", "step b");
        let c = builder.add_step(StepKind::Execute, "execute", "low", "step c");

        builder.add_sequential(a, b);
        builder.add_sequential(b, c);
        builder.add_sequential(c, a);

        assert!(builder.validate().is_err());
    }

    #[test]
    fn run_builder_detects_self_edge() {
        let mut builder = RunBuilder::new("u1".into(), "self ref".into(), "auto".into());
        let a = builder.add_step(StepKind::Execute, "execute", "low", "step a");
        builder.add_sequential(a, a);

        assert!(builder.validate().is_err());
    }

    #[test]
    fn run_builder_empty_is_invalid() {
        let builder = RunBuilder::new("u1".into(), "empty".into(), "auto".into());
        assert!(builder.validate().is_err());
    }

    #[test]
    fn scheduler_round_robin() {
        let mut state = SchedulerState::new();

        // User A has 3 ready steps, User B has 1
        for i in 0..3 {
            state.enqueue_ready_step(StepRef {
                step_id: format!("a-{i}"),
                run_id: "r1".into(),
                user_id: "user-a".into(),
                kind: StepKind::Execute,
                work_kind: Some(WorkKind::Modify),
                tier: "execute".into(),
                risk: "low".into(),
                objective: format!("task a-{i}"),
            });
        }
        state.enqueue_ready_step(StepRef {
            step_id: "b-0".into(),
            run_id: "r2".into(),
            user_id: "user-b".into(),
            kind: StepKind::Execute,
            work_kind: Some(WorkKind::Modify),
            tier: "execute".into(),
            risk: "low".into(),
            objective: "task b-0".into(),
        });

        let first = state.next_assignable().unwrap();
        let second = state.next_assignable().unwrap();

        // Round-robin: first from user-a, then user-b
        assert_eq!(first.user_id, "user-a");
        assert_eq!(second.user_id, "user-b");
    }

    #[test]
    fn scheduler_respects_concurrency_limit() {
        let mut state = SchedulerState::new();

        // Set max_concurrent to 1 for user-a
        state
            .user_queues
            .entry("user-a".into())
            .or_default()
            .max_concurrent = 1;

        for i in 0..3 {
            state.enqueue_ready_step(StepRef {
                step_id: format!("a-{i}"),
                run_id: "r1".into(),
                user_id: "user-a".into(),
                kind: StepKind::Execute,
                work_kind: Some(WorkKind::Modify),
                tier: "execute".into(),
                risk: "low".into(),
                objective: format!("task {i}"),
            });
        }

        let first = state.next_assignable().unwrap();
        assert_eq!(first.step_id, "a-0");

        // Second call should return None (at capacity)
        assert!(state.next_assignable().is_none());

        // Mark done, should be able to get next
        state.mark_step_done("user-a");
        let next = state.next_assignable().unwrap();
        assert_eq!(next.step_id, "a-1");
    }

    #[test]
    fn check_run_all_verified() {
        let steps = vec![
            ("s1".into(), StepStatus::Verified),
            ("s2".into(), StepStatus::Verified),
        ];
        assert_eq!(check_run_completion(&steps), Some(RunStatus::Succeeded));
    }

    #[test]
    fn check_run_any_failed() {
        let steps = vec![
            ("s1".into(), StepStatus::Verified),
            ("s2".into(), StepStatus::Failed),
        ];
        assert_eq!(check_run_completion(&steps), Some(RunStatus::Failed));
    }

    #[test]
    fn check_run_recovered_is_terminal_but_not_failed() {
        let steps = vec![
            ("s1".into(), StepStatus::Recovered),
            ("s2".into(), StepStatus::Verified),
        ];
        assert_eq!(check_run_completion(&steps), Some(RunStatus::Succeeded));
    }

    #[test]
    fn check_run_any_cancelled() {
        let steps = vec![
            ("s1".into(), StepStatus::Verified),
            ("s2".into(), StepStatus::Cancelled),
        ];
        assert_eq!(check_run_completion(&steps), Some(RunStatus::Cancelled));
    }

    #[test]
    fn check_run_still_running() {
        let steps = vec![
            ("s1".into(), StepStatus::Verified),
            ("s2".into(), StepStatus::Running),
        ];
        assert_eq!(check_run_completion(&steps), None);
    }

    #[test]
    fn heal_plan_creates_valid_ids() {
        let plan = plan_heal(
            "step-123",
            StepKind::Test,
            "run tests",
            "execute",
            "medium",
            Some("assertion failed: expected 4, got 3"),
            1,
        );

        assert_ne!(plan.heal_step_id, plan.retry_step_id);
        assert!(plan.heal_objective.contains("step-123"));
        assert!(plan.heal_objective.contains("assertion failed"));
        assert!(plan.retry_objective.contains("run tests"));
    }

    #[test]
    fn step_kind_lease_durations() {
        assert_eq!(StepKind::Search.lease_duration_ms(), 120_000);
        assert_eq!(StepKind::Execute.lease_duration_ms(), 600_000);
        assert_eq!(StepKind::Think.lease_duration_ms(), 900_000);
    }

    #[test]
    fn step_kind_healable() {
        assert!(StepKind::Execute.is_healable());
        assert!(StepKind::Test.is_healable());
        assert!(StepKind::Build.is_healable());
        assert!(StepKind::Lint.is_healable());
        assert!(!StepKind::Think.is_healable());
        assert!(!StepKind::Heal.is_healable());
    }

    #[test]
    fn heal_step_depends_on_failed_via_completion_required() {
        // Verify the heal workflow edge types
        // heal step depends on failed step with completion_required
        // retry step depends on heal step with success_required
        let mut builder = RunBuilder::new("u1".into(), "fix and test".into(), "auto".into());

        let edit = builder.add_step(StepKind::Execute, "execute", "medium", "fix bug");
        let test = builder.add_step(StepKind::Test, "execute", "medium", "run tests");
        builder.add_sequential(edit, test);

        // Simulate test failure → insert heal chain
        let heal = builder.add_step(StepKind::Heal, "execute", "medium", "fix the failure");
        let retry = builder.add_step(StepKind::Test, "execute", "medium", "retry tests");

        // heal depends on test being terminal (completion_required)
        builder.add_edge(test, heal, EdgeType::CompletionRequired);
        // retry depends on heal succeeding
        builder.add_sequential(heal, retry);

        assert!(builder.validate().is_ok());

        // Verify: heal's dependency on test is satisfied when test fails
        assert!(EdgeType::CompletionRequired.is_satisfied(StepStatus::Failed));
        // Verify: retry's dependency on heal requires success
        assert!(!EdgeType::SuccessRequired.is_satisfied(StepStatus::Failed));
        assert!(EdgeType::SuccessRequired.is_satisfied(StepStatus::Verified));
    }
}
