//! Engines do the work in a session: they get a brief and a worktree, and
//! change files. What an engine says about its own result is only a note;
//! Cortex's checks decide.

pub mod api;
pub mod claude;
pub mod fake;

use crate::Result;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct SessionInput {
    pub brief: String,
    pub worktree: PathBuf,
    /// The frozen checks, which the engine's `test` tool runs read-only.
    pub checks_dir: PathBuf,
    /// 1-based session number within the mission.
    pub session: u32,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionOutcome {
    Ended { note: String },
    TimedOut,
    Failed { reason: String },
    RateLimited { retry_after: Duration },
}

pub trait Engine {
    fn name(&self) -> &str;
    fn version(&self) -> Result<String>;
    /// Runs one session. `on_line` receives transcript lines as they
    /// happen; a cancelled token ends the session early.
    fn run(
        &mut self,
        input: &SessionInput,
        on_line: &mut dyn FnMut(&str),
        cancel: &CancelToken,
    ) -> Result<SessionOutcome>;
}

/// Shared stop flag: any clone can cancel, every clone sees it.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}
