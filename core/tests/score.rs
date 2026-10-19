//! Results v2: what an attempt costs, how often a task passes every try,
//! and the schema-2 file a bench run writes.

use cortex::bench::score::{Attempt, PerAccepted, Story, cost_per_accepted, pass_hat_k};
use cortex::bench::task::Kind;
use cortex::payload::Spend;
use std::path::Path;

fn attempt(accepted: bool, cost: Option<u64>) -> Attempt {
    Attempt {
        task: "t00-counter".into(),
        kind: Kind::Fresh,
        try_no: 1,
        mission_id: "m".into(),
        mission_done: accepted,
        accepted,
        hidden_failed: Vec::new(),
        sessions: 1,
        seconds: 1.0,
        tokens: 0,
        estimated_usd_micros: cost,
        spent: Spend::Unknown,
        story: Story::default(),
    }
}

#[test]
fn no_accepted_is_unbounded() {
    let all_failed = [
        attempt(false, Some(4)),
        attempt(false, Some(2)),
        attempt(false, Some(6)),
    ];
    assert_eq!(cost_per_accepted(&all_failed), PerAccepted::Unbounded);
    assert_eq!(cost_per_accepted(&[]), PerAccepted::Unbounded);
}

#[test]
fn unknown_cost_is_never_zero() {
    let batch = [
        attempt(true, Some(4)),
        attempt(true, Some(2)),
        attempt(false, None),
    ];
    assert_eq!(cost_per_accepted(&batch), PerAccepted::Unknown);
}

#[test]
fn failed_attempts_count_in_cost() {
    let batch = [
        attempt(false, Some(4)),
        attempt(true, Some(2)),
        attempt(false, Some(6)),
    ];
    assert_eq!(cost_per_accepted(&batch), PerAccepted::Usd { micros: 12 });
}

#[test]
fn pass_hat_k_matches_formula() {
    assert_eq!(pass_hat_k(3, 4, 2), 0.5);
    assert_eq!(pass_hat_k(4, 4, 3), 1.0);
    assert_eq!(pass_hat_k(1, 4, 2), 0.0);
}

#[test]
fn per_accepted_survives_a_round_trip() {
    for p in [
        PerAccepted::Usd { micros: 12 },
        PerAccepted::Unbounded,
        PerAccepted::Unknown,
    ] {
        let text = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<PerAccepted>(&text).unwrap(), p);
    }
}

#[test]
fn schema1_results_are_refused_by_name() {
    let slice1 =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench/results/2026-10-18-claude.json");
    let err = cortex::bench::load(&slice1).unwrap_err().to_string();
    assert!(err.contains("schema 1"), "{err}");
}

#[cfg(target_os = "linux")]
mod runs {
    use cortex::bench::task::{Task, Tier};
    use cortex::bench::{self, Report, RunOptions};
    use cortex::engine::Engine;
    use cortex::engine::fake::FakeEngine;
    use cortex::store::Store;
    use std::path::{Path, PathBuf};

    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let dest = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &dest);
            } else {
                std::fs::copy(entry.path(), dest).unwrap();
            }
        }
    }

    /// A bench holding only the fixture task, run with `solution` as the
    /// fake worker's answer.
    fn run_fixture(solution: &str, tries: u32) -> (tempfile::TempDir, Report, PathBuf) {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let tmp = tempfile::tempdir().unwrap();
        let bench_dir = tmp.path().join("bench");
        copy(
            &manifest.join("tests/fixtures/multi/t00-counter"),
            &bench_dir.join("multi/t00-counter"),
        );
        let store = Store::open(&tmp.path().join("cortex.db")).unwrap();
        let solution = solution.to_string();
        let mut fake = move |t: &Task| -> cortex::Result<Box<dyn Engine>> {
            Ok(Box::new(FakeEngine::from_overlay(
                &t.dir.join("solutions").join(&solution),
            )?))
        };
        let opts = RunOptions {
            tier: Some(Tier::Multi),
            tries,
            ..RunOptions::default()
        };
        let (report, path) = bench::run(
            &bench_dir,
            &opts,
            &tmp.path().join("home"),
            &store,
            &mut fake,
            &tmp.path().join("results"),
        )
        .unwrap();
        (tmp, report, path)
    }

    #[test]
    fn fake_multi_run_writes_schema2() {
        let (_tmp, report, path) = run_fixture("reference", 2);
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["schema"], 2);
        assert_eq!(v["arm"], "baseline");
        assert_eq!(v["tier"], "multi");
        assert_eq!(v["provisional"], false);
        assert_eq!(v["summary"]["total"], 2);
        assert_eq!(v["summary"]["accepted"], v["summary"]["total"]);
        assert_eq!(v["summary"]["pass_hat_k"]["t00-counter"], 1.0);
        // The fake engine reports no price, so cost is unknown, never 0.
        assert_eq!(v["summary"]["cost_per_accepted"]["kind"], "unknown");
        let attempts = v["attempts"].as_array().unwrap();
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0]["try"], 1);
        assert_eq!(attempts[1]["try"], 2);
        assert_ne!(attempts[0]["mission_id"], attempts[1]["mission_id"]);
        for a in &report.attempts {
            assert!(a.accepted && a.mission_done, "{a:?}");
            assert!(!a.story.files_changed.is_empty(), "{a:?}");
            assert!(a.story.commit.is_some(), "{a:?}");
            assert!(a.story.failures.is_empty(), "{a:?}");
            assert!(a.story.request.len() > 20, "{a:?}");
            assert_eq!(a.estimated_usd_micros, None);
        }
        assert_eq!(bench::load(&path).unwrap().summary.accepted, 2);
    }

    #[test]
    fn gamer_mission_done_but_not_accepted() {
        let (_tmp, report, _path) = run_fixture("bad-gamer", 1);
        let a = &report.attempts[0];
        assert!(a.mission_done);
        assert!(!a.accepted);
        assert!(!a.hidden_failed.is_empty());
        assert!(
            a.story.failures.iter().any(|f| f.hidden),
            "{:?}",
            a.story.failures
        );
        assert_eq!(report.summary.accepted, 0);
        assert_eq!(
            report.summary.cost_per_accepted,
            cortex::bench::score::PerAccepted::Unbounded
        );
    }
}
