use cortex::event::Event;
use cortex::store::{MissionId, ProjectId, Store};
use std::path::PathBuf;
use std::time::Duration;

fn temp_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("cortex.db")).unwrap();
    (dir, store)
}

fn project(name: &str) -> ProjectId {
    ProjectId(name.to_string())
}

fn every_kind() -> Vec<Event> {
    vec![
        Event::MissionStarted {
            project_id: "p".into(),
            person_id: "josh".into(),
            request: "add a greeting".into(),
            repo_path: PathBuf::from("/repo"),
            base_commit: "abc".into(),
        },
        Event::PlanVersion {
            version: 1,
            checks_fingerprint: "f1".into(),
            note: "first".into(),
        },
        Event::CheckRed {
            check_id: "c1".into(),
            exit_code: 1,
        },
        Event::CheckBlocked {
            check_id: "c2".into(),
            reason: "passes before work".into(),
        },
        Event::SessionStarted {
            session: 1,
            engine: "fake".into(),
            engine_version: "0".into(),
            worktree: PathBuf::from("/wt/1"),
            brief: "make c1 pass".into(),
        },
        Event::Transcript {
            session: 1,
            line: "edited main.rs".into(),
        },
        Event::SessionEnded {
            session: 1,
            engine_note: "done, I think".into(),
        },
        Event::SessionAbandoned {
            session: 2,
            reason: "crash".into(),
        },
        Event::CheckRun {
            session: 1,
            check_id: "c1".into(),
            exit_code: 0,
            passed: true,
            output_tail: "ok".into(),
        },
        Event::CommitAccepted {
            session: 1,
            commit: "def".into(),
        },
        Event::Steer {
            text: "use British spelling".into(),
        },
        Event::Interrupt { text: None },
        Event::Interrupt {
            text: Some("stop that".into()),
        },
        Event::Parked {
            until_unix: 1_800_000_000,
            reason: "rate limit".into(),
        },
        Event::MissionDone {
            commit: "def".into(),
        },
        Event::MissionStuck {
            reason: "budget".into(),
        },
        Event::MissionPaused,
        Event::MissionResumed,
    ]
}

#[test]
fn append_assigns_increasing_seq() {
    let (_d, store) = temp_store();
    let m = store.create_mission(&project("p"), "josh").unwrap();
    let e = Event::Steer { text: "x".into() };
    assert_eq!(store.append(&m, &e).unwrap(), 1);
    assert_eq!(store.append(&m, &e).unwrap(), 2);
    assert_eq!(store.append(&m, &e).unwrap(), 3);
    // Sequences are per mission.
    let other = store.create_mission(&project("p"), "josh").unwrap();
    assert_eq!(store.append(&other, &e).unwrap(), 1);
    let after: Vec<u64> = store
        .events_after(&m, 1)
        .unwrap()
        .into_iter()
        .map(|(s, _)| s)
        .collect();
    assert_eq!(after, vec![2, 3]);
}

#[test]
fn events_round_trip_every_kind() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cortex.db");
    let m = {
        let store = Store::open(&path).unwrap();
        let m = store.create_mission(&project("p"), "josh").unwrap();
        for e in every_kind() {
            store.append(&m, &e).unwrap();
        }
        m
    };
    // Read back through a fresh connection.
    let store = Store::open(&path).unwrap();
    let read: Vec<Event> = store
        .events(&m)
        .unwrap()
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    assert_eq!(read, every_kind());
}

#[test]
fn events_cannot_be_updated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cortex.db");
    let store = Store::open(&path).unwrap();
    let m = store.create_mission(&project("p"), "josh").unwrap();
    store.append(&m, &Event::MissionPaused).unwrap();
    let raw = rusqlite::Connection::open(&path).unwrap();
    // Positive control: the row is there to change.
    let n: i64 = raw
        .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
    let upd = raw.execute("UPDATE events SET kind = 'mission_resumed'", []);
    assert!(
        upd.unwrap_err().to_string().contains("append-only"),
        "update must be refused"
    );
    let del = raw.execute("DELETE FROM events", []);
    assert!(
        del.unwrap_err().to_string().contains("append-only"),
        "delete must be refused"
    );
    assert_eq!(store.events(&m).unwrap()[0].1, Event::MissionPaused);
}

#[test]
fn missions_are_scoped_to_project() {
    let (_d, store) = temp_store();
    let a1 = store.create_mission(&project("a"), "josh").unwrap();
    let a2 = store.create_mission(&project("a"), "josh").unwrap();
    let b1 = store.create_mission(&project("b"), "josh").unwrap();
    let mut a = store.missions(&project("a")).unwrap();
    a.sort_by(|x, y| x.0.cmp(&y.0));
    let mut want = vec![a1, a2];
    want.sort_by(|x, y| x.0.cmp(&y.0));
    assert_eq!(a, want);
    assert_eq!(store.missions(&project("b")).unwrap(), vec![b1]);
    assert!(store.missions(&project("c")).unwrap().is_empty());
}

#[test]
fn lease_blocks_second_holder_until_stale() {
    let (_d, store) = temp_store();
    // Generous margins: a slow CI runner once took over 300 ms between two
    // calls.
    let store = store.with_lease_ttl(Duration::from_millis(1500));
    let m = store.create_mission(&project("p"), "josh").unwrap();
    assert!(store.take_lease(&m, "one").unwrap());
    assert!(!store.take_lease(&m, "two").unwrap());
    // Renewing keeps it held.
    std::thread::sleep(Duration::from_millis(1000));
    store.renew_lease(&m, "one").unwrap();
    std::thread::sleep(Duration::from_millis(1000));
    assert!(!store.take_lease(&m, "two").unwrap());
    // Once stale, another holder may take it.
    std::thread::sleep(Duration::from_millis(2000));
    assert!(store.take_lease(&m, "two").unwrap());
    assert!(!store.take_lease(&m, "one").unwrap());
    // Releasing frees it at once.
    store.release_lease(&m, "two").unwrap();
    assert!(store.take_lease(&m, "one").unwrap());
}

#[test]
fn default_lease_ttl_is_fifteen_seconds() {
    let (_d, store) = temp_store();
    assert_eq!(store.lease_ttl(), Duration::from_secs(15));
}

#[test]
fn concurrent_appends_from_two_connections_get_distinct_seqs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cortex.db");
    let m = Store::open(&path)
        .unwrap()
        .create_mission(&project("p"), "josh")
        .unwrap();
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let path = path.clone();
            let m = MissionId(m.0.clone());
            std::thread::spawn(move || {
                let store = Store::open(&path).unwrap();
                (0..50)
                    .map(|_| store.append(&m, &Event::MissionPaused).unwrap())
                    .collect::<Vec<u64>>()
            })
        })
        .collect();
    let mut seqs: Vec<u64> = threads
        .into_iter()
        .flat_map(|t| t.join().unwrap())
        .collect();
    seqs.sort();
    assert_eq!(seqs, (1..=100).collect::<Vec<u64>>());
}
