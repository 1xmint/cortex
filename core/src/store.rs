//! The mission log: an append-only event list per mission in SQLite
//! (ADR 0003), plus leases so only one process drives a mission at a time.

use crate::event::{Event, SCHEMA_VERSION, upgrade};
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MissionId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectId(pub String);

/// An event with the format version and build that wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEvent {
    pub seq: u64,
    pub schema_version: u32,
    pub build: String,
    pub prompt_version: String,
    pub event: Event,
}

/// Columns added after slice 1, with the values old rows read as.
const ADDED_COLUMNS: [(&str, &str); 3] = [
    ("schema_version", "INTEGER NOT NULL DEFAULT 1"),
    ("build", "TEXT NOT NULL DEFAULT 'unknown'"),
    ("prompt_version", "TEXT NOT NULL DEFAULT ''"),
];

pub struct Store {
    conn: Mutex<Connection>,
    lease_ttl: Duration,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS projects (
    id   TEXT PRIMARY KEY,
    path TEXT
);
CREATE TABLE IF NOT EXISTS missions (
    id         TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    person_id  TEXT NOT NULL,
    created    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS missions_by_project ON missions(project_id);
CREATE TABLE IF NOT EXISTS events (
    mission_id TEXT NOT NULL REFERENCES missions(id),
    seq        INTEGER NOT NULL,
    kind       TEXT NOT NULL,
    body_json  TEXT NOT NULL,
    at         INTEGER NOT NULL,
    PRIMARY KEY (mission_id, seq)
);
CREATE TRIGGER IF NOT EXISTS events_no_update BEFORE UPDATE ON events
BEGIN SELECT RAISE(ABORT, 'append-only'); END;
CREATE TRIGGER IF NOT EXISTS events_no_delete BEFORE DELETE ON events
BEGIN SELECT RAISE(ABORT, 'append-only'); END;
CREATE TABLE IF NOT EXISTS leases (
    mission_id TEXT PRIMARY KEY REFERENCES missions(id),
    holder     TEXT NOT NULL,
    heartbeat  INTEGER NOT NULL
);
";

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Store(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Store(format!("event encoding: {e}"))
    }
}

/// Adds the post-slice-1 columns to an older database, once, in one
/// transaction. Old rows keep their defaults and are never rewritten.
fn add_missing_columns(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let have: Vec<String> = {
        let mut stmt = tx.prepare("SELECT name FROM pragma_table_info('events')")?;
        stmt.query_map([], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?
    };
    for (name, decl) in ADDED_COLUMNS {
        if !have.iter().any(|h| h == name) {
            tx.execute_batch(&format!("ALTER TABLE events ADD COLUMN {name} {decl}"))?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(10))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        add_missing_columns(&mut conn)?;
        Ok(Store {
            conn: Mutex::new(conn),
            lease_ttl: Duration::from_secs(15),
        })
    }

    /// How long a lease survives without a heartbeat (15 s by default).
    pub fn with_lease_ttl(mut self, ttl: Duration) -> Store {
        self.lease_ttl = ttl;
        self
    }

    pub fn lease_ttl(&self) -> Duration {
        self.lease_ttl
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn create_mission(&self, project: &ProjectId, person: &str) -> Result<MissionId> {
        let id = MissionId(uuid::Uuid::new_v4().to_string());
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT OR IGNORE INTO projects (id) VALUES (?1)",
            params![project.0],
        )?;
        tx.execute(
            "INSERT INTO missions (id, project_id, person_id, created) VALUES (?1, ?2, ?3, ?4)",
            params![id.0, project.0, person, now_ms()],
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// Appends one event in its own transaction and returns its sequence
    /// number (1 for a mission's first event).
    pub fn append(&self, m: &MissionId, e: &Event) -> Result<u64> {
        let kind = e.kind();
        let body = serde_json::to_string(e)?;
        let seq = {
            let mut conn = self.conn();
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let seq: i64 = tx.query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE mission_id = ?1",
                params![m.0],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT INTO events (mission_id, seq, kind, body_json, at, schema_version, build, prompt_version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    m.0,
                    seq,
                    kind,
                    body,
                    now_ms(),
                    SCHEMA_VERSION,
                    crate::BUILD,
                    crate::engine::PROMPT_VERSION
                ],
            )?;
            tx.commit()?;
            seq as u64
        };
        // Kill tests (ADR 0003): die right after this kind is durable.
        if let Ok(fault) = std::env::var("CORTEX_FAULT")
            && fault.strip_prefix("after:") == Some(kind.as_str())
        {
            std::process::abort();
        }
        Ok(seq)
    }

    pub fn events(&self, m: &MissionId) -> Result<Vec<(u64, Event)>> {
        self.events_after(m, 0)
    }

    pub fn events_after(&self, m: &MissionId, seq: u64) -> Result<Vec<(u64, Event)>> {
        Ok(self
            .stored_events_after(m, seq)?
            .into_iter()
            .map(|e| (e.seq, e.event))
            .collect())
    }

    /// Every event of the mission with the version and build that wrote it.
    pub fn stored_events(&self, m: &MissionId) -> Result<Vec<StoredEvent>> {
        self.stored_events_after(m, 0)
    }

    fn stored_events_after(&self, m: &MissionId, seq: u64) -> Result<Vec<StoredEvent>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT seq, body_json, schema_version, build, prompt_version FROM events
             WHERE mission_id = ?1 AND seq > ?2 ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![m.0, seq as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (seq, body, version, build, prompt_version) = row?;
            let version = u32::try_from(version)
                .map_err(|_| Error::Invalid(format!("event {seq} has schema version {version}")))?;
            out.push(StoredEvent {
                seq: seq as u64,
                schema_version: version,
                build,
                prompt_version,
                event: upgrade(version, serde_json::from_str(&body)?)?,
            });
        }
        Ok(out)
    }

    pub fn missions(&self, project: &ProjectId) -> Result<Vec<MissionId>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT id FROM missions WHERE project_id = ?1 ORDER BY created, rowid")?;
        let rows = stmt.query_map(params![project.0], |r| r.get::<_, String>(0))?;
        rows.map(|r| r.map(MissionId).map_err(Error::from))
            .collect()
    }

    /// Takes the mission's lease if it is free, stale, or already ours.
    pub fn take_lease(&self, m: &MissionId, holder: &str) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = now_ms();
        let current: Option<(String, i64)> = tx
            .query_row(
                "SELECT holder, heartbeat FROM leases WHERE mission_id = ?1",
                params![m.0],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let free = match current {
            None => true,
            Some((h, beat)) => h == holder || now - beat > self.lease_ttl.as_millis() as i64,
        };
        if free {
            tx.execute(
                "INSERT INTO leases (mission_id, holder, heartbeat) VALUES (?1, ?2, ?3)
                 ON CONFLICT(mission_id) DO UPDATE SET holder = ?2, heartbeat = ?3",
                params![m.0, holder, now],
            )?;
        }
        tx.commit()?;
        Ok(free)
    }

    /// Refreshes the heartbeat; fails if the lease is no longer ours.
    pub fn renew_lease(&self, m: &MissionId, holder: &str) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE leases SET heartbeat = ?3 WHERE mission_id = ?1 AND holder = ?2",
            params![m.0, holder, now_ms()],
        )?;
        if n == 0 {
            return Err(Error::Store(format!(
                "lease on {} is not held by {holder}",
                m.0
            )));
        }
        Ok(())
    }

    /// Who holds the mission's lease, if anyone.
    pub fn lease_holder(&self, m: &MissionId) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT holder FROM leases WHERE mission_id = ?1",
                params![m.0],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Hands the lease from `old` to `new`, only if `old` still holds it.
    pub fn replace_lease(&self, m: &MissionId, old: &str, new: &str) -> Result<bool> {
        let n = self.conn().execute(
            "UPDATE leases SET holder = ?3, heartbeat = ?4 WHERE mission_id = ?1 AND holder = ?2",
            params![m.0, old, new, now_ms()],
        )?;
        Ok(n == 1)
    }

    pub fn release_lease(&self, m: &MissionId, holder: &str) -> Result<()> {
        self.conn().execute(
            "DELETE FROM leases WHERE mission_id = ?1 AND holder = ?2",
            params![m.0, holder],
        )?;
        Ok(())
    }
}
