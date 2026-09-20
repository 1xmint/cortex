//! Phase A of the execution substrate: run verification checks in a container
//! the worker cannot reach.
//!
//! Task **V1** of `cortex/plan/VERIFIER.md`, second half. The contract lives in
//! [`cortex_core::verification`]; this is the implementation that makes a
//! verdict mean something.
//!
//! Every constraint below is load-bearing, so none of them are configurable:
//!
//! - **A fresh container per check.** No state survives from the worker, from
//!   another customer, or from a previous attempt.
//! - **`--network none`.** A check that can reach the network can fetch a
//!   passing result.
//! - **The tree is mounted read-only.** A check cannot edit its way to green.
//! - **One writable scratch tmpfs, outside the tree.** See below — this is not
//!   a relaxation of the line above, it is what makes it survivable.
//! - **No *inherited* environment.** Not a filtered environment and not an
//!   empty one: exactly [`SCRATCH_ENV`], a compile-time constant. Nothing is
//!   read from the process environment, so the runner cannot receive provider
//!   keys, Clerk secrets, or the production database path even by accident.
//! - **The image digest is recorded on every execution.** "It passed" is only
//!   reproducible if *where* it passed is pinned.
//!
//! # Why there is a scratch tmpfs (F9)
//!
//! There was not one, and the consequence was that **no check the derivation
//! produces could execute**. `ecosystem:cargo-check` and `ecosystem:cargo-test`
//! write to `target/`; with the tree mounted read-only, the rootfs read-only,
//! and no writable path anywhere in the container, every cargo check failed
//! before compiling a line — and `CheckOutcome::Failed` is a verdict *about the
//! customer's work*. So the verifier's failure mode was to blame the customer
//! for its own missing mount.
//!
//! Nothing caught it because every test of the driver uses a `ScriptedRunner`.
//! The specs were tested, the verdict arithmetic was tested, and the one place
//! a spec becomes a running process had no test at all — the same shape as F0,
//! at a different boundary.
//!
//! The fix keeps the invariant that matters. The *tree* is still read-only, so
//! a check still cannot edit its way to green: the source it grades is the
//! source that was delivered. What it gains is somewhere to put build output,
//! at a path outside the tree, on a tmpfs that dies with the container. A check
//! can write freely to `/scratch` and change nothing about what it is grading.
//!
//! **Known gap, not papered over:** `ecosystem:npm-ci` installs into
//! `node_modules/` *inside* the tree, so a scratch directory does not rescue
//! it. Redirecting npm's install prefix changes resolution semantics, and a
//! writable tree copy would give up the property above. The npm floor is
//! therefore still unexecutable here and is recorded as such rather than
//! silently producing `Failed`.
//!
//! Phase B swaps this for Firecracker-class microVMs behind the same trait,
//! which is a deployment decision rather than a redesign.

use std::time::{Duration, Instant};

use bollard::container::{
    Config, CreateContainerOptions, LogsOptions, RemoveContainerOptions, StartContainerOptions,
    WaitContainerOptions,
};
use bollard::models::HostConfig;
use bollard::Docker;
use cortex_core::verification::{
    CheckExecution, CheckOutcome, CheckRunner, CheckSpec, RunnerError, TreeSnapshot,
};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};

/// Where the delivered tree is mounted inside the container.
const WORKDIR: &str = "/work";

/// The one writable path in the container. A tmpfs, so it never touches the
/// host and cannot outlive the check.
const SCRATCH: &str = "/scratch";

/// Size cap on the scratch tmpfs. A tmpfs is host RAM, so an unbounded one is a
/// way for a check to take the machine down; this is generous for a build tree
/// and far short of dangerous.
const SCRATCH_BYTES: usize = 4 * 1024 * 1024 * 1024;

/// The complete environment of a check container.
///
/// A `const`, not a function over `std::env`. That is the point: there is no
/// expression here that *could* read the host's environment, so "the runner
/// never sees a provider key" is a property of the type rather than of a filter
/// somebody has to keep correct. Adding a variable means editing this list, in
/// a diff, next to this comment.
///
/// Every entry exists to move a tool's write path off the read-only tree.
/// `PATH` is deliberately absent — Docker merges the image's own `ENV` with
/// what is supplied here, so the image's `PATH` survives, and restating it
/// would let this list silently override the image.
const SCRATCH_ENV: &[&str] = &[
    // Build output. Without this cargo writes to /work/target and dies on a
    // read-only filesystem — F9, and the reason this list exists.
    "CARGO_TARGET_DIR=/scratch/target",
    // The registry cache and cargo's own config. Cargo refuses to run at all
    // if it cannot establish a home directory.
    "CARGO_HOME=/scratch/cargo",
    // The mount point itself, not a subdirectory of it. The tmpfs is mounted
    // over `/scratch` at start and hides anything the image created there, so
    // `/scratch/home` would not exist when a tool looked for it. Cargo does
    // `mkdir -p` on its target and home and copes either way; a tool that does
    // a plain `mkdir` does not, and pointing at the mount point needs nothing
    // to have gone right beforehand.
    "HOME=/scratch",
    "TMPDIR=/scratch",
    "npm_config_cache=/scratch/npm",
];
/// Bytes of output retained verbatim on the execution record. The full log is
/// digest-addressed and stored separately; this is what a dispute reads first.
const OUTPUT_TAIL_BYTES: usize = 16 * 1024;

const MEMORY_BYTES: i64 = 2 * 1024 * 1024 * 1024;
/// One full CPU: quota / period.
const CPU_QUOTA: i64 = 100_000;
const CPU_PERIOD: i64 = 100_000;
/// Enough for a test runner spawning workers, far short of a fork bomb.
const PIDS_LIMIT: i64 = 512;

pub struct ContainerCheckRunner {
    docker: Docker,
    /// Pinned by digest (`repo@sha256:…`), not by tag. A mutable tag would
    /// make every past receipt unreproducible the next time it moved.
    image: String,
}

impl ContainerCheckRunner {
    pub fn new(image: impl Into<String>) -> Result<Self, String> {
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| format!("failed to connect to Docker: {e}"))?;
        Ok(Self {
            docker,
            image: image.into(),
        })
    }

    fn container_config(&self, tree: &TreeSnapshot, check: &CheckSpec) -> Config<String> {
        container_config(&self.image, tree, check)
    }
}

/// The configuration a check container is created with.
///
/// A free function taking the image rather than a method on the runner, so it
/// can be asserted on without a Docker client. That is not a stylistic
/// preference: the reason F9 lived here is that reaching this code required a
/// daemon, so no test ever reached it.
fn container_config(image: &str, tree: &TreeSnapshot, check: &CheckSpec) -> Config<String> {
    Config {
        image: Some(image.to_string()),
        cmd: Some(check.command.clone()),
        working_dir: Some(WORKDIR.to_string()),
        // A constant, not a filter. See `SCRATCH_ENV` and the module docs.
        env: Some(SCRATCH_ENV.iter().map(|s| (*s).to_string()).collect()),
        user: Some("sandbox".to_string()),
        network_disabled: Some(true),
        host_config: Some(HostConfig {
            binds: Some(vec![format!("{}:{}:ro", tree.path, WORKDIR)]),
            // The only writable path, and it is not the tree. `exec` is
            // required rather than lax: cargo runs build scripts and test
            // binaries out of its target directory, so a `noexec` scratch
            // would fail every check it was added to rescue.
            tmpfs: Some(std::collections::HashMap::from([(
                SCRATCH.to_string(),
                format!("rw,exec,nosuid,nodev,size={SCRATCH_BYTES},mode=1777"),
            )])),
            network_mode: Some("none".to_string()),
            memory: Some(MEMORY_BYTES),
            memory_swap: Some(MEMORY_BYTES),
            cpu_quota: Some(CPU_QUOTA),
            cpu_period: Some(CPU_PERIOD),
            pids_limit: Some(PIDS_LIMIT),
            cap_drop: Some(vec!["ALL".to_string()]),
            security_opt: Some(vec!["no-new-privileges:true".to_string()]),
            // The tree mount is read-only and so is the rest of the
            // filesystem, so a check cannot cache a result for the next one.
            // The scratch tmpfs above is the single deliberate exception, and
            // it dies with the container.
            readonly_rootfs: Some(true),
            // Removal is explicit below, in every path including timeout,
            // so a leaked container is a bug rather than a race.
            auto_remove: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    }
}

impl ContainerCheckRunner {
    /// Best-effort teardown. A failure here cannot change a verdict — it is an
    /// operational problem, and the reaper is the backstop.
    async fn remove(&self, container_id: &str) {
        let opts = RemoveContainerOptions {
            force: true,
            v: true,
            ..Default::default()
        };
        if let Err(e) = self.docker.remove_container(container_id, Some(opts)).await {
            tracing::warn!(container_id, error = %e, "failed to remove check container");
        }
    }

    /// Collect stdout and stderr after the container exits, interleaved in the
    /// order Docker recorded them.
    async fn collect_output(&self, container_id: &str) -> String {
        let opts = LogsOptions::<String> {
            stdout: true,
            stderr: true,
            follow: false,
            ..Default::default()
        };
        let mut stream = self.docker.logs(container_id, Some(opts));
        let mut output = String::new();
        while let Some(item) = stream.next().await {
            match item {
                Ok(chunk) => output.push_str(&chunk.to_string()),
                Err(e) => {
                    tracing::warn!(container_id, error = %e, "log stream error while collecting check output");
                    break;
                }
            }
        }
        output
    }

    fn execution(
        &self,
        check: &CheckSpec,
        outcome: CheckOutcome,
        exit_code: Option<i32>,
        started: Instant,
        output: &str,
    ) -> CheckExecution {
        CheckExecution {
            spec_id: check.id.clone(),
            exit_code,
            outcome,
            duration_ms: started.elapsed().as_millis() as u64,
            output_digest: digest_of(output),
            output_tail: tail_of(output, OUTPUT_TAIL_BYTES),
            runner_image: self.image.clone(),
        }
    }
}

impl CheckRunner for ContainerCheckRunner {
    async fn run(
        &self,
        tree: &TreeSnapshot,
        check: &CheckSpec,
    ) -> Result<CheckExecution, RunnerError> {
        if check.command.is_empty() {
            return Err(RunnerError::InvalidSpec(format!(
                "check {} has an empty command",
                check.id
            )));
        }

        let started = Instant::now();
        let name = format!("cortex-check-{}", uuid::Uuid::new_v4());
        let config = self.container_config(tree, check);

        let create = self
            .docker
            .create_container(
                Some(CreateContainerOptions {
                    name: name.clone(),
                    ..Default::default()
                }),
                config,
            )
            .await
            .map_err(|e| RunnerError::SnapshotFailed {
                tree_hash: tree.tree_hash.clone(),
                reason: format!("failed to create check container: {e}"),
            })?;
        let container_id = create.id;

        if let Err(e) = self
            .docker
            .start_container(&container_id, None::<StartContainerOptions<String>>)
            .await
        {
            self.remove(&container_id).await;
            return Err(RunnerError::ExecutionFailed(format!(
                "failed to start check container: {e}"
            )));
        }

        let mut wait = self
            .docker
            .wait_container(&container_id, None::<WaitContainerOptions<String>>);
        let waited =
            tokio::time::timeout(Duration::from_secs(check.timeout_secs), wait.next()).await;

        // Timeout is a verdict about the work, not about us: a hung test is
        // the defect the customer is paying to be protected from. Collect what
        // it managed to print before tearing it down, so the receipt shows
        // where it hung.
        let Ok(waited) = waited else {
            let output = self.collect_output(&container_id).await;
            self.remove(&container_id).await;
            return Ok(self.execution(check, CheckOutcome::TimedOut, None, started, &output));
        };

        let exit_code = match waited {
            Some(Ok(response)) => response.status_code,
            // A check that ran and failed. Bollard reports a non-zero exit from
            // `wait_container` as an *error* carrying the code, not as an `Ok`
            // response, so without this arm every genuinely failing check was
            // absorbed by the one below as "we could not run it" — and
            // `CheckOutcome::Failed` was unreachable from the container runner.
            //
            // That was F20, and it is the last place the same mistake lived:
            // the verifier could execute a check, watch it fail, and record
            // that it had not been executed. A `Verdict::Failed` cannot be
            // reached if no check can be recorded as failing, so the refund
            // path stayed unreachable even after F14 and F19.
            Some(Err(ref e)) if exit_code_from_wait_error(e).is_some() => {
                exit_code_from_wait_error(e).expect("guarded by the arm above")
            }
            // The container vanished, or Docker failed mid-wait. We genuinely
            // do not know what happened to the check, so this is ours to
            // absorb — INCONCLUSIVE, never a charge and never a refund.
            Some(Err(e)) => {
                self.remove(&container_id).await;
                return Err(RunnerError::ExecutionFailed(format!(
                    "wait failed for check {}: {e}",
                    check.id
                )));
            }
            None => {
                self.remove(&container_id).await;
                return Err(RunnerError::ExecutionFailed(format!(
                    "wait stream ended without a status for check {}",
                    check.id
                )));
            }
        };

        let output = self.collect_output(&container_id).await;
        self.remove(&container_id).await;

        let outcome = if exit_code == 0 {
            CheckOutcome::Passed
        } else {
            CheckOutcome::Failed
        };

        Ok(self.execution(check, outcome, Some(exit_code as i32), started, &output))
    }

    fn runner_image(&self) -> &str {
        &self.image
    }
}

/// The exit code hiding inside a `wait_container` error, if that is what it is.
///
/// Bollard does not report a non-zero container exit as an `Ok` response. It
/// reports it as `Error::DockerContainerWaitError`, carrying the code. So the
/// obvious reading of the wait stream — `Ok` is an exit, `Err` is a failure of
/// ours — classifies **every check that runs and fails** as a check that could
/// not be run.
///
/// That was F20. Its consequence was that `CheckOutcome::Failed` was
/// unreachable from the container runner: the verifier could start a check,
/// watch it fail, and record `NotExecuted`. A verdict of `Failed` needs a check
/// recorded as failed, so the refund path stayed unreachable even after the
/// worker (F14) and the brain (F19) stopped discarding failing work.
///
/// Split out from the wait so it can be tested without a Docker daemon. The
/// reason F20 survived is that nothing in this file's tests ever reached the
/// container path — the same shape as F9, which its own comment warns about
/// twenty lines up.
fn exit_code_from_wait_error(e: &bollard::errors::Error) -> Option<i64> {
    match e {
        bollard::errors::Error::DockerContainerWaitError { code, .. } => Some(*code),
        _ => None,
    }
}

/// `sha256:…` over the full output, so a retained tail can be shown to be a
/// genuine excerpt of what actually ran.
fn digest_of(output: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(output.as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

/// Last `limit` bytes, cut at a character boundary and marked when truncated.
fn tail_of(output: &str, limit: usize) -> String {
    if output.len() <= limit {
        return output.to_string();
    }
    let mut start = output.len() - limit;
    while start < output.len() && !output.is_char_boundary(start) {
        start += 1;
    }
    format!("…[truncated]\n{}", &output[start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The configuration a check container would actually be created with.
    ///
    /// These assertions did not exist, which is why F9 survived: the driver's
    /// tests all use a `ScriptedRunner`, so nothing ever looked at the config
    /// this function returns. It needs no Docker daemon — `container_config` is
    /// pure — so there is no gate on it and no excuse for it to be absent.
    fn config_for(command: &[&str]) -> Config<String> {
        let tree = TreeSnapshot {
            tree_hash: "deadbeef".to_string(),
            path: "/tmp/cortex-verify-test".to_string(),
        };
        let check = CheckSpec {
            id: "ecosystem:cargo-check".to_string(),
            source: cortex_core::verification::CheckSource::Ecosystem,
            command: command.iter().map(|s| (*s).to_string()).collect(),
            timeout_secs: 600,
            required: true,
        };
        container_config("cortex/runner:test", &tree, &check)
    }

    #[test]
    fn a_cargo_check_has_somewhere_to_write() {
        // F9, at the boundary. `cargo check` writes to `target/`; the tree is
        // read-only and so is the rootfs, so without a writable path outside
        // the tree every cargo check failed before compiling anything — and
        // reported that failure as a verdict about the customer's work.
        let config = config_for(&["cargo", "check", "--locked"]);
        let host = config.host_config.expect("a host config is always set");

        let tmpfs = host.tmpfs.expect("the check has a writable scratch mount");
        let options = tmpfs
            .get(SCRATCH)
            .unwrap_or_else(|| panic!("scratch is mounted at {SCRATCH}: {tmpfs:?}"));
        assert!(options.contains("rw"), "scratch is not writable: {options}");
        assert!(
            options.contains("exec"),
            "cargo runs build scripts and test binaries out of its target \
             directory; a noexec scratch fails every check it was added to \
             rescue: {options}"
        );
        assert!(
            options.contains("size="),
            "an unbounded tmpfs is host RAM a check can exhaust: {options}"
        );

        let target = config
            .env
            .as_ref()
            .expect("env is always set")
            .iter()
            .find(|e| e.starts_with("CARGO_TARGET_DIR="))
            .expect("cargo is told where to write");
        assert!(
            target.contains(SCRATCH),
            "cargo's target directory is not on the scratch mount: {target}"
        );
    }

    #[test]
    fn the_tree_itself_stays_read_only() {
        // The half of the invariant the scratch mount must not have cost. A
        // check that can edit the tree can edit its way to green, and then the
        // verdict is about a tree nobody delivered.
        let config = config_for(&["cargo", "test", "--locked"]);
        let host = config.host_config.expect("a host config is always set");

        let binds = host.binds.expect("the tree is mounted");
        assert!(
            binds.iter().all(|b| b.ends_with(":ro")),
            "the delivered tree is writable: {binds:?}"
        );
        assert_eq!(
            host.readonly_rootfs,
            Some(true),
            "the root filesystem is writable"
        );

        // And the writable path is not inside the tree.
        let tmpfs = host.tmpfs.expect("scratch exists");
        for path in tmpfs.keys() {
            assert!(
                !path.starts_with(WORKDIR),
                "the writable mount is inside the tree at {path}; a check can \
                 now edit what it is grading"
            );
        }
    }

    #[test]
    fn no_host_variable_reaches_a_check() {
        // The environment is a `const`, so this cannot fail by construction —
        // which is the reason it is a `const`. The test pins it anyway,
        // because the failure it guards against is somebody replacing the
        // constant with a filter, and a filter is a thing that gets a hole in
        // it. A check container that could read `ANTHROPIC_API_KEY` would let
        // a customer's test file exfiltrate Cortex's provider key.
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test-not-a-real-key");
        std::env::set_var("CLERK_SECRET_KEY", "sk-test-not-a-real-clerk-key");

        let env = config_for(&["cargo", "check", "--locked"])
            .env
            .expect("env is always set");

        assert_eq!(
            env,
            SCRATCH_ENV
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>(),
            "the check environment is no longer exactly the constant"
        );
        for forbidden in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "CLERK_SECRET_KEY"] {
            assert!(
                !env.iter().any(|e| e.starts_with(forbidden)),
                "a host secret reached a check container: {forbidden}"
            );
        }

        // PATH is deliberately not restated: Docker merges the image's ENV
        // with this list, and an entry here would silently override the image.
        assert!(
            !env.iter().any(|e| e.starts_with("PATH=")),
            "PATH is set here rather than by the image, which overrides it"
        );
    }

    #[test]
    fn a_check_still_cannot_reach_the_network() {
        // Adjacent to the change and cheap to pin: a check that can fetch can
        // fetch a passing result. Asserted here because this is the first test
        // of this config at all.
        let config = config_for(&["cargo", "test", "--locked"]);
        assert_eq!(config.network_disabled, Some(true));
        assert_eq!(
            config
                .host_config
                .expect("a host config is always set")
                .network_mode,
            Some("none".to_string())
        );
    }

    #[test]
    fn digest_is_stable_and_content_addressed() {
        assert_eq!(digest_of("ok"), digest_of("ok"));
        assert_ne!(digest_of("ok"), digest_of("ok "));
        assert!(digest_of("").starts_with("sha256:"));
    }

    #[test]
    fn short_output_is_kept_whole() {
        assert_eq!(tail_of("test result: ok", 1024), "test result: ok");
    }

    #[test]
    fn long_output_is_tailed_and_marked() {
        let output = "a".repeat(100);
        let tail = tail_of(&output, 10);
        assert!(tail.starts_with("…[truncated]\n"));
        assert!(tail.ends_with(&"a".repeat(10)));
    }

    #[test]
    fn tail_never_splits_a_character() {
        // Multi-byte characters, cut at a boundary that lands mid-character.
        let output = "é".repeat(50);
        let tail = tail_of(&output, 11);
        assert!(tail.len() >= "…[truncated]\n".len());
        // The point of the test: this must not panic on a byte-index slice,
        // and the result must be valid UTF-8 by construction.
        assert!(tail.chars().count() > 0);
    }

    #[test]
    fn a_check_that_ran_and_failed_is_not_a_check_that_could_not_run() {
        // F20. Bollard reports a non-zero container exit from `wait_container`
        // as an error carrying the code, so reading `Err` as "our failure"
        // makes every failing check `NotExecuted` — and `Verdict::Failed`
        // unreachable from the container runner, which is the verdict the
        // refund path exists to serve.
        let failed = bollard::errors::Error::DockerContainerWaitError {
            error: "test failed, to rerun pass `--lib`".to_string(),
            code: 101,
        };
        assert_eq!(
            exit_code_from_wait_error(&failed),
            Some(101),
            "a non-zero exit must be read as an exit, not as a runner failure"
        );
    }

    #[test]
    fn a_genuine_docker_failure_stays_inconclusive() {
        // The other direction, and the reason the arm is narrow: if Docker
        // itself fails we do not know what happened to the check, and that
        // must stay ours to absorb — no charge, no refund. Widening this to
        // "any wait error means the check failed" would charge a customer for
        // our outage.
        let broken = bollard::errors::Error::DockerResponseServerError {
            status_code: 500,
            message: "server error".to_string(),
        };
        assert_eq!(
            exit_code_from_wait_error(&broken),
            None,
            "a Docker failure must not be reported as the customer's check failing"
        );
    }
}
