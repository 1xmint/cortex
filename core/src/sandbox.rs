//! Runs model-written commands in two layers (ADR 0004).

use crate::{Error, Result};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RunSpec {
    pub cmd: String,
    pub workdir: PathBuf,
    pub checks_dir: Option<PathBuf>,
    pub timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct RunOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub truncated: bool,
}

pub fn self_test() -> Result<()> {
    Err(Error::Blocked("not built yet".into()))
}

pub fn run(_spec: &RunSpec) -> Result<RunOutput> {
    Err(Error::Blocked("not built yet".into()))
}
