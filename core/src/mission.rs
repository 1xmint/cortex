//! The mission loop (stub; implemented next).

use crate::engine::Engine;
use crate::store::{MissionId, Store};
use crate::{Error, Result};
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Budgets {
    pub sessions: u32,
    pub session_timeout: Duration,
    pub mission_timeout: Duration,
    pub replans: u32,
}

impl Default for Budgets {
    fn default() -> Self {
        Budgets {
            sessions: 4,
            session_timeout: Duration::from_secs(20 * 60),
            mission_timeout: Duration::from_secs(2 * 60 * 60),
            replans: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done { commit: String },
    Stuck { reason: String },
    Paused,
}

pub fn start(
    _store: &Store,
    _home: &Path,
    _repo: &Path,
    _checks: &Path,
    _request: &str,
    _person: &str,
) -> Result<MissionId> {
    Err(Error::Invalid("not yet".into()))
}

pub fn drive(
    _store: &Store,
    _home: &Path,
    _m: &MissionId,
    _engine: &mut dyn Engine,
    _budgets: &Budgets,
) -> Result<Outcome> {
    Err(Error::Invalid("not yet".into()))
}
