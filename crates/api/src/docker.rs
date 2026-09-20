use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bollard::container::{
    Config, CreateContainerOptions, StartContainerOptions, StopContainerOptions,
};
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::models::HostConfig;
use bollard::Docker;
use futures_util::StreamExt;
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

use crate::metrics::{record_container_op, ContainerOp};
use crate::state::AppState;

const BYOS_IMAGE: &str = "cortex-byos:latest";
const IDLE_TIMEOUT_SECS: i64 = 900;
const CONTAINER_CPU_QUOTA: i64 = 100_000;
const CONTAINER_MEMORY: i64 = 512 * 1024 * 1024;
/// Enough for a provider CLI and the processes it spawns, far short of a fork
/// bomb. Unlike the other limits this one cannot break a workload that was
/// behaving — a container legitimately needing 512 processes to log in does
/// not exist.
const CONTAINER_PIDS_LIMIT: i64 = 512;

pub struct ContainerManager {
    docker: Docker,
    cache: RwLock<HashMap<String, String>>,
}

pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i64,
}

pub struct PendingContainerAuth {
    pub container_id: String,
    pub provider: String,
    pub exec_id: String,
    pub started_at: i64,
}

impl ContainerManager {
    pub fn new() -> Result<Self, String> {
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| format!("failed to connect to Docker: {e}"))?;
        Ok(Self {
            docker,
            cache: RwLock::new(HashMap::new()),
        })
    }

    fn container_name(user_id: &str) -> String {
        format!(
            "cortex-byos-{}",
            user_id.replace(|c: char| !c.is_alphanumeric() && c != '-', "_")
        )
    }

    pub async fn ensure_container(
        &self,
        db: &crate::db::Database,
        user_id: &str,
        provider: &str,
    ) -> Result<String, String> {
        // Check in-memory cache first
        {
            let cache = self.cache.read().await;
            if let Some(id) = cache.get(user_id) {
                let inspect_start = std::time::Instant::now();
                let inspect = self.docker.inspect_container(id, None).await;
                record_container_op(
                    ContainerOp::Inspect,
                    inspect_start,
                    if inspect.is_ok() { "ok" } else { "error" },
                );
                match inspect {
                    Ok(info) => {
                        let running = info.state.as_ref().and_then(|s| s.running).unwrap_or(false);
                        if running {
                            tracing::debug!(user_id, container_id = %id, "reusing running container (cache hit)");
                            return Ok(id.clone());
                        }
                        // Container exists but stopped — start it
                        let start = std::time::Instant::now();
                        let res = self
                            .docker
                            .start_container(id, None::<StartContainerOptions<String>>)
                            .await;
                        record_container_op(
                            ContainerOp::Start,
                            start,
                            if res.is_ok() { "ok" } else { "error" },
                        );
                        res.map_err(|e| {
                            tracing::warn!(user_id, container_id = %id, error = %e, "failed to start cached container");
                            format!("failed to start container: {e}")
                        })?;
                        db.update_container_status(user_id, "running");
                        tracing::info!(user_id, container_id = %id, "started stopped container (cache hit)");
                        return Ok(id.clone());
                    }
                    Err(_) => {
                        // Container gone, remove from cache
                    }
                }
            }
        }

        // Check DB
        if let Some(uc) = db.get_user_container(user_id) {
            let inspect_start = std::time::Instant::now();
            let inspect = self.docker.inspect_container(&uc.container_id, None).await;
            record_container_op(
                ContainerOp::Inspect,
                inspect_start,
                if inspect.is_ok() { "ok" } else { "error" },
            );
            match inspect {
                Ok(info) => {
                    let running = info.state.as_ref().and_then(|s| s.running).unwrap_or(false);
                    if !running {
                        let start = std::time::Instant::now();
                        let res = self
                            .docker
                            .start_container(
                                &uc.container_id,
                                None::<StartContainerOptions<String>>,
                            )
                            .await;
                        record_container_op(
                            ContainerOp::Start,
                            start,
                            if res.is_ok() { "ok" } else { "error" },
                        );
                        res.map_err(|e| {
                            tracing::warn!(user_id, container_id = %uc.container_id, error = %e, "failed to start db-tracked container");
                            format!("failed to start container: {e}")
                        })?;
                        tracing::info!(user_id, container_id = %uc.container_id, "started stopped container (db hit)");
                    }
                    db.update_container_status(user_id, "running");
                    self.cache
                        .write()
                        .await
                        .insert(user_id.to_string(), uc.container_id.clone());
                    return Ok(uc.container_id);
                }
                Err(_) => {
                    // Container was removed externally, clean up DB
                    tracing::warn!(user_id, container_id = %uc.container_id, "db-tracked container missing from docker; cleaning up");
                    db.delete_user_container(user_id);
                }
            }
        }

        // Create new container
        let name = Self::container_name(user_id);
        let config = Config {
            image: Some(BYOS_IMAGE.to_string()),
            cmd: Some(vec!["sleep".into(), "infinity".into()]),
            hostname: Some("cortex-sandbox".into()),
            user: Some("sandbox".into()),
            host_config: Some(HostConfig {
                cpu_quota: Some(CONTAINER_CPU_QUOTA),
                memory: Some(CONTAINER_MEMORY),
                memory_swap: Some(CONTAINER_MEMORY),
                pids_limit: Some(CONTAINER_PIDS_LIMIT),
                security_opt: Some(vec!["no-new-privileges:true".into()]),
                // Two hardening measures the check runner has and this
                // deliberately does not, because they would change behaviour
                // that cannot be tested from CI (there is no Docker daemon in
                // the `rust` job) and this path is how a user connects their
                // credentials — breaking it breaks onboarding:
                //
                //   network_mode "none" — impossible here by design. A
                //     provider CLI has to reach the provider. What this needs
                //     instead is an egress allowlist per provider (PLAN 3.4),
                //     which means a proxy or a custom network, not a flag.
                //   cap_drop ALL — almost certainly safe for a userspace CLI,
                //     but "almost certainly" is not a claim to ship untested
                //     onto the credential path. It wants one manual run
                //     against a real daemon first.
                //
                // Recorded rather than silently omitted: VERIFIER.md's
                // executor trust gap is about exactly this container, and the
                // gap between "the check runner is hardened" and "the executor
                // is hardened" should be visible where the executor is built.
                ..Default::default()
            }),
            ..Default::default()
        };

        let create_opts = CreateContainerOptions {
            name: name.clone(),
            ..Default::default()
        };
        let create_start = std::time::Instant::now();
        let create_res = self
            .docker
            .create_container(Some(create_opts), config)
            .await;
        record_container_op(
            ContainerOp::Create,
            create_start,
            if create_res.is_ok() { "ok" } else { "error" },
        );

        let container_id = match create_res {
            Ok(resp) => resp.id,
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 409, ..
            }) => {
                tracing::info!(user_id, container_name = %name, "container name conflict, reusing existing");
                let inspect = self
                    .docker
                    .inspect_container(&name, None)
                    .await
                    .map_err(|e| format!("failed to inspect conflicting container: {e}"))?;
                let id = inspect.id.ok_or("conflicting container has no id")?;
                let running = inspect
                    .state
                    .as_ref()
                    .and_then(|s| s.running)
                    .unwrap_or(false);
                if !running {
                    self.docker
                        .start_container(&id, None::<StartContainerOptions<String>>)
                        .await
                        .map_err(|e| format!("failed to start existing container: {e}"))?;
                }
                let record_id = uuid::Uuid::new_v4().to_string();
                db.upsert_user_container(&record_id, user_id, &id, provider, "running");
                self.cache
                    .write()
                    .await
                    .insert(user_id.to_string(), id.clone());
                return Ok(id);
            }
            Err(e) => {
                tracing::error!(user_id, error = %e, "failed to create BYOS container");
                return Err(format!("failed to create container: {e}"));
            }
        };

        let start = std::time::Instant::now();
        let start_res = self
            .docker
            .start_container(&container_id, None::<StartContainerOptions<String>>)
            .await;
        record_container_op(
            ContainerOp::Start,
            start,
            if start_res.is_ok() { "ok" } else { "error" },
        );
        start_res.map_err(|e| {
            tracing::error!(user_id, container_id = %container_id, error = %e, "failed to start newly created container");
            format!("failed to start container: {e}")
        })?;

        let id = Uuid::new_v4().to_string();
        db.upsert_user_container(&id, user_id, &container_id, provider, "running");
        self.cache
            .write()
            .await
            .insert(user_id.to_string(), container_id.clone());

        db.audit_log(
            user_id,
            "system",
            "container.created",
            Some("container"),
            Some(&container_id),
            Some(&format!("{{\"provider\":\"{provider}\"}}")),
            None,
        );
        tracing::info!(user_id, container_id = %container_id, "created BYOS container");
        Ok(container_id)
    }

    pub async fn exec_in_container(
        &self,
        container_id: &str,
        cmd: &[&str],
        stdin_data: Option<&str>,
        timeout: Duration,
    ) -> Result<ExecResult, String> {
        let op_start = std::time::Instant::now();
        let exec_opts = CreateExecOptions {
            cmd: Some(cmd.iter().map(|s| s.to_string()).collect()),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            attach_stdin: stdin_data.is_some().then_some(true),
            ..Default::default()
        };

        let exec = match self.docker.create_exec(container_id, exec_opts).await {
            Ok(e) => e,
            Err(e) => {
                record_container_op(ContainerOp::Exec, op_start, "error");
                return Err(format!("failed to create exec: {e}"));
            }
        };

        let start_result = match self.docker.start_exec(&exec.id, None).await {
            Ok(r) => r,
            Err(e) => {
                record_container_op(ContainerOp::Exec, op_start, "error");
                return Err(format!("failed to start exec: {e}"));
            }
        };

        let mut stdout = String::new();
        let mut stderr = String::new();

        if let StartExecResults::Attached {
            mut output,
            mut input,
        } = start_result
        {
            if let Some(data) = stdin_data {
                use tokio::io::AsyncWriteExt;
                let _ = input.write_all(data.as_bytes()).await;
                let _ = input.write_all(b"\n").await;
                let _ = input.shutdown().await;
            }

            let collect_fut = async {
                while let Some(Ok(msg)) = output.next().await {
                    match msg {
                        bollard::container::LogOutput::StdOut { message } => {
                            stdout.push_str(&String::from_utf8_lossy(&message));
                        }
                        bollard::container::LogOutput::StdErr { message } => {
                            stderr.push_str(&String::from_utf8_lossy(&message));
                        }
                        _ => {}
                    }
                }
            };

            if tokio::time::timeout(timeout, collect_fut).await.is_err() {
                record_container_op(ContainerOp::Exec, op_start, "timeout");
                tracing::warn!(
                    container_id,
                    timeout_secs = timeout.as_secs(),
                    "container exec timed out"
                );
                return Err(format!("exec timed out after {}s", timeout.as_secs()));
            }
        }

        let inspect = match self.docker.inspect_exec(&exec.id).await {
            Ok(i) => i,
            Err(e) => {
                record_container_op(ContainerOp::Exec, op_start, "error");
                return Err(format!("failed to inspect exec: {e}"));
            }
        };
        let exit_code = inspect.exit_code.unwrap_or(-1);

        record_container_op(ContainerOp::Exec, op_start, "ok");
        if exit_code != 0 {
            crate::metrics::metrics()
                .container_exec_failures_total
                .inc();
            tracing::debug!(
                container_id,
                exit_code,
                "container exec returned non-zero exit code"
            );
        }

        Ok(ExecResult {
            stdout,
            stderr,
            exit_code,
        })
    }

    pub async fn exec_stream(
        &self,
        container_id: &str,
        cmd: &[&str],
        tx: mpsc::Sender<String>,
        timeout: Duration,
    ) -> Result<i64, String> {
        let exec_opts = CreateExecOptions {
            cmd: Some(cmd.iter().map(|s| s.to_string()).collect()),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            ..Default::default()
        };

        let exec = self
            .docker
            .create_exec(container_id, exec_opts)
            .await
            .map_err(|e| format!("failed to create exec: {e}"))?;

        let start_result = self
            .docker
            .start_exec(&exec.id, None)
            .await
            .map_err(|e| format!("failed to start exec: {e}"))?;

        if let StartExecResults::Attached { mut output, .. } = start_result {
            let stream_fut = async {
                while let Some(Ok(msg)) = output.next().await {
                    if let bollard::container::LogOutput::StdOut { message } = msg {
                        let text = String::from_utf8_lossy(&message).to_string();
                        if tx.send(text).await.is_err() {
                            break;
                        }
                    }
                }
            };

            if tokio::time::timeout(timeout, stream_fut).await.is_err() {
                return Err(format!(
                    "exec stream timed out after {}s",
                    timeout.as_secs()
                ));
            }
        }

        let inspect = self
            .docker
            .inspect_exec(&exec.id)
            .await
            .map_err(|e| format!("failed to inspect exec: {e}"))?;
        Ok(inspect.exit_code.unwrap_or(-1))
    }

    pub async fn stop_container(&self, container_id: &str) -> Result<(), String> {
        let op_start = std::time::Instant::now();
        let opts = StopContainerOptions { t: 10 };
        let res = self.docker.stop_container(container_id, Some(opts)).await;
        record_container_op(
            ContainerOp::Stop,
            op_start,
            if res.is_ok() { "ok" } else { "error" },
        );
        res.map_err(|e| {
            tracing::warn!(container_id, error = %e, "failed to stop container");
            format!("failed to stop container: {e}")
        })
    }

    pub async fn remove_container(&self, container_id: &str) -> Result<(), String> {
        // Stop first if running
        let _ = self.stop_container(container_id).await;
        let op_start = std::time::Instant::now();
        let res = self.docker.remove_container(container_id, None).await;
        record_container_op(
            ContainerOp::Remove,
            op_start,
            if res.is_ok() { "ok" } else { "error" },
        );
        res.map_err(|e| {
            tracing::warn!(container_id, error = %e, "failed to remove container");
            format!("failed to remove container: {e}")
        })
    }

    /// Start an interactive exec for CLI login (returns exec_id for later stdin piping)
    pub async fn start_login_exec(
        &self,
        container_id: &str,
        cmd: &[&str],
    ) -> Result<(String, String), String> {
        let op_start = std::time::Instant::now();
        let exec_opts = CreateExecOptions {
            cmd: Some(cmd.iter().map(|s| s.to_string()).collect()),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            attach_stdin: Some(true),
            tty: Some(false),
            ..Default::default()
        };

        let exec = match self.docker.create_exec(container_id, exec_opts).await {
            Ok(e) => e,
            Err(e) => {
                record_container_op(ContainerOp::LoginExec, op_start, "error");
                return Err(format!("failed to create login exec: {e}"));
            }
        };

        let start_result = match self.docker.start_exec(&exec.id, None).await {
            Ok(r) => r,
            Err(e) => {
                record_container_op(ContainerOp::LoginExec, op_start, "error");
                return Err(format!("failed to start login exec: {e}"));
            }
        };

        // Read initial stdout to capture the OAuth URL
        let mut captured_output = String::new();
        if let StartExecResults::Attached { mut output, .. } = start_result {
            let read_url = async {
                while let Some(Ok(msg)) = output.next().await {
                    match msg {
                        bollard::container::LogOutput::StdOut { message }
                        | bollard::container::LogOutput::StdErr { message } => {
                            let text = String::from_utf8_lossy(&message);
                            captured_output.push_str(&text);
                            // Check if we've captured a URL
                            if captured_output.contains("http://")
                                || captured_output.contains("https://")
                            {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            };

            // Give the CLI 15 seconds to output the OAuth URL
            if tokio::time::timeout(Duration::from_secs(15), read_url)
                .await
                .is_err()
            {
                record_container_op(ContainerOp::LoginExec, op_start, "timeout");
                tracing::warn!(
                    container_id,
                    "timeout waiting for login CLI to output auth URL"
                );
                return Err("timeout waiting for CLI to output auth URL".into());
            }
        }

        record_container_op(ContainerOp::LoginExec, op_start, "ok");
        Ok((exec.id, captured_output))
    }

    /// Pipe auth code to a running login exec
    pub async fn complete_login_exec(
        &self,
        container_id: &str,
        provider: &str,
        code: &str,
    ) -> Result<String, String> {
        // Run a separate exec to complete auth by piping the code
        let cmd: Vec<&str> = match provider {
            "claude" => vec!["claude", "auth", "login", "--code", code],
            "openai" | "codex" => vec!["codex", "login", "--code", code],
            _ => return Err(format!("unknown provider: {provider}")),
        };

        let result = self
            .exec_in_container(container_id, &cmd, None, Duration::from_secs(30))
            .await?;

        if result.exit_code == 0 {
            Ok(result.stdout)
        } else {
            Err(format!(
                "auth failed (exit {}): {}",
                result.exit_code, result.stderr
            ))
        }
    }
}

pub fn spawn_idle_reaper(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            let (cm, db) = match (&state.container_manager, &state.db) {
                (Some(cm), Some(db)) => (cm, db),
                _ => continue,
            };

            let threshold = chrono::Utc::now().timestamp() - IDLE_TIMEOUT_SECS;
            let idle = db.list_idle_containers(threshold);
            for container in &idle {
                match cm.stop_container(&container.container_id).await {
                    Ok(()) => {
                        db.update_container_status(&container.user_id, "stopped");
                        crate::metrics::metrics().containers_reaped_total.inc();
                        tracing::info!(
                            user_id = %container.user_id,
                            container_id = %container.container_id,
                            "reaped idle BYOS container"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            user_id = %container.user_id,
                            "failed to reap idle container: {e}"
                        );
                    }
                }
            }

            // Refresh the container population gauge on each reaper tick so the
            // Prometheus snapshot stays current even with no /metrics scrapes.
            let all = db.list_all_containers();
            let running = all.iter().filter(|c| c.status == "running").count() as i64;
            let stopped = (all.len() as i64) - running;
            crate::metrics::set_container_population(running, stopped);
        }
    });
}
