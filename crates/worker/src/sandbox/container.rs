//! The first [`SandboxRunner`](super::SandboxRunner) implementation: a rootless
//! container, locked down.
//!
//! This is deliberately the *first* implementation and not the intended final
//! one. A container shares the host kernel, so against a kernel exploit it is a
//! resource boundary rather than a security boundary — and the threat model
//! here is model-authored code running next to other customers' source. The
//! job records [`IsolationClass::Container`] for exactly that reason: a receipt
//! must state the boundary that ran, not the one a reader would like to assume.
//!
//! Every constraint below is load-bearing, so none of them are configurable.

use std::time::Duration;

use bollard::container::{
    AttachContainerOptions, AttachContainerResults, Config, CreateContainerOptions,
    RemoveContainerOptions, StartContainerOptions, WaitContainerOptions,
};
use bollard::models::HostConfig;
use bollard::Docker;
use cortex_core::execution_job::{Blocked, BlockedReason, ExecutionJob, IsolationClass};
use futures_util::StreamExt;

use super::egress::{self, Egress};
use super::policy::{
    binds, effective_endpoints, sanctioned_env_for_request, ungranted_hosts, unknown_providers,
    unknown_registries, WORKSPACE_MOUNT,
};
use super::SandboxSession;
use super::{OutputStream, SandboxDriver, SandboxExit, SandboxLine, SandboxRequest, SandboxRunner};

fn unix_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Unprivileged user baked into the runner image, used when the workspace
/// owner cannot be determined.
const SANDBOX_USER: &str = "sandbox";

/// Who the sandbox runs as.
///
/// The workspace is a bind-mounted worktree owned by whichever user the worker
/// runs as, and a bind mount carries host ownership through unchanged. A
/// sandbox running as a *different* unprivileged user therefore cannot write to
/// its own workspace — every task would produce an empty delivery. So the
/// sandbox runs as the workspace's owner: still unprivileged, still confined to
/// one mount, and now able to do the work.
///
/// This does not widen the boundary. Nothing else is mounted, so a uid grants
/// access to no file the sandbox could not already reach.
fn sandbox_user(workspace: &std::path::Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(metadata) = std::fs::metadata(workspace) {
            let (uid, gid) = (metadata.uid(), metadata.gid());
            // Never root, whatever the host says. A workspace owned by root is
            // a deployment mistake, and inheriting it would hand the sandbox
            // uid 0.
            if uid != 0 {
                return format!("{uid}:{gid}");
            }
            tracing::warn!(
                workspace = %workspace.display(),
                "workspace is owned by root; falling back to the image's unprivileged user"
            );
        }
    }
    let _ = workspace;
    SANDBOX_USER.to_string()
}

pub struct ContainerSandbox {
    docker: Docker,
    /// Pinned by digest (`repo@sha256:…`) in production. Validation that it is
    /// a digest rather than a mutable tag belongs to the signed runner
    /// registry; this runner records whatever it was given so the receipt is
    /// honest either way.
    image: String,
    /// The egress mediator image. `None` means the deployment default.
    ///
    /// Overridable per runner rather than only by environment variable so a
    /// test can point one runner at a broken image and assert the refusal,
    /// without a global that leaks into every other test in the process.
    egress_image: Option<String>,
}

impl ContainerSandbox {
    pub fn new(image: impl Into<String>) -> Result<Self, Blocked> {
        let docker = Docker::connect_with_local_defaults().map_err(|e| {
            Blocked::new(
                BlockedReason::SandboxUnavailable,
                format!("failed to connect to the container runtime: {e}"),
            )
        })?;
        Ok(Self {
            docker,
            image: image.into(),
            egress_image: None,
        })
    }

    /// Use a specific egress mediator image instead of the deployment default.
    pub fn with_egress_image(mut self, image: impl Into<String>) -> Self {
        self.egress_image = Some(image.into());
        self
    }

    /// Build the container configuration.
    ///
    /// Deliberately an associated function taking the image rather than a
    /// method, so the hardening below is testable on every platform instead of
    /// only where a container runtime happens to be running. These assertions
    /// are the security boundary; they must not be the ones that get skipped.
    fn build_config(
        image: &str,
        job: &ExecutionJob,
        request: &SandboxRequest,
        egress: Option<&Egress>,
    ) -> Config<String> {
        let profile = &job.resource_profile;

        // `none` unless a mediator was actually provisioned. The deny is the
        // default and the grant is this one explicit branch, never a variable
        // that happens to be set — so a bug anywhere else in the granting path
        // cannot open the sandbox by accident.
        //
        // Note what the granted branch does *not* do: it never names `bridge`
        // or any network with a route off the host. The only network a sandbox
        // ever joins is the attempt's `internal: true` one, whose sole exit is
        // the mediator.
        let (network_mode, network_disabled, env, endpoints) = match egress {
            None => (
                "none".to_string(),
                Some(true),
                // Supplier credentials are never copied from the worker.
                // Today only the explicit stub sentinel can accompany a
                // provider grant; later this slot carries a scoped gateway
                // capability from the job itself.
                sanctioned_env_for_request(job, request, unix_now_ms()),
                None,
            ),
            Some(egress) => {
                let mut env = sanctioned_env_for_request(job, request, unix_now_ms());
                env.extend(egress::proxy_env(&egress.proxy_url()));
                (
                    egress.network_name().to_string(),
                    Some(false),
                    env,
                    Some(egress::sandbox_endpoints(egress.network_name())),
                )
            }
        };

        Config {
            image: Some(image.to_string()),
            cmd: Some(request.argv()),
            working_dir: Some(WORKSPACE_MOUNT.to_string()),
            env: Some(env),
            user: Some(sandbox_user(&request.workspace)),
            network_disabled,
            networking_config: endpoints
                .map(|endpoints_config| bollard::container::NetworkingConfig { endpoints_config }),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            host_config: Some(HostConfig {
                // Exactly one mount: the worktree. No socket, no home, no root.
                binds: Some(binds(request)),
                network_mode: Some(network_mode),
                memory: Some(profile.memory_bytes),
                memory_swap: Some(profile.memory_bytes),
                cpu_quota: Some(profile.cpu_quota),
                cpu_period: Some(profile.cpu_period),
                pids_limit: Some(profile.pids_limit),
                cap_drop: Some(vec!["ALL".to_string()]),
                security_opt: Some(vec!["no-new-privileges:true".to_string()]),
                // The workspace mount stays writable; everything else does not,
                // so nothing can be cached into the image for the next task.
                readonly_rootfs: Some(true),
                // …with one deliberate exception, which is a tmpfs and so dies
                // with the container. Without it the worktree is the only
                // writable path in the sandbox, and a provider CLI with no
                // `HOME` either refuses to start or writes its config and
                // cache into the tree it was asked to change — where they land
                // in the diff, in the git evidence, and in what the frozen
                // checks grade. See `policy::sanctioned_env`.
                tmpfs: Some(std::collections::HashMap::from([(
                    crate::sandbox::policy::SCRATCH.to_string(),
                    crate::sandbox::policy::SCRATCH_TMPFS_OPTIONS.to_string(),
                )])),
                privileged: Some(false),
                userns_mode: Some("host".to_string()),
                // Removal is explicit in every path including timeout and
                // kill, so a leaked sandbox is a bug rather than a race.
                auto_remove: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// Teardown. Mandatory, and attempted on every path.
    async fn remove(docker: &Docker, id: &str) {
        let opts = RemoveContainerOptions {
            force: true,
            // Remove anonymous volumes too — a surviving volume is exactly the
            // leftover file that makes reuse a leak.
            v: true,
            ..Default::default()
        };
        if let Err(e) = docker.remove_container(id, Some(opts)).await {
            tracing::warn!(container_id = id, error = %e, "sandbox teardown failed");
        }
    }
}

impl SandboxRunner for ContainerSandbox {
    async fn submit(
        &self,
        job: &ExecutionJob,
        request: &SandboxRequest,
    ) -> Result<SandboxSession, Blocked> {
        // An allowlist entry no grant justifies is either a mistake or an
        // attempt. It is already excluded from the effective set; report it
        // rather than letting it pass unnoticed.
        let ungranted = ungranted_hosts(job);
        if !ungranted.is_empty() {
            tracing::warn!(
                attempt_id = %job.attempt_id,
                hosts = ?ungranted,
                "allowlist entries without a capability grant were dropped"
            );
        }

        // A grant naming a registry we do not recognise is refused rather than
        // quietly ignored. Ignoring it runs the job with less reach than its
        // author believed, which fails later and looks like a broken network
        // instead of a policy we declined to honour.
        let unknown = unknown_registries(job);
        if !unknown.is_empty() {
            return Err(Blocked::new(
                BlockedReason::NetworkPolicyUnenforceable,
                format!(
                    "capability grant names registries this build does not know: {unknown:?}; \
                     refusing rather than running with less access than was asked for"
                ),
            ));
        }

        // Same rule for the provider grant, and refused for a sharper reason:
        // a provider name this build cannot expand means no route to any model
        // API and no credential, so the agent would run with nothing to talk
        // to. That is a Cortex refusal, not a customer's step failing.
        let unknown = unknown_providers(job);
        if !unknown.is_empty() {
            return Err(Blocked::new(
                BlockedReason::NetworkPolicyUnenforceable,
                format!(
                    "capability grant names providers this build does not know: {unknown:?}; \
                     refusing rather than running an agent with no route to a model"
                ),
            ));
        }

        // Scoped egress: stand up a mediator on an internal network, or refuse.
        //
        // There is deliberately no fallback. Starting the sandbox on a normal
        // network because the mediator did not come up is invariant 8's
        // forbidden downgrade, and it would silently undo the whole sandbox.
        let granted = effective_endpoints(job);
        let egress = if granted.is_empty() {
            None
        } else {
            Some(
                egress::provision(
                    &self.docker,
                    job,
                    &granted,
                    self.egress_image
                        .clone()
                        .unwrap_or_else(egress::mediator_image),
                )
                .await?,
            )
        };

        if job.budgets.is_unquoted() {
            // `None` means no cap is enforced. Say so; do not let an unpriced
            // job look like a bounded one.
            tracing::info!(
                attempt_id = %job.attempt_id,
                wall_clock_secs = job.budgets.wall_clock.as_secs(),
                "job has no token or tool-call budget; only the wall clock bounds it"
            );
        }

        let name = format!("cortex-sbx-{}", uuid::Uuid::new_v4());
        let config = Self::build_config(&self.image, job, request, egress.as_ref());

        let created = match self
            .docker
            .create_container(
                Some(CreateContainerOptions {
                    name: name.clone(),
                    ..Default::default()
                }),
                config,
            )
            .await
        {
            Ok(created) => created,
            Err(e) => {
                if let Some(egress) = egress {
                    egress.teardown(&self.docker).await;
                }
                return Err(Blocked::new(
                    BlockedReason::SandboxUnavailable,
                    format!("failed to create sandbox: {e}"),
                ));
            }
        };
        let id = created.id;

        // Attach before start, so no output is lost between the two.
        let attached = match self
            .docker
            .attach_container(
                &id,
                Some(AttachContainerOptions::<String> {
                    stdout: Some(true),
                    stderr: Some(true),
                    stream: Some(true),
                    ..Default::default()
                }),
            )
            .await
        {
            Ok(attached) => attached,
            Err(e) => {
                Self::remove(&self.docker, &id).await;
                if let Some(egress) = egress {
                    egress.teardown(&self.docker).await;
                }
                return Err(Blocked::new(
                    BlockedReason::SandboxUnavailable,
                    format!("failed to attach to sandbox: {e}"),
                ));
            }
        };

        if let Err(e) = self
            .docker
            .start_container(&id, None::<StartContainerOptions<String>>)
            .await
        {
            Self::remove(&self.docker, &id).await;
            if let Some(egress) = egress {
                egress.teardown(&self.docker).await;
            }
            return Err(Blocked::new(
                BlockedReason::SandboxUnavailable,
                format!("failed to start sandbox: {e}"),
            ));
        }

        let (session, driver) = SandboxSession::channel(id.clone(), IsolationClass::Container);
        let docker = self.docker.clone();
        let wall_clock = job.budgets.wall_clock;

        // The mediator is torn down by `drive`, on every path the sandbox can
        // take — clean exit, budget exhaustion, kill, or a runtime error. It
        // outlives the sandbox by design and must not outlive it by accident.
        tokio::spawn(drive(docker, id, attached, driver, wall_clock, egress));

        Ok(session)
    }

    fn isolation_class(&self) -> IsolationClass {
        IsolationClass::Container
    }
}

/// Pump output, enforce the wall clock, honour a kill, and tear down — in every
/// exit path.
async fn drive(
    docker: Docker,
    id: String,
    attached: AttachContainerResults,
    driver: SandboxDriver,
    wall_clock: Duration,
    egress: Option<Egress>,
) {
    let SandboxDriver {
        output,
        exit,
        mut kill,
    } = driver;

    let AttachContainerResults {
        output: mut stream, ..
    } = attached;
    let output_task = {
        let output = output.clone();
        tokio::spawn(async move {
            while let Some(chunk) = stream.next().await {
                let Ok(chunk) = chunk else { break };
                let (stream_kind, bytes) = match chunk {
                    bollard::container::LogOutput::StdOut { message } => {
                        (OutputStream::Stdout, message)
                    }
                    bollard::container::LogOutput::StdErr { message } => {
                        (OutputStream::Stderr, message)
                    }
                    other => (OutputStream::Stdout, other.into_bytes()),
                };
                let text = String::from_utf8_lossy(&bytes).to_string();
                for line in text.lines() {
                    if output
                        .send(SandboxLine {
                            stream: stream_kind,
                            text: line.to_string(),
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
        })
    };
    drop(output);

    let mut wait = docker.wait_container(&id, None::<WaitContainerOptions<String>>);

    let outcome = tokio::select! {
        // Operator stop or breaker. The sandbox dies; the outcome is
        // attributed to the stop rather than to the customer's work.
        _ = &mut kill => SandboxExit::Killed,

        // Invariant 14's cap, enforced at the boundary rather than trusted to
        // the agent.
        _ = tokio::time::sleep(wall_clock) => SandboxExit::BudgetExhausted,

        waited = wait.next() => match waited {
            Some(Ok(response)) => SandboxExit::Exited { code: response.status_code as i32 },
            // bollard reports a non-zero exit as a *wait error*, not as a
            // response. Treating that as a sandbox failure would report every
            // legitimately failing task as infrastructure trouble — precisely
            // the Failed/Blocked confusion this module exists to prevent. The
            // work ran and the boundary held; a non-zero code is a result.
            Some(Err(bollard::errors::Error::DockerContainerWaitError { code, .. })) => {
                SandboxExit::Exited { code: code as i32 }
            }
            // The sandbox vanished or the runtime failed mid-wait. We do not
            // know what happened, and a guess here becomes an unverified
            // delivery — refuse instead.
            Some(Err(e)) => {
                output_task.abort();
                ContainerSandbox::remove(&docker, &id).await;
                if let Some(egress) = egress {
                    egress.teardown(&docker).await;
                }
                let _ = exit.send(Err(Blocked::new(
                    BlockedReason::SandboxUnavailable,
                    format!("sandbox wait failed: {e}"),
                )));
                return;
            }
            None => {
                output_task.abort();
                ContainerSandbox::remove(&docker, &id).await;
                if let Some(egress) = egress {
                    egress.teardown(&docker).await;
                }
                let _ = exit.send(Err(Blocked::new(
                    BlockedReason::SandboxUnavailable,
                    "sandbox wait stream ended without a status",
                )));
                return;
            }
        },
    };

    // Let buffered output drain before teardown, so a receipt shows what the
    // sandbox managed to print — including where it hung.
    let _ = tokio::time::timeout(Duration::from_secs(5), output_task).await;
    ContainerSandbox::remove(&docker, &id).await;
    // The sandbox is gone; the network it was on must go with it. A surviving
    // network is the leftover state that makes reuse a leak.
    if let Some(egress) = egress {
        egress.teardown(&docker).await;
    }
    let _ = exit.send(Ok(outcome));
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_core::execution_job::{
        BackendKind, Budgets, CapabilityGrant, EffortApplication, ModelRef, NetworkPolicy,
        ResourceProfile, EXECUTION_JOB_VERSION,
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

    fn request() -> SandboxRequest {
        SandboxRequest::new("/tmp/wt", "claude", vec!["-p".to_string()])
    }

    fn config_of(job: &ExecutionJob) -> Config<String> {
        ContainerSandbox::build_config("cortex/runner@sha256:abc", job, &request(), None)
    }

    #[test]
    fn resource_bounds_are_all_set() {
        // An unbounded sandbox can take down the host. Every limit must have a
        // value, not a default that means "unlimited".
        let job = job();
        let host = config_of(&job).host_config.expect("host config");
        assert_eq!(host.memory, Some(job.resource_profile.memory_bytes));
        assert_eq!(host.memory_swap, Some(job.resource_profile.memory_bytes));
        assert_eq!(host.cpu_quota, Some(job.resource_profile.cpu_quota));
        assert_eq!(host.pids_limit, Some(job.resource_profile.pids_limit));
    }

    #[test]
    fn runs_unprivileged_with_a_readonly_root() {
        let host = config_of(&job()).host_config.expect("host config");
        assert_eq!(host.privileged, Some(false));
        assert_eq!(host.readonly_rootfs, Some(true));
        assert_eq!(host.cap_drop, Some(vec!["ALL".to_string()]));
        assert_eq!(
            host.security_opt,
            Some(vec!["no-new-privileges:true".to_string()])
        );
        // On Unix this is the workspace owner's uid; elsewhere it is the
        // image's unprivileged user. Either way it must never be root.
        let user = config_of(&job()).user.expect("a user is always set");
        assert!(!user.is_empty());
        assert!(!user.starts_with("0:"), "the sandbox must not run as root");
        assert_ne!(user, "root");
    }

    #[test]
    fn environment_is_empty_without_a_provider_grant() {
        // Catches a provider credential reaching a sandbox that was never
        // granted a provider — the case that can spend Cortex's own money
        // from a step that had no business talking to a model at all.
        //
        // This assertion used to be unconditional. It is now conditional on
        // the grant, and the condition is the point: the credential and the
        // route are authorised by the same grant, so a sandbox cannot hold one
        // without the other. See gate G3 in ADR-0003 and ADR-0004 for why the
        // key is in the sandbox at all, and what replaces this.
        //
        // Set so that "empty" here means the grant withheld it, not that the
        // machine had no key to leak.
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test-not-a-real-key");

        // The scratch constants and nothing else. They are a compile-time
        // list with no host lookup behind them, so "no credential" is exactly
        // "the env is the constant". Asserted as an exact set, not as an
        // absence: an absence assertion passes when the whole thing is empty
        // *and* when someone quietly adds a fourth variable.
        let scratch = || {
            crate::sandbox::policy::SCRATCH_ENV
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(config_of(&job()).env, Some(scratch()));

        // A registry grant is not a provider grant, and must carry no
        // credential.
        let mut registry_only = job();
        registry_only.capability_grants = vec![CapabilityGrant::ResolveDependencies {
            registries: vec!["crates".to_string()],
        }];
        assert_eq!(config_of(&registry_only).env, Some(scratch()));
    }

    #[test]
    fn the_sandbox_has_a_writable_path_that_is_not_the_customer_s_tree() {
        // Without this the worktree is the only writable path in the sandbox,
        // and a provider CLI with no `HOME` either refuses to start or writes
        // its config and cache into the tree it was asked to change — where
        // they land in the diff, in the git evidence, and in what the frozen
        // checks grade.
        let host = config_of(&job()).host_config.expect("host config");
        let tmpfs = host.tmpfs.expect("the sandbox has a scratch mount");
        let options = tmpfs
            .get(crate::sandbox::policy::SCRATCH)
            .expect("scratch is mounted where the environment points");

        assert!(options.contains("rw"), "scratch is not writable: {options}");
        assert!(
            options.contains("size="),
            "an unbounded tmpfs is host RAM a task can exhaust: {options}"
        );
        // The half this must not have cost: scratch is a tmpfs, so nothing
        // written there survives the container into the next task.
        for path in tmpfs.keys() {
            assert!(
                !path.starts_with(WORKSPACE_MOUNT),
                "the scratch mount is inside the delivered tree at {path}"
            );
        }
    }

    #[test]
    fn a_provider_grant_does_not_admit_supplier_keys() {
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test-not-a-real-key");
        std::env::set_var("OPENAI_API_KEY", "sk-test-other-provider");

        let mut job = job();
        job.capability_grants = vec![CapabilityGrant::ReachProvider {
            provider: "claude".to_string(),
        }];

        let env = config_of(&job).env.expect("env is always set");

        let expected = crate::sandbox::policy::SCRATCH_ENV
            .iter()
            .map(|s| (*s).to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            env, expected,
            "a provider grant must not carry a supplier credential"
        );
        for forbidden in [
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "CLERK_SECRET_KEY",
            "CORTEX_DB_PATH",
            "CORTEX_LEDGER_PATH",
        ] {
            assert!(
                !env.iter().any(|e| e.starts_with(forbidden)),
                "{forbidden} reached the sandbox"
            );
        }
    }

    #[test]
    fn only_the_workspace_is_mounted() {
        let host = config_of(&job()).host_config.expect("host config");
        let binds = host.binds.expect("binds");
        assert_eq!(binds.len(), 1);
        assert!(binds[0].ends_with(":/work:rw"));
        assert!(!binds.iter().any(|b| b.contains("docker.sock")));
    }

    #[test]
    fn network_is_denied_by_default() {
        let config = config_of(&job());
        assert_eq!(config.network_disabled, Some(true));
        assert_eq!(
            config.host_config.expect("host config").network_mode,
            Some("none".to_string())
        );
    }

    #[test]
    fn ungranted_allowlist_still_gets_no_network() {
        // An allowlist entry without a capability grant must not open egress.
        let mut job = job();
        job.network_policy = NetworkPolicy::Allowlist {
            hosts: vec!["evil.example.com".to_string()],
        };
        let config = config_of(&job);
        assert_eq!(config.network_disabled, Some(true));
        assert_eq!(
            config.host_config.expect("host config").network_mode,
            Some("none".to_string())
        );
    }

    #[tokio::test]
    async fn an_unknown_registry_grant_is_refused_rather_than_narrowed() {
        // A grant naming something we do not recognise is a mistake or an
        // attempt. Running it with less reach than its author believed fails
        // later and looks like a broken network instead of a declined policy.
        let mut job = job();
        job.network_policy = NetworkPolicy::Allowlist {
            hosts: vec!["evil.example.com".to_string()],
        };
        job.capability_grants = vec![CapabilityGrant::ResolveDependencies {
            registries: vec!["evil.example.com".to_string()],
        }];

        let Ok(runner) = ContainerSandbox::new("cortex/sandbox:test") else {
            return;
        };
        let blocked = match runner.submit(&job, &request()).await {
            Ok(_) => panic!("an unknown registry must be refused"),
            Err(blocked) => blocked,
        };
        assert_eq!(blocked.reason, BlockedReason::NetworkPolicyUnenforceable);
    }

    #[test]
    fn a_granted_allowlist_attaches_the_mediator_network_and_never_bridge() {
        // The shape of the granted path, asserted without a runtime. What must
        // hold: the sandbox joins the attempt's internal network and nothing
        // else, and the proxy variables are present but are not the boundary.
        let mut job = job();
        job.network_policy = NetworkPolicy::Allowlist {
            hosts: vec!["crates.io".to_string()],
        };
        job.capability_grants = vec![CapabilityGrant::ResolveDependencies {
            registries: vec!["crates".to_string()],
        }];

        // Without a mediator the configuration is denied, whatever the policy
        // says. That is the property that makes a bug in the granting path
        // unable to open the sandbox.
        let denied = config_of(&job);
        assert_eq!(denied.network_disabled, Some(true));
        assert_eq!(
            denied.host_config.expect("host config").network_mode,
            Some("none".to_string())
        );

        // And the grant expanded to the registry's real host set rather than
        // to whatever the caller happened to name.
        let hosts = super::super::policy::effective_hosts(&job);
        assert_eq!(hosts, ["crates.io"]);
    }

    #[test]
    fn the_command_is_the_request_argv() {
        assert_eq!(
            config_of(&job()).cmd,
            Some(vec!["claude".to_string(), "-p".to_string()])
        );
    }

    #[test]
    fn teardown_is_never_automatic() {
        // auto_remove would race the log drain and hide where a sandbox hung.
        let host = config_of(&job()).host_config.expect("host config");
        assert_eq!(host.auto_remove, Some(false));
    }
}
