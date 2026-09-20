use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use cortex_core::error::CortexError;
use cortex_core::ledger::LedgerEntry;

pub struct Ledger {
    path: PathBuf,
}

impl Ledger {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn append(&self, entry: &LedgerEntry) -> Result<(), CortexError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CortexError::LedgerWrite(e.to_string()))?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| CortexError::LedgerWrite(e.to_string()))?;

        let line =
            serde_json::to_string(entry).map_err(|e| CortexError::LedgerWrite(e.to_string()))?;

        writeln!(file, "{line}").map_err(|e| CortexError::LedgerWrite(e.to_string()))?;
        Ok(())
    }

    pub fn read_all(&self) -> Result<Vec<LedgerEntry>, CortexError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(&self.path).map_err(|e| CortexError::LedgerWrite(e.to_string()))?;

        let reader = BufReader::new(file);
        let mut entries = Vec::new();

        for line in reader.lines() {
            let line = line.map_err(|e| CortexError::LedgerWrite(e.to_string()))?;
            if line.trim().is_empty() {
                continue;
            }
            let entry: LedgerEntry =
                serde_json::from_str(&line).map_err(|e| CortexError::LedgerWrite(e.to_string()))?;
            entries.push(entry);
        }

        Ok(entries)
    }

    pub fn recent(&self, count: usize) -> Result<Vec<LedgerEntry>, CortexError> {
        let all = self.read_all()?;
        let start = all.len().saturating_sub(count);
        Ok(all[start..].to_vec())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_core::ledger::LedgerEvent;
    use cortex_core::provider::{ProviderId, Tier};
    use cortex_core::routing::{RationaleCode, RiskLevel};
    use uuid::Uuid;

    #[test]
    fn append_and_read() {
        let dir = std::env::temp_dir().join(format!("cortex-test-{}", Uuid::new_v4()));
        let ledger = Ledger::new(dir.join("ledger.jsonl"));

        let entry = LedgerEntry::new(LedgerEvent::RoutingDecision {
            task_id: Uuid::new_v4(),
            provider: ProviderId::Claude,
            tier: Tier::Execute,
            risk: RiskLevel::Medium,
            rationale: vec![RationaleCode::BestAvailableForTier],
            score: 135.0,
            model: Some("claude-sonnet-4-6".into()),
            alternatives_considered: vec![],
        });

        ledger.append(&entry).unwrap();
        ledger.append(&entry).unwrap();

        let entries = ledger.read_all().unwrap();
        assert_eq!(entries.len(), 2);

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn read_empty_file() {
        let dir = std::env::temp_dir().join(format!("cortex-test-{}", Uuid::new_v4()));
        let ledger = Ledger::new(dir.join("nonexistent.jsonl"));

        let entries = ledger.read_all().unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn recent_returns_last_n() {
        let dir = std::env::temp_dir().join(format!("cortex-test-{}", Uuid::new_v4()));
        let ledger = Ledger::new(dir.join("ledger.jsonl"));

        for _ in 0..5 {
            let entry = LedgerEntry::new(LedgerEvent::ProviderStatus {
                provider: ProviderId::Claude,
                authenticated: true,
                pressure: 0.5,
            });
            ledger.append(&entry).unwrap();
        }

        let recent = ledger.recent(3).unwrap();
        assert_eq!(recent.len(), 3);

        std::fs::remove_dir_all(dir).ok();
    }
}
