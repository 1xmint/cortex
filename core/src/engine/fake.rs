//! A scripted engine for tests: each session writes fixed files.
//!
//! ```toml
//! [[session]]
//! delay_ms = 0
//! note = "done"
//! [[session.write]]
//! path = "src/lib.rs"
//! content = "..."
//! ```
//!
//! Session N uses entry N; past the end, the last entry repeats.

use super::{CancelToken, Engine, SessionInput, SessionOutcome};
use crate::{Error, Result};
use serde::Deserialize;
use std::path::{Component, Path};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Deserialize)]
struct Script {
    session: Vec<Session>,
}

#[derive(Debug, Clone, Deserialize)]
struct Session {
    #[serde(default)]
    delay_ms: u64,
    note: Option<String>,
    #[serde(default)]
    write: Vec<Write>,
}

#[derive(Debug, Clone, Deserialize)]
struct Write {
    path: String,
    content: String,
}

#[derive(Debug, Clone)]
pub struct FakeEngine {
    sessions: Vec<Session>,
}

const BRIEF_CHARS: usize = 200;
const TICK: Duration = Duration::from_millis(20);

impl FakeEngine {
    pub fn from_toml(text: &str) -> Result<FakeEngine> {
        let script: Script =
            toml::from_str(text).map_err(|e| Error::Invalid(format!("fake script: {e}")))?;
        if script.session.is_empty() {
            return Err(Error::Invalid("fake script has no [[session]]".into()));
        }
        Ok(FakeEngine {
            sessions: script.session,
        })
    }

    pub fn load(path: &Path) -> Result<FakeEngine> {
        FakeEngine::from_toml(&std::fs::read_to_string(path)?)
    }
}

/// Only plain relative paths: no root, drive, `.` or `..`.
fn inside_worktree(path: &str) -> bool {
    let p = Path::new(path);
    !path.is_empty() && p.components().all(|c| matches!(c, Component::Normal(_)))
}

impl Engine for FakeEngine {
    fn name(&self) -> &str {
        "fake"
    }

    fn version(&self) -> Result<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }

    fn run(
        &mut self,
        input: &SessionInput,
        on_line: &mut dyn FnMut(&str),
        cancel: &CancelToken,
    ) -> Result<SessionOutcome> {
        let idx = (input.session.saturating_sub(1) as usize).min(self.sessions.len() - 1);
        let session = &self.sessions[idx];
        let brief: String = input
            .brief
            .chars()
            .take(BRIEF_CHARS)
            .map(|c| if c == '\n' { ' ' } else { c })
            .collect();
        on_line(&format!("brief: {brief}"));

        // Refuse the whole session before writing anything.
        if let Some(bad) = session.write.iter().find(|w| !inside_worktree(&w.path)) {
            return Ok(SessionOutcome::Failed {
                reason: format!("path outside worktree: {}", bad.path),
            });
        }

        let start = Instant::now();
        let delay = Duration::from_millis(session.delay_ms);
        while start.elapsed() < delay {
            if cancel.is_cancelled() {
                return Ok(SessionOutcome::Failed {
                    reason: "cancelled".into(),
                });
            }
            if start.elapsed() >= input.timeout {
                return Ok(SessionOutcome::TimedOut);
            }
            std::thread::sleep(TICK.min(delay - start.elapsed().min(delay)));
        }
        if cancel.is_cancelled() {
            return Ok(SessionOutcome::Failed {
                reason: "cancelled".into(),
            });
        }

        for w in &session.write {
            let dest = input.worktree.join(&w.path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dest, &w.content)?;
            on_line(&format!("wrote {}", w.path));
        }
        Ok(SessionOutcome::Ended {
            note: session
                .note
                .clone()
                .unwrap_or_else(|| "fake session ended".into()),
        })
    }
}
