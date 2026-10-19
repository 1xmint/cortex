//! What can happen in a mission. Events are only ever appended (ADR 0003).

use crate::payload::{PayloadRef, Spend, Tokens};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The format events are written in. Version 1 is slice 1's; an event's
/// version is stored beside it and old events are upgraded as they are
/// read, never rewritten.
pub const SCHEMA_VERSION: u32 = 2;

/// Reads an event body written in format `version`.
pub fn upgrade(version: u32, body: serde_json::Value) -> Result<Event> {
    if version > SCHEMA_VERSION {
        return Err(Error::Invalid(format!(
            "event from a newer Cortex (schema version {version}, this build reads up to {SCHEMA_VERSION})"
        )));
    }
    // 1 → 2 adds only new kinds and columns, so old bodies read as they are.
    serde_json::from_value(body).map_err(|e| Error::Store(format!("event encoding: {e}")))
}

// Events are read and written one at a time, so the size of the largest
// variant costs nothing worth boxing for.
#[allow(clippy::large_enum_variant)]
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
        /// What the engine itself said the session cost, if it said.
        #[serde(default)]
        reported_estimate_usd_micros: Option<u64>,
    },
    /// One exchange with a model. The bodies live in the payload table.
    ModelCall {
        session: u32,
        role: String,
        provider: String,
        profile: String,
        model: String,
        /// Settings as sent (max tokens and the like).
        #[serde(default)]
        params: serde_json::Value,
        tokens: Tokens,
        estimated_usd_micros: Option<u64>,
        spent: Spend,
        millis: u64,
        request: Option<PayloadRef>,
        response: Option<PayloadRef>,
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
