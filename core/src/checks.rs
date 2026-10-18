//! Acceptance checks: frozen outside the worker's copy, fingerprinted, seen
//! failing before work starts, and run by Cortex itself in the sandbox.

use crate::store::{MissionId, Store};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub id: String,
    pub command: String,
    pub expect_exit: i32,
}

#[derive(Debug, Clone)]
pub struct CheckSet {
    pub checks: Vec<Check>,
    /// The `files/` folder beside the TOML; mounted at `/checks/files`.
    pub files_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Frozen {
    pub fingerprint: String,
    pub dir: PathBuf,
    pub set: CheckSet,
}

#[derive(Debug, Clone, Default)]
pub struct RedReport {
    pub blocked: Vec<String>,
}

impl CheckSet {
    pub fn load(_toml_path: &Path) -> Result<CheckSet> {
        Err(Error::Invalid("not built yet".into()))
    }
}

impl Frozen {
    pub fn verify(&self) -> Result<()> {
        Err(Error::Invalid("not built yet".into()))
    }
}

pub fn freeze(_set: &CheckSet, _cortex_home: &Path, _mission: &MissionId) -> Result<Frozen> {
    Err(Error::Invalid("not built yet".into()))
}

pub fn red_run(
    _frozen: &Frozen,
    _workdir: &Path,
    _store: &Store,
    _mission: &MissionId,
) -> Result<RedReport> {
    Err(Error::Invalid("not built yet".into()))
}

pub fn check_all(
    _frozen: &Frozen,
    _workdir: &Path,
    _session: u32,
    _store: &Store,
    _mission: &MissionId,
) -> Result<bool> {
    Err(Error::Invalid("not built yet".into()))
}
