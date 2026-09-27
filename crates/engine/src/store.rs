use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use uuid::Uuid;

use cortex_core::contamination::EvidenceSignal;
use cortex_core::ledger::{LedgerEntry, LedgerEvent};

use crate::bandit::{ArmKey, ArmStats, TaskFamily};
use cortex_core::provider::ProviderId;
use cortex_core::routing::RiskLevel;

pub struct CortexStore {
    conn: Connection,
}

/// A hash of the whole schema `init()` produces (every table, index, trigger
/// and view in `sqlite_master`), pinned in `store::tests::schema_fingerprint_matches_pinned_value`.
///
/// `routing.db` (opened here, at `crates/api/src/state.rs`'s `cortex_store_path`)
/// has no version counter like `crates/api/src/db.rs`'s `SCHEMA_VERSION`, and
/// it is not covered by `deploy-receive.sh`'s schema guard or its backup —
/// see `docs/DEPLOY.md`. A change to this schema needs a manual deploy
/// (`workflow_dispatch` with `allow_migration=true`), not an automatic one.
/// This constant exists only so a schema edit here fails a test instead of
/// shipping unnoticed.
pub const ROUTING_SCHEMA_FINGERPRINT: u64 = 0x6b406d1582bf4bca;

impl CortexStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    fn init(&self) -> Result<()> {
        self.conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS events (
                id TEXT PRIMARY KEY,
                timestamp TEXT NOT NULL,
                event_type TEXT NOT NULL,
                event_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS arm_stats (
                task_family TEXT NOT NULL,
                risk_level TEXT NOT NULL,
                provider TEXT NOT NULL,
                successes INTEGER NOT NULL,
                trials INTEGER NOT NULL,
                total_reward REAL NOT NULL,
                last_updated TEXT NOT NULL,
                PRIMARY KEY (task_family, risk_level, provider)
            );
            CREATE TABLE IF NOT EXISTS evidence_signals (
                id TEXT PRIMARY KEY,
                task_id TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                tier TEXT NOT NULL,
                source TEXT NOT NULL,
                contamination REAL NOT NULL,
                raw_reward REAL NOT NULL,
                effective_reward REAL NOT NULL,
                description TEXT NOT NULL
            );",
        )?;
        Ok(())
    }

    pub fn append_event(&self, entry: &LedgerEntry) -> Result<()> {
        let event_type = match &entry.event {
            LedgerEvent::RoutingDecision { .. } => "routing_decision",
            LedgerEvent::TaskOutcome { .. } => "task_outcome",
            LedgerEvent::UserOverride { .. } => "user_override",
            LedgerEvent::ProviderStatus { .. } => "provider_status",
        };
        let event_json = serde_json::to_string(&entry.event)?;

        self.conn.execute(
            "INSERT OR REPLACE INTO events (id, timestamp, event_type, event_json) VALUES (?1, ?2, ?3, ?4)",
            params![
                entry.id.to_string(),
                entry.timestamp.to_rfc3339(),
                event_type,
                event_json,
            ],
        )?;
        Ok(())
    }

    pub fn recent_events(&self, count: usize) -> Result<Vec<LedgerEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, timestamp, event_json FROM events ORDER BY timestamp DESC LIMIT ?1",
        )?;

        let entries = stmt
            .query_map(params![count as i64], |row| {
                let id_str: String = row.get(0)?;
                let ts_str: String = row.get(1)?;
                let json_str: String = row.get(2)?;
                Ok((id_str, ts_str, json_str))
            })?
            .filter_map(|r| r.ok())
            .filter_map(|(id_str, ts_str, json_str)| {
                let id = Uuid::parse_str(&id_str).ok()?;
                let timestamp = DateTime::parse_from_rfc3339(&ts_str)
                    .ok()?
                    .with_timezone(&Utc);
                let event: LedgerEvent = serde_json::from_str(&json_str).ok()?;
                Some(LedgerEntry {
                    id,
                    timestamp,
                    event,
                })
            })
            .collect();

        Ok(entries)
    }

    pub fn save_arm_stats(&self, key: &ArmKey, stats: &ArmStats) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO arm_stats (task_family, risk_level, provider, successes, trials, total_reward, last_updated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                format!("{:?}", key.task_family),
                format!("{:?}", key.risk_level),
                format!("{:?}", key.provider),
                stats.successes,
                stats.trials,
                stats.total_reward,
                stats.last_updated.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn load_arm_stats(&self) -> Result<HashMap<ArmKey, ArmStats>> {
        let mut stmt = self.conn.prepare(
            "SELECT task_family, risk_level, provider, successes, trials, total_reward, last_updated FROM arm_stats",
        )?;

        let mut map = HashMap::new();
        let rows = stmt.query_map([], |row| {
            let tf: String = row.get(0)?;
            let rl: String = row.get(1)?;
            let prov: String = row.get(2)?;
            let successes: u32 = row.get(3)?;
            let trials: u32 = row.get(4)?;
            let total_reward: f64 = row.get(5)?;
            let last_updated: String = row.get(6)?;
            Ok((tf, rl, prov, successes, trials, total_reward, last_updated))
        })?;

        for row in rows {
            let (tf, rl, prov, successes, trials, total_reward, last_updated) = row?;

            let task_family = parse_task_family(&tf);
            let risk_level = parse_risk_level(&rl);
            let provider = parse_provider(&prov);

            if let (Some(task_family), Some(risk_level), Some(provider)) =
                (task_family, risk_level, provider)
            {
                let last_updated = DateTime::parse_from_rfc3339(&last_updated)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());

                map.insert(
                    ArmKey {
                        task_family,
                        risk_level,
                        provider,
                    },
                    ArmStats {
                        successes,
                        trials,
                        total_reward,
                        last_updated,
                    },
                );
            }
        }

        Ok(map)
    }

    pub fn append_evidence(&self, task_id: &Uuid, signal: &EvidenceSignal) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO evidence_signals (id, task_id, timestamp, tier, source, contamination, raw_reward, effective_reward, description)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                signal.id.to_string(),
                task_id.to_string(),
                signal.timestamp.to_rfc3339(),
                format!("{:?}", signal.tier),
                format!("{:?}", signal.source),
                signal.contamination.raw_value,
                signal.raw_reward,
                signal.effective_reward,
                signal.description,
            ],
        )?;
        Ok(())
    }

    pub fn evidence_for_task(&self, task_id: &Uuid) -> Result<Vec<EvidenceSignal>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, timestamp, tier, source, contamination, raw_reward, effective_reward, description
             FROM evidence_signals WHERE task_id = ?1 ORDER BY timestamp",
        )?;

        let signals: Vec<EvidenceSignal> = stmt
            .query_map(params![task_id.to_string()], |row| {
                let id_str: String = row.get(0)?;
                let ts_str: String = row.get(1)?;
                let _tier_str: String = row.get(2)?;
                let _source_str: String = row.get(3)?;
                let contamination: f64 = row.get(4)?;
                let raw_reward: f64 = row.get(5)?;
                let effective_reward: f64 = row.get(6)?;
                let description: String = row.get(7)?;
                Ok((
                    id_str,
                    ts_str,
                    contamination,
                    raw_reward,
                    effective_reward,
                    description,
                ))
            })?
            .filter_map(|r| r.ok())
            .filter_map(
                |(id_str, ts_str, contamination, raw_reward, effective_reward, description)| {
                    use cortex_core::contamination::*;
                    let id = Uuid::parse_str(&id_str).ok()?;
                    let timestamp = DateTime::parse_from_rfc3339(&ts_str)
                        .ok()?
                        .with_timezone(&Utc);
                    Some(EvidenceSignal {
                        id,
                        timestamp,
                        tier: SignalTier::HardObjective,
                        source: EvidenceSource::CompilerOutput,
                        contamination: ContaminationScore {
                            raw_value: contamination,
                            source: EvidenceSource::CompilerOutput,
                            generator_model: None,
                            verifier_model: None,
                            same_model_penalty: false,
                        },
                        raw_reward,
                        effective_reward,
                        description,
                    })
                },
            )
            .collect();

        Ok(signals)
    }

    pub fn event_count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))?;
        Ok(count as usize)
    }
}

fn parse_task_family(s: &str) -> Option<TaskFamily> {
    match s {
        "CodeEdit" => Some(TaskFamily::CodeEdit),
        "CodeReview" => Some(TaskFamily::CodeReview),
        "Search" => Some(TaskFamily::Search),
        "Testing" => Some(TaskFamily::Testing),
        "Planning" => Some(TaskFamily::Planning),
        "Deployment" => Some(TaskFamily::Deployment),
        _ => None,
    }
}

fn parse_risk_level(s: &str) -> Option<RiskLevel> {
    match s {
        "Low" => Some(RiskLevel::Low),
        "Medium" => Some(RiskLevel::Medium),
        "High" => Some(RiskLevel::High),
        "Critical" => Some(RiskLevel::Critical),
        _ => None,
    }
}

fn parse_provider(s: &str) -> Option<ProviderId> {
    match s {
        "Claude" => Some(ProviderId::Claude),
        "Openai" => Some(ProviderId::Openai),
        "Gemini" => Some(ProviderId::Gemini),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_core::ledger::LedgerEvent;
    use cortex_core::provider::Tier;
    use cortex_core::routing::RationaleCode;

    fn test_entry() -> LedgerEntry {
        LedgerEntry::new(LedgerEvent::RoutingDecision {
            task_id: Uuid::new_v4(),
            provider: ProviderId::Claude,
            tier: Tier::Execute,
            risk: RiskLevel::Medium,
            rationale: vec![RationaleCode::BestAvailableForTier],
            score: 100.0,
            model: Some("claude-sonnet-4-6".into()),
            alternatives_considered: vec![],
        })
    }

    #[test]
    fn test_open_memory() {
        let store = CortexStore::open_memory().unwrap();
        assert_eq!(store.event_count().unwrap(), 0);
    }

    /// Normalises whitespace (including CRLF vs LF) in a `sqlite_master` dump
    /// so the fingerprint below does not depend on incidental formatting of
    /// the SQL text SQLite echoes back, only on the schema it describes.
    fn normalize_schema_sql(sql: &str) -> String {
        sql.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// FNV-1a: dependency-free, deterministic across processes and platforms,
    /// which is all this needs — it is a change-detector, not a security hash.
    fn fnv1a_64(bytes: &[u8]) -> u64 {
        const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
        const PRIME: u64 = 0x100000001b3;
        bytes.iter().fold(OFFSET_BASIS, |hash, &b| {
            (hash ^ u64::from(b)).wrapping_mul(PRIME)
        })
    }

    /// The full schema a fresh `CortexStore::init` produces, hashed.
    fn schema_fingerprint(store: &CortexStore) -> u64 {
        let mut stmt = store
            .conn
            .prepare(
                "SELECT type, name, tbl_name, sql FROM sqlite_master \
                 ORDER BY type, name",
            )
            .expect("failed to query sqlite_master");
        let rows: Vec<String> = stmt
            .query_map([], |r| {
                let kind: String = r.get(0)?;
                let name: String = r.get(1)?;
                let tbl_name: String = r.get(2)?;
                let sql: Option<String> = r.get(3)?;
                Ok(format!(
                    "{}|{}|{}|{}",
                    kind,
                    name,
                    tbl_name,
                    normalize_schema_sql(sql.as_deref().unwrap_or(""))
                ))
            })
            .expect("failed to map sqlite_master rows")
            .collect::<Result<_, _>>()
            .expect("failed to read sqlite_master row");
        fnv1a_64(rows.join("\n").as_bytes())
    }

    /// `routing.db` has no `SCHEMA_VERSION` counter and is outside
    /// `deploy-receive.sh`'s automatic-deploy schema guard (see
    /// `docs/DEPLOY.md`). This test hashes the schema a fresh `init()`
    /// actually produces and pins it, so a schema change here fails a build
    /// instead of shipping into production unannounced — routing.db changes
    /// need a manual deploy (`workflow_dispatch` with `allow_migration=true`).
    #[test]
    fn schema_fingerprint_matches_pinned_value() {
        let store = CortexStore::open_memory().unwrap();
        let actual = schema_fingerprint(&store);
        assert_eq!(
            actual, ROUTING_SCHEMA_FINGERPRINT,
            "routing.db schema changed — update ROUTING_SCHEMA_FINGERPRINT in \
             crates/engine/src/store.rs, and note in your PR that routing.db \
             changes need a manual deploy (workflow_dispatch, allow_migration=true) \
             per docs/DEPLOY.md (computed fingerprint: {:#x})",
            actual
        );
    }

    #[test]
    fn test_append_and_read_events() {
        let store = CortexStore::open_memory().unwrap();
        let entry = test_entry();

        store.append_event(&entry).unwrap();
        store.append_event(&test_entry()).unwrap();

        assert_eq!(store.event_count().unwrap(), 2);

        let recent = store.recent_events(10).unwrap();
        assert_eq!(recent.len(), 2);
    }

    #[test]
    fn test_save_and_load_arm_stats() {
        let store = CortexStore::open_memory().unwrap();

        let key = ArmKey {
            task_family: TaskFamily::CodeEdit,
            risk_level: RiskLevel::High,
            provider: ProviderId::Claude,
        };
        let stats = ArmStats {
            successes: 8,
            trials: 10,
            total_reward: 7.5,
            last_updated: Utc::now(),
        };

        store.save_arm_stats(&key, &stats).unwrap();

        let loaded = store.load_arm_stats().unwrap();
        let loaded_stats = loaded.get(&key).expect("arm stats should exist");
        assert_eq!(loaded_stats.successes, 8);
        assert_eq!(loaded_stats.trials, 10);
        assert!((loaded_stats.total_reward - 7.5).abs() < 1e-9);
    }

    #[test]
    fn test_append_and_query_evidence() {
        use cortex_core::contamination::*;

        let store = CortexStore::open_memory().unwrap();
        let task_id = Uuid::new_v4();

        let signal = EvidenceSignal::new(
            SignalTier::HardObjective,
            EvidenceSource::CompilerOutput,
            None,
            None,
            1.0,
            "compile passed",
        );

        store.append_evidence(&task_id, &signal).unwrap();

        let signals = store.evidence_for_task(&task_id).unwrap();
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].description, "compile passed");
    }

    #[test]
    fn test_recent_events_ordering() {
        let store = CortexStore::open_memory().unwrap();

        for _ in 0..5 {
            store.append_event(&test_entry()).unwrap();
        }

        let recent = store.recent_events(3).unwrap();
        assert_eq!(recent.len(), 3);

        for i in 0..recent.len() - 1 {
            assert!(recent[i].timestamp >= recent[i + 1].timestamp);
        }
    }
}
