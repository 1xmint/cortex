#![cfg(target_os = "linux")]

use cortex::Error;
use cortex::checks::{CheckSet, Frozen, check_all, freeze, red_run};
use cortex::event::Event;
use cortex::store::{MissionId, ProjectId, Store};
use std::path::Path;

/// Writes `checks.toml` and `files/expected.txt` into `dir`.
fn write_set(dir: &Path, toml: &str) -> CheckSet {
    std::fs::create_dir_all(dir.join("files")).unwrap();
    std::fs::write(dir.join("files/expected.txt"), "hello\n").unwrap();
    std::fs::write(dir.join("checks.toml"), toml).unwrap();
    CheckSet::load(&dir.join("checks.toml")).unwrap()
}

const MATCHES_EXPECTED: &str = r#"
[[check]]
id = "greeting"
command = "cmp -s /checks/files/expected.txt out.txt"
expect_exit = 0
"#;

struct Fixture {
    dir: tempfile::TempDir,
    store: Store,
    mission: MissionId,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("cortex.db")).unwrap();
    let mission = store
        .create_mission(&ProjectId("p".into()), "josh")
        .unwrap();
    std::fs::create_dir(dir.path().join("work")).unwrap();
    Fixture {
        dir,
        store,
        mission,
    }
}

impl Fixture {
    fn home(&self) -> std::path::PathBuf {
        self.dir.path().join("home")
    }
    fn work(&self) -> std::path::PathBuf {
        self.dir.path().join("work")
    }
    fn frozen(&self, toml: &str) -> Frozen {
        let set = write_set(&self.dir.path().join("src"), toml);
        freeze(&set, &self.home(), &self.mission).unwrap()
    }
    fn events(&self) -> Vec<Event> {
        self.store
            .events(&self.mission)
            .unwrap()
            .into_iter()
            .map(|(_, e)| e)
            .collect()
    }
}

#[test]
fn freeze_is_stable_and_read_only() {
    let f = fixture();
    let a = f.frozen(MATCHES_EXPECTED);
    let again = freeze(&a.set, &f.home(), &MissionId("other".into())).unwrap();
    assert_eq!(a.fingerprint, again.fingerprint);
    assert_eq!(a.fingerprint.len(), 64);
    assert!(
        a.dir
            .starts_with(f.home().join("checks").join(&f.mission.0))
    );
    assert!(a.dir.ends_with(&a.fingerprint));
    a.verify().unwrap();
    let frozen_file = a.dir.join("files/expected.txt");
    assert_eq!(std::fs::read_to_string(&frozen_file).unwrap(), "hello\n");
    assert!(std::fs::write(&frozen_file, "changed").is_err());
    assert!(std::fs::write(a.dir.join("checks.toml"), "").is_err());
    // A different check set gets a different fingerprint.
    let other = fixture().frozen(&MATCHES_EXPECTED.replace("greeting", "other"));
    assert_ne!(a.fingerprint, other.fingerprint);
}

#[test]
fn tampered_frozen_checks_are_refused() {
    let f = fixture();
    let frozen = f.frozen(MATCHES_EXPECTED);
    let file = frozen.dir.join("files/expected.txt");
    let mut perms = std::fs::metadata(&file).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&file, perms).unwrap();
    std::fs::write(&file, "tampered\n").unwrap();
    assert!(matches!(frozen.verify(), Err(Error::Blocked(_))));
    std::fs::write(f.work().join("out.txt"), "tampered\n").unwrap();
    assert!(matches!(
        check_all(&frozen, &f.work(), 1, &f.store, &f.mission),
        Err(Error::Blocked(_))
    ));
    assert!(
        !f.events()
            .iter()
            .any(|e| matches!(e, Event::CheckRun { .. }))
    );
}

#[test]
fn check_that_never_failed_is_blocked() {
    let f = fixture();
    let frozen = f.frozen(
        r#"
[[check]]
id = "always"
command = "true"
expect_exit = 0
"#,
    );
    let report = red_run(&frozen, &f.work(), &f.store, &f.mission).unwrap();
    assert_eq!(report.blocked, vec!["always".to_string()]);
    assert_eq!(
        f.events(),
        vec![Event::CheckBlocked {
            check_id: "always".into(),
            reason: "passed before any work".into(),
        }]
    );
}

#[test]
fn failing_then_passing_check() {
    let f = fixture();
    let frozen = f.frozen(MATCHES_EXPECTED);
    let report = red_run(&frozen, &f.work(), &f.store, &f.mission).unwrap();
    assert!(report.blocked.is_empty());
    assert!(matches!(
        &f.events()[..],
        [Event::CheckRed { check_id, exit_code }] if check_id == "greeting" && *exit_code != 0
    ));
    assert!(!check_all(&frozen, &f.work(), 1, &f.store, &f.mission).unwrap());
    std::fs::write(f.work().join("out.txt"), "hello\n").unwrap();
    assert!(check_all(&frozen, &f.work(), 2, &f.store, &f.mission).unwrap());
    let runs: Vec<(u32, bool)> = f
        .events()
        .into_iter()
        .filter_map(|e| match e {
            Event::CheckRun {
                session, passed, ..
            } => Some((session, passed)),
            _ => None,
        })
        .collect();
    assert_eq!(runs, vec![(1, false), (2, true)]);
}

#[test]
fn mission_with_blocked_check_is_never_done() {
    let f = fixture();
    let frozen = f.frozen(&format!(
        "{MATCHES_EXPECTED}
[[check]]
id = \"always\"
command = \"true\"
expect_exit = 0
"
    ));
    let report = red_run(&frozen, &f.work(), &f.store, &f.mission).unwrap();
    assert_eq!(report.blocked, vec!["always".to_string()]);
    std::fs::write(f.work().join("out.txt"), "hello\n").unwrap();
    assert!(!check_all(&frozen, &f.work(), 1, &f.store, &f.mission).unwrap());
    // The real check did run and pass; only the blocked one holds it back.
    assert!(f.events().iter().any(|e| matches!(
        e,
        Event::CheckRun { check_id, passed: true, .. } if check_id == "greeting"
    )));
    assert!(!f.events().iter().any(|e| matches!(
        e,
        Event::CheckRun { check_id, .. } if check_id == "always"
    )));
}

#[test]
fn empty_check_set_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let toml = dir.path().join("checks.toml");
    std::fs::write(&toml, "check = []\n").unwrap();
    // No checks means nothing could ever fail, so nothing is proven.
    assert!(CheckSet::load(&toml).is_err());
}
