//! Acceptance checks: frozen outside the worker's copy, fingerprinted, seen
//! failing before work starts, and run by Cortex itself in the sandbox.

use crate::event::Event;
use crate::sandbox::{RunSpec, run};
use crate::store::{MissionId, Store};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

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

#[derive(Serialize, Deserialize)]
struct ChecksFile {
    check: Vec<Check>,
}

const CHECK_TIMEOUT: Duration = Duration::from_secs(5 * 60);
pub(crate) const TAIL: usize = 4000;
const PASSED_BEFORE_WORK: &str = "passed before any work";

impl CheckSet {
    /// Reads `[[check]]` entries; `files/` beside the TOML holds their files.
    pub fn load(toml_path: &Path) -> Result<CheckSet> {
        let text = std::fs::read_to_string(toml_path)?;
        let file: ChecksFile = toml::from_str(&text)
            .map_err(|e| Error::Invalid(format!("{}: {e}", toml_path.display())))?;
        if file.check.is_empty() {
            return Err(Error::Invalid(format!(
                "{} has no checks; a check set that cannot fail proves nothing",
                toml_path.display()
            )));
        }
        let mut seen = std::collections::HashSet::new();
        for c in &file.check {
            if !seen.insert(c.id.as_str()) {
                return Err(Error::Invalid(format!("check id `{}` is repeated", c.id)));
            }
        }
        let parent = toml_path.parent().unwrap_or(Path::new("."));
        Ok(CheckSet {
            checks: file.check,
            files_dir: parent.join("files"),
        })
    }
}

impl Frozen {
    /// Recomputes the fingerprint; any change to the frozen copy blocks.
    pub fn verify(&self) -> Result<()> {
        if fingerprint(&self.dir)? == self.fingerprint {
            Ok(())
        } else {
            Err(Error::Blocked(format!(
                "frozen checks at {} were changed",
                self.dir.display()
            )))
        }
    }
}

/// Copies the checks to `<home>/checks/<mission>/<fingerprint>/` and makes
/// them read-only.
pub fn freeze(set: &CheckSet, cortex_home: &Path, mission: &MissionId) -> Result<Frozen> {
    let parent = cortex_home.join("checks").join(&mission.0);
    let staging = parent.join(format!(".staging-{}", std::process::id()));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(staging.join("files"))?;
    let toml = toml::to_string(&ChecksFile {
        check: set.checks.clone(),
    })
    .map_err(|e| Error::Invalid(e.to_string()))?;
    std::fs::write(staging.join("checks.toml"), toml)?;
    if set.files_dir.is_dir() {
        copy_dir(&set.files_dir, &staging.join("files"))?;
    }
    let fp = fingerprint(&staging)?;
    let dir = parent.join(&fp);
    if dir.exists() {
        std::fs::remove_dir_all(&staging)?;
    } else {
        std::fs::rename(&staging, &dir)?;
        set_read_only(&dir)?;
    }
    let frozen = Frozen {
        fingerprint: fp,
        set: CheckSet {
            checks: set.checks.clone(),
            files_dir: dir.join("files"),
        },
        dir,
    };
    frozen.verify()?;
    Ok(frozen)
}

/// Runs every check before any work. A check that already passes cannot
/// prove anything, so it is blocked (invariant 2).
pub fn red_run(
    frozen: &Frozen,
    workdir: &Path,
    store: &Store,
    mission: &MissionId,
) -> Result<RedReport> {
    frozen.verify()?;
    let mut report = RedReport::default();
    for check in &frozen.set.checks {
        let out = run_one(frozen, check, workdir)?;
        if out.exit_code == check.expect_exit {
            store.append(
                mission,
                &Event::CheckBlocked {
                    check_id: check.id.clone(),
                    reason: PASSED_BEFORE_WORK.into(),
                },
            )?;
            report.blocked.push(check.id.clone());
        } else {
            store.append(
                mission,
                &Event::CheckRed {
                    check_id: check.id.clone(),
                    exit_code: out.exit_code,
                },
            )?;
        }
    }
    Ok(report)
}

/// Runs every non-blocked check and records each result. True only when
/// all of them passed and none is blocked.
pub fn check_all(
    frozen: &Frozen,
    workdir: &Path,
    session: u32,
    store: &Store,
    mission: &MissionId,
) -> Result<bool> {
    frozen.verify()?;
    let blocked = blocked_since_last_plan(store, mission)?;
    let mut all_passed = blocked.is_empty();
    for check in &frozen.set.checks {
        if blocked.contains(&check.id) {
            continue;
        }
        let out = run_one(frozen, check, workdir)?;
        let passed = !out.timed_out && out.exit_code == check.expect_exit;
        all_passed &= passed;
        let both = format!("{}{}", out.stdout, out.stderr);
        store.append(
            mission,
            &Event::CheckRun {
                session,
                check_id: check.id.clone(),
                exit_code: out.exit_code,
                passed,
                output_tail: tail(&both, TAIL),
            },
        )?;
    }
    Ok(all_passed)
}

fn run_one(frozen: &Frozen, check: &Check, workdir: &Path) -> Result<crate::sandbox::RunOutput> {
    run(&RunSpec {
        cmd: check.command.clone(),
        workdir: workdir.to_path_buf(),
        checks_dir: Some(frozen.dir.clone()),
        timeout: CHECK_TIMEOUT,
    })
}

/// Checks blocked by the red run of the current plan version.
fn blocked_since_last_plan(store: &Store, mission: &MissionId) -> Result<Vec<String>> {
    let mut blocked = Vec::new();
    for (_, e) in store.events(mission)? {
        match e {
            Event::PlanVersion { .. } => blocked.clear(),
            Event::CheckBlocked { check_id, .. } => blocked.push(check_id),
            _ => {}
        }
    }
    Ok(blocked)
}

pub(crate) fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}

/// SHA-256 over every file's relative path and contents, in path order.
fn fingerprint(dir: &Path) -> Result<String> {
    let mut files = Vec::new();
    list_files(dir, dir, &mut files)?;
    files.sort();
    let mut h = Sha256::new();
    for rel in files {
        let bytes = std::fs::read(dir.join(&rel))?;
        h.update(rel.as_bytes());
        h.update([0u8]);
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(&bytes);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn list_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            return Err(Error::Invalid(format!(
                "checks may not contain links: {}",
                path.display()
            )));
        }
        if meta.is_dir() {
            list_files(root, &path, out)?;
        } else {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let parts: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            out.push(parts.join("/"));
        }
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    for entry in std::fs::read_dir(from)? {
        let path = entry?.path();
        let meta = std::fs::symlink_metadata(&path)?;
        let dest = to.join(path.file_name().unwrap_or_default());
        if meta.file_type().is_symlink() {
            return Err(Error::Invalid(format!(
                "checks may not contain links: {}",
                path.display()
            )));
        } else if meta.is_dir() {
            std::fs::create_dir_all(&dest)?;
            copy_dir(&path, &dest)?;
        } else {
            std::fs::copy(&path, &dest)?;
        }
    }
    Ok(())
}

/// Files first, then their folder, so the walk can still list each folder.
fn set_read_only(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() {
        for entry in std::fs::read_dir(path)? {
            set_read_only(&entry?.path())?;
        }
    }
    let mut perms = meta.permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}
