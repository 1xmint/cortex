// Every multi-feature task's graders, proven both ways: each check fails on
// `start/`, the reference passes everything, and every bad solution fails
// something. Loops over `bench/multi/` and the fixture, so new tasks are
// covered without new tests.

use cortex::bench::task::{Kind, Task, Tier, tasks};
use cortex::checks::CheckSet;
use std::path::{Path, PathBuf};

fn all_tasks() -> Vec<Task> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = tasks(&root.join("../bench"), Some(Tier::Multi)).unwrap();
    out.extend(tasks(&root.join("tests/fixtures"), Some(Tier::Multi)).unwrap());
    assert!(
        out.iter().any(|t| t.id == "t00-counter"),
        "the fixture task is missing"
    );
    out
}

fn bads(task: &Task) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(task.dir.join("solutions"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("bad-"))
        .collect();
    out.sort();
    out
}

#[test]
fn task_files_are_complete() {
    for task in all_tasks() {
        let d = &task.dir;
        for f in [
            "prompt.md",
            "task.toml",
            "visible/checks.toml",
            "hidden/checks.toml",
        ] {
            assert!(d.join(f).is_file(), "{}: missing {f}", task.id);
        }
        for f in [
            "start",
            "visible/files",
            "hidden/files",
            "solutions/reference",
        ] {
            assert!(d.join(f).is_dir(), "{}: missing {f}/", task.id);
        }
        CheckSet::load(&d.join("visible/checks.toml")).unwrap();
        CheckSet::load(&d.join("hidden/checks.toml")).unwrap();
        let bads = bads(&task);
        assert!(bads.len() >= 2, "{}: needs at least two bads", task.id);
        assert!(
            d.join("solutions/bad-gamer").is_dir(),
            "{}: needs bad-gamer",
            task.id
        );
        for bad in &bads {
            assert!(
                bad.join("WHY.md").is_file(),
                "{}: {} has no WHY.md",
                task.id,
                bad.display()
            );
        }
        assert!(
            (5..=10).contains(&task.features.len()),
            "{}: {} features, wants 5 to 10",
            task.id,
            task.features.len()
        );
        if task.kind == Kind::Foreign {
            let source = std::fs::read_to_string(d.join("start/SOURCE.md"))
                .unwrap_or_else(|_| panic!("{}: foreign task needs start/SOURCE.md", task.id));
            for word in ["http", "commit", "licen"] {
                assert!(
                    source.to_lowercase().contains(word),
                    "{}: SOURCE.md must name the upstream URL, commit and licence",
                    task.id
                );
            }
        }
    }
}

// These run checks, so they need the Linux sandbox.
#[cfg(target_os = "linux")]
mod sandboxed {
    use super::{all_tasks, bads};
    use cortex::bench::task::{grade, stage_checks};
    use cortex::checks::{CheckSet, freeze, red_run};
    use cortex::store::{ProjectId, Store};
    use std::path::Path;
    use std::sync::Mutex;

    // One browser at a time: these checks start Chromium.
    static ONE_BROWSER: Mutex<()> = Mutex::new(());

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let dest = to.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy_dir(&e.path(), &dest);
            } else {
                std::fs::copy(e.path(), dest).unwrap();
            }
        }
    }

    #[test]
    fn start_fails_every_check() {
        let _one = ONE_BROWSER.lock().unwrap_or_else(|e| e.into_inner());
        for task in all_tasks() {
            let tmp = tempfile::tempdir().unwrap();
            let store = Store::open(&tmp.path().join("cortex.db")).unwrap();
            let work = tmp.path().join("work");
            copy_dir(&task.dir.join("start"), &work);
            for side in ["visible", "hidden"] {
                let staged = stage_checks(&task.dir.join(side), &tmp.path().join(side)).unwrap();
                let mission = store
                    .create_mission(&ProjectId(task.id.clone()), "bench")
                    .unwrap();
                let set = CheckSet::load(&staged).unwrap();
                let frozen = freeze(&set, &tmp.path().join("home"), &mission).unwrap();
                let report = red_run(&frozen, &work, &store, &mission).unwrap();
                assert!(
                    report.blocked.is_empty(),
                    "{} {side}: these checks pass on start/: {:?}",
                    task.id,
                    report.blocked
                );
            }
        }
    }

    #[test]
    fn reference_passes_everything() {
        let _one = ONE_BROWSER.lock().unwrap_or_else(|e| e.into_inner());
        for task in all_tasks() {
            let home = tempfile::tempdir().unwrap();
            let g = grade(&task, &task.dir.join("solutions/reference"), home.path()).unwrap();
            assert!(
                g.visible_failed.is_empty() && g.hidden_failed.is_empty(),
                "{} reference failed: {g:?}",
                task.id
            );
        }
    }

    #[test]
    fn every_bad_solution_fails() {
        let _one = ONE_BROWSER.lock().unwrap_or_else(|e| e.into_inner());
        for task in all_tasks() {
            for bad in bads(&task) {
                let home = tempfile::tempdir().unwrap();
                let g = grade(&task, &bad, home.path()).unwrap();
                assert!(
                    !g.visible_failed.is_empty() || !g.hidden_failed.is_empty(),
                    "{} {} passed every check",
                    task.id,
                    bad.display()
                );
            }
        }
    }

    #[test]
    fn gamer_passes_visible_fails_hidden() {
        let _one = ONE_BROWSER.lock().unwrap_or_else(|e| e.into_inner());
        for task in all_tasks() {
            let home = tempfile::tempdir().unwrap();
            let g = grade(&task, &task.dir.join("solutions/bad-gamer"), home.path()).unwrap();
            assert!(g.visible_failed.is_empty(), "{} gamer: {g:?}", task.id);
            assert!(!g.hidden_failed.is_empty(), "{} gamer: {g:?}", task.id);
        }
    }
}
