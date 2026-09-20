//! The Vera tracker's resource bounds and retired legacy HTTP surface.
//!
//! `VeraTracker` is Cortex-owned in-memory state with no persistence and no
//! database access, which makes it easy to treat as harmless. It is not: it is
//! The tracker remains an internal component, but the combined legacy router
//! that exposed it has been removed.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use cortex_api::state::AppState;
use cortex_api::vera::{heart_id_from_user, VeraTracker};

async fn test_app() -> (axum::Router, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("failed to create temp dir");
    let workspace = tmp.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".cortex")).unwrap();

    let ledger_path = workspace.join(".cortex/ledger.jsonl");
    let state = AppState::new(ledger_path, workspace, None).await;

    (cortex_api::build_cortex_router(state), tmp)
}

/// The combined router's Vera endpoints are not inherited by Cortex.
#[tokio::test]
async fn the_legacy_vera_http_surface_is_gone() {
    let (app, _tmp) = test_app().await;

    async fn request(app: &axum::Router, method: &str, uri: &str) -> StatusCode {
        app.clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    for (method, path) in [
        ("POST", "/api/vera/simulate?agents=1000&per_agent=100"),
        ("POST", "/api/admin/vera/simulate"),
        ("GET", "/api/vera/network"),
        ("GET", "/api/vera/me"),
    ] {
        assert_eq!(request(&app, method, path).await, StatusCode::NOT_FOUND);
    }
}

/// The bound lives in the tracker, not in the handler.
///
/// The handler's caps were the only bound before, they were applied to each
/// factor independently, and their product was 100,000. Putting the bound on the
/// tracker means a second caller — a future admin tool, a test, a handler added
/// without reading this one — inherits it instead of having to remember it.
#[test]
fn simulation_is_bounded_however_it_is_asked() {
    let tracker = VeraTracker::new(heart_id_from_user("cortex"));

    // The old maximum request.
    let generated = tracker.simulate_ecosystem(1_000, 100);
    assert!(
        generated <= VeraTracker::MAX_SIMULATED_INTERACTIONS,
        "generated {generated}, above the {} bound",
        VeraTracker::MAX_SIMULATED_INTERACTIONS
    );

    // Absurd values in either factor, and in both.
    for (agents, per_agent) in [(usize::MAX, 1), (1, usize::MAX), (usize::MAX, usize::MAX)] {
        let generated = tracker.simulate_ecosystem(agents, per_agent);
        assert!(
            generated <= VeraTracker::MAX_SIMULATED_INTERACTIONS,
            "simulate_ecosystem({agents}, {per_agent}) generated {generated}"
        );
    }

    // Zero is not an error and not an overflow — it clamps up to one.
    assert!(tracker.simulate_ecosystem(0, 0) >= 1);
}

/// A request within the bound is honoured exactly, not silently reduced.
#[test]
fn a_reasonable_simulation_is_not_clamped() {
    let tracker = VeraTracker::new(heart_id_from_user("cortex"));
    assert_eq!(tracker.simulate_ecosystem(20, 10), 200);
}

/// Compaction history does not grow without limit.
///
/// `interactions` is drained every ten records, so that side was already
/// self-limiting. Level-2 compactions were not: one more every hundred
/// interactions, retained for the life of the process, and serialised in full
/// into every in-process snapshot.
#[test]
fn compaction_history_stays_bounded() {
    let tracker = VeraTracker::new(heart_id_from_user("cortex"));

    // 50,000 interactions — enough to produce 5,000 level-1 distillations and
    // 500 level-2 rollups, well past any sane retention.
    for _ in 0..25 {
        tracker.simulate_ecosystem(20, 100);
    }

    let snapshot = tracker.snapshot();
    assert!(
        snapshot.compaction.history.len() <= 256,
        "compaction history grew to {} entries",
        snapshot.compaction.history.len()
    );
    assert!(
        !snapshot.compaction.history.is_empty(),
        "bounding the history must not empty it"
    );
}
