//! The Soma fence, checked from outside the crate.
//!
//! Soma is a work-in-progress project that is not part of Cortex. The `soma`
//! feature is off by default, and these tests assert what that means at the
//! HTTP boundary: the endpoints are absent, the health report does not describe
//! a subsystem this build does not have, and the `Soma ` authorization scheme
//! is refused rather than misread as a malformed JWT.
//!
//! The whole file is compiled out when the feature is on, because every
//! assertion in it is about the feature being off.
#![cfg(not(feature = "soma"))]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use cortex_api::state::AppState;

async fn test_app() -> (axum::Router, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("failed to create temp dir");
    let workspace = tmp.path().to_path_buf();
    std::fs::create_dir_all(workspace.join(".cortex")).unwrap();

    let ledger_path = workspace.join(".cortex/ledger.jsonl");
    let state = AppState::new(ledger_path, workspace, None).await;

    (cortex_api::build_cortex_router(state), tmp)
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("response body is not valid JSON")
}

/// Every `/api/soma/*` route is absent, not stubbed.
///
/// A 404 says this build does not have the subsystem. A 200 carrying
/// `{"error": "not initialized"}` would say it has one that is broken, and a
/// client cannot tell the difference between that and a real outage.
#[tokio::test]
async fn soma_routes_are_absent() {
    let (app, _tmp) = test_app().await;

    for path in [
        "/api/soma/identity",
        "/api/soma/me",
        "/api/soma/spend",
        "/api/soma/spend/some-delegation",
    ] {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "{path} should not exist in a build without the soma feature"
        );
    }
}

/// The health report describes the subsystems this build actually has.
///
/// Reporting `soma: {ok: false}` forever would train an operator to read a red
/// light as normal — the same habit that let seven dependency advisories
/// accumulate behind a non-blocking check.
#[tokio::test]
async fn health_does_not_report_a_soma_subsystem() {
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

    let body = body_json(resp).await;

    assert!(
        body.get("soma").is_none(),
        "health body should carry no soma block, got {body}"
    );
    assert!(
        body["checks"].get("soma").is_none(),
        "health checks should carry no soma entry, got {}",
        body["checks"]
    );
    // The rest of the report is unchanged — the fence removes a subsystem, it
    // does not reshape the contract.
    assert!(body["checks"]["database"].is_object());
    assert!(body["checks"]["scheduler"].is_object());
}

/// The `Soma ` authorization scheme is refused as unsupported.
///
/// Falling through to Clerk would answer "invalid JWT" for a credential that
/// was never a JWT, which sends whoever is debugging it to the wrong subsystem.
#[tokio::test]
async fn soma_authorization_scheme_is_refused() {
    let (app, _tmp) = test_app().await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/mc/snapshot")
                .header("authorization", "Soma some-delegation-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "a Soma-scheme credential must not authenticate a request"
    );
}
