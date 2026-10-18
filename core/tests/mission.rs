#![cfg(target_os = "linux")]

use cortex::engine::fake::FakeEngine;
use cortex::event::Event;
use cortex::mission::{Budgets, Outcome, drive, start};
use cortex::store::{MissionId, Store};
use std::path::{Path, PathBuf};
use std::process::Command;

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
const WRONG: &str = "[[session]]\n[[session.write]]\npath = \"out.txt\"\ncontent = \"nope\\n\"\n";

struct Env {
    dir: tempfile::TempDir,
    store: Store,
    repo: PathBuf,
    checks: PathBuf,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    plain_git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("readme.txt"), "hello project\n").unwrap();
    plain_git(&repo, &["add", "."]);
    plain_git(&repo, &["commit", "-qm", "first"]);
    let checks_dir = dir.path().join("checks");
    std::fs::create_dir_all(checks_dir.join("files")).unwrap();
    std::fs::write(checks_dir.join("files/expected.txt"), "hello\n").unwrap();
    std::fs::write(checks_dir.join("checks.toml"), CHECKS).unwrap();
    let store = Store::open(&dir.path().join("cortex.db")).unwrap();
    Env {
        store,
        repo,
        checks: checks_dir.join("checks.toml"),
        dir,
    }
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
            &self.checks,
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
    fn accepted(&self, m: &MissionId) -> usize {
        self.events(m)
            .iter()
            .filter(|e| matches!(e, Event::CommitAccepted { .. }))
            .count()
    }
    fn branch(&self, m: &MissionId) -> String {
        plain_git(
            &self.repo,
            &[
                "rev-parse",
                "-q",
                "--verify",
                &format!("refs/heads/cortex/{}", m.0),
            ],
        )
    }
}

#[test]
fn solves_on_first_session() {
    let e = env();
    let m = e.start();
    let outcome = e.drive(&m, RIGHT, &Budgets::default());
    let Outcome::Done { commit } = outcome else {
        panic!("{outcome:?}: {:#?}", e.events(&m));
    };
    assert_eq!(e.branch(&m), commit);
    assert_eq!(
        plain_git(&e.repo, &["show", &format!("{commit}:out.txt")]),
        "hello"
    );
    let events = e.events(&m);
    assert!(matches!(events[0], Event::MissionStarted { .. }));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::PlanVersion { version: 1, .. }))
    );
    assert!(events.iter().any(|e| matches!(e, Event::CheckRed { .. })));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::CheckRun { passed: true, .. }))
    );
    assert_eq!(e.accepted(&m), 1);
    assert_eq!(
        events.last(),
        Some(&Event::MissionDone {
            commit: commit.clone()
        })
    );
    let briefs = e.briefs(&m);
    assert_eq!(briefs.len(), 1);
    assert!(briefs[0].contains("write hello to out.txt"));
    // Every worktree Cortex made is gone again.
    assert_eq!(plain_git(&e.repo, &["worktree", "list"]).lines().count(), 1);
    // Driving a finished mission just reports the result.
    assert_eq!(
        e.drive(&m, WRONG, &Budgets::default()),
        Outcome::Done { commit }
    );
}

#[test]
fn retries_with_failure_output_in_brief() {
    let e = env();
    let m = e.start();
    let script = format!("{WRONG}{RIGHT}");
    let outcome = e.drive(&m, &script, &Budgets::default());
    assert!(matches!(outcome, Outcome::Done { .. }), "{outcome:?}");
    let briefs = e.briefs(&m);
    assert_eq!(briefs.len(), 2);
    assert!(!briefs[0].contains("> nope"));
    assert!(briefs[1].contains("greeting"), "{}", briefs[1]);
    assert!(briefs[1].contains("> nope"), "{}", briefs[1]);
    assert!(briefs[1].contains("write hello to out.txt"));
    assert_eq!(e.accepted(&m), 1);
}

#[test]
fn stops_stuck_after_session_budget() {
    let e = env();
    let m = e.start();
    let budgets = Budgets {
        sessions: 2,
        ..Budgets::default()
    };
    let outcome = e.drive(&m, WRONG, &budgets);
    assert!(matches!(outcome, Outcome::Stuck { .. }), "{outcome:?}");
    assert_eq!(e.briefs(&m).len(), 2);
    assert_eq!(e.accepted(&m), 0);
    assert!(matches!(
        e.events(&m).last(),
        Some(Event::MissionStuck { .. })
    ));
    assert_eq!(e.branch(&m), "");
    assert_eq!(plain_git(&e.repo, &["worktree", "list"]).lines().count(), 1);
}

#[test]
fn engine_claim_is_only_a_note() {
    let e = env();
    let m = e.start();
    let budgets = Budgets {
        sessions: 1,
        ..Budgets::default()
    };
    let outcome = e.drive(&m, "[[session]]\nnote = \"all tests pass\"\n", &budgets);
    assert!(matches!(outcome, Outcome::Stuck { .. }), "{outcome:?}");
    assert!(e.events(&m).iter().any(|e| matches!(
        e,
        Event::SessionEnded { engine_note, .. } if engine_note == "all tests pass"
    )));
    assert_eq!(e.accepted(&m), 0);
    assert_eq!(e.branch(&m), "");
}

#[test]
fn result_is_on_cortex_branch_only() {
    let e = env();
    let before = plain_git(&e.repo, &["rev-parse", "HEAD"]);
    let m = e.start();
    let outcome = e.drive(&m, RIGHT, &Budgets::default());
    assert!(matches!(outcome, Outcome::Done { .. }), "{outcome:?}");
    assert_eq!(plain_git(&e.repo, &["rev-parse", "HEAD"]), before);
    assert_eq!(
        plain_git(&e.repo, &["symbolic-ref", "HEAD"]),
        "refs/heads/main"
    );
    assert!(!e.repo.join("out.txt").exists());
    assert_eq!(plain_git(&e.repo, &["status", "--porcelain"]), "");
}

#[test]
fn check_passing_before_work_makes_mission_stuck() {
    let e = env();
    std::fs::write(
        &e.checks,
        "[[check]]\nid = \"always\"\ncommand = \"true\"\nexpect_exit = 0\n",
    )
    .unwrap();
    let m = e.start();
    let outcome = e.drive(&m, RIGHT, &Budgets::default());
    assert!(matches!(outcome, Outcome::Stuck { .. }), "{outcome:?}");
    // No session was spent on a mission that can never be proven.
    assert!(e.briefs(&m).is_empty());
}

#[test]
fn checks_run_on_the_commit_not_the_worktree() {
    let e = env();
    let m = e.start();
    // The right answer is on disk but ignored, so the commit lacks it.
    let script = r#"
[[session]]
[[session.write]]
path = "out.txt"
content = "hello\n"
[[session.write]]
path = ".gitignore"
content = "out.txt\n"
"#;
    let budgets = Budgets {
        sessions: 1,
        ..Budgets::default()
    };
    let outcome = e.drive(&m, script, &budgets);
    assert!(matches!(outcome, Outcome::Stuck { .. }), "{outcome:?}");
    assert_eq!(e.accepted(&m), 0);
    assert_eq!(plain_git(&e.repo, &["worktree", "list"]).lines().count(), 1);
}
