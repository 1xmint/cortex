#![cfg(target_os = "linux")]
//! Kill tests (ADR 0003): the process dies right after each kind of event
//! is durable, and `cortex resume` must finish the mission exactly as an
//! uninterrupted run would.

use cortex::event::Event;
use cortex::mission::start;
use cortex::store::{MissionId, ProjectId, Store};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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

struct Env {
    dir: tempfile::TempDir,
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
    std::fs::write(dir.path().join("script.toml"), RIGHT).unwrap();
    Env { dir, repo }
}

impl Env {
    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }
    fn cortex(&self, args: &[&str], fault: Option<&str>) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cortex"));
        cmd.args(args).env("CORTEX_HOME", self.home());
        match fault {
            Some(kind) => cmd.env("CORTEX_FAULT", format!("after:{kind}")),
            None => cmd.env_remove("CORTEX_FAULT"),
        };
        cmd.output().unwrap()
    }
    fn engine_args(&self) -> Vec<String> {
        vec![
            "--engine".into(),
            "fake".into(),
            "--fake-script".into(),
            self.dir.path().join("script.toml").display().to_string(),
        ]
    }
    fn run(&self, fault: Option<&str>) -> Output {
        let mut args = vec![
            "run".to_string(),
            "--repo".into(),
            self.repo.display().to_string(),
            "--checks".into(),
            self.dir
                .path()
                .join("checks/checks.toml")
                .display()
                .to_string(),
        ];
        args.extend(self.engine_args());
        args.push("write hello to out.txt".into());
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        self.cortex(&args, fault)
    }
    fn resume(&self, m: &MissionId) -> Output {
        let mut args = vec!["resume".to_string(), m.0.clone()];
        args.extend(self.engine_args());
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        self.cortex(&args, None)
    }
    fn store(&self) -> Store {
        Store::open(&self.home().join("cortex.db")).unwrap()
    }
    fn only_mission(&self) -> MissionId {
        let project = ProjectId(self.repo.to_string_lossy().into_owned());
        let missions = self.store().missions(&project).unwrap();
        assert_eq!(missions.len(), 1, "{missions:?}");
        missions[0].clone()
    }
    fn events(&self, m: &MissionId) -> Vec<Event> {
        self.store()
            .events(m)
            .unwrap()
            .into_iter()
            .map(|(_, e)| e)
            .collect()
    }
    fn result_tree(&self, m: &MissionId) -> String {
        plain_git(
            &self.repo,
            &[
                "rev-parse",
                "-q",
                "--verify",
                &format!("refs/heads/cortex/{}^{{tree}}", m.0),
            ],
        )
    }
}

fn clean_tree() -> String {
    let e = env();
    let out = e.run(None);
    assert!(out.status.success(), "{out:?}");
    e.result_tree(&e.only_mission())
}

fn kill_then_resume(kind: &str) {
    let e = env();
    let out = e.run(Some(kind));
    assert_eq!(out.status.signal(), Some(SIGABRT), "{kind}: {out:?}");
    let m = e.only_mission();
    assert!(
        !matches!(e.events(&m).last(), Some(Event::MissionDone { .. })),
        "{kind}: finished before the fault"
    );

    let out = e.resume(&m);
    assert!(out.status.success(), "{kind}: {out:?}");
    let events = e.events(&m);
    assert!(
        matches!(events.last(), Some(Event::MissionDone { .. })),
        "{kind}: {events:#?}"
    );
    let accepted = events
        .iter()
        .filter(|e| matches!(e, Event::CommitAccepted { .. }))
        .count();
    assert_eq!(accepted, 1, "{kind}: {events:#?}");
    assert_eq!(e.result_tree(&m), clean_tree(), "{kind}");
    // Nothing the dead process left behind is still registered.
    assert_eq!(
        plain_git(&e.repo, &["worktree", "list"]).lines().count(),
        1,
        "{kind}"
    );
}

const SIGABRT: i32 = 6;

#[test]
fn kill_after_mission_started_then_resume_matches_clean_run() {
    kill_then_resume("mission_started");
}

#[test]
fn kill_after_plan_version_then_resume_matches_clean_run() {
    kill_then_resume("plan_version");
}

#[test]
fn kill_after_check_red_then_resume_matches_clean_run() {
    kill_then_resume("check_red");
}

#[test]
fn kill_after_session_started_then_resume_matches_clean_run() {
    kill_then_resume("session_started");
}

#[test]
fn kill_after_transcript_then_resume_matches_clean_run() {
    kill_then_resume("transcript");
}

#[test]
fn kill_after_session_ended_then_resume_matches_clean_run() {
    kill_then_resume("session_ended");
}

#[test]
fn kill_after_check_run_then_resume_matches_clean_run() {
    kill_then_resume("check_run");
}

#[test]
fn kill_after_commit_accepted_then_resume_matches_clean_run() {
    kill_then_resume("commit_accepted");
}

#[test]
fn resume_refuses_while_lease_is_live() {
    let e = env();
    std::fs::create_dir_all(e.home()).unwrap();
    let store = e.store();
    let m = start(
        &store,
        &e.home(),
        &e.repo,
        &e.dir.path().join("checks/checks.toml"),
        "write hello to out.txt",
        "josh",
    )
    .unwrap();
    // This test process is alive, so its lease must be respected.
    assert!(
        store
            .take_lease(&m, &format!("{}-test", std::process::id()))
            .unwrap()
    );
    let out = e.resume(&m);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("another process"),
        "{out:?}"
    );
    // Nothing ran.
    assert!(
        !e.events(&m)
            .iter()
            .any(|e| matches!(e, Event::SessionStarted { .. }))
    );
}
