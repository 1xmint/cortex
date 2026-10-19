//! Bench tasks: the slice-1 small prompts and the multi-feature tier.
//!
//! A multi task lives at `<bench>/multi/<id>/`:
//! - `prompt.md` and `task.toml` (`tier`, `kind`, `features`);
//! - `start/`, the project the worker begins from;
//! - `visible/` checks, which the worker's `test` tool runs, and `hidden/`
//!   checks, which only the bench runs after acceptance; each is a
//!   `checks.toml` plus `files/`;
//! - `solutions/reference/` and `solutions/bad-<name>/`, file overlays on
//!   top of `start/`. Each bad has a `WHY.md` saying what is wrong with it.

use super::copy_dir;
use crate::checks::CheckSet;
use crate::sandbox::{RunSpec, run};
use crate::{Error, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The note in each bad overlay. It explains the overlay; it is never part
/// of the solution.
pub const WHY: &str = "WHY.md";

const CHECK_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Small,
    Multi,
}

/// How the task starts: from nothing, from Cortex-written code to change,
/// or from someone else's project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Fresh,
    Change,
    Foreign,
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub dir: PathBuf,
    pub tier: Tier,
    pub kind: Kind,
    pub features: Vec<String>,
}

#[derive(Deserialize)]
struct TaskFile {
    tier: Tier,
    kind: Kind,
    features: Vec<String>,
}

/// What the visible and hidden checks said about one solution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grade {
    pub visible_failed: Vec<String>,
    pub hidden_failed: Vec<String>,
}

/// The tasks under `bench`: `prompts/` as `Small`, `multi/` as `Multi`, in
/// folder-name order. A tier with no folder has no tasks.
pub fn tasks(bench: &Path, tier: Option<Tier>) -> Result<Vec<Task>> {
    let mut out = Vec::new();
    if tier.is_none_or(|t| t == Tier::Small) {
        for (id, dir) in subdirs(&bench.join("prompts"))? {
            out.push(Task {
                id,
                dir,
                tier: Tier::Small,
                kind: Kind::Change,
                features: Vec::new(),
            });
        }
    }
    if tier.is_none_or(|t| t == Tier::Multi) {
        for (id, dir) in subdirs(&bench.join("multi"))? {
            let path = dir.join("task.toml");
            let file: TaskFile = toml::from_str(&std::fs::read_to_string(&path)?)
                .map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?;
            if file.tier != Tier::Multi {
                return Err(Error::Invalid(format!(
                    "{}: a task under multi/ must say tier = \"multi\"",
                    path.display()
                )));
            }
            out.push(Task {
                id,
                dir,
                tier: Tier::Multi,
                kind: file.kind,
                features: file.features,
            });
        }
    }
    Ok(out)
}

fn subdirs(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            out.push((
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
            ));
        }
    }
    out.sort();
    Ok(out)
}

/// Copies the checks folder `dir` to `out`, adds the browser-check helper
/// as `files/lib/cortex-check.mjs`, and returns the staged `checks.toml`,
/// ready to freeze.
pub fn stage_checks(dir: &Path, out: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(out)?;
    copy_dir(dir, out)?;
    let lib = out.join("files").join("lib");
    std::fs::create_dir_all(&lib)?;
    let helper = include_str!("../../../bench/browser/cortex-check.mjs");
    std::fs::write(lib.join("cortex-check.mjs"), helper)?;
    Ok(out.join("checks.toml"))
}

/// Copies `overlay` on top of `work`, leaving out its top-level `WHY.md`.
pub fn apply_overlay(overlay: &Path, work: &Path) -> Result<()> {
    for entry in std::fs::read_dir(overlay)? {
        let entry = entry?;
        if entry.file_name() == WHY {
            continue;
        }
        let dest = work.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            std::fs::create_dir_all(&dest)?;
            copy_dir(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

/// Builds `start/` with `overlay` on top under `home`, then runs the
/// task's visible and hidden checks on it in the sandbox.
pub fn grade(task: &Task, overlay: &Path, home: &Path) -> Result<Grade> {
    if task.tier != Tier::Multi {
        return Err(Error::Invalid(format!(
            "{} is a small prompt; only multi tasks have visible and hidden checks",
            task.id
        )));
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let root = home
        .join("grade")
        .join(format!("{}-{nanos}-{}", task.id, std::process::id()));
    let work = root.join("work");
    std::fs::create_dir_all(&work)?;
    copy_dir(&task.dir.join("start"), &work)?;
    apply_overlay(overlay, &work)?;
    Ok(Grade {
        visible_failed: failing(&task.dir.join("visible"), &root.join("visible"), &work)?,
        hidden_failed: failing(&task.dir.join("hidden"), &root.join("hidden"), &work)?,
    })
}

/// The ids of the checks in `dir` that do not pass on `work`.
fn failing(dir: &Path, staged: &Path, work: &Path) -> Result<Vec<String>> {
    let toml = stage_checks(dir, staged)?;
    let set = CheckSet::load(&toml)?;
    let mut failed = Vec::new();
    for check in &set.checks {
        let out = run(&RunSpec {
            cmd: check.command.clone(),
            workdir: work.to_path_buf(),
            checks_dir: Some(staged.to_path_buf()),
            timeout: CHECK_TIMEOUT,
        })?;
        if out.timed_out || out.exit_code != check.expect_exit {
            failed.push(check.id.clone());
        }
    }
    Ok(failed)
}
