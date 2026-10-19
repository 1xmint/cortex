//! The bench: tasks with hidden checks, run as ordinary missions so the
//! score measures the whole system. Results files are only ever written
//! here, from real runs.

use crate::engine::Engine;
use crate::event::Event;
use crate::mission::{self, Budgets, Outcome};
use crate::payload::Spend;
use crate::store::{MissionId, Store};
use crate::{Error, Result, git};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub mod compare;
pub mod page;
pub mod score;
pub mod task;

use score::{Attempt, Failure, Story, Summary};
use task::{Task, Tier};

/// The results-file layout this code writes and reads.
pub const SCHEMA: u32 = 2;

/// One bench run: which brain ran it, every attempt, and the sums.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub schema: u32,
    /// Which brain design ran: `baseline` is the slice-1 loop.
    pub arm: String,
    /// `small`, `multi` or `all`.
    pub tier: String,
    pub worker_profile: String,
    pub engine: String,
    pub engine_version: String,
    /// True when the worker's price or limits are not yet confirmed.
    pub provisional: bool,
    pub cortex_commit: String,
    pub started: String,
    pub attempts: Vec<Attempt>,
    pub summary: Summary,
}

/// Reads a schema-2 results file. Slice-1 files have no `schema` field and
/// are refused by name rather than misread.
pub fn load(path: &Path) -> Result<Report> {
    let text = std::fs::read_to_string(path)?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?;
    match value.get("schema").and_then(serde_json::Value::as_u64) {
        None => Err(Error::Invalid(format!(
            "{} is a schema 1 (slice-1) results file; only schema {SCHEMA} can be loaded",
            path.display()
        ))),
        Some(n) if n != u64::from(SCHEMA) => Err(Error::Invalid(format!(
            "{} is schema {n}; only schema {SCHEMA} can be loaded",
            path.display()
        ))),
        Some(_) => serde_json::from_value(value)
            .map_err(|e| Error::Invalid(format!("{}: {e}", path.display()))),
    }
}

/// What to run and how to label it.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// `None` runs every tier.
    pub tier: Option<Tier>,
    pub only: Option<String>,
    pub arm: String,
    /// Tries per task; pass^k uses k = tries.
    pub tries: u32,
    pub worker_profile: String,
    pub provisional: bool,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            tier: None,
            only: None,
            arm: "baseline".into(),
            tries: 1,
            worker_profile: String::new(),
            provisional: false,
        }
    }
}

/// `<bench>/bench.toml`, which is optional.
#[derive(Deserialize)]
struct BenchFile {
    #[serde(default)]
    holdout: bool,
}

/// Whether `<bench>/bench.toml` marks this bench as a private holdout set.
pub fn is_holdout(bench: &Path) -> Result<bool> {
    let path = bench.join("bench.toml");
    if !path.exists() {
        return Ok(false);
    }
    let file: BenchFile = toml::from_str(&std::fs::read_to_string(&path)?)
        .map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?;
    Ok(file.holdout)
}

/// A holdout bench's results may only go to its own `results/`, so
/// nothing about its tasks can reach the public repository.
fn guard_results(bench: &Path, results: &Path) -> Result<()> {
    if !is_holdout(bench)? {
        return Ok(());
    }
    let own = std::path::absolute(bench.join("results"))?;
    let asked = std::path::absolute(results)?;
    if asked.components().any(|c| c == Component::ParentDir) || !asked.starts_with(&own) {
        return Err(Error::Invalid(format!(
            "holdout bench: results must stay in {}, not {}",
            own.display(),
            results.display()
        )));
    }
    println!("holdout run: results stay in {}", bench.display());
    Ok(())
}

/// Copies a task's starting project into `repo` and commits it.
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

/// Runs each chosen task `tries` times as a fresh mission with an engine
/// from `make_engine`, grades every done multi mission on its hidden
/// checks, and writes `<results>/<UTC date>-<arm>-<engine>.json` (with
/// `-2`, `-3`… if that name is taken) with its `.html` page beside it.
/// Returns the report and the file it wrote. A holdout bench refuses any
/// `results` outside its own `results/`. If an attempt fails to run, the
/// attempts finished so far are still written before the error returns.
pub fn run(
    bench: &Path,
    opts: &RunOptions,
    home: &Path,
    store: &Store,
    make_engine: &mut dyn FnMut(&Task) -> Result<Box<dyn Engine>>,
    results: &Path,
) -> Result<(Report, PathBuf)> {
    guard_results(bench, results)?;
    if opts.tries == 0 {
        return Err(Error::Invalid("tries must be at least 1".into()));
    }
    let chosen: Vec<Task> = task::tasks(bench, opts.tier)?
        .into_iter()
        .filter(|t| opts.only.as_deref().is_none_or(|id| t.id == id))
        .collect();
    if chosen.is_empty() {
        return Err(Error::Invalid(match &opts.only {
            Some(id) => format!("no bench task {id}"),
            None => "no bench tasks".into(),
        }));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let started = utc(now);
    let runs = home.join("bench").join(started.replace([':', '-'], ""));
    let mut engine_name = String::new();
    let mut engine_version = String::new();
    let mut attempts = Vec::new();
    let mut failed = None;
    'tasks: for task in &chosen {
        for try_no in 1..=opts.tries {
            let mut one = || -> Result<Attempt> {
                let mut engine = make_engine(task)?;
                if engine_name.is_empty() {
                    engine_name = engine.name().to_string();
                    engine_version = engine.version()?;
                }
                attempt(
                    task,
                    try_no,
                    &runs.join(&task.id),
                    home,
                    store,
                    engine.as_mut(),
                )
            };
            match one() {
                Ok(a) => {
                    println!(
                        "{} {} try {} ({} sessions, {:.0} s)",
                        if a.accepted { "pass" } else { "FAIL" },
                        a.task,
                        a.try_no,
                        a.sessions,
                        a.seconds
                    );
                    attempts.push(a);
                }
                Err(e) => {
                    failed = Some((format!("{} try {try_no}", task.id), e));
                    break 'tasks;
                }
            }
        }
    }
    // Nothing finished, so there is nothing to keep.
    if attempts.is_empty()
        && let Some((_, e)) = failed
    {
        return Err(e);
    }
    let summary = score::summarize(&attempts, opts.tries);
    let report = Report {
        schema: SCHEMA,
        arm: opts.arm.clone(),
        tier: match opts.tier {
            Some(Tier::Small) => "small",
            Some(Tier::Multi) => "multi",
            None => "all",
        }
        .into(),
        worker_profile: opts.worker_profile.clone(),
        engine: engine_name,
        engine_version,
        provisional: opts.provisional,
        cortex_commit: git::head(bench).unwrap_or_else(|_| "unknown".into()),
        started: started.clone(),
        attempts,
        summary,
    };
    std::fs::create_dir_all(results)?;
    let base = format!("{}-{}-{}", &started[..10], report.arm, report.engine);
    let mut path = results.join(format!("{base}.json"));
    let mut n = 2;
    while path.exists() {
        path = results.join(format!("{base}-{n}.json"));
        n += 1;
    }
    let text = serde_json::to_string_pretty(&report)
        .map_err(|e| Error::Invalid(format!("results: {e}")))?;
    std::fs::write(&path, text + "\n")?;
    std::fs::write(path.with_extension("html"), page::render(&report))?;
    if let Some((which, e)) = failed {
        return Err(Error::Invalid(format!(
            "{which} could not run ({e}); the attempts before it are in {}",
            path.display()
        )));
    }
    Ok((report, path))
}

/// Writes the page for a results file to `out` (default: beside it). A
/// results file inside a holdout bench may only have its page written
/// inside that bench's `results/`.
pub fn write_page(results: &Path, out: Option<&Path>) -> Result<PathBuf> {
    let report = load(results)?;
    let out = out.map_or_else(|| results.with_extension("html"), Path::to_path_buf);
    let abs = std::path::absolute(results)?;
    for dir in abs.ancestors().skip(1) {
        if is_holdout(dir)? {
            guard_results(dir, &out)?;
            break;
        }
    }
    std::fs::write(&out, page::render(&report))?;
    Ok(out)
}

/// One try at `task`, worked under `dir/try-<n>`.
fn attempt(
    task: &Task,
    try_no: u32,
    dir: &Path,
    home: &Path,
    store: &Store,
    engine: &mut dyn Engine,
) -> Result<Attempt> {
    let repo = dir.join(format!("try-{try_no}"));
    make_repo(&task.dir.join("start"), &repo)?;
    let request = std::fs::read_to_string(task.dir.join("prompt.md"))?;
    let request = request.trim();
    let checks = match task.tier {
        Tier::Small => task.dir.join("checks.toml"),
        Tier::Multi => task::stage_checks(
            &task.dir.join("visible"),
            &dir.join(format!("try-{try_no}-visible")),
        )?,
    };
    let m = mission::start(store, home, &repo, &checks, request, "bench")?;
    let clock = Instant::now();
    let outcome = mission::drive(store, home, &m, engine, &budgets(task.tier))?;
    let seconds = clock.elapsed().as_secs_f64();
    let commit = match outcome {
        Outcome::Done { commit } => Some(commit),
        _ => None,
    };
    let hidden = match (&commit, task.tier) {
        (Some(commit), Tier::Multi) => hidden_failures(task, &repo, commit, dir, try_no)?,
        _ => Vec::new(),
    };
    let log = read_log(store, &m)?;
    let files_changed = match (&commit, &log.base_commit) {
        (Some(c), Some(base)) => git::changed_files(&repo, base, c)?,
        _ => Vec::new(),
    };
    let mut failures = log.failures;
    failures.extend(hidden.iter().map(|(check_id, output_tail)| Failure {
        check_id: check_id.clone(),
        hidden: true,
        output_tail: output_tail.clone(),
    }));
    Ok(Attempt {
        task: task.id.clone(),
        kind: task.kind,
        try_no,
        mission_id: m.0,
        mission_done: commit.is_some(),
        accepted: commit.is_some() && hidden.is_empty(),
        hidden_failed: hidden.into_iter().map(|(id, _)| id).collect(),
        sessions: log.sessions,
        seconds,
        tokens: log.tokens,
        estimated_usd_micros: log.estimated_usd_micros,
        spent: score::total_spend(&log.spends),
        story: Story {
            request: request.to_string(),
            commit,
            files_changed,
            failures,
            stuck_reason: log.stuck_reason,
        },
    })
}

/// Runs the task's hidden checks, in the sandbox, on `commit` checked out
/// in a fresh worktree, and forgets the worktree after.
fn hidden_failures(
    task: &Task,
    repo: &Path,
    commit: &str,
    dir: &Path,
    try_no: u32,
) -> Result<Vec<(String, String)>> {
    let dest = dir.join(format!("try-{try_no}-graded"));
    let wt = git::create_worktree(repo, commit, &dest)?;
    let failed = task::failing(
        &task.dir.join("hidden"),
        &dir.join(format!("try-{try_no}-hidden")),
        &wt.path,
    );
    git::forget_worktree(repo, &dest)?;
    failed
}

/// What the event log says about one mission.
#[derive(Default)]
struct Log {
    base_commit: Option<String>,
    sessions: u32,
    tokens: u64,
    estimated_usd_micros: Option<u64>,
    spends: Vec<Spend>,
    /// Visible checks still failing at their last run, in the last session
    /// that ran checks.
    failures: Vec<Failure>,
    stuck_reason: Option<String>,
}

fn read_log(store: &Store, m: &MissionId) -> Result<Log> {
    let mut log = Log::default();
    let mut priced = true;
    let mut cost = 0u64;
    let mut last_session = None;
    let mut last_runs: BTreeMap<String, (bool, String)> = BTreeMap::new();
    for (_, e) in store.events(m)? {
        match e {
            Event::MissionStarted { base_commit, .. } => log.base_commit = Some(base_commit),
            Event::SessionStarted { .. } => log.sessions += 1,
            Event::ModelCall {
                tokens,
                estimated_usd_micros,
                spent,
                ..
            } => {
                log.tokens += tokens.input.unwrap_or(0) + tokens.output.unwrap_or(0);
                match estimated_usd_micros {
                    Some(c) => cost += c,
                    None => priced = false,
                }
                log.spends.push(spent);
            }
            Event::CheckRun {
                session,
                check_id,
                passed,
                output_tail,
                ..
            } => {
                if last_session != Some(session) {
                    last_session = Some(session);
                    last_runs.clear();
                }
                last_runs.insert(check_id, (passed, output_tail));
            }
            Event::MissionStuck { reason } => log.stuck_reason = Some(reason),
            _ => {}
        }
    }
    // No model calls means no price was ever reported: unknown, not 0.
    if priced && !log.spends.is_empty() {
        log.estimated_usd_micros = Some(cost);
    }
    log.failures = last_runs
        .into_iter()
        .filter(|(_, (passed, _))| !passed)
        .map(|(check_id, (_, output_tail))| Failure {
            check_id,
            hidden: false,
            output_tail,
        })
        .collect();
    Ok(log)
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
