//! Provisioning and teardown of the per-attempt egress mediator.
//!
//! The design in one sentence: **the sandbox gets a network with no route off
//! the host, and the mediator is the only thing on it that has one.**
//!
//! That is the whole argument. A task that unsets `HTTPS_PROXY` and opens a raw
//! socket to an allowlisted host's IP address does not get filtered — it gets
//! no route. Enforcement is topology, not configuration, because anything that
//! depends on the task cooperating is not enforcement.
//!
//! Concretely, per attempt:
//!
//! 1. A Docker network created with `internal: true`. Docker installs no
//!    gateway for it, so nothing on it can reach anything off the host.
//! 2. The mediator container, attached to that network *and* to `bridge`. It is
//!    the single dual-homed host.
//! 3. The sandbox container, attached to the internal network and nothing else.
//!
//! Both are named from the attempt and torn down with it. A shared mediator
//! would let one task's allowlist serve another task's traffic; a leaked
//! network is the leftover state that makes reuse a leak.
//!
//! See `docs/adr/ADR-0002-egress-mediation.md`.

use std::collections::HashMap;
use std::time::Duration;

use bollard::container::{
    Config, CreateContainerOptions, LogsOptions, RemoveContainerOptions, StartContainerOptions,
};
use bollard::models::{EndpointSettings, HostConfig};
use bollard::network::{ConnectNetworkOptions, CreateNetworkOptions};
use bollard::Docker;
use cortex_core::execution_job::{Blocked, BlockedReason, ExecutionJob};
use futures_util::StreamExt;

use super::policy::Endpoint;

/// Port the mediator listens on inside its network. Fixed, because it is an
/// address on a private network with exactly two members and nothing gains
/// from making it configurable.
const MEDIATOR_PORT: u16 = 3128;

/// The line the mediator prints once its listener is bound.
///
/// The sandbox is not started until this is seen, so a task never races the
/// mediator and reports a network failure that was really a startup order bug.
const READY_MARKER: &str = "cortex-egress ready";

/// How long to wait for that line before giving up and blocking the job.
const READY_TIMEOUT: Duration = Duration::from_secs(20);

/// The mediator image, digest-pinned in production exactly like the runner
/// image. Recorded on the job, because "what could this task reach" is only
/// answerable if you know what was enforcing the answer.
pub fn mediator_image() -> String {
    std::env::var("CORTEX_EGRESS_IMAGE").unwrap_or_else(|_| "cortex/egress:phase-a".to_string())
}

/// A provisioned mediator and its network. Dropping this does **not** tear it
/// down — teardown is async and must be awaited, so every caller is explicit
/// about it. See [`Egress::teardown`].
pub struct Egress {
    network_id: String,
    network_name: String,
    container_id: String,
    endpoints: Vec<Endpoint>,
    image: String,
}

impl Egress {
    /// The network the sandbox must join, and the only one it may join.
    pub fn network_name(&self) -> &str {
        &self.network_name
    }

    /// The proxy URL for `HTTP_PROXY` / `HTTPS_PROXY`.
    ///
    /// Resolved by name: Docker's embedded DNS resolves container names on a
    /// user-defined network, and the name is derived from the attempt, so it
    /// cannot collide with another attempt's mediator.
    pub fn proxy_url(&self) -> String {
        format!(
            "http://{}:{MEDIATOR_PORT}",
            mediator_name_from(&self.network_name)
        )
    }

    /// What was actually reachable, for the receipt.
    pub fn endpoints(&self) -> &[Endpoint] {
        &self.endpoints
    }

    /// The image that enforced it, for the receipt.
    pub fn image(&self) -> &str {
        &self.image
    }

    /// Remove the mediator and its network.
    ///
    /// Attempted on every path — success, failure, timeout, kill — and failures
    /// are logged rather than propagated, because a caller that is already
    /// tearing down has nothing useful to do with a second error. A survivor is
    /// a bug, not a race.
    pub async fn teardown(self, docker: &Docker) {
        let opts = RemoveContainerOptions {
            force: true,
            v: true,
            ..Default::default()
        };
        if let Err(e) = docker
            .remove_container(&self.container_id, Some(opts))
            .await
        {
            tracing::warn!(
                container_id = %self.container_id,
                error = %e,
                "egress mediator teardown failed"
            );
        }
        if let Err(e) = docker.remove_network(&self.network_id).await {
            tracing::warn!(
                network = %self.network_name,
                error = %e,
                "egress network teardown failed"
            );
        }
    }
}

fn network_name_for(job: &ExecutionJob) -> String {
    // The same key `execution_jobs` uses for one logical execution, so a
    // resubmission under the same attempt cannot end up with two networks.
    format!("cortex-egress-{}-{}", job.attempt_id, job.lease_gen)
}

fn mediator_name_from(network_name: &str) -> String {
    format!("{network_name}-mediator")
}

/// Stand up the mediator for a job, or refuse.
///
/// Every failure path returns [`Blocked`]. In particular there is no path here
/// that falls back to a normal bridge network: starting the sandbox with real
/// internet because the proxy did not come up is the exact downgrade invariant
/// 8 forbids, and it would silently undo the sandbox.
pub async fn provision(
    docker: &Docker,
    job: &ExecutionJob,
    endpoints: &[Endpoint],
    image: String,
) -> Result<Egress, Blocked> {
    let network_name = network_name_for(job);
    let mediator_name = mediator_name_from(&network_name);

    // `internal: true` is the load-bearing flag. Docker attaches no gateway, so
    // nothing on this network can reach off the host by any route at all.
    let network = docker
        .create_network(CreateNetworkOptions {
            name: network_name.clone(),
            driver: "bridge".to_string(),
            internal: true,
            check_duplicate: true,
            ..Default::default()
        })
        .await
        .map_err(|e| {
            Blocked::new(
                BlockedReason::NetworkPolicyUnenforceable,
                format!("could not create the isolated egress network: {e}"),
            )
        })?;
    let network_id = network.id;

    let allow = endpoints
        .iter()
        .map(|endpoint| endpoint.to_string())
        .collect::<Vec<_>>()
        .join(",");

    let config = Config {
        image: Some(image.clone()),
        cmd: Some(vec![
            "--listen".to_string(),
            format!("0.0.0.0:{MEDIATOR_PORT}"),
            "--allow".to_string(),
            allow,
        ]),
        // The mediator is in the trust boundary and holds nothing a task could
        // want: its whole configuration is the allowlist above. No environment,
        // no mounts, no socket.
        env: Some(Vec::new()),
        attach_stdout: Some(true),
        attach_stderr: Some(true),
        host_config: Some(HostConfig {
            // Starts on the internal network; `bridge` is attached below, so
            // the dual-homing is one explicit step rather than a default.
            network_mode: Some(network_name.clone()),
            binds: Some(Vec::new()),
            cap_drop: Some(vec!["ALL".to_string()]),
            security_opt: Some(vec!["no-new-privileges:true".to_string()]),
            readonly_rootfs: Some(true),
            privileged: Some(false),
            auto_remove: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    };

    let created = match docker
        .create_container(
            Some(CreateContainerOptions {
                name: mediator_name.clone(),
                platform: None,
            }),
            config,
        )
        .await
    {
        Ok(created) => created,
        Err(e) => {
            let _ = docker.remove_network(&network_id).await;
            return Err(Blocked::new(
                BlockedReason::NetworkPolicyUnenforceable,
                format!("could not create the egress mediator: {e}"),
            ));
        }
    };
    let container_id = created.id;

    let egress = Egress {
        network_id,
        network_name,
        container_id,
        endpoints: endpoints.to_vec(),
        image,
    };

    // The second home. Without this the mediator is as landlocked as the
    // sandbox and every request fails; with it, it is the only way out.
    if let Err(e) = docker
        .connect_network(
            "bridge",
            ConnectNetworkOptions {
                container: egress.container_id.clone(),
                endpoint_config: EndpointSettings::default(),
            },
        )
        .await
    {
        egress.teardown(docker).await;
        return Err(Blocked::new(
            BlockedReason::NetworkPolicyUnenforceable,
            format!("could not give the egress mediator a route out: {e}"),
        ));
    }

    if let Err(e) = docker
        .start_container(&egress.container_id, None::<StartContainerOptions<String>>)
        .await
    {
        egress.teardown(docker).await;
        return Err(Blocked::new(
            BlockedReason::NetworkPolicyUnenforceable,
            format!("could not start the egress mediator: {e}"),
        ));
    }

    if let Err(reason) = wait_until_ready(docker, &egress.container_id).await {
        egress.teardown(docker).await;
        return Err(Blocked::new(
            BlockedReason::NetworkPolicyUnenforceable,
            reason,
        ));
    }

    tracing::info!(
        attempt_id = %job.attempt_id,
        network = %egress.network_name,
        allow = ?egress.endpoints.iter().map(|e| e.to_string()).collect::<Vec<_>>(),
        "egress mediator ready"
    );
    Ok(egress)
}

/// Read the mediator's logs until it says it is listening.
///
/// Polling a port would be the obvious alternative and is worse: the mediator's
/// listener is on the internal network, so a successful connect from the host
/// would prove something about the bridge rather than about the network the
/// sandbox will use.
async fn wait_until_ready(docker: &Docker, container_id: &str) -> Result<(), String> {
    let mut logs = docker.logs(
        container_id,
        Some(LogsOptions::<String> {
            follow: true,
            stdout: true,
            stderr: true,
            ..Default::default()
        }),
    );

    let deadline = tokio::time::sleep(READY_TIMEOUT);
    tokio::pin!(deadline);
    let mut tail = String::new();

    loop {
        tokio::select! {
            _ = &mut deadline => {
                return Err(format!(
                    "the egress mediator did not report ready within {}s; last output: {}",
                    READY_TIMEOUT.as_secs(),
                    if tail.is_empty() { "(none)" } else { tail.trim() }
                ));
            }
            next = logs.next() => match next {
                Some(Ok(chunk)) => {
                    let text = chunk.to_string();
                    if text.contains(READY_MARKER) {
                        return Ok(());
                    }
                    tail.push_str(&text);
                }
                Some(Err(e)) => return Err(format!("could not read mediator output: {e}")),
                None => {
                    // The stream ended without the marker, which means the
                    // process exited. Its output is the diagnosis.
                    return Err(format!(
                        "the egress mediator exited before reporting ready: {}",
                        if tail.is_empty() { "(no output)" } else { tail.trim() }
                    ));
                }
            }
        }
    }
}

/// The three proxy variables handed to the sandbox.
///
/// `sanctioned_env` returns an empty vector, and the reasoning behind that —
/// a filtered environment is a denylist somebody has to keep correct forever —
/// still holds. These are an explicit exception with a stated reason, not the
/// start of a passthrough:
///
/// - They carry no secret. The value is an address on a private network with
///   two members, one of which is the task.
/// - They are not the enforcement. The task can unset all three and still
///   reach nothing: there is no route.
/// - Without them every well-behaved package manager fails for a reason that
///   looks like a bug in Cortex rather than a policy.
///
/// `NO_PROXY` names nothing, because there is nothing the sandbox may reach
/// directly.
pub fn proxy_env(proxy_url: &str) -> Vec<String> {
    // Both cases: some clients read the lowercase form only, and a client that
    // reads neither is not made less safe by that.
    let mut env = Vec::with_capacity(6);
    for name in ["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy"] {
        env.push(format!("{name}={proxy_url}"));
    }
    env.push("NO_PROXY=".to_string());
    env.push("no_proxy=".to_string());
    env
}

/// The endpoint config that attaches a sandbox to the mediator's network and
/// to nothing else.
pub fn sandbox_endpoints(network_name: &str) -> HashMap<String, EndpointSettings> {
    let mut map = HashMap::new();
    map.insert(network_name.to_string(), EndpointSettings::default());
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_env_carries_no_secret_and_exempts_nothing() {
        let env = proxy_env("http://mediator:3128");
        assert_eq!(env.len(), 6);
        assert!(env.iter().any(|v| v == "HTTPS_PROXY=http://mediator:3128"));
        // An entry here would be a host the sandbox reaches without the
        // mediator, which is the one thing this design does not permit.
        assert!(env.iter().any(|v| v == "NO_PROXY="));
        assert!(env.iter().any(|v| v == "no_proxy="));
        assert!(
            !env.iter().any(|v| v.contains("TOKEN") || v.contains("KEY")),
            "the proxy variables are the exception; they are not a passthrough"
        );
    }

    #[test]
    fn the_sandbox_joins_exactly_one_network() {
        let endpoints = sandbox_endpoints("cortex-egress-a1-4");
        assert_eq!(endpoints.len(), 1);
        assert!(endpoints.contains_key("cortex-egress-a1-4"));
        assert!(
            !endpoints.contains_key("bridge"),
            "a sandbox on bridge has a route out and the mediator is pointless"
        );
    }

    #[test]
    fn names_are_derived_from_the_attempt() {
        // Two attempts must not share a mediator: one task's allowlist would
        // be serving another task's traffic.
        let a = "cortex-egress-attempt-1-4";
        let b = "cortex-egress-attempt-1-5";
        assert_ne!(mediator_name_from(a), mediator_name_from(b));
    }
}
