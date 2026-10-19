//! The mission loop: brief an engine, let it work in a fresh worktree, run
//! the frozen checks, and accept the work only when every check passes.
//! All state is read back from the mission's events.

use crate::checks::{CheckSet, Frozen, check_all, freeze, red_run};
use crate::engine::{CancelToken, Engine, SessionInput, SessionOutcome};
use crate::event::Event;
use crate::git::{Worktree, accept, create_worktree, forget_worktree, head, open_worktree};
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
    if !store.take_lease(m, &holder)? && !take_from_dead(store, m, &holder)? {
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

/// Takes over a lease whose holder process has died, so a crashed run can
/// be resumed at once instead of after the lease goes stale.
fn take_from_dead(store: &Store, m: &MissionId, holder: &str) -> Result<bool> {
    match store.lease_holder(m)? {
        Some(old) if !holder_alive(&old) => store.replace_lease(m, &old, holder),
        _ => Ok(false),
    }
}

/// Whether the process named in a lease holder (`<pid>-<uuid>`) still runs
/// on this machine. Unknown means alive.
fn holder_alive(holder: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        let Some(pid) = holder
            .split('-')
            .next()
            .and_then(|p| p.parse::<i32>().ok())
            .filter(|p| *p > 0)
        else {
            return true;
        };
        // Signal 0 only asks whether the process exists.
        let r = unsafe { libc::kill(pid, 0) };
        r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = holder;
        true
    }
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
    let red_dir = work_dir(home, m).join("red");
    if !red_run_done(&only_events(store, m)?, &frozen) {
        let wt = fresh_worktree(&st.repo, &st.base, &red_dir)?;
        let report = red_run(&frozen, &wt.path, store, m);
        wt.discard()?;
        report?;
    } else if red_dir.exists() {
        forget_worktree(&st.repo, &red_dir)?;
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
    let checker = Checker {
        repo: &st.repo,
        dir: work_dir(home, m).join("checks"),
        frozen: &frozen,
    };
    // A check checkout a dead process left behind.
    forget_worktree(&st.repo, &checker.dir)?;
    if let Some(done) = recover(store, m, &st, &checker)? {
        return Ok(done);
    }

    let mut parked = Duration::ZERO;
    loop {
        let logged = store.events(m)?;
        let seen = logged.last().map_or(0, |(seq, _)| *seq);
        let events: Vec<Event> = logged.into_iter().map(|(_, e)| e).collect();
        if pause_requested(&events) {
            store.append(m, &Event::MissionPaused)?;
            return Ok(Outcome::Paused);
        }
        let replans = replans_asked(&events);
        if replans > budgets.replans {
            return stuck(
                store,
                m,
                format!(
                    "the plan was changed {replans} times, more than the {} allowed",
                    budgets.replans
                ),
            );
        }
        let used = sessions_used(&events);
        if used >= budgets.sessions {
            return stuck(
                store,
                m,
                format!("no session passed every check after {used} tries"),
            );
        }
        // A usage limit, possibly set by a process that has since died.
        if let Some(until) = parked_until(&events)
            && until > unix_now()
        {
            parked += park(store, m, seen, until);
            continue;
        }
        // Time spent parked is the provider's, not the mission's.
        let Some(left) = budgets
            .mission_timeout
            .checked_sub(begun.elapsed().saturating_sub(parked))
        else {
            return stuck(store, m, "the mission's time limit ran out".into());
        };
        let session = last_session(&events) + 1;
        let text = brief(
            &st.request,
            &last_failures(&events),
            &added_by_person(&events),
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
            checks_dir: frozen.dir.clone(),
            session,
            timeout: budgets.session_timeout.min(left),
        };
        let mut transcript_err = None;
        let mut record_err = None;
        let cancel = CancelToken::new();
        let ended = AtomicBool::new(false);
        let outcome = std::thread::scope(|s| {
            s.spawn(|| watch_for_stop(store, m, seen, &cancel, &ended));
            let r = engine.run(
                &input,
                &mut |line| {
                    if transcript_err.is_none()
                        && let Err(e) = store.put_payload(m, line.as_bytes()).and_then(|r| {
                            store.append(
                                m,
                                &Event::Transcript {
                                    session,
                                    payload: Some(r),
                                },
                            )
                        })
                    {
                        transcript_err = Some(e);
                    }
                },
                &mut |rec| {
                    if record_err.is_none()
                        && let Err(e) = store.record_model_call(m, rec)
                    {
                        record_err = Some(e);
                    }
                },
                &cancel,
            );
            ended.store(true, Ordering::SeqCst);
            r
        })?;
        if let Some(e) = transcript_err.or(record_err) {
            return Err(e);
        }
        // The person stopped it: whatever the session did is dropped.
        if cancel.is_cancelled() {
            abandon(store, m, wt, session, INTERRUPTED)?;
            continue;
        }
        match outcome {
            SessionOutcome::Ended {
                note,
                reported_usd_micros,
            } => {
                store.append(
                    m,
                    &Event::SessionEnded {
                        session,
                        engine_note: note,
                        reported_estimate_usd_micros: reported_usd_micros,
                    },
                )?;
                if let Some(commit) = check_session(store, m, &checker, &wt, session, &base)? {
                    return complete(store, m, &st, wt, session, commit);
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
                let secs = retry_after.as_secs() + u64::from(retry_after.subsec_nanos() > 0);
                // The wait itself happens at the top of the loop, where a
                // restarted process finds it too.
                store.append(
                    m,
                    &Event::Parked {
                        until_unix: unix_now() + secs as i64,
                        reason: RATE_LIMITED.into(),
                    },
                )?;
            }
        }
    }
}

const RATE_LIMITED: &str = "rate limited";
const INTERRUPTED: &str = "interrupted";
const STOP_POLL: Duration = Duration::from_millis(500);

/// Cancels the running session once the person stops the mission, that
/// is, once an `Interrupt` lands after event `seen`.
fn watch_for_stop(
    store: &Store,
    m: &MissionId,
    seen: u64,
    cancel: &CancelToken,
    ended: &AtomicBool,
) {
    let mut polled = Instant::now();
    while !ended.load(Ordering::SeqCst) {
        if polled.elapsed() >= STOP_POLL {
            polled = Instant::now();
            if stop_asked(store, m, seen) {
                cancel.cancel();
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Whether the person stopped the mission after event `seen`.
fn stop_asked(store: &Store, m: &MissionId, seen: u64) -> bool {
    store.events_after(m, seen).is_ok_and(|new| {
        new.iter()
            .any(|(_, e)| matches!(e, Event::Interrupt { .. }))
    })
}

/// Waits until `until` (unix seconds), or until the person stops the
/// mission after event `seen`. The time it waited.
fn park(store: &Store, m: &MissionId, seen: u64, until: i64) -> Duration {
    let since = Instant::now();
    let mut polled = Instant::now();
    while unix_now() < until {
        if polled.elapsed() >= STOP_POLL {
            polled = Instant::now();
            if stop_asked(store, m, seen) {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    since.elapsed()
}

/// When the latest usage limit lifts.
fn parked_until(events: &[Event]) -> Option<i64> {
    events.iter().rev().find_map(|e| match e {
        Event::Parked { until_unix, .. } => Some(*until_unix),
        _ => None,
    })
}

/// Where and against what a session's work is checked.
struct Checker<'a> {
    repo: &'a Path,
    /// A clean checkout made for each check run, then removed.
    dir: PathBuf,
    frozen: &'a Frozen,
}

impl Checker<'_> {
    /// Runs every check on a clean checkout of `rev`, so only what was
    /// committed counts: files the session left out of the commit (ignored
    /// ones, say) cannot make a check pass.
    fn run(&self, store: &Store, m: &MissionId, rev: &str, session: u32) -> Result<bool> {
        let wt = fresh_worktree(self.repo, rev, &self.dir)?;
        let passed = check_all(self.frozen, &wt.path, session, store, m);
        wt.discard()?;
        passed
    }
}

/// Adds an instruction from the person; every later brief includes it.
pub fn steer(store: &Store, m: &MissionId, text: &str) -> Result<()> {
    still_open(store, m)?;
    store.append(
        m,
        &Event::Steer {
            text: text.to_string(),
        },
    )?;
    Ok(())
}

/// Stops the running session. With text, the mission re-plans with it;
/// without, the mission pauses until `resume`.
pub fn stop(store: &Store, m: &MissionId, text: Option<&str>) -> Result<()> {
    still_open(store, m)?;
    store.append(
        m,
        &Event::Interrupt {
            text: text.map(str::to_string),
        },
    )?;
    Ok(())
}

/// Lets a paused mission, or one asked to pause, carry on when driven.
pub fn resume(store: &Store, m: &MissionId) -> Result<()> {
    let events = only_events(store, m)?;
    started(&events)?;
    if finished(&events) == Some(Outcome::Paused) || pause_requested(&events) {
        store.append(m, &Event::MissionResumed)?;
    }
    Ok(())
}

fn still_open(store: &Store, m: &MissionId) -> Result<()> {
    let events = only_events(store, m)?;
    started(&events)?;
    match finished(&events) {
        Some(Outcome::Done { .. } | Outcome::Stuck { .. }) => {
            Err(Error::Invalid(format!("mission {} is finished", m.0)))
        }
        _ => Ok(()),
    }
}

/// Commits the session's work and runs every check on that commit (on the
/// base when the session changed nothing, so its failures still reach the
/// next brief). The commit, if all of them passed.
fn check_session(
    store: &Store,
    m: &MissionId,
    checker: &Checker,
    wt: &Worktree,
    session: u32,
    base: &str,
) -> Result<Option<String>> {
    let Some(commit) = session_commit(store, m, wt, session, base)? else {
        return Ok(None);
    };
    let passed = checker.run(store, m, commit.as_deref().unwrap_or(base), session)?;
    Ok(commit.filter(|_| passed))
}

/// The session's work as a commit: `Some(None)` when it changed nothing,
/// `None` when its `.git` was damaged and the session is abandoned. A
/// commit made before a crash is found again rather than lost.
fn session_commit(
    store: &Store,
    m: &MissionId,
    wt: &Worktree,
    session: u32,
    base: &str,
) -> Result<Option<Option<String>>> {
    let made = wt
        .commit_all(&format!("cortex: mission {} session {session}", m.0))
        .and_then(|c| match c {
            Some(c) => Ok(Some(c)),
            None => head(&wt.path).map(|h| (h != base).then_some(h)),
        });
    match made {
        Ok(c) => Ok(Some(c)),
        // The worker damaged its `.git`; its work cannot be trusted.
        Err(Error::Blocked(reason)) => {
            store.append(m, &Event::SessionAbandoned { session, reason })?;
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// Records the accepted commit, moves the result branch to it and ends
/// the mission. The worktree goes before `MissionDone`, so a finished
/// mission never leaves one behind.
fn complete(
    store: &Store,
    m: &MissionId,
    st: &Started,
    wt: Worktree,
    session: u32,
    commit: String,
) -> Result<Outcome> {
    store.append(
        m,
        &Event::CommitAccepted {
            session,
            commit: commit.clone(),
        },
    )?;
    wt.discard()?;
    finish(store, m, st, commit)
}

fn finish(store: &Store, m: &MissionId, st: &Started, commit: String) -> Result<Outcome> {
    accept(&st.repo, &result_branch(m), &commit)?;
    store.append(
        m,
        &Event::MissionDone {
            commit: commit.clone(),
        },
    )?;
    Ok(Outcome::Done { commit })
}

/// Finishes or clears away the last session a dead process left behind.
/// `Some` when that session's work completes the mission.
fn recover(
    store: &Store,
    m: &MissionId,
    st: &Started,
    checker: &Checker,
) -> Result<Option<Outcome>> {
    let events = only_events(store, m)?;
    let Some(i) = events
        .iter()
        .rposition(|e| matches!(e, Event::SessionStarted { .. }))
    else {
        return Ok(None);
    };
    let Event::SessionStarted {
        session, worktree, ..
    } = &events[i]
    else {
        unreachable!("rposition matched SessionStarted");
    };
    let (session, dest) = (*session, worktree.clone());
    let after = &events[i + 1..];

    if let Some(commit) = after.iter().find_map(|e| match e {
        Event::CommitAccepted { commit, .. } => Some(commit.clone()),
        _ => None,
    }) {
        forget_worktree(&st.repo, &dest)?;
        return finish(store, m, st, commit).map(Some);
    }
    if after
        .iter()
        .any(|e| matches!(e, Event::SessionAbandoned { .. }))
    {
        forget_worktree(&st.repo, &dest)?;
        return Ok(None);
    }
    if !after
        .iter()
        .any(|e| matches!(e, Event::SessionEnded { .. }))
    {
        return abandon_dead(store, m, st, &dest, session);
    }

    // The session ended; what its checks said decides.
    let blocked = blocked_checks(&events);
    let runs: Vec<(&str, bool)> = after
        .iter()
        .filter_map(|e| match e {
            Event::CheckRun {
                session: s,
                check_id,
                passed,
                ..
            } if *s == session => Some((check_id.as_str(), *passed)),
            _ => None,
        })
        .collect();
    let complete_set = checker
        .frozen
        .set
        .checks
        .iter()
        .filter(|c| !blocked.contains(&c.id))
        .all(|c| runs.iter().any(|(id, _)| *id == c.id));
    let all_passed = runs.iter().all(|(_, p)| *p);
    if complete_set && !all_passed {
        forget_worktree(&st.repo, &dest)?;
        return Ok(None);
    }
    if !dest.exists() {
        return abandon_dead(store, m, st, &dest, session);
    }
    let wt = match open_worktree(&st.repo, &dest) {
        Ok(wt) => wt,
        Err(Error::Blocked(reason)) => {
            store.append(m, &Event::SessionAbandoned { session, reason })?;
            forget_worktree(&st.repo, &dest)?;
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    let base = last_accepted(&events).unwrap_or_else(|| st.base.clone());
    let commit = if complete_set {
        session_commit(store, m, &wt, session, &base)?.flatten()
    } else {
        check_session(store, m, checker, &wt, session, &base)?
    };
    match commit {
        Some(c) => complete(store, m, st, wt, session, c).map(Some),
        None => {
            wt.discard()?;
            Ok(None)
        }
    }
}

fn abandon_dead(
    store: &Store,
    m: &MissionId,
    st: &Started,
    dest: &Path,
    session: u32,
) -> Result<Option<Outcome>> {
    store.append(
        m,
        &Event::SessionAbandoned {
            session,
            reason: "process ended".into(),
        },
    )?;
    forget_worktree(&st.repo, dest)?;
    Ok(None)
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
    forget_worktree(repo, dest)?;
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

/// True once every check of the current plan has a red-run result, so a
/// red run cut short by a crash is run again.
fn red_run_done(events: &[Event], frozen: &Frozen) -> bool {
    let recent = since_plan(events);
    frozen.set.checks.iter().all(|c| {
        recent.iter().any(|e| match e {
            Event::CheckRed { check_id, .. } | Event::CheckBlocked { check_id, .. } => {
                *check_id == c.id
            }
            _ => false,
        })
    })
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

/// Sessions that count against the budget: rate-limited ones and ones the
/// person stopped do not.
fn sessions_used(events: &[Event]) -> u32 {
    let started = events
        .iter()
        .filter(|e| matches!(e, Event::SessionStarted { .. }))
        .count();
    let excused = events
        .iter()
        .filter(|e| {
            matches!(e, Event::SessionAbandoned { reason, .. }
                if reason == RATE_LIMITED || reason == INTERRUPTED)
        })
        .count();
    (started - excused) as u32
}

/// A stop without text that no pause or resume has answered yet.
fn pause_requested(events: &[Event]) -> bool {
    let start = events
        .iter()
        .rposition(|e| matches!(e, Event::MissionPaused | Event::MissionResumed))
        .map_or(0, |i| i + 1);
    events[start..]
        .iter()
        .any(|e| matches!(e, Event::Interrupt { text: None }))
}

/// How many times the person stopped the mission with a new direction.
fn replans_asked(events: &[Event]) -> u32 {
    events
        .iter()
        .filter(|e| matches!(e, Event::Interrupt { text: Some(_) }))
        .count() as u32
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

/// Everything the person added, in order. Each session starts fresh, so
/// all of it goes into every brief.
fn added_by_person(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Steer { text } => Some(text.clone()),
            Event::Interrupt { text: Some(text) } => Some(text.clone()),
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
