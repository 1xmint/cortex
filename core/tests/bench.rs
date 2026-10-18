#![cfg(target_os = "linux")]
//! The bench: ten small prompts, each with hidden checks. Every check must
//! fail on the starting project (or the prompt proves nothing), and the
//! fake engine's known solutions must pass all ten.

use cortex::bench::{self, Report};
use cortex::checks::{CheckSet, freeze, red_run};
use cortex::engine::Engine;
use cortex::engine::fake::FakeEngine;
use cortex::store::{ProjectId, Store};
use std::path::{Path, PathBuf};

fn bench_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench")
}

fn fake(prompt: &bench::Prompt) -> cortex::Result<Box<dyn Engine>> {
    Ok(Box::new(FakeEngine::load(&prompt.fake_solution())?))
}

fn run_fake(only: Option<&str>) -> (tempfile::TempDir, Report, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("cortex.db")).unwrap();
    let (report, path) = bench::run(
        &bench_dir(),
        only,
        &dir.path().join("home"),
        &store,
        &mut fake,
        &dir.path().join("results"),
    )
    .unwrap();
    (dir, report, path)
}

#[test]
fn every_prompt_check_fails_on_start() {
    let prompts = bench::prompts(&bench_dir()).unwrap();
    assert_eq!(prompts.len(), 10);
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("cortex.db")).unwrap();
    for prompt in &prompts {
        let repo = dir.path().join(&prompt.id);
        bench::make_repo(&prompt.dir.join("start"), &repo).unwrap();
        let set = CheckSet::load(&prompt.dir.join("checks.toml")).unwrap();
        assert!(!set.checks.is_empty(), "{}", prompt.id);
        let m = store
            .create_mission(&ProjectId(prompt.id.clone()), "bench")
            .unwrap();
        let frozen = freeze(&set, &dir.path().join("home"), &m).unwrap();
        let red = red_run(&frozen, &repo, &store, &m).unwrap();
        assert!(red.blocked.is_empty(), "{}: {:?}", prompt.id, red.blocked);
        assert!(
            std::fs::read_to_string(prompt.dir.join("prompt.md"))
                .unwrap()
                .trim()
                .len()
                > 20,
            "{}",
            prompt.id
        );
    }
}

#[test]
fn fake_bench_passes_all_ten() {
    let (_dir, report, path) = run_fake(None);
    assert!(path.exists());
    let failed: Vec<_> = report
        .prompts
        .iter()
        .filter(|p| !p.passed)
        .map(|p| &p.id)
        .collect();
    assert!(failed.is_empty(), "failed: {failed:?}");
    assert_eq!((report.passed, report.total), (10, 10));
    for p in &report.prompts {
        assert_eq!(p.sessions, 1, "{}", p.id);
        assert!(p.blocked_checks.is_empty(), "{}", p.id);
    }
}

#[test]
fn results_file_matches_schema() {
    let (dir, report, path) = run_fake(Some("06-brackets"));
    assert_eq!(path.parent().unwrap(), dir.path().join("results"));
    let name = path.file_name().unwrap().to_str().unwrap();
    assert!(name.ends_with("-fake.json"), "{name}");
    assert_eq!(name.len(), "2026-10-18-fake.json".len(), "{name}");

    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(|k| k.as_str()).collect();
    let mut want = vec![
        "cortex_commit",
        "engine",
        "engine_version",
        "passed",
        "prompts",
        "started",
        "total",
    ];
    let mut got = keys.clone();
    got.sort();
    want.sort();
    assert_eq!(got, want);
    assert_eq!(v["engine"], "fake");
    assert!(v["engine_version"].is_string());
    assert!(!v["cortex_commit"].as_str().unwrap().is_empty());
    let started = v["started"].as_str().unwrap();
    assert!(started.ends_with('Z') && started.contains('T'), "{started}");
    assert_eq!(
        (v["passed"].as_u64(), v["total"].as_u64()),
        (Some(1), Some(1))
    );

    let prompts = v["prompts"].as_array().unwrap();
    assert_eq!(prompts.len(), 1);
    let p = prompts[0].as_object().unwrap();
    let mut got: Vec<&str> = p.keys().map(|k| k.as_str()).collect();
    got.sort();
    assert_eq!(
        got,
        [
            "blocked_checks",
            "id",
            "mission_id",
            "passed",
            "seconds",
            "sessions"
        ]
    );
    assert_eq!(p["id"], "06-brackets");
    assert_eq!(p["passed"], true);
    assert!(p["seconds"].is_number());
    assert!(p["blocked_checks"].is_array());
    assert_eq!(p["mission_id"], report.prompts[0].mission_id.as_str());

    let unknown = bench::run(
        &bench_dir(),
        Some("99-nope"),
        &dir.path().join("home"),
        &Store::open(&dir.path().join("cortex.db")).unwrap(),
        &mut fake,
        &dir.path().join("results"),
    );
    assert!(unknown.is_err());
}
