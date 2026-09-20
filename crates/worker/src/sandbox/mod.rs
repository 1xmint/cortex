//! The boundary between Cortex and model-authored code.
//!
//! Before this module, provider CLIs ran as host processes: a git worktree was
//! attempted for isolation, and when that attempt failed the agent ran in the
//! caller's directory with the worker's full environment, network, filesystem,
//! and credentials. The isolation was documented as best-effort. This module
//! removes the "best" and the "effort" both.
//!
//! # The contract
//!
//! A [`SandboxRunner`] either returns a running [`SandboxSession`] that
//! satisfies every property below, or it returns [`Blocked`]. There is no third
//! outcome, and in particular there is no degraded one:
//!
//! - **Fresh per task attempt.** No writable state survives an attempt, and no
//!   sandbox is ever reused across tenants. Reuse makes every kernel bug a
//!   cross-customer disclosure and every leftover file a leak.
//! - **Egress denied by default**, opened only by an explicit task-scoped
//!   allowlist. A dependency grant names a package registry, and exactly a
//!   package registry.
//! - **No credentials inside.** Not the customer's, and not Cortex's own — a
//!   sandbox holding a provider key can spend Cortex's money. The environment
//!   is empty rather than filtered, because a filter is a list someone has to
//!   keep correct. Git credentials belong to the runner service, which performs
//!   sanctioned fetch and push on the sandbox's behalf.
//! - **Bounded.** CPU, memory, pids, disk, and wall clock all have values.
//!   Unbounded is not representable.
//! - **Torn down.** Teardown runs on success, on failure, on timeout, and on
//!   panic. A leaked sandbox is a bug, not a race.
//!
//! # Why the trait exists
//!
//! A shared-kernel container is a resource boundary and a convenience boundary.
//! Against a kernel exploit it is not a security boundary, and the threat here
//! is model-authored code running adjacent to other customers' source. The
//! requirement is kernel-level isolation.
//!
//! [`ContainerSandbox`] is therefore the *first* implementation, not the
//! intended final one. The trait is deliberately free of container vocabulary
//! so a microVM implementation replaces it without touching a single caller,
//! and [`IsolationClass`] travels on the job so a receipt states which boundary
//! actually ran rather than leaving a reader to assume the stronger one.

pub mod container;
pub mod egress;
pub mod policy;

use cortex_core::execution_job::{Blocked, ExecutionJob, IsolationClass};
use tokio::sync::{mpsc, oneshot};

pub use container::ContainerSandbox;
pub use policy::{sanctioned_env, sanctioned_env_for_request, SandboxRequest};

/// How many output lines may be buffered before the producer waits. Bounded so
/// a chatty agent applies backpressure instead of consuming the host's memory.
const OUTPUT_CHANNEL_DEPTH: usize = 256;

/// Which stream a line came from. Kept distinct because stderr is diagnostic
/// and stdout is the provider's structured protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// One line of sandbox output.
#[derive(Debug, Clone)]
pub struct SandboxLine {
    pub stream: OutputStream,
    pub text: String,
}

/// How a sandbox finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxExit {
    /// The process ran to completion. A non-zero code is a result, not an
    /// error — the work failed, the boundary held.
    Exited { code: i32 },
    /// The wall-clock budget was reached and the sandbox was torn down. This
    /// is the cap in invariant 14 doing its job.
    BudgetExhausted,
    /// Teardown was requested by an operator or by a breaker.
    Killed,
}

/// A running sandbox.
///
/// Concrete rather than a trait so that implementations differ only in how they
/// *drive* it. A test double is a few lines, which is what makes the adversarial
/// and blocking tests possible without a container runtime.
pub struct SandboxSession {
    sandbox_id: String,
    isolation_class: IsolationClass,
    output: mpsc::Receiver<SandboxLine>,
    exit: oneshot::Receiver<Result<SandboxExit, Blocked>>,
    kill: Option<oneshot::Sender<()>>,
}

/// The producer half, held by whichever implementation is driving a session.
pub struct SandboxDriver {
    pub output: mpsc::Sender<SandboxLine>,
    pub exit: oneshot::Sender<Result<SandboxExit, Blocked>>,
    pub kill: oneshot::Receiver<()>,
}

impl SandboxSession {
    /// Build a session and the driver half that feeds it.
    pub fn channel(
        sandbox_id: impl Into<String>,
        isolation_class: IsolationClass,
    ) -> (Self, SandboxDriver) {
        let (output_tx, output_rx) = mpsc::channel(OUTPUT_CHANNEL_DEPTH);
        let (exit_tx, exit_rx) = oneshot::channel();
        let (kill_tx, kill_rx) = oneshot::channel();

        let session = Self {
            sandbox_id: sandbox_id.into(),
            isolation_class,
            output: output_rx,
            exit: exit_rx,
            kill: Some(kill_tx),
        };
        let driver = SandboxDriver {
            output: output_tx,
            exit: exit_tx,
            kill: kill_rx,
        };
        (session, driver)
    }

    pub fn sandbox_id(&self) -> &str {
        &self.sandbox_id
    }

    /// The boundary that actually ran, for the receipt.
    pub fn isolation_class(&self) -> IsolationClass {
        self.isolation_class
    }

    /// Next line of output, or `None` once the sandbox has stopped producing.
    pub async fn next_line(&mut self) -> Option<SandboxLine> {
        self.output.recv().await
    }

    /// Wait for the sandbox to finish.
    ///
    /// A dropped driver means the implementation went away without reporting,
    /// which is a refusal rather than a silent success — never treat it as a
    /// zero exit.
    pub async fn wait(self) -> Result<SandboxExit, Blocked> {
        match self.exit.await {
            Ok(result) => result,
            Err(_) => Err(Blocked::new(
                cortex_core::execution_job::BlockedReason::SandboxUnavailable,
                "sandbox driver stopped without reporting an outcome",
            )),
        }
    }

    /// Request teardown of an in-flight sandbox.
    ///
    /// This is the primitive an operator stop and an automatic breaker call.
    /// The surface that calls it is not built here; the primitive is, because
    /// a stop with nothing to call is not a stop.
    pub fn kill(&mut self) {
        if let Some(kill) = self.kill.take() {
            let _ = kill.send(());
        }
    }
}

/// Runs an [`ExecutionJob`] inside a boundary, or refuses.
///
/// Implementations must not expose container-, VM-, or host-specific concepts
/// through this trait. That constraint is what makes replacing the runtime a
/// swap rather than a rewrite.
pub trait SandboxRunner {
    /// Start the job. Any failure to establish the full boundary — the
    /// sandbox, the workspace, the network policy, the resource bounds —
    /// returns [`Blocked`] and the provider is never invoked.
    fn submit(
        &self,
        job: &ExecutionJob,
        request: &SandboxRequest,
    ) -> impl std::future::Future<Output = Result<SandboxSession, Blocked>> + Send;

    /// The boundary class this runner provides. Recorded on the job.
    fn isolation_class(&self) -> IsolationClass;
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_core::execution_job::BlockedReason;

    #[tokio::test]
    async fn session_streams_lines_then_reports_exit() {
        let (mut session, driver) = SandboxSession::channel("sbx-1", IsolationClass::Container);
        assert_eq!(session.sandbox_id(), "sbx-1");
        assert_eq!(session.isolation_class(), IsolationClass::Container);

        tokio::spawn(async move {
            driver
                .output
                .send(SandboxLine {
                    stream: OutputStream::Stdout,
                    text: "working".to_string(),
                })
                .await
                .unwrap();
            drop(driver.output);
            let _ = driver.exit.send(Ok(SandboxExit::Exited { code: 0 }));
        });

        let line = session.next_line().await.expect("one line");
        assert_eq!(line.stream, OutputStream::Stdout);
        assert_eq!(line.text, "working");
        assert!(session.next_line().await.is_none());
        assert_eq!(session.wait().await, Ok(SandboxExit::Exited { code: 0 }));
    }

    #[tokio::test]
    async fn dropped_driver_is_blocked_not_a_zero_exit() {
        // The failure this catches: an implementation dying mid-run and the
        // caller recording a successful, unverified delivery.
        let (session, driver) = SandboxSession::channel("sbx-2", IsolationClass::Container);
        drop(driver);

        let result = session.wait().await;
        assert_eq!(
            result.expect_err("must not be a success").reason,
            BlockedReason::SandboxUnavailable
        );
    }

    #[tokio::test]
    async fn kill_reaches_the_driver() {
        // The teardown primitive an operator stop and a breaker both call.
        let (mut session, driver) = SandboxSession::channel("sbx-3", IsolationClass::Container);
        let killed = tokio::spawn(async move { driver.kill.await.is_ok() });

        session.kill();
        assert!(killed.await.unwrap());
    }

    #[tokio::test]
    async fn budget_exhaustion_is_distinguishable_from_a_clean_exit() {
        let (session, driver) = SandboxSession::channel("sbx-4", IsolationClass::Container);
        let _ = driver.exit.send(Ok(SandboxExit::BudgetExhausted));
        assert_eq!(session.wait().await, Ok(SandboxExit::BudgetExhausted));
    }
}
