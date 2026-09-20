//! The versioned job handed to a sandbox runner, and the typed refusal it can
//! return instead of running.
//!
//! `ExecutionJob` is the narrowest waist in the system: routing, pricing,
//! receipts, and the router's feedback all join on it. Re-cutting it later is
//! expensive, so it ships **complete** — including the fields nothing reads
//! yet. A field that is not populated is `None`; it is never absent.
//!
//! Two rules are load-bearing and are enforced by the types rather than by
//! convention:
//!
//! - **The backend reports what it actually applied.** [`EffortApplication`] is
//!   not derived from the request; it is written by whichever backend ran, so a
//!   CLI that cannot honour an effort level says [`EffortApplication::Unsupported`]
//!   rather than silently swallowing it. A receipt never claims a setting the
//!   backend did not apply.
//! - **A job that cannot be run safely produces a [`Blocked`], not a fallback.**
//!   There is no path in this module that degrades to a less isolated execution.
//!
//! Effort is fixed for the lifetime of an attempt. Two independent constraints
//! agree on this: effort shapes the rendered prompt, so varying it mid-session
//! discards the provider's cached prefix; and escalation is required to retry
//! with fresh context anyway. Escalation therefore creates a *new attempt* and
//! never mutates a running job.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Bumped on any breaking change to the field set. Recorded on every persisted
/// job so an old row is still interpretable after the struct moves.
pub const EXECUTION_JOB_VERSION: u32 = 1;

/// Which model actually ran, and which catalog said so.
///
/// `catalog_version` is `None` until the versioned model catalog exists. The
/// field is present from the first version because provenance that is added
/// later cannot describe work that already ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRef {
    pub catalog_id: String,
    pub catalog_version: Option<String>,
}

impl ModelRef {
    /// A model identity with no catalog behind it yet.
    pub fn uncatalogued(catalog_id: impl Into<String>) -> Self {
        Self {
            catalog_id: catalog_id.into(),
            catalog_version: None,
        }
    }
}

/// How the provider was reached. Recorded because the two paths have different
/// capabilities — notably, a CLI may not expose an effort control at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    Cli,
    Http,
}

impl BackendKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Http => "http",
        }
    }
}

/// The five effort positions, deliberately borrowing the vocabulary developers
/// already know so the control needs no explanation.
///
/// A position does not map to a model. It selects a policy row, and the policy
/// resolves a point in the capability/reasoning/workflow/attempts space *per
/// task class*. That resolution is not this module's job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffortLevel {
    Low,
    Medium,
    High,
    XHigh,
    Ultra,
}

impl EffortLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Ultra => "ultra",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::XHigh),
            "ultra" => Some(Self::Ultra),
            _ => None,
        }
    }
}

/// What the backend did with the requested effort — written by the backend,
/// never inferred from the request.
///
/// The existence of [`Unsupported`](Self::Unsupported) is the point. A CLI
/// backend invoked with model flags only cannot honour an effort request, and
/// the failure mode this type prevents is that backend accepting the request
/// and quietly ignoring it while the receipt claims it was applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum EffortApplication {
    /// No effort level was requested, so there is nothing to honour.
    NotRequested,
    /// The backend applied exactly what was asked for.
    Applied { level: EffortLevel },
    /// The backend applied a lower level than requested. Lowering effort is
    /// permitted without consent and must be recorded; raising it is not.
    Downgraded {
        requested: EffortLevel,
        applied: EffortLevel,
    },
    /// The backend has no effort control. The work may still run; the receipt
    /// must not claim the level was applied.
    Unsupported { requested: EffortLevel },
}

impl EffortApplication {
    /// The level a receipt may honestly claim, which is `None` unless a level
    /// was actually applied.
    pub fn applied_level(&self) -> Option<EffortLevel> {
        match self {
            Self::Applied { level } => Some(*level),
            Self::Downgraded { applied, .. } => Some(*applied),
            Self::NotRequested | Self::Unsupported { .. } => None,
        }
    }

    /// True when the backend did not deliver what was asked. Surfaced on the
    /// receipt rather than logged and forgotten.
    pub fn is_honoured(&self) -> bool {
        matches!(self, Self::NotRequested | Self::Applied { .. })
    }
}

/// Egress policy for the sandbox. The default is [`Deny`](Self::Deny), and the
/// default is what matters: a policy that is permissive when unset is not a
/// policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
#[derive(Default)]
pub enum NetworkPolicy {
    /// No egress at all.
    #[default]
    Deny,
    /// Egress to exactly these hosts, and nothing else. A dependency
    /// resolution grant names a package registry, and exactly a registry.
    Allowlist { hosts: Vec<String> },
}

impl NetworkPolicy {
    pub fn is_deny(&self) -> bool {
        matches!(self, Self::Deny)
    }

    /// The hosts this policy permits. Empty for [`Deny`](Self::Deny), which is
    /// why an empty allowlist and a denial are the same thing at the boundary.
    pub fn allowed_hosts(&self) -> &[String] {
        match self {
            Self::Deny => &[],
            Self::Allowlist { hosts } => hosts,
        }
    }
}

/// A capability the task was granted, enforceable at the sandbox edge rather
/// than only checked in the API.
///
/// Grants are issued by the planning path; this crate carries and enforces
/// them. An empty grant list means the sandbox permits nothing beyond reading
/// and writing its own workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "capability")]
pub enum CapabilityGrant {
    /// Resolve dependencies from the named registries. Implies the
    /// corresponding allowlist entries and nothing more.
    ResolveDependencies { registries: Vec<String> },
    /// Reach the named model provider's API. Implies exactly that provider's
    /// host and nothing else — not a provider list, not a wildcard.
    ///
    /// A separate variant rather than another `ResolveDependencies` registry,
    /// because the two answer different questions and are derived from
    /// different inputs: a registry grant comes from the repository's
    /// manifests, and this one comes from the routing decision. Collapsing
    /// them into one list of names would let a manifest in a customer's
    /// repository name a provider.
    ///
    /// It does not authorize a supplier credential to enter the sandbox. The
    /// private gateway owns that credential; this grant only states which
    /// provider a later per-attempt gateway capability may name.
    ReachProvider { provider: String },
    /// Read a named secret. The secret is delivered by the runner, never by
    /// the sandbox environment.
    ReadSecret { secret_id: String },
}

/// Bounded resources for one sandbox. Every field has a value; there is no
/// "unlimited" representation, because an unbounded sandbox is the failure
/// this type exists to prevent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceProfile {
    /// Identifies the tuning of these numbers. Recorded on the job so a
    /// receipt can state which profile actually ran.
    pub profile_version: String,
    pub memory_bytes: i64,
    pub cpu_quota: i64,
    pub cpu_period: i64,
    pub pids_limit: i64,
    pub disk_bytes: i64,
}

impl Default for ResourceProfile {
    fn default() -> Self {
        Self {
            profile_version: "rp-1".to_string(),
            memory_bytes: 4 * 1024 * 1024 * 1024,
            // One full CPU: quota / period.
            cpu_quota: 100_000,
            cpu_period: 100_000,
            // Enough for a toolchain spawning workers, far short of a fork bomb.
            pids_limit: 512,
            disk_bytes: 8 * 1024 * 1024 * 1024,
        }
    }
}

/// What kind of boundary actually ran.
///
/// A shared-kernel container is a resource boundary and a convenience
/// boundary; against a kernel exploit it is not a security boundary. The
/// requirement is kernel-level isolation, and this field exists so a receipt
/// states which class was in force rather than leaving a reader to assume the
/// stronger one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationClass {
    Container,
    MicroVm,
}

impl IsolationClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Container => "container",
            Self::MicroVm => "micro_vm",
        }
    }
}

/// Reference to the packed context bundle the attempt was given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleRef {
    pub bundle_id: String,
    /// Recorded per attempt so context growth is measurable rather than
    /// anecdotal.
    pub packed_bytes: Option<u64>,
    /// What the bundle was made of, by provenance.
    ///
    /// The question this answers, and which a packed prompt cannot answer after
    /// the fact: how much of what this step was told came from the repository
    /// it was pointed at? Also carries any directive found in observed content,
    /// so a receipt can show that a repository tried to give the step orders.
    ///
    /// `Option`, and absent rather than empty when there is no record —
    /// present-but-empty would claim "nothing was composed", which is a
    /// different fact from "this attempt predates the record".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composition: Option<crate::provenance::ContextComposition>,
}

/// Cost containment, enforced at the sandbox boundary rather than trusted to
/// the agent.
///
/// `wall_clock` is deliberately not optional: a job with no time bound can
/// consume a host indefinitely, and that must not be representable. The token
/// and tool-call bounds are `None` until the estimator exists, and `None` means
/// *no cap is enforced* — which is logged, not silently treated as acceptable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budgets {
    pub token_budget: Option<u64>,
    pub wall_clock: Duration,
    pub max_tool_calls: Option<u32>,
}

impl Budgets {
    /// The default ceiling applied when nothing has quoted this attempt yet.
    pub const DEFAULT_WALL_CLOCK: Duration = Duration::from_secs(30 * 60);

    pub fn unquoted() -> Self {
        Self {
            token_budget: None,
            wall_clock: Self::DEFAULT_WALL_CLOCK,
            max_tool_calls: None,
        }
    }

    /// True when a bound exists only on wall clock. Callers log this rather
    /// than treating an unquoted job as though it had been priced.
    pub fn is_unquoted(&self) -> bool {
        self.token_budget.is_none() && self.max_tool_calls.is_none()
    }
}

/// One provider invocation, fully described.
///
/// Everything downstream — receipt, ledger, router fact — joins on
/// `quote_id`, `plan_receipt_id`, `attempt_id`, and `lease_gen`, which is why
/// they are present from the first version even while three of them are unset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionJob {
    pub job_id: String,
    pub job_version: u32,

    pub run_id: String,
    pub step_id: String,
    pub attempt_id: String,
    pub lease_gen: i64,

    pub model_ref: ModelRef,
    pub backend_kind: BackendKind,
    pub effort: Option<EffortLevel>,
    pub effort_applied: EffortApplication,

    pub budgets: Budgets,
    pub network_policy: NetworkPolicy,
    pub capability_grants: Vec<CapabilityGrant>,
    pub context_bundle: Option<BundleRef>,

    pub quote_id: Option<String>,
    pub plan_receipt_id: Option<String>,

    pub image_ref: String,
    pub isolation_class: IsolationClass,
    pub resource_profile: ResourceProfile,

    /// What was actually reachable, as `host:port`, after the allowlist was
    /// intersected with the capability grants and the registry names expanded.
    ///
    /// `network_policy` above records what was *asked for*. This records what
    /// was *enforced*, and the two can legitimately differ — an allowlist entry
    /// no grant justifies is dropped. A receipt that says "allowlist" without
    /// saying which hosts is not an answer to "what could this task reach".
    ///
    /// `Some(vec![])` means nothing was reachable. `None` means the field was
    /// not recorded, which is what a job from a worker predating scoped egress
    /// looks like — deliberately distinguishable, because a plausible default
    /// here would be a claim we did not verify.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_egress: Option<Vec<String>>,

    /// The image that enforced it, digest-pinned in production.
    ///
    /// Recorded for the same reason `image_ref` is: "it could only reach the
    /// registry" is only checkable if you know what was deciding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub egress_mediator: Option<String>,
}

impl ExecutionJob {
    /// The idempotency key for one logical execution.
    ///
    /// A resubmission under the same attempt and lease generation is the same
    /// job and must not produce a second sandbox. This mirrors the claim key
    /// already used on the dispatch path.
    pub fn idempotency_key(&self) -> (&str, i64) {
        (self.attempt_id.as_str(), self.lease_gen)
    }

    /// Record what the backend did with the effort request.
    ///
    /// Callers must invoke this even when nothing was requested, so that
    /// `effort_applied` is always a statement the backend made rather than a
    /// default nobody checked.
    pub fn record_effort_application(&mut self, application: EffortApplication) {
        self.effort_applied = application;
    }
}

/// Why a job was refused. Returned instead of running it — never alongside a
/// degraded execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockedReason {
    /// The sandbox could not be created or started.
    SandboxUnavailable,
    /// An isolated worktree could not be established. Historically this
    /// degraded to executing in the caller's directory; it no longer does.
    WorktreeUnavailable,
    /// The requested network policy could not be applied as specified.
    /// Refusing is correct: a policy that could not be applied is not a policy.
    NetworkPolicyUnenforceable,
    /// A resource bound could not be enforced by the runtime.
    ResourceLimitUnenforceable,
    /// The configured runner image is missing or unusable.
    ImageUnavailable,
    /// Teardown of a previous sandbox for this attempt did not complete, so a
    /// fresh one cannot be guaranteed.
    TeardownIncomplete,
}

impl BlockedReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SandboxUnavailable => "sandbox_unavailable",
            Self::WorktreeUnavailable => "worktree_unavailable",
            Self::NetworkPolicyUnenforceable => "network_policy_unenforceable",
            Self::ResourceLimitUnenforceable => "resource_limit_unenforceable",
            Self::ImageUnavailable => "image_unavailable",
            Self::TeardownIncomplete => "teardown_incomplete",
        }
    }
}

/// A typed refusal to execute.
///
/// This is the replacement for the previous behaviour, where a failure to
/// isolate fell through to running the agent on the host. It carries enough
/// detail for an operator to fix the cause without being a channel for the
/// agent to influence anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blocked {
    pub reason: BlockedReason,
    pub detail: String,
}

impl Blocked {
    pub fn new(reason: BlockedReason, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for Blocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "blocked ({}): {}", self.reason.as_str(), self.detail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> ExecutionJob {
        ExecutionJob {
            job_id: "job-1".to_string(),
            job_version: EXECUTION_JOB_VERSION,
            run_id: "run-1".to_string(),
            step_id: "step-1".to_string(),
            attempt_id: "attempt-1".to_string(),
            lease_gen: 7,
            model_ref: ModelRef::uncatalogued("claude-opus-5"),
            backend_kind: BackendKind::Cli,
            effort: None,
            effort_applied: EffortApplication::NotRequested,
            budgets: Budgets::unquoted(),
            network_policy: NetworkPolicy::default(),
            capability_grants: Vec::new(),
            context_bundle: None,
            quote_id: None,
            plan_receipt_id: None,
            image_ref: "cortex/runner@sha256:abc".to_string(),
            isolation_class: IsolationClass::Container,
            resource_profile: ResourceProfile::default(),
            effective_egress: Some(Vec::new()),
            egress_mediator: None,
        }
    }

    #[test]
    fn network_policy_defaults_to_deny() {
        // The default is the whole control. A permissive default would make
        // every unset job an open sandbox.
        assert_eq!(NetworkPolicy::default(), NetworkPolicy::Deny);
        assert!(NetworkPolicy::default().is_deny());
        assert!(NetworkPolicy::default().allowed_hosts().is_empty());
    }

    #[test]
    fn allowlist_permits_only_what_it_names() {
        let policy = NetworkPolicy::Allowlist {
            hosts: vec!["registry.npmjs.org".to_string()],
        };
        assert!(!policy.is_deny());
        assert_eq!(policy.allowed_hosts(), ["registry.npmjs.org"]);
    }

    #[test]
    fn unsupported_effort_is_not_claimable_as_applied() {
        // The failure this prevents: a CLI backend swallowing an effort
        // request while the receipt says it was honoured.
        let application = EffortApplication::Unsupported {
            requested: EffortLevel::Ultra,
        };
        assert_eq!(application.applied_level(), None);
        assert!(!application.is_honoured());
    }

    #[test]
    fn downgraded_effort_reports_what_ran_not_what_was_asked() {
        let application = EffortApplication::Downgraded {
            requested: EffortLevel::Ultra,
            applied: EffortLevel::High,
        };
        assert_eq!(application.applied_level(), Some(EffortLevel::High));
        assert!(!application.is_honoured());
    }

    #[test]
    fn budgets_always_bound_wall_clock() {
        // There is no representation of an unbounded job, by construction.
        let budgets = Budgets::unquoted();
        assert_eq!(budgets.wall_clock, Budgets::DEFAULT_WALL_CLOCK);
        assert!(budgets.is_unquoted());
    }

    #[test]
    fn idempotency_key_is_attempt_and_lease_gen() {
        // A resubmission under the same attempt must not create a second
        // sandbox.
        let a = job();
        let mut b = job();
        b.job_id = "job-2".to_string();
        assert_eq!(a.idempotency_key(), b.idempotency_key());

        let mut c = job();
        c.lease_gen = 8;
        assert_ne!(a.idempotency_key(), c.idempotency_key());
    }

    #[test]
    fn carries_full_field_set_through_serialization() {
        // Guards against a future refactor quietly dropping a field: every
        // field must survive a round trip, and the assertions below name each
        // one so a removal fails here rather than silently in production.
        let mut original = job();
        original.effort = Some(EffortLevel::High);
        original.record_effort_application(EffortApplication::Applied {
            level: EffortLevel::High,
        });
        original.budgets.token_budget = Some(500_000);
        original.budgets.max_tool_calls = Some(200);
        original.network_policy = NetworkPolicy::Allowlist {
            hosts: vec!["crates.io".to_string()],
        };
        original.capability_grants = vec![CapabilityGrant::ResolveDependencies {
            registries: vec!["crates.io".to_string()],
        }];
        original.context_bundle = Some(BundleRef {
            bundle_id: "bundle-1".to_string(),
            packed_bytes: Some(4096),
            composition: None,
        });
        original.quote_id = Some("quote-1".to_string());
        original.plan_receipt_id = Some("receipt-1".to_string());
        original.model_ref.catalog_version = Some("catalog-3".to_string());

        let encoded = serde_json::to_string(&original).expect("job serializes");
        let decoded: ExecutionJob = serde_json::from_str(&encoded).expect("job deserializes");

        assert_eq!(decoded, original);
        assert_eq!(decoded.job_version, EXECUTION_JOB_VERSION);
        assert_eq!(decoded.isolation_class, IsolationClass::Container);
        assert_eq!(decoded.resource_profile.profile_version, "rp-1");
        assert!(decoded.image_ref.contains("sha256:"));
    }

    #[test]
    fn blocked_reasons_are_distinguishable_on_the_wire() {
        // An operator reading a receipt needs to tell "no sandbox" from
        // "no worktree" — they have different fixes.
        assert_ne!(
            BlockedReason::SandboxUnavailable.as_str(),
            BlockedReason::WorktreeUnavailable.as_str()
        );
        let blocked = Blocked::new(BlockedReason::WorktreeUnavailable, "not a git repository");
        assert!(blocked.to_string().contains("worktree_unavailable"));
        assert!(blocked.to_string().contains("not a git repository"));
    }
}
