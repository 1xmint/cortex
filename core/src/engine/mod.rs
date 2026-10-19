//! Engines do the work in a session: they get a brief and a worktree, and
//! change files. What an engine says about its own result is only a note;
//! Cortex's checks decide.

pub mod api;
pub mod claude;
pub mod fake;

use crate::Result;
use crate::payload::{Spend, Tokens};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Names the brief templates' wording; bump it whenever any brief
/// template changes, so results can be grouped by prompt.
pub const PROMPT_VERSION: &str = "2026-10-18";

/// Profile name used until model profiles exist.
pub const DEFAULT_PROFILE: &str = "default";

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
    Ended {
        note: String,
        /// The engine's own cost estimate for the session, if it gave one.
        reported_usd_micros: Option<u64>,
    },
    TimedOut,
    Failed {
        reason: String,
    },
    RateLimited {
        retry_after: Duration,
    },
}

pub trait Engine {
    fn name(&self) -> &str;
    fn version(&self) -> Result<String>;
    /// Runs one session. `on_line` receives transcript lines as they
    /// happen and `record` each model exchange; a cancelled token ends
    /// the session early.
    fn run(
        &mut self,
        input: &SessionInput,
        on_line: &mut dyn FnMut(&str),
        record: &mut dyn FnMut(ModelCallRecord),
        cancel: &CancelToken,
    ) -> Result<SessionOutcome>;
}

/// One model exchange as an engine saw it, with raw bodies. The mission
/// loop stores the bodies as payloads and logs a `ModelCall` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCallRecord {
    pub session: u32,
    /// `worker` for now.
    pub role: String,
    /// `anthropic-api`, `claude-cli` or `fake`.
    pub provider: String,
    pub profile: String,
    pub model: String,
    pub params: serde_json::Value,
    pub tokens: Tokens,
    pub estimated_usd_micros: Option<u64>,
    pub spent: Spend,
    pub millis: u64,
    /// `None` when the provider does not show it, as with the CLI.
    pub request: Option<Vec<u8>>,
    pub response: Option<Vec<u8>>,
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
