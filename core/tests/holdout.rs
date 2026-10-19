#![cfg(target_os = "linux")]
//! A holdout bench (`bench.toml` says `holdout = true`) keeps its results
//! inside its own folder, so nothing about it can land in the public repo.

use cortex::Error;
use cortex::bench::task::{Task, Tier};
use cortex::bench::{self, RunOptions};
use cortex::engine::Engine;
use cortex::engine::fake::FakeEngine;
use cortex::store::Store;
use std::path::{Path, PathBuf};

fn fake(t: &Task) -> cortex::Result<Box<dyn Engine>> {
    Ok(Box::new(match t.tier {
        Tier::Small => FakeEngine::load(&t.dir.join("fake-solution.toml"))?,
        Tier::Multi => FakeEngine::from_overlay(&t.dir.join("solutions/reference"))?,
    }))
}

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

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_type().unwrap().is_dir() {
                out.extend(files_under(&entry.path()));
            } else {
                out.push(entry.path());
            }
        }
    }
    out
}

#[test]
fn holdout_results_stay_in_holdout_dir() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let tmp = tempfile::tempdir().unwrap();
    let holdout = tmp.path().join("holdout");
    std::fs::create_dir_all(&holdout).unwrap();
    std::fs::write(holdout.join("bench.toml"), "holdout = true\n").unwrap();
    copy(
        &manifest.join("tests/fixtures/multi/t00-counter"),
        &holdout.join("multi/t00-counter"),
    );
    copy(
        &manifest.join("../bench/prompts/01-fizzbuzz"),
        &holdout.join("prompts/01-fizzbuzz"),
    );
    let store = Store::open(&tmp.path().join("cortex.db")).unwrap();
    let home = tmp.path().join("home");
    let opts = RunOptions::default();

    // A public results folder is refused before anything runs.
    let public = tmp.path().join("public-results");
    let refused = bench::run(&holdout, &opts, &home, &store, &mut fake, &public);
    assert!(matches!(refused, Err(Error::Invalid(_))), "{refused:?}");
    assert!(files_under(&public).is_empty());
    // So is a path that only looks inside the holdout.
    let sneaky = holdout.join("results/../../public-results");
    let refused = bench::run(&holdout, &opts, &home, &store, &mut fake, &sneaky);
    assert!(matches!(refused, Err(Error::Invalid(_))), "{refused:?}");
    assert!(files_under(&public).is_empty());

    // Its own results folder is fine, and the file lands there.
    let own = holdout.join("results");
    let (report, path) = bench::run(&holdout, &opts, &home, &store, &mut fake, &own).unwrap();
    // One small prompt and one multi task.
    assert_eq!(report.summary.total, 2);
    assert_eq!(path.parent().unwrap(), own);
    // Its page sits beside it, and nothing else is written.
    let mut got = files_under(&own);
    got.sort();
    let mut want = vec![path.clone(), path.with_extension("html")];
    want.sort();
    assert_eq!(got, want);

    // The page tool will not put a holdout page anywhere public either.
    let outside = tmp.path().join("public-page.html");
    let refused = bench::write_page(&path, Some(&outside));
    assert!(matches!(refused, Err(Error::Invalid(_))), "{refused:?}");
    assert!(!outside.exists());
    let inside = own.join("again.html");
    assert_eq!(bench::write_page(&path, Some(&inside)).unwrap(), inside);
}

#[test]
fn a_public_bench_may_write_anywhere() {
    let tmp = tempfile::tempdir().unwrap();
    let public = tmp.path().join("bench");
    std::fs::create_dir_all(&public).unwrap();
    std::fs::write(public.join("bench.toml"), "holdout = false\n").unwrap();
    assert!(!bench::is_holdout(&public).unwrap());
    std::fs::remove_file(public.join("bench.toml")).unwrap();
    assert!(!bench::is_holdout(&public).unwrap());
    std::fs::write(public.join("bench.toml"), "holdout = \"yes\"\n").unwrap();
    assert!(bench::is_holdout(&public).is_err());
}
