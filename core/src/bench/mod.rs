//! The bench: fixed prompts with hidden checks, run as ordinary missions so
//! the score measures the whole system. Results files are only ever
//! written here, from real runs.

use crate::engine::Engine;
use crate::event::Event;
use crate::mission::{self, Budgets, Outcome};
use crate::store::{MissionId, Store};
use crate::{Error, Result, git};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub mod task;

use task::Tier;

/// One prompt folder: `prompt.md`, `checks.toml`, `files/`, `start/` and
/// `fake-solution.toml`.
#[derive(Debug, Clone)]
pub struct Prompt {
    pub id: String,
    pub dir: PathBuf,
}

impl Prompt {
    pub fn fake_solution(&self) -> PathBuf {
        self.dir.join("fake-solution.toml")
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PromptResult {
    pub id: String,
    pub passed: bool,
    pub sessions: u32,
    pub seconds: f64,
    pub blocked_checks: Vec<String>,
    pub mission_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub engine: String,
    pub engine_version: String,
    pub cortex_commit: String,
    pub started: String,
    pub prompts: Vec<PromptResult>,
    pub passed: u32,
    pub total: u32,
}

/// The prompts under `<bench>/prompts`, in folder-name order.
pub fn prompts(bench: &Path) -> Result<Vec<Prompt>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(bench.join("prompts"))? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            out.push(Prompt {
                id: entry.file_name().to_string_lossy().into_owned(),
                dir: entry.path(),
            });
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Copies a prompt's starting project into `repo` and commits it.
pub fn make_repo(start: &Path, repo: &Path) -> Result<()> {
    std::fs::create_dir_all(repo)?;
    copy_dir(start, repo)?;
    git::init_and_commit(repo, "start")
}

pub(crate) fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            std::fs::create_dir_all(&dest)?;
            copy_dir(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

/// The mission budgets for a tier. Multi-feature tasks get a 30-minute
/// mission cap; small prompts keep the slice-1 defaults.
pub fn budgets(tier: Tier) -> Budgets {
    match tier {
        Tier::Small => Budgets::default(),
        Tier::Multi => Budgets {
            mission_timeout: Duration::from_secs(30 * 60),
            ..Budgets::default()
        },
    }
}

/// Runs every prompt (or just `only`) as a mission with a fresh engine
/// from `make_engine`, and writes `<results>/<UTC date>-<engine>.json`
/// (with `-2`, `-3`… if that name is taken). Returns the report and the
/// file it wrote.
pub fn run(
    bench: &Path,
    only: Option<&str>,
    home: &Path,
    store: &Store,
    make_engine: &mut dyn FnMut(&Prompt) -> Result<Box<dyn Engine>>,
    results: &Path,
) -> Result<(Report, PathBuf)> {
    let chosen: Vec<Prompt> = prompts(bench)?
        .into_iter()
        .filter(|p| only.is_none_or(|id| p.id == id))
        .collect();
    if chosen.is_empty() {
        return Err(Error::Invalid(match only {
            Some(id) => format!("no bench prompt {id}"),
            None => "no bench prompts".into(),
        }));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let started = utc(now);
    let runs = home.join("bench").join(started.replace([':', '-'], ""));
    let mut report = Report {
        engine: String::new(),
        engine_version: String::new(),
        cortex_commit: git::head(bench).unwrap_or_else(|_| "unknown".into()),
        started: started.clone(),
        prompts: Vec::new(),
        passed: 0,
        total: 0,
    };
    for prompt in &chosen {
        let mut engine = make_engine(prompt)?;
        if report.engine.is_empty() {
            report.engine = engine.name().to_string();
            report.engine_version = engine.version()?;
        }
        let result = run_one(prompt, &runs.join(&prompt.id), home, store, engine.as_mut())?;
        println!(
            "{} {} ({} sessions, {:.0} s)",
            if result.passed { "pass" } else { "FAIL" },
            result.id,
            result.sessions,
            result.seconds
        );
        report.passed += u32::from(result.passed);
        report.total += 1;
        report.prompts.push(result);
    }
    std::fs::create_dir_all(results)?;
    let base = format!("{}-{}", &started[..10], report.engine);
    let mut path = results.join(format!("{base}.json"));
    let mut n = 2;
    while path.exists() {
        path = results.join(format!("{base}-{n}.json"));
        n += 1;
    }
    let text = serde_json::to_string_pretty(&report)
        .map_err(|e| Error::Invalid(format!("results: {e}")))?;
    std::fs::write(&path, text + "\n")?;
    Ok((report, path))
}

fn run_one(
    prompt: &Prompt,
    repo: &Path,
    home: &Path,
    store: &Store,
    engine: &mut dyn Engine,
) -> Result<PromptResult> {
    make_repo(&prompt.dir.join("start"), repo)?;
    let request = std::fs::read_to_string(prompt.dir.join("prompt.md"))?;
    let checks = prompt.dir.join("checks.toml");
    let m = mission::start(store, home, repo, &checks, request.trim(), "bench")?;
    let clock = Instant::now();
    let outcome = mission::drive(store, home, &m, engine, &budgets(Tier::Small))?;
    let seconds = clock.elapsed().as_secs_f64();
    let (sessions, blocked_checks) = tally(store, &m)?;
    Ok(PromptResult {
        id: prompt.id.clone(),
        passed: matches!(outcome, Outcome::Done { .. }),
        sessions,
        seconds,
        blocked_checks,
        mission_id: m.0,
    })
}

fn tally(store: &Store, m: &MissionId) -> Result<(u32, Vec<String>)> {
    let mut sessions = 0;
    let mut blocked = Vec::new();
    for (_, e) in store.events(m)? {
        match e {
            Event::SessionStarted { .. } => sessions += 1,
            Event::CheckBlocked { check_id, .. } if !blocked.contains(&check_id) => {
                blocked.push(check_id)
            }
            _ => {}
        }
    }
    Ok((sessions, blocked))
}

/// `YYYY-MM-DDTHH:MM:SSZ` for seconds since 1970, by the days-to-civil
/// algorithm of Howard Hinnant.
fn utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_tier_caps_the_mission_at_30_minutes() {
        let multi = budgets(Tier::Multi);
        assert_eq!(multi.mission_timeout, Duration::from_secs(30 * 60));
        assert_eq!(multi.sessions, 4);
        assert_eq!(multi.session_timeout, Duration::from_secs(20 * 60));
        assert_eq!(
            budgets(Tier::Small).mission_timeout,
            Budgets::default().mission_timeout
        );
    }

    #[test]
    fn utc_formats_known_instants() {
        assert_eq!(super::utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(super::utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(super::utc(1_792_352_096), "2026-10-18T19:34:56Z");
    }
}
