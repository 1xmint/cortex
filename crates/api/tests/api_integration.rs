use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use cortex_api::scheduler;
use cortex_api::state::AppState;

/// Build a test app with auth disabled and a temp workspace directory.
/// Returns the Router and the temp dir handle (must be kept alive).
async fn test_app() -> (axum::Router, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("failed to create temp dir");
    let workspace = tmp.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".cortex")).unwrap();

    let ledger_path = workspace.join(".cortex/ledger.jsonl");
    let state = AppState::new(ledger_path, workspace, None).await;

    // Start the scheduler so run creation works
    let scheduler_tx = scheduler::spawn_scheduler(state.clone());
    state.set_scheduler_tx(scheduler_tx).await;

    let app = cortex_api::build_cortex_router(state);
    (app, tmp)
}

/// Helper to read a response body as a serde_json::Value.
async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("response body is not valid JSON")
}

// ─── Health ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_health() {
    let (app, _tmp) = test_app().await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert_eq!(json["status"], "ok");
    assert_eq!(json["service"], "cortex");
}

#[tokio::test]
async fn test_deployment_status() {
    let (app, _tmp) = test_app().await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/deployment/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert!(json["status"].is_string());
    assert_eq!(json["service"], "cortex");
    assert!(json["backend"].is_object());
    assert_eq!(json["backend"]["service"], "cortex");
    assert!(json["frontend"].is_object());
    assert!(json["frontend"]["expected_assets"].is_object());
}

#[tokio::test]
async fn test_deployment_events_include_status_inspection() {
    let (app, _tmp) = test_app().await;

    let status_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/deployment/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status_resp.status(), StatusCode::OK);

    let events_resp = app
        .oneshot(
            Request::builder()
                .uri("/api/deployment/events?limit=5")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(events_resp.status(), StatusCode::OK);

    let json = body_json(events_resp).await;
    assert_eq!(json["scope_id"], "cortex");
    assert_eq!(json["entity_type"], "deployment");
    assert_eq!(json["limit"], 5);
    assert_eq!(json["events"][0]["event_type"], "deploy.inspected");
    assert_eq!(json["events"][0]["entity_type"], "deployment");
    assert!(json["events"][0]["payload"]["backend"].is_object());
}

// ─── Runs ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_create_run() {
    let (app, _tmp) = test_app().await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"goal": "explore the codebase"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert!(
        json.get("run_id").is_some(),
        "response should contain 'run_id'"
    );
    let run_id = json["run_id"].as_str().unwrap();
    assert!(!run_id.is_empty(), "run_id should not be empty");
}

#[tokio::test]
async fn test_get_run() {
    let (app, _tmp) = test_app().await;

    // First, create a run
    let create_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"goal": "explore the codebase"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(create_resp.status(), StatusCode::OK);
    let create_json = body_json(create_resp).await;
    let run_id = create_json["run_id"].as_str().unwrap();

    // Now fetch it
    let get_resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/runs/{run_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(get_resp.status(), StatusCode::OK);

    let json = body_json(get_resp).await;
    assert_eq!(json["id"], run_id);
    assert_eq!(json["goal"], "explore the codebase");
    assert!(
        json.get("steps").is_some(),
        "response should contain 'steps'"
    );
}

#[tokio::test]
async fn test_get_run_not_found() {
    let (app, _tmp) = test_app().await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/runs/nonexistent-id-12345")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ─── Admin ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_admin_stats() {
    let (app, _tmp) = test_app().await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/admin/stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    // The stats endpoint returns a JSON object with various count fields
    assert!(json.is_object(), "stats should be a JSON object");
}

#[tokio::test]
async fn test_admin_workers() {
    let (app, _tmp) = test_app().await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/admin/workers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert!(
        json.get("connected").is_some(),
        "should have 'connected' field"
    );
    assert!(json.get("count").is_some(), "should have 'count' field");
    assert_eq!(json["count"], 0, "no workers connected in test");
}

// ─── Chat (SSE) ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_chat_with_greeting() {
    let (app, _tmp) = test_app().await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/chat")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"message": "hello"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    // SSE responses have content-type text/event-stream
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.contains("text/event-stream"),
        "expected text/event-stream, got: {content_type}"
    );
}

/// OpenCode Zen BYOK chat was removed (2026-09-25). A request still naming a
/// `"zen:"` model must get a clean 4xx, not a panic or a route to nowhere.
#[tokio::test]
async fn test_chat_rejects_zen_model_cleanly() {
    let (app, _tmp) = test_app().await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/chat")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"message": "hello", "model": "zen:some-model"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert_eq!(json["error"], "provider_not_available");
}

// ─── Rate Limiting ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_rate_limit() {
    let (app, _tmp) = test_app().await;

    // The rate limiter allows 60 requests burst for "anonymous" user.
    // Send 61 requests to a rate-limited endpoint and verify the last one is 429.
    //
    // `/api/runs/estimate` rather than `/api/route`: the latter was the dead
    // router and is gone. This test is about the limiter, not about whichever
    // handler sits behind it, so it wants any live route in the rate-limited
    // group that does not create state -- an estimate does not.
    let mut last_status = StatusCode::OK;
    for i in 0..=60 {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/runs/estimate")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"goal": "fix something"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        last_status = resp.status();

        // Once we hit 429, we can stop early
        if last_status == StatusCode::TOO_MANY_REQUESTS {
            assert!(
                i >= 60,
                "rate limit triggered too early at request {i}, expected at least 60"
            );
            break;
        }
    }

    assert_eq!(
        last_status,
        StatusCode::TOO_MANY_REQUESTS,
        "61st request should be rate-limited"
    );
}
