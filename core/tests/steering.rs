#![cfg(target_os = "linux")]
//! The person can add instructions or stop a mission while it runs.

use cortex::engine::fake::FakeEngine;
use cortex::event::Event;
use cortex::mission::{Budgets, Outcome, drive, resume, start, steer, stop};
use cortex::store::{MissionId, Store};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn plain_git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

const CHECKS: &str = r#"
[[check]]
id = "greeting"
command = "diff /checks/files/expected.txt out.txt"
expect_exit = 0
"#;

const RIGHT: &str = "[[session]]\n[[session.write]]\npath = \"out.txt\"\ncontent = \"hello\\n\"\n";

/// A session that writes `content` after `delay_ms`.
fn session(delay_ms: u64, content: &str) -> String {
    format!(
        "[[session]]\ndelay_ms = {delay_ms}\n[[session.write]]\npath = \"out.txt\"\ncontent = \"{content}\\n\"\n"
    )
}

struct Env {
    dir: tempfile::TempDir,
    store: Store,
    repo: PathBuf,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    plain_git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("readme.txt"), "hello project\n").unwrap();
    plain_git(&repo, &["add", "."]);
    plain_git(&repo, &["commit", "-qm", "first"]);
    let checks = dir.path().join("checks");
    std::fs::create_dir_all(checks.join("files")).unwrap();
    std::fs::write(checks.join("files/expected.txt"), "hello\n").unwrap();
    std::fs::write(checks.join("checks.toml"), CHECKS).unwrap();
    let store = Store::open(&dir.path().join("cortex.db")).unwrap();
    Env { store, repo, dir }
}

impl Env {
    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }
    fn start(&self) -> MissionId {
        start(
            &self.store,
            &self.home(),
            &self.repo,
            &self.dir.path().join("checks/checks.toml"),
            "write hello to out.txt",
            "josh",
        )
        .unwrap()
    }
    fn drive(&self, m: &MissionId, script: &str, budgets: &Budgets) -> Outcome {
        let mut engine = FakeEngine::from_toml(script).unwrap();
        drive(&self.store, &self.home(), m, &mut engine, budgets).unwrap()
    }
    fn events(&self, m: &MissionId) -> Vec<Event> {
        self.store
            .events(m)
            .unwrap()
            .into_iter()
            .map(|(_, e)| e)
            .collect()
    }
    fn briefs(&self, m: &MissionId) -> Vec<String> {
        self.events(m)
            .into_iter()
            .filter_map(|e| match e {
                Event::SessionStarted { brief, .. } => Some(brief),
                _ => None,
            })
            .collect()
    }
    /// Waits until `n` sessions have started.
    fn wait_for_sessions(&self, m: &MissionId, n: usize) {
        let since = Instant::now();
        while self.briefs(m).len() < n {
            assert!(
                since.elapsed() < Duration::from_secs(10),
                "session {n} never started: {:#?}",
                self.events(m)
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn interrupted(&self, m: &MissionId) -> usize {
        self.events(m)
            .iter()
            .filter(|e| matches!(e, Event::SessionAbandoned { reason, .. } if reason == "interrupted"))
            .count()
    }
    fn worktrees(&self) -> usize {
        plain_git(&self.repo, &["worktree", "list"]).lines().count()
    }
}

#[test]
fn steer_during_session_reaches_next_brief() {
    let e = env();
    let m = e.start();
    let script = format!("{}{RIGHT}", session(1500, "nope"));
    let outcome = std::thread::scope(|s| {
        let run = s.spawn(|| e.drive(&m, &script, &Budgets::default()));
        e.wait_for_sessions(&m, 1);
        steer(&e.store, &m, "say it in lowercase").unwrap();
        run.join().unwrap()
    });
    assert!(matches!(outcome, Outcome::Done { .. }), "{outcome:?}");
    let briefs = e.briefs(&m);
    assert_eq!(briefs.len(), 2);
    assert!(!briefs[0].contains("say it in lowercase"));
    assert!(briefs[1].contains("say it in lowercase"), "{}", briefs[1]);
    // A steer does not stop the session it arrives in.
    assert_eq!(e.interrupted(&m), 0);
    // A finished mission takes no more instructions.
    assert!(steer(&e.store, &m, "too late").is_err());
    assert!(stop(&e.store, &m, None).is_err());
}

#[test]
fn steer_between_sessions_reaches_next_brief() {
    let e = env();
    let m = e.start();
    steer(&e.store, &m, "keep it short").unwrap();
    let script = format!("{}{RIGHT}", session(0, "nope"));
    let outcome = e.drive(&m, &script, &Budgets::default());
    assert!(matches!(outcome, Outcome::Done { .. }), "{outcome:?}");
    let briefs = e.briefs(&m);
    assert_eq!(briefs.len(), 2);
    // Sessions start fresh, so what the person added stays in every brief.
    assert!(briefs[0].contains("keep it short"), "{}", briefs[0]);
    assert!(briefs[1].contains("keep it short"), "{}", briefs[1]);
}

#[test]
fn stop_with_text_cancels_and_replans() {
    let e = env();
    let m = e.start();
    let script = format!("{}{RIGHT}", session(30_000, "hello"));
    let begun = Instant::now();
    let outcome = std::thread::scope(|s| {
        let run = s.spawn(|| e.drive(&m, &script, &Budgets::default()));
        e.wait_for_sessions(&m, 1);
        stop(&e.store, &m, Some("make the greeting lowercase")).unwrap();
        run.join().unwrap()
    });
    assert!(begun.elapsed() < Duration::from_secs(15), "{:?}", begun.elapsed());
    assert!(matches!(outcome, Outcome::Done { .. }), "{outcome:?}");
    assert_eq!(e.interrupted(&m), 1, "{:#?}", e.events(&m));
    let briefs = e.briefs(&m);
    assert_eq!(briefs.len(), 2);
    assert!(
        briefs[1].contains("make the greeting lowercase"),
        "{}",
        briefs[1]
    );
    assert_eq!(e.worktrees(), 1);
}

#[test]
fn stop_without_text_pauses_then_resume_finishes() {
    let e = env();
    let m = e.start();
    let script = session(30_000, "hello");
    let begun = Instant::now();
    let outcome = std::thread::scope(|s| {
        let run = s.spawn(|| e.drive(&m, &script, &Budgets::default()));
        e.wait_for_sessions(&m, 1);
        stop(&e.store, &m, None).unwrap();
        run.join().unwrap()
    });
    assert!(begun.elapsed() < Duration::from_secs(15), "{:?}", begun.elapsed());
    assert_eq!(outcome, Outcome::Paused);
    assert_eq!(e.interrupted(&m), 1);
    assert!(matches!(e.events(&m).last(), Some(Event::MissionPaused)));
    assert_eq!(e.worktrees(), 1);
    // Driving a paused mission does nothing until it is resumed.
    assert_eq!(e.drive(&m, RIGHT, &Budgets::default()), Outcome::Paused);
    assert_eq!(e.briefs(&m).len(), 1);

    resume(&e.store, &m).unwrap();
    assert!(matches!(e.events(&m).last(), Some(Event::MissionResumed)));
    let outcome = e.drive(&m, RIGHT, &Budgets::default());
    assert!(matches!(outcome, Outcome::Done { .. }), "{outcome:?}");
    assert_eq!(e.briefs(&m).len(), 2);
}

#[test]
fn replan_budget_exhausted_is_stuck() {
    let e = env();
    let m = e.start();
    let script = session(30_000, "hello");
    let budgets = Budgets {
        replans: 1,
        ..Budgets::default()
    };
    let outcome = std::thread::scope(|s| {
        let run = s.spawn(|| e.drive(&m, &script, &budgets));
        e.wait_for_sessions(&m, 1);
        stop(&e.store, &m, Some("try plan b")).unwrap();
        e.wait_for_sessions(&m, 2);
        stop(&e.store, &m, Some("try plan c")).unwrap();
        run.join().unwrap()
    });
    assert!(matches!(outcome, Outcome::Stuck { .. }), "{outcome:?}");
    assert_eq!(e.briefs(&m).len(), 2);
    assert_eq!(e.interrupted(&m), 2);
    assert!(matches!(
        e.events(&m).last(),
        Some(Event::MissionStuck { .. })
    ));
    assert_eq!(e.worktrees(), 1);
}
