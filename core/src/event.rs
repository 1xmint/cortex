//! What can happen in a mission. Events are only ever appended (ADR 0003).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    MissionStarted {
        project_id: String,
        person_id: String,
        request: String,
        repo_path: PathBuf,
        base_commit: String,
    },
    PlanVersion {
        version: u32,
        checks_fingerprint: String,
        note: String,
    },
    CheckRed {
        check_id: String,
        exit_code: i32,
    },
    CheckBlocked {
        check_id: String,
        reason: String,
    },
    SessionStarted {
        session: u32,
        engine: String,
        engine_version: String,
        worktree: PathBuf,
        brief: String,
    },
    Transcript {
        session: u32,
        line: String,
    },
    SessionEnded {
        session: u32,
        engine_note: String,
    },
    SessionAbandoned {
        session: u32,
        reason: String,
    },
    CheckRun {
        session: u32,
        check_id: String,
        exit_code: i32,
        passed: bool,
        output_tail: String,
    },
    CommitAccepted {
        session: u32,
        commit: String,
    },
    Steer {
        text: String,
    },
    Interrupt {
        text: Option<String>,
    },
    Parked {
        until_unix: i64,
        reason: String,
    },
    MissionDone {
        commit: String,
    },
    MissionStuck {
        reason: String,
    },
    MissionPaused,
    MissionResumed,
}

impl Event {
    /// The snake_case name stored in the `kind` column, e.g. `check_red`.
    pub fn kind(&self) -> String {
        match serde_json::to_value(self) {
            Ok(serde_json::Value::Object(map)) => map
                .get("kind")
                .and_then(|k| k.as_str())
                .unwrap_or_default()
                .to_string(),
            _ => String::new(),
        }
    }
}
