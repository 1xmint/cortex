//! Admin-only provider-hold routes, driven through the real
//! `build_cortex_router` so the auth and admin middleware run as in
//! production.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use cortex_api::state::AppState;

/// A non-admin user must never pass `authorize_admin` once the admin
/// list is empty and a `clerk_secret_key` is configured (fail-closed,
/// see `authorize_admin`'s doc comment). Drives a real request through
/// `build_cortex_router` so the `ClerkUser`/admin middleware run exactly
/// as in production, and builds the admin list the way
/// `tests/agent_confirm_routes.rs`'s `router_as_non_premium_user` does.
/// It lives in its own file because it sets process-wide env vars: every
/// file under `tests/` runs as its own process, so they cannot leak into
/// the library's unit tests.
#[tokio::test]
async fn non_admin_request_through_the_router_is_403() {
    std::env::set_var("CORTEX_AUTH_DISABLED", "1");
    std::env::remove_var("CORTEX_ADMIN_EMAILS");
    std::env::remove_var("CORTEX_ADMIN_USERS");
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".cortex")).unwrap();
    let state = AppState::new(
        dir.path().join(".cortex/ledger.jsonl"),
        dir.path().to_path_buf(),
        Some("sk_test_fake_for_router_tests".to_string()),
    )
    .await;
    let router = cortex_api::build_cortex_router(state.clone());

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/admin/provider-holds/chat:a/release")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "reason": "operator review" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    // Drain the body so the assertion above is the only thing that can fail.
    let _ = response.into_body().collect().await;
}
