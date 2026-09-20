//! What a sandbox is allowed to see, reach, and consume.
//!
//! The rules here are the ones that would otherwise be spread across an
//! implementation and quietly relaxed one commit at a time.

use std::path::{Path, PathBuf};

use cortex_core::execution_job::{CapabilityGrant, ExecutionJob};
use cortex_core::protocol::ProviderGatewayAccess;

/// Where the workspace is mounted inside the sandbox. Fixed, so a command
/// built for one runtime is valid in the next one.
pub const WORKSPACE_MOUNT: &str = "/work";

/// Everything the runner needs to start a job that the job itself does not
/// describe: the tree to work in and the command to run.
///
/// The job says *what policy applies*; the request says *what to run*. Keeping
/// them apart is what stops a caller smuggling an argument past a policy.
pub struct SandboxRequest {
    /// Host path of the isolated worktree. Mounted read-write at
    /// [`WORKSPACE_MOUNT`] and nowhere else.
    pub workspace: PathBuf,
    /// Program to execute inside the sandbox.
    pub program: String,
    /// Arguments, including the task prompt.
    pub args: Vec<String>,
    /// Non-persisted bearer authority for the private model gateway.
    pub provider_gateway: Option<ProviderGatewayAccess>,
}

impl SandboxRequest {
    pub fn new(workspace: impl AsRef<Path>, program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            workspace: workspace.as_ref().to_path_buf(),
            program: program.into(),
            args,
            provider_gateway: None,
        }
    }

    pub fn with_provider_gateway(mut self, access: Option<ProviderGatewayAccess>) -> Self {
        self.provider_gateway = access;
        self
    }

    /// The full argv, as the sandbox will see it.
    pub fn argv(&self) -> Vec<String> {
        let mut argv = Vec::with_capacity(self.args.len() + 1);
        argv.push(self.program.clone());
        argv.extend(self.args.iter().cloned());
        argv
    }
}

/// The environment handed to the sandbox.
///
/// Supplier credentials are never admitted. The private gateway owns them;
/// capability issuance is not yet wired through the worker protocol, so a real
/// provider CLI currently fails closed as unauthenticated. The one exception is
/// an exact, public sentinel used by the zero-cost stub integration image. It is
/// not accepted by a supplier and exists only to keep that test executable.
///
/// # The scratch variables are not an exception to any of that
///
/// [`SCRATCH_ENV`] is appended, and it is a compile-time constant with no
/// lookup behind it, so it cannot carry a value off the host. It exists
/// because the sandbox's *only* writable path was the customer's worktree.
/// Every real CLI writes somewhere — a config directory, a cache, a temp file —
/// and with no `HOME` and no `TMPDIR` a provider CLI either fails to start or
/// writes its droppings into the tree it was asked to change. The second is
/// worse than the first: those files land in the diff, in the git evidence,
/// and in what the frozen checks grade.
///
/// So the sandbox gets a `/scratch` tmpfs and is pointed at it. The worktree
/// stays the only *persistent* writable path, which is the property that
/// mattered; scratch dies with the container.
pub fn sanctioned_env(job: &ExecutionJob) -> Vec<String> {
    let mut env = sanctioned_env_from(job, |var| std::env::var(var).ok());
    env.extend(SCRATCH_ENV.iter().map(|s| (*s).to_string()));
    env
}

/// Environment for a concrete sandbox request. Gateway bearer material is
/// admitted only after every durable job identity and endpoint invariant is
/// checked at the final boundary.
pub fn sanctioned_env_for_request(
    job: &ExecutionJob,
    request: &SandboxRequest,
    now_ms: i64,
) -> Vec<String> {
    let mut env = sanctioned_env(job);
    let Some(access) = request.provider_gateway.as_ref() else {
        return env;
    };
    if access.provider != "claude"
        || access.authorization_id.trim().is_empty()
        || access.run_id != job.run_id
        || access.attempt_id != job.attempt_id
        || access.model != job.model_ref.catalog_id
        || access.expires_at_ms <= now_ms
        || access.bearer.expose().trim().is_empty()
        || !access.base_url.starts_with("https://cortex.heyvera.org/")
        || !job
            .capability_grants
            .iter()
            .any(|grant| matches!(grant, CapabilityGrant::ReachProvider { provider } if provider == "claude"))
    {
        tracing::warn!("invalid provider gateway access refused at sandbox boundary");
        return env;
    }
    env.push(format!("ANTHROPIC_BASE_URL={}", access.base_url));
    env.push(format!("ANTHROPIC_AUTH_TOKEN={}", access.bearer.expose()));
    env.push(format!(
        "ANTHROPIC_CUSTOM_HEADERS=X-Cortex-Attempt: {}",
        access.attempt_id
    ));
    env
}

/// The one writable path in the sandbox that is not the customer's tree.
pub const SCRATCH: &str = "/scratch";

/// Mount options for the sandbox scratch tmpfs.
///
/// `exec` because an agent legitimately runs what it builds. Size-capped
/// because a tmpfs is host RAM.
pub const SCRATCH_TMPFS_OPTIONS: &str = "rw,exec,nosuid,nodev,size=2147483648,mode=1777";

/// Where a tool that writes should write. A constant, never a lookup — see
/// [`sanctioned_env`].
///
/// Every value is the mount point **itself**, not a subdirectory of it. That
/// looks untidy and it is the only version that works: the tmpfs is mounted
/// over `/scratch` at start, which hides anything the image created there, so
/// `/scratch/home` would not exist when the process looked for it. A tool that
/// does `mkdir -p` would cope and one that does a plain `mkdir` would not, and
/// the provider CLI is the second kind — it fails with `EACCES` on
/// `$HOME/.claude/debug` before printing its own version number.
///
/// The mount point always exists, is mode 1777, and dies with the container, so
/// pointing everything at it needs nothing to have gone right beforehand.
///
/// The build-tool variables below are the deliberate exception to the paragraph
/// above, and they can be because cargo and npm both create their own
/// directories with the equivalent of `mkdir -p`. They are separate paths
/// rather than the bare mount point because a build tree and a package cache
/// sharing one directory is how you get a cache that a `cargo clean` deletes.
///
/// They exist for F15. Without them the required checks run `cargo test` with
/// its target directory defaulting to `target/` **inside the worktree**, and
/// the auto-commit then sweeps hundreds of build artifacts into the customer's
/// delivery — into the diff, into the git evidence, and into what the frozen
/// checks grade. The verifier's own runner has had the same three variables
/// since F9 for the same reason.
pub const SCRATCH_ENV: &[&str] = &[
    "HOME=/scratch",
    "TMPDIR=/scratch",
    "XDG_CACHE_HOME=/scratch",
    "XDG_CONFIG_HOME=/scratch",
    "CARGO_TARGET_DIR=/scratch/target",
    "CARGO_HOME=/scratch/cargo",
    "npm_config_cache=/scratch/npm",
];

/// The rule, with the environment lookup passed in.
///
/// Split out so the rule is testable without mutating the process environment.
/// A test that sets a real variable races every other test in the binary, and
/// the one that lost the race would report this function as safe.
fn sanctioned_env_from(job: &ExecutionJob, lookup: impl Fn(&str) -> Option<String>) -> Vec<String> {
    const STUB_SENTINEL: &str = "STUB-PROVIDER-NOT-A-REAL-KEY";
    let Some(provider) = granted_provider(job) else {
        return Vec::new();
    };
    if provider != "claude" {
        return Vec::new();
    }
    if lookup("ANTHROPIC_API_KEY").as_deref() == Some(STUB_SENTINEL) {
        return vec![format!("ANTHROPIC_API_KEY={STUB_SENTINEL}")];
    }

    tracing::warn!(
        provider = %provider,
        "supplier credentials are never admitted to a sandbox; provider \
         execution remains blocked until a gateway capability is attached"
    );
    Vec::new()
}

/// The provider this job was granted, if any.
///
/// Read off the grants rather than off the routing decision, deliberately.
/// The grant is what the planner recorded and what the receipt will show; a
/// second source for the same fact is a second thing that can disagree with it,
/// and this one decides whether a credential enters the sandbox.
pub fn granted_provider(job: &ExecutionJob) -> Option<String> {
    job.capability_grants.iter().find_map(|grant| match grant {
        CapabilityGrant::ReachProvider { provider } => Some(provider.clone()),
        CapabilityGrant::ResolveDependencies { .. } | CapabilityGrant::ReadSecret { .. } => None,
    })
}

/// Hosts the sandbox may reach, derived from the policy *and* the grants.
///
/// The port an allowlist entry permits when it does not name one.
///
/// 443 and only 443. An entry that names a host but no port is an allowlist for
/// every service on that host — an SSH daemon, a database, an internal admin
/// port — which is not what anyone means by "let this task reach the registry".
pub const DEFAULT_EGRESS_PORT: u16 = 443;

// The registry table moved to `cortex_core::egress`. Both ends of the fence
// need it — the planner names the allowlist, the enforcer refuses anything no
// grant justifies — and two copies would agree only until one was edited. The
// enforcement below is unchanged; it now reads the same table the planner did.
use cortex_core::egress::{expand_provider, expand_registry};

/// A host and the port it is permitted on. Both halves matter: the port is not
/// a detail of the host, it is half of what was granted.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

impl Endpoint {
    /// Parse an allowlist entry. `host` means `host:443`; `host:80` means what
    /// it says. A malformed port is rejected rather than defaulted, because a
    /// typo that silently becomes 443 is a policy nobody wrote.
    fn parse(entry: &str) -> Option<Self> {
        let entry = entry.trim().to_ascii_lowercase();
        if entry.is_empty() {
            return None;
        }
        match entry.rsplit_once(':') {
            Some((host, port)) => Some(Self {
                host: host.to_string(),
                port: port.parse().ok()?,
            }),
            None => Some(Self {
                host: entry,
                port: DEFAULT_EGRESS_PORT,
            }),
        }
    }
}

/// Registry names in the grants that we do not recognise.
///
/// Not silently ignored: a grant naming an unknown registry is a mistake or an
/// attempt, and the caller refuses the job rather than quietly running it with
/// less access than was asked for. A job that reaches less than its author
/// believed fails in a way that looks like a broken network.
pub fn unknown_registries(job: &ExecutionJob) -> Vec<String> {
    job.capability_grants
        .iter()
        .flat_map(|grant| match grant {
            CapabilityGrant::ResolveDependencies { registries } => registries.clone(),
            CapabilityGrant::ReachProvider { .. } | CapabilityGrant::ReadSecret { .. } => {
                Vec::new()
            }
        })
        .filter(|name| expand_registry(name).is_none())
        .collect()
}

/// Provider names in the grants that we do not recognise.
///
/// The same rule as [`unknown_registries`] and refused at the same place, but
/// kept as its own function because the two grants are separately derived and a
/// caller should be able to say which kind of grant it could not honour.
///
/// It matters more here than for a registry. An unrecognised provider name
/// means the sandbox gets no route to any model API and no credential, so the
/// step would run an agent that cannot reach anything and report whatever a
/// disconnected CLI reports. Refusing turns that into a typed refusal against
/// Cortex instead of a mystery failure charged to the customer.
pub fn unknown_providers(job: &ExecutionJob) -> Vec<String> {
    job.capability_grants
        .iter()
        .filter_map(|grant| match grant {
            CapabilityGrant::ReachProvider { provider } => Some(provider.clone()),
            CapabilityGrant::ResolveDependencies { .. } | CapabilityGrant::ReadSecret { .. } => {
                None
            }
        })
        .filter(|name| expand_provider(name).is_none())
        .collect()
}

/// A capability grant to resolve dependencies is what justifies a registry
/// being reachable; the allowlist is what makes it reachable. Both must agree,
/// so a grant without an allowlist entry opens nothing and an allowlist entry
/// without a grant is reported by [`ungranted_hosts`].
pub fn effective_endpoints(job: &ExecutionJob) -> Vec<Endpoint> {
    if job.network_policy.is_deny() {
        return Vec::new();
    }

    let granted = granted_hosts(job);
    let mut endpoints: Vec<Endpoint> = job
        .network_policy
        .allowed_hosts()
        .iter()
        .filter_map(|entry| Endpoint::parse(entry))
        .filter(|endpoint| granted.contains(&endpoint.host))
        .collect();
    endpoints.sort();
    endpoints.dedup();
    endpoints
}

/// The host names of [`effective_endpoints`], for callers that only need the
/// names.
pub fn effective_hosts(job: &ExecutionJob) -> Vec<String> {
    effective_endpoints(job)
        .into_iter()
        .map(|endpoint| endpoint.host)
        .collect()
}

/// Allowlist entries that no capability grant justifies.
///
/// These are not silently dropped — [`effective_endpoints`] already excludes
/// them, and a caller reports them, because an allowlist entry nobody granted
/// is either a mistake or an attempt.
pub fn ungranted_hosts(job: &ExecutionJob) -> Vec<String> {
    let granted = granted_hosts(job);
    job.network_policy
        .allowed_hosts()
        .iter()
        .filter(|entry| match Endpoint::parse(entry) {
            Some(endpoint) => !granted.contains(&endpoint.host),
            // An unparseable entry justifies nothing and is worth reporting.
            None => true,
        })
        .cloned()
        .collect()
}

fn granted_hosts(job: &ExecutionJob) -> Vec<String> {
    job.capability_grants
        .iter()
        .flat_map(|grant| match grant {
            CapabilityGrant::ResolveDependencies { registries } => registries
                .iter()
                .filter_map(|name| expand_registry(name))
                .flatten()
                .collect::<Vec<_>>(),
            // Same rule as a registry: the grant names the provider, and the
            // table in `cortex_core::egress` decides what that name reaches. A
            // grant naming a provider the table does not know expands to
            // nothing, so it cannot be used to smuggle a hostname through.
            CapabilityGrant::ReachProvider { provider } => expand_provider(provider)
                .map(|host| vec![host.to_string()])
                .unwrap_or_default(),
            CapabilityGrant::ReadSecret { .. } => Vec::new(),
        })
        .collect()
}

/// Whether the runtime must be asked for a network at all.
///
/// Deny and "an allowlist that nothing justifies" are the same thing at the
/// boundary: no network.
pub fn needs_network(job: &ExecutionJob) -> bool {
    !effective_endpoints(job).is_empty()
}

/// The mount set. Exactly one entry, read-write, and never the host root, the
/// Docker socket, the home directory, or the production database.
pub fn binds(request: &SandboxRequest) -> Vec<String> {
    vec![format!(
        "{}:{}:rw",
        request.workspace.display(),
        WORKSPACE_MOUNT
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_core::execution_job::{
        BackendKind, Budgets, EffortApplication, ExecutionJob, IsolationClass, ModelRef,
        NetworkPolicy, ResourceProfile, EXECUTION_JOB_VERSION,
    };

    fn job() -> ExecutionJob {
        ExecutionJob {
            job_id: "job-1".to_string(),
            job_version: EXECUTION_JOB_VERSION,
            run_id: "run-1".to_string(),
            step_id: "step-1".to_string(),
            attempt_id: "attempt-1".to_string(),
            lease_gen: 1,
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
    fn nothing_from_the_host_survives_without_a_grant() {
        // The case this prevents: a Clerk secret, the production database
        // path, or a provider key for a provider this step was never routed
        // to, reaching a sandbox because a denylist missed it.
        //
        // The lookup returns a value for *everything*, so this asserts the
        // allowlist rather than asserting that the machine happens to be
        // missing the variables.
        let everything = |var: &str| Some(format!("{var}-value"));

        // The rule itself — everything the host could offer, nothing taken.
        assert!(sanctioned_env_from(&job(), everything).is_empty());

        // And what the sandbox is actually handed: the scratch constants and
        // not one thing more. Asserted as an exact set rather than as "does
        // not contain a secret", so a variable added here has to be added to
        // the constant in a diff somebody reads.
        assert_eq!(
            sanctioned_env(&job()),
            SCRATCH_ENV
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_scratch_variables_cannot_carry_a_host_value() {
        // The security property the scratch addition must not have cost. These
        // are a `const` with no lookup behind them, so this cannot fail by
        // construction — which is why they are a `const`. Pinned because the
        // failure it guards against is somebody making one of them
        // configurable, and the first configurable one is the leak.
        for entry in SCRATCH_ENV {
            let (name, value) = entry.split_once('=').expect("every entry is NAME=value");
            assert!(
                value.starts_with(SCRATCH),
                "{name} points somewhere other than the scratch mount: {value}"
            );
            assert!(
                !value.contains(std::path::MAIN_SEPARATOR) || value.starts_with('/'),
                "{name} carries a host path: {value}"
            );
        }
    }

    #[test]
    fn a_writable_home_is_not_inside_the_workspace() {
        // The whole point of the scratch mount. A `HOME` under the worktree
        // means a provider CLI's config and cache land in the customer's diff,
        // in the git evidence, and in what the frozen checks grade.
        for entry in SCRATCH_ENV {
            let (_, value) = entry.split_once('=').expect("every entry is NAME=value");
            assert!(
                !value.starts_with(WORKSPACE_MOUNT),
                "a tool's write path is inside the delivered tree: {value}"
            );
        }
    }

    #[test]
    fn a_provider_grant_never_admits_a_supplier_key() {
        let mut job = job();
        job.capability_grants = vec![
            CapabilityGrant::ResolveDependencies {
                registries: vec!["crates".to_string()],
            },
            CapabilityGrant::ReachProvider {
                provider: "claude".to_string(),
            },
        ];

        let env = sanctioned_env_from(&job, |var| Some(format!("live-{var}-value")));

        assert!(env.is_empty());
        for absent in [
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "CLERK_SECRET_KEY",
            "CORTEX_DB_PATH",
        ] {
            assert!(
                !env.iter().any(|entry| entry.starts_with(absent)),
                "{absent} reached the sandbox"
            );
        }
    }

    #[test]
    fn every_provider_supplier_key_is_rejected() {
        for provider in ["claude", "openai", "gemini"] {
            let mut job = job();
            job.capability_grants = vec![CapabilityGrant::ReachProvider {
                provider: provider.to_string(),
            }];

            let env = sanctioned_env_from(&job, |var| Some(format!("{var}-value")));
            assert!(env.is_empty(), "{provider} admitted a supplier credential");
        }
    }

    #[test]
    fn the_zero_cost_stub_sentinel_remains_usable() {
        let mut job = job();
        job.capability_grants = vec![CapabilityGrant::ReachProvider {
            provider: "claude".to_string(),
        }];
        let env = sanctioned_env_from(&job, |_| Some("STUB-PROVIDER-NOT-A-REAL-KEY".to_string()));
        assert_eq!(env, ["ANTHROPIC_API_KEY=STUB-PROVIDER-NOT-A-REAL-KEY"]);
    }

    fn gateway_access() -> ProviderGatewayAccess {
        ProviderGatewayAccess {
            authorization_id: "auth-1".into(),
            run_id: "run-1".into(),
            attempt_id: "attempt-1".into(),
            provider: "claude".into(),
            model: "claude-opus-5".into(),
            base_url: "https://cortex.heyvera.org/internal/provider".into(),
            expires_at_ms: 2_000,
            bearer: cortex_core::protocol::GatewayBearer::new("signed-capability"),
        }
    }

    #[test]
    fn matching_gateway_access_reaches_the_sandbox_without_a_supplier_key() {
        let mut job = job();
        job.capability_grants = vec![CapabilityGrant::ReachProvider {
            provider: "claude".into(),
        }];
        let request = SandboxRequest::new("/tmp/wt", "claude", Vec::new())
            .with_provider_gateway(Some(gateway_access()));
        let env = sanctioned_env_for_request(&job, &request, 1_000);
        assert!(env.iter().any(|entry| {
            entry == "ANTHROPIC_BASE_URL=https://cortex.heyvera.org/internal/provider"
        }));
        assert!(env
            .iter()
            .any(|entry| entry == "ANTHROPIC_AUTH_TOKEN=signed-capability"));
        assert!(env
            .iter()
            .any(|entry| entry == "ANTHROPIC_CUSTOM_HEADERS=X-Cortex-Attempt: attempt-1"));
        assert!(!env.iter().any(|entry| entry.starts_with("OPENAI_API_KEY=")));
    }

    #[test]
    fn mismatched_or_expired_gateway_access_is_refused() {
        let mut job = job();
        job.capability_grants = vec![CapabilityGrant::ReachProvider {
            provider: "claude".into(),
        }];
        for access in [
            ProviderGatewayAccess {
                attempt_id: "other-attempt".into(),
                ..gateway_access()
            },
            ProviderGatewayAccess {
                model: "other-model".into(),
                ..gateway_access()
            },
            ProviderGatewayAccess {
                base_url: "https://api.anthropic.com".into(),
                ..gateway_access()
            },
            ProviderGatewayAccess {
                expires_at_ms: 1_000,
                ..gateway_access()
            },
        ] {
            let request = SandboxRequest::new("/tmp/wt", "claude", Vec::new())
                .with_provider_gateway(Some(access));
            let env = sanctioned_env_for_request(&job, &request, 1_000);
            assert!(!env
                .iter()
                .any(|entry| entry.starts_with("ANTHROPIC_AUTH_TOKEN=")));
        }
    }

    #[test]
    fn an_unset_key_is_not_invented() {
        // A missing credential must produce a truthful unauthenticated
        // failure, never an empty-string key that looks set.
        let mut job = job();
        job.capability_grants = vec![CapabilityGrant::ReachProvider {
            provider: "claude".to_string(),
        }];

        assert!(sanctioned_env_from(&job, |_| None).is_empty());
    }

    #[test]
    fn an_unknown_provider_grant_admits_nothing() {
        // The grant is attacker-shaped input the moment anything but the
        // planner can write one: a name the table does not know must open no
        // host and carry no credential.
        let mut job = job();
        job.network_policy = NetworkPolicy::Allowlist {
            hosts: vec!["evil.example.com".to_string()],
        };
        job.capability_grants = vec![CapabilityGrant::ReachProvider {
            provider: "evil.example.com".to_string(),
        }];

        assert!(sanctioned_env_from(&job, |var| Some(format!("{var}-value"))).is_empty());
        assert!(effective_hosts(&job).is_empty());
        assert_eq!(ungranted_hosts(&job), ["evil.example.com"]);
    }

    #[test]
    fn a_provider_grant_opens_that_provider_and_nothing_else() {
        let mut job = job();
        job.network_policy = NetworkPolicy::Allowlist {
            hosts: vec![
                "cortex.heyvera.org".to_string(),
                // Present in the allowlist but justified by no grant: the
                // shape a widened plan would have.
                "api.openai.com".to_string(),
            ],
        };
        job.capability_grants = vec![CapabilityGrant::ReachProvider {
            provider: "claude".to_string(),
        }];

        assert_eq!(effective_hosts(&job), ["cortex.heyvera.org"]);
        assert!(needs_network(&job));
        assert_eq!(ungranted_hosts(&job), ["api.openai.com"]);
    }

    #[test]
    fn default_job_gets_no_network() {
        // Asserts the default, not just the explicit case.
        assert!(!needs_network(&job()));
        assert!(effective_hosts(&job()).is_empty());
    }

    #[test]
    fn allowlist_without_a_grant_opens_nothing() {
        let mut job = job();
        job.network_policy = NetworkPolicy::Allowlist {
            hosts: vec!["evil.example.com".to_string()],
        };
        assert!(effective_hosts(&job).is_empty());
        assert!(!needs_network(&job));
        assert_eq!(ungranted_hosts(&job), ["evil.example.com"]);
    }

    #[test]
    fn grant_and_allowlist_together_open_exactly_the_registry() {
        let mut job = job();
        job.network_policy = NetworkPolicy::Allowlist {
            hosts: vec!["crates.io".to_string(), "evil.example.com".to_string()],
        };
        job.capability_grants = vec![CapabilityGrant::ResolveDependencies {
            registries: vec!["crates.io".to_string()],
        }];

        assert_eq!(effective_hosts(&job), ["crates.io"]);
        assert_eq!(ungranted_hosts(&job), ["evil.example.com"]);
        assert!(needs_network(&job));
    }

    #[test]
    fn a_secret_grant_does_not_open_the_network() {
        // Reading a secret is delivered by the runner, not fetched by the
        // sandbox. Conflating the two would make every secret a network hole.
        let mut job = job();
        job.network_policy = NetworkPolicy::Allowlist {
            hosts: vec!["vault.internal".to_string()],
        };
        job.capability_grants = vec![CapabilityGrant::ReadSecret {
            secret_id: "s-1".to_string(),
        }];
        assert!(effective_hosts(&job).is_empty());
    }

    #[test]
    fn workspace_is_the_only_mount() {
        // Catches a Docker socket, a home directory, or the host root being
        // added to the mount set.
        let request = SandboxRequest::new("/tmp/wt", "claude", vec!["-p".to_string()]);
        let binds = binds(&request);
        assert_eq!(binds.len(), 1);
        assert!(binds[0].ends_with(":/work:rw"));
        assert!(!binds[0].contains("docker.sock"));
    }

    #[test]
    fn argv_puts_the_program_first() {
        let request = SandboxRequest::new(
            "/tmp/wt",
            "claude",
            vec!["--model".to_string(), "opus".to_string()],
        );
        assert_eq!(request.argv(), ["claude", "--model", "opus"]);
    }
}
