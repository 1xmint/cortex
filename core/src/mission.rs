//! The mission loop: brief an engine, let it work in a fresh worktree, run
//! the frozen checks, and accept the work only when every check passes.
//! All state is read back from the mission's events.

use crate::checks::{CheckSet, Frozen, check_all, freeze, red_run};
use crate::engine::{CancelToken, Engine, SessionInput, SessionOutcome};
use crate::event::Event;
use crate::git::{Worktree, accept, create_worktree, head};
use crate::store::{MissionId, ProjectId, Store};
use crate::{Error, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

/// A check that failed in the last checked session, for the next brief.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub check_id: String,
    pub output_tail: String,
}

/// What the mission is, from its `MissionStarted` event.
struct Started {
    request: String,
    repo: PathBuf,
    base: String,
}

/// The branch a mission's accepted work lands on.
pub fn result_branch(m: &MissionId) -> String {
    format!("cortex/{}", m.0)
}

/// Freezes the checks and records the mission. Nothing runs yet.
pub fn start(
    store: &Store,
    home: &Path,
    repo: &Path,
    checks: &Path,
    request: &str,
    person: &str,
) -> Result<MissionId> {
    let repo = std::path::absolute(repo)?;
    let base = head(&repo)?;
    let set = CheckSet::load(checks)?;
    let project = ProjectId(repo.to_string_lossy().into_owned());
    let m = store.create_mission(&project, person)?;
    freeze(&set, home, &m)?;
    store.append(
        &m,
        &Event::MissionStarted {
            project_id: project.0,
            person_id: person.to_string(),
            request: request.to_string(),
            repo_path: repo,
            base_commit: base,
        },
    )?;
    Ok(m)
}

/// Works the mission until it is done, stuck or paused. Only one process
/// may drive a mission at a time.
pub fn drive(
    store: &Store,
    home: &Path,
    m: &MissionId,
    engine: &mut dyn Engine,
    budgets: &Budgets,
) -> Result<Outcome> {
    let holder = format!("{}-{}", std::process::id(), uuid::Uuid::new_v4());
    if !store.take_lease(m, &holder)? {
        return Err(Error::Blocked(format!(
            "mission {} is being run by another process",
            m.0
        )));
    }
    let stop = AtomicBool::new(false);
    let result = std::thread::scope(|s| {
        s.spawn(|| heartbeat(store, m, &holder, &stop));
        let r = drive_held(store, home, m, engine, budgets);
        stop.store(true, Ordering::SeqCst);
        r
    });
    store.release_lease(m, &holder)?;
    result
}

fn heartbeat(store: &Store, m: &MissionId, holder: &str, stop: &AtomicBool) {
    let every = store.lease_ttl() / 3;
    loop {
        let since = Instant::now();
        while since.elapsed() < every {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = store.renew_lease(m, holder);
    }
}

fn drive_held(
    store: &Store,
    home: &Path,
    m: &MissionId,
    engine: &mut dyn Engine,
    budgets: &Budgets,
) -> Result<Outcome> {
    let begun = Instant::now();
    let events = only_events(store, m)?;
    if let Some(done) = finished(&events) {
        return Ok(done);
    }
    let st = started(&events)?;
    let frozen = plan(store, home, m, &events)?;
    if !red_run_done(&only_events(store, m)?) {
        let wt = fresh_worktree(&st.repo, &st.base, &work_dir(home, m).join("red"))?;
        let report = red_run(&frozen, &wt.path, store, m);
        wt.discard()?;
        report?;
    }
    let blocked = blocked_checks(&only_events(store, m)?);
    if !blocked.is_empty() {
        return stuck(
            store,
            m,
            format!(
                "these checks pass before any work, so they cannot prove anything: {}",
                blocked.join(", ")
            ),
        );
    }

    loop {
        let events = only_events(store, m)?;
        let used = sessions_used(&events);
        if used >= budgets.sessions {
            return stuck(
                store,
                m,
                format!("no session passed every check after {used} tries"),
            );
        }
        let Some(left) = budgets.mission_timeout.checked_sub(begun.elapsed()) else {
            return stuck(store, m, "the mission's time limit ran out".into());
        };
        let session = last_session(&events) + 1;
        let text = brief(
            &st.request,
            &last_failures(&events),
            &steers_since_session(&events),
        );
        let base = last_accepted(&events).unwrap_or_else(|| st.base.clone());
        let dest = work_dir(home, m).join(format!("s{session}"));
        let wt = fresh_worktree(&st.repo, &base, &dest)?;
        store.append(
            m,
            &Event::SessionStarted {
                session,
                engine: engine.name().to_string(),
                engine_version: engine.version()?,
                worktree: dest.clone(),
                brief: text.clone(),
            },
        )?;
        let input = SessionInput {
            brief: text,
            worktree: dest,
            session,
            timeout: budgets.session_timeout.min(left),
        };
        let mut transcript_err = None;
        let outcome = engine.run(
            &input,
            &mut |line| {
                if transcript_err.is_none()
                    && let Err(e) = store.append(
                        m,
                        &Event::Transcript {
                            session,
                            line: line.to_string(),
                        },
                    )
                {
                    transcript_err = Some(e);
                }
            },
            &CancelToken::new(),
        )?;
        if let Some(e) = transcript_err {
            return Err(e);
        }
        match outcome {
            SessionOutcome::Ended { note } => {
                store.append(
                    m,
                    &Event::SessionEnded {
                        session,
                        engine_note: note,
                    },
                )?;
                if let Some(commit) = check_session(store, m, &frozen, &wt, session)? {
                    store.append(
                        m,
                        &Event::CommitAccepted {
                            session,
                            commit: commit.clone(),
                        },
                    )?;
                    accept(&st.repo, &result_branch(m), &commit)?;
                    store.append(
                        m,
                        &Event::MissionDone {
                            commit: commit.clone(),
                        },
                    )?;
                    wt.discard()?;
                    return Ok(Outcome::Done { commit });
                }
                wt.discard()?;
            }
            SessionOutcome::TimedOut => {
                abandon(store, m, wt, session, "the session's time limit ran out")?;
            }
            SessionOutcome::Failed { reason } => {
                abandon(store, m, wt, session, &format!("engine failed: {reason}"))?;
            }
            SessionOutcome::RateLimited { retry_after } => {
                abandon(store, m, wt, session, RATE_LIMITED)?;
                let wait = retry_after.min(left);
                store.append(
                    m,
                    &Event::Parked {
                        until_unix: unix_now() + wait.as_secs() as i64,
                        reason: RATE_LIMITED.into(),
                    },
                )?;
                std::thread::sleep(wait);
            }
        }
    }
}

const RATE_LIMITED: &str = "rate limited";

/// Commits the session's work and runs every check on it. The commit, if
/// all of them passed.
fn check_session(
    store: &Store,
    m: &MissionId,
    frozen: &Frozen,
    wt: &Worktree,
    session: u32,
) -> Result<Option<String>> {
    let commit = match wt.commit_all(&format!("cortex: mission {} session {session}", m.0)) {
        Ok(c) => c,
        // The worker damaged its `.git`; its work cannot be trusted.
        Err(Error::Blocked(reason)) => {
            store.append(m, &Event::SessionAbandoned { session, reason })?;
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    let passed = check_all(frozen, &wt.path, session, store, m)?;
    Ok(commit.filter(|_| passed))
}

fn abandon(store: &Store, m: &MissionId, wt: Worktree, session: u32, reason: &str) -> Result<()> {
    store.append(
        m,
        &Event::SessionAbandoned {
            session,
            reason: reason.to_string(),
        },
    )?;
    wt.discard()
}

fn stuck(store: &Store, m: &MissionId, reason: String) -> Result<Outcome> {
    store.append(
        m,
        &Event::MissionStuck {
            reason: reason.clone(),
        },
    )?;
    Ok(Outcome::Stuck { reason })
}

/// The brief for the next session: the request, anything the person added,
/// and what failed last time.
pub fn brief(request: &str, failing: &[Failure], steers: &[String]) -> String {
    let mut b = format!("{request}\n");
    if !steers.is_empty() {
        b.push_str("\nThe person added:\n");
        for s in steers {
            b.push_str(&format!("- {s}\n"));
        }
    }
    if !failing.is_empty() {
        b.push_str("\nLast time these checks failed. Their output:\n");
        for f in failing {
            b.push_str(&format!("\n### {}\n{}\n", f.check_id, f.output_tail));
        }
    }
    b.push_str(
        "\nWork in the current folder. When you finish, Cortex runs the checks \
         itself; your own report is only a note.\n",
    );
    b
}

fn only_events(store: &Store, m: &MissionId) -> Result<Vec<Event>> {
    Ok(store.events(m)?.into_iter().map(|(_, e)| e).collect())
}

fn work_dir(home: &Path, m: &MissionId) -> PathBuf {
    home.join("work").join(&m.0)
}

/// A worktree at `dest`, replacing anything a crashed run left there.
fn fresh_worktree(repo: &Path, base: &str, dest: &Path) -> Result<Worktree> {
    if dest.exists() {
        std::fs::remove_dir_all(dest)?;
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    create_worktree(repo, base, dest)
}

fn finished(events: &[Event]) -> Option<Outcome> {
    let mut paused = false;
    for e in events {
        match e {
            Event::MissionDone { commit } => {
                return Some(Outcome::Done {
                    commit: commit.clone(),
                });
            }
            Event::MissionStuck { reason } => {
                return Some(Outcome::Stuck {
                    reason: reason.clone(),
                });
            }
            Event::MissionPaused => paused = true,
            Event::MissionResumed => paused = false,
            _ => {}
        }
    }
    paused.then_some(Outcome::Paused)
}

fn started(events: &[Event]) -> Result<Started> {
    events
        .iter()
        .find_map(|e| match e {
            Event::MissionStarted {
                request,
                repo_path,
                base_commit,
                ..
            } => Some(Started {
                request: request.clone(),
                repo: repo_path.clone(),
                base: base_commit.clone(),
            }),
            _ => None,
        })
        .ok_or_else(|| Error::Invalid("mission has no MissionStarted event".into()))
}

/// The frozen checks of the latest plan version, recording version 1 from
/// the checks `start` froze if no plan was recorded yet.
fn plan(store: &Store, home: &Path, m: &MissionId, events: &[Event]) -> Result<Frozen> {
    let latest = events.iter().rev().find_map(|e| match e {
        Event::PlanVersion {
            checks_fingerprint, ..
        } => Some(checks_fingerprint.clone()),
        _ => None,
    });
    let parent = home.join("checks").join(&m.0);
    let fingerprint = match latest {
        Some(fp) => fp,
        None => {
            let mut found = Vec::new();
            for entry in std::fs::read_dir(&parent)? {
                let name = entry?.file_name().to_string_lossy().into_owned();
                if !name.starts_with('.') {
                    found.push(name);
                }
            }
            let [fp] = &found[..] else {
                return Err(Error::Invalid(format!(
                    "expected one frozen check set in {}, found {}",
                    parent.display(),
                    found.len()
                )));
            };
            store.append(
                m,
                &Event::PlanVersion {
                    version: 1,
                    checks_fingerprint: fp.clone(),
                    note: "checks given at start".into(),
                },
            )?;
            fp.clone()
        }
    };
    let dir = parent.join(&fingerprint);
    let frozen = Frozen {
        set: CheckSet::load(&dir.join("checks.toml"))?,
        fingerprint,
        dir,
    };
    frozen.verify()?;
    Ok(frozen)
}

/// Events since the latest plan version.
fn since_plan(events: &[Event]) -> &[Event] {
    let start = events
        .iter()
        .rposition(|e| matches!(e, Event::PlanVersion { .. }))
        .map_or(0, |i| i + 1);
    &events[start..]
}

fn red_run_done(events: &[Event]) -> bool {
    since_plan(events)
        .iter()
        .any(|e| matches!(e, Event::CheckRed { .. } | Event::CheckBlocked { .. }))
}

fn blocked_checks(events: &[Event]) -> Vec<String> {
    since_plan(events)
        .iter()
        .filter_map(|e| match e {
            Event::CheckBlocked { check_id, .. } => Some(check_id.clone()),
            _ => None,
        })
        .collect()
}

/// Sessions that count against the budget: rate-limited ones do not.
fn sessions_used(events: &[Event]) -> u32 {
    let started = events
        .iter()
        .filter(|e| matches!(e, Event::SessionStarted { .. }))
        .count();
    let rate_limited = events
        .iter()
        .filter(|e| matches!(e, Event::SessionAbandoned { reason, .. } if reason == RATE_LIMITED))
        .count();
    (started - rate_limited) as u32
}

fn last_session(events: &[Event]) -> u32 {
    events
        .iter()
        .filter_map(|e| match e {
            Event::SessionStarted { session, .. } => Some(*session),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

fn last_failures(events: &[Event]) -> Vec<Failure> {
    let Some(last) = events.iter().rev().find_map(|e| match e {
        Event::CheckRun { session, .. } => Some(*session),
        _ => None,
    }) else {
        return Vec::new();
    };
    events
        .iter()
        .filter_map(|e| match e {
            Event::CheckRun {
                session,
                check_id,
                passed: false,
                output_tail,
                ..
            } if *session == last => Some(Failure {
                check_id: check_id.clone(),
                output_tail: output_tail.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// Steers sent since the last session started, including during it.
fn steers_since_session(events: &[Event]) -> Vec<String> {
    let start = events
        .iter()
        .rposition(|e| matches!(e, Event::SessionStarted { .. }))
        .map_or(0, |i| i + 1);
    events[start..]
        .iter()
        .filter_map(|e| match e {
            Event::Steer { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn last_accepted(events: &[Event]) -> Option<String> {
    events.iter().rev().find_map(|e| match e {
        Event::CommitAccepted { commit, .. } => Some(commit.clone()),
        _ => None,
    })
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}
