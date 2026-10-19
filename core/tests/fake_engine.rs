use cortex::engine::fake::FakeEngine;
use cortex::engine::{CancelToken, Engine, SessionInput, SessionOutcome};
use std::path::Path;
use std::time::{Duration, Instant};

fn input(worktree: &Path, session: u32, brief: &str) -> SessionInput {
    SessionInput {
        brief: brief.to_string(),
        worktree: worktree.to_path_buf(),
        checks_dir: worktree.join("checks"),
        session,
        timeout: Duration::from_secs(30),
    }
}

fn run(engine: &mut FakeEngine, input: &SessionInput) -> (SessionOutcome, Vec<String>) {
    let mut lines = Vec::new();
    let out = engine
        .run(
            input,
            &mut |l| lines.push(l.to_string()),
            &mut |_| {},
            &CancelToken::new(),
        )
        .unwrap();
    (out, lines)
}

const TWO_SESSIONS: &str = r#"
[[session]]
[[session.write]]
path = "a.txt"
content = "first"
[[session.write]]
path = "sub/b.txt"
content = "second"

[[session]]
note = "all tests pass"
[[session.write]]
path = "a.txt"
content = "again"
"#;

#[test]
fn fake_applies_session_writes_in_order() {
    let wt = tempfile::tempdir().unwrap();
    let mut e = FakeEngine::from_toml(TWO_SESSIONS).unwrap();
    assert_eq!(e.name(), "fake");
    assert!(!e.version().unwrap().is_empty());
    let (out, lines) = run(&mut e, &input(wt.path(), 1, "do it"));
    assert!(matches!(out, SessionOutcome::Ended { .. }), "{out:?}");
    assert_eq!(
        std::fs::read_to_string(wt.path().join("a.txt")).unwrap(),
        "first"
    );
    assert_eq!(
        std::fs::read_to_string(wt.path().join("sub/b.txt")).unwrap(),
        "second"
    );
    let writes: Vec<&String> = lines.iter().filter(|l| l.starts_with("wrote ")).collect();
    assert_eq!(writes, ["wrote a.txt", "wrote sub/b.txt"]);
}

#[test]
fn fake_repeats_last_session() {
    let wt = tempfile::tempdir().unwrap();
    let mut e = FakeEngine::from_toml(TWO_SESSIONS).unwrap();
    for session in [2, 3, 7] {
        std::fs::write(wt.path().join("a.txt"), "reset").unwrap();
        let (out, _) = run(&mut e, &input(wt.path(), session, "x"));
        assert_eq!(
            out,
            SessionOutcome::Ended {
                note: "all tests pass".into(),
                reported_usd_micros: None,
            }
        );
        assert_eq!(
            std::fs::read_to_string(wt.path().join("a.txt")).unwrap(),
            "again"
        );
    }
}

#[test]
fn fake_refuses_paths_outside_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().join("wt");
    std::fs::create_dir(&wt).unwrap();
    let outside = dir.path().join("escaped.txt");
    for bad in [
        "../escaped.txt".to_string(),
        outside.display().to_string().replace('\\', "/"),
        "sub/../../escaped.txt".to_string(),
    ] {
        let script = format!(
            "[[session]]\n[[session.write]]\npath = \"ok.txt\"\ncontent = \"x\"\n[[session.write]]\npath = {bad:?}\ncontent = \"x\"\n"
        );
        let mut e = FakeEngine::from_toml(&script).unwrap();
        let (out, _) = run(&mut e, &input(&wt, 1, "x"));
        assert!(
            matches!(out, SessionOutcome::Failed { .. }),
            "{bad}: {out:?}"
        );
        assert!(!outside.exists(), "{bad}");
        // Nothing from a refused session is written, not even the good path.
        assert!(!wt.join("ok.txt").exists(), "{bad}");
    }
}

#[test]
fn fake_stops_when_cancelled() {
    let wt = tempfile::tempdir().unwrap();
    let mut e = FakeEngine::from_toml(
        "[[session]]\ndelay_ms = 20000\n[[session.write]]\npath = \"a.txt\"\ncontent = \"x\"\n",
    )
    .unwrap();
    let cancel = CancelToken::new();
    let c2 = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        c2.cancel();
    });
    let start = Instant::now();
    let out = e
        .run(&input(wt.path(), 1, "x"), &mut |_| {}, &mut |_| {}, &cancel)
        .unwrap();
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(cancel.is_cancelled());
    assert!(matches!(out, SessionOutcome::Failed { .. }), "{out:?}");
    assert!(!wt.path().join("a.txt").exists());
}

#[test]
fn fake_times_out_past_session_timeout() {
    let wt = tempfile::tempdir().unwrap();
    let mut e = FakeEngine::from_toml("[[session]]\ndelay_ms = 20000\n").unwrap();
    let mut i = input(wt.path(), 1, "x");
    i.timeout = Duration::from_millis(200);
    let start = Instant::now();
    let (out, _) = run(&mut e, &i);
    assert_eq!(out, SessionOutcome::TimedOut);
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn fake_reports_brief_in_transcript() {
    let wt = tempfile::tempdir().unwrap();
    let mut e = FakeEngine::from_toml(TWO_SESSIONS).unwrap();
    let long = "é".repeat(300);
    let (_, lines) = run(&mut e, &input(wt.path(), 1, &long));
    let brief = lines.iter().find(|l| l.starts_with("brief: ")).unwrap();
    assert_eq!(brief.trim_start_matches("brief: ").chars().count(), 200);
    let (_, lines) = run(&mut e, &input(wt.path(), 1, "short brief"));
    assert!(lines.contains(&"brief: short brief".to_string()));
}
