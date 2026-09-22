//! Router-level tests for `POST /api/agent/actions/{id}/confirm` and
//! `.../cancel` (`crates/api/src/agent_confirm.rs`). These exercise the real
//! `axum::Router` built by `build_cortex_router`, the same router-oneshot
//! pattern as `cortex_router_serves_only_the_manifest_and_no_socials_surface`
//! in `route_ownership.rs`, so the auth extractors (`ClerkUser`,
//! `PremiumUser`) run exactly as they do in production.
//!
//! None of these tests confirm a valid row — that would attempt to run the
//! underlying tool (a real `git push` for `open_pr`), which is out of scope
//! here.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use cortex_api::state::AppState;

/// Builds a real `AppState` (own tempdir, own sqlite database) and the
/// production Cortex router. `CORTEX_AUTH_DISABLED=1` plus no
/// `clerk_secret_key` puts `ClerkUser`/`PremiumUser` in local-dev mode: every
/// request authenticates as `user_id == "local"` and `PremiumUser` allows it
/// through unconditionally, matching `AppState::new(..., None)` used by
/// `route_ownership.rs`.
async fn router_as_local_user() -> (tempfile::TempDir, axum::Router, std::sync::Arc<AppState>) {
    std::env::set_var("CORTEX_AUTH_DISABLED", "1");
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(temp.path().join(".cortex")).unwrap();
    let state = AppState::new(
        temp.path().join(".cortex/ledger.jsonl"),
        temp.path().to_path_buf(),
        None,
    )
    .await;
    let router = cortex_api::build_cortex_router(state.clone());
    (temp, router, state)
}

/// Builds the router with `clerk_secret_key` configured, so `PremiumUser`
/// takes the real premium-check branch instead of the local-dev bypass.
/// `CORTEX_AUTH_DISABLED=1` still short-circuits `ClerkUser` to
/// `user_id == "local"` with no bearer token needed, and clearing
/// `CORTEX_ADMIN_EMAILS`/`CORTEX_ADMIN_USERS` while setting the real
/// `CLERK_SECRET_KEY` env var (read by `admin::admin_set`, separately from
/// `AppState::clerk_secret_key`) makes the admin bypass fail closed without
/// any network call, leaving only the subscription check — which "local" has
/// none of.
async fn router_as_non_premium_user() -> (tempfile::TempDir, axum::Router, std::sync::Arc<AppState>)
{
    std::env::set_var("CORTEX_AUTH_DISABLED", "1");
    std::env::set_var("CLERK_SECRET_KEY", "sk_test_fake_for_router_tests");
    std::env::remove_var("CORTEX_ADMIN_EMAILS");
    std::env::remove_var("CORTEX_ADMIN_USERS");
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(temp.path().join(".cortex")).unwrap();
    let state = AppState::new(
        temp.path().join(".cortex/ledger.jsonl"),
        temp.path().to_path_buf(),
        Some("sk_test_fake_for_router_tests".to_string()),
    )
    .await;
    let router = cortex_api::build_cortex_router(state.clone());
    (temp, router, state)
}

async fn post_json(
    app: axum::Router,
    path: &str,
    body: serde_json::Value,
) -> axum::response::Response {
    app.oneshot(
        Request::builder()
            .method(Method::POST)
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("request"),
    )
    .await
    .expect("response")
}

async fn response_json(response: axum::response::Response) -> serde_json::Value {
    let body = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).expect("JSON response")
}

/// Another user's row, a missing id, and a wrong nonce are all a plain 404
/// with an identical body on confirm — none of the three lets a caller tell
/// "no such row" apart from "wrong nonce" or "not yours".
#[tokio::test]
async fn confirm_refusals_are_identical_404() {
    let (_temp, router, state) = router_as_local_user().await;
    let db = state.db.as_ref().expect("db configured");
    let now = chrono::Utc::now().timestamp();

    // Case 1: a row that belongs to another user entirely.
    let other_users_row = db.insert_pending_action(
        "someone-else",
        "conv-1",
        "open_pr",
        &serde_json::json!({"run_id": "r1"}),
        "Open a pull request for run r1",
        now,
    );

    // Case 3 setup: a row `local` really owns, but the request will send the
    // wrong nonce for it.
    let owned_row = db.insert_pending_action(
        "local",
        "conv-2",
        "open_pr",
        &serde_json::json!({"run_id": "r2"}),
        "Open a pull request for run r2",
        now,
    );

    let cases: Vec<(String, String)> = vec![
        (other_users_row.id.clone(), other_users_row.nonce.clone()),
        ("no-such-id".to_string(), "irrelevant-nonce".to_string()),
        (owned_row.id.clone(), "wrong-nonce".to_string()),
    ];

    let mut bodies = Vec::new();
    for (id, nonce) in cases {
        let path = format!("/api/agent/actions/{id}/confirm");
        let response =
            post_json(router.clone(), &path, serde_json::json!({ "nonce": nonce })).await;
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "case id={id} must read as 404"
        );
        bodies.push(response_json(response).await);
    }

    assert_eq!(
        bodies[0], bodies[1],
        "another-user and missing-id bodies must match"
    );
    assert_eq!(
        bodies[1], bodies[2],
        "missing-id and wrong-nonce bodies must match"
    );

    // Neither row was touched by these refusals.
    let reread_other = db
        .get_pending_action(&other_users_row.id, "someone-else")
        .expect("other user's row still there");
    assert_eq!(reread_other.status, "pending");
    let reread_owned = db
        .get_pending_action(&owned_row.id, "local")
        .expect("owned row still there");
    assert_eq!(reread_owned.status, "pending");
}

/// Same three refusal cases as confirm, on cancel.
#[tokio::test]
async fn cancel_refusals_are_identical_404() {
    let (_temp, router, state) = router_as_local_user().await;
    let db = state.db.as_ref().expect("db configured");
    let now = chrono::Utc::now().timestamp();

    let other_users_row = db.insert_pending_action(
        "someone-else",
        "conv-1",
        "open_pr",
        &serde_json::json!({"run_id": "r1"}),
        "Open a pull request for run r1",
        now,
    );

    let owned_row = db.insert_pending_action(
        "local",
        "conv-2",
        "open_pr",
        &serde_json::json!({"run_id": "r2"}),
        "Open a pull request for run r2",
        now,
    );

    let cases: Vec<(String, String)> = vec![
        (other_users_row.id.clone(), other_users_row.nonce.clone()),
        ("no-such-id".to_string(), "irrelevant-nonce".to_string()),
        (owned_row.id.clone(), "wrong-nonce".to_string()),
    ];

    let mut bodies = Vec::new();
    for (id, nonce) in cases {
        let path = format!("/api/agent/actions/{id}/cancel");
        let response =
            post_json(router.clone(), &path, serde_json::json!({ "nonce": nonce })).await;
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "case id={id} must read as 404"
        );
        bodies.push(response_json(response).await);
    }

    assert_eq!(
        bodies[0], bodies[1],
        "another-user and missing-id bodies must match"
    );
    assert_eq!(
        bodies[1], bodies[2],
        "missing-id and wrong-nonce bodies must match"
    );

    let reread_other = db
        .get_pending_action(&other_users_row.id, "someone-else")
        .expect("other user's row still there");
    assert_eq!(reread_other.status, "pending");
    let reread_owned = db
        .get_pending_action(&owned_row.id, "local")
        .expect("owned row still there");
    assert_eq!(reread_owned.status, "pending");
}

/// A non-premium user is refused on both routes — whatever status
/// `PremiumUser` returns for that — and the row it targeted stays pending,
/// confirming the rejection happens before either handler body runs.
#[tokio::test]
async fn non_premium_user_cannot_confirm_or_cancel() {
    let (_temp, router, state) = router_as_non_premium_user().await;
    let db = state.db.as_ref().expect("db configured");
    let now = chrono::Utc::now().timestamp();

    let row = db.insert_pending_action(
        "local",
        "conv-1",
        "open_pr",
        &serde_json::json!({"run_id": "r1"}),
        "Open a pull request for run r1",
        now,
    );

    let confirm_response = post_json(
        router.clone(),
        &format!("/api/agent/actions/{}/confirm", row.id),
        serde_json::json!({ "nonce": row.nonce }),
    )
    .await;
    assert_ne!(
        confirm_response.status(),
        StatusCode::OK,
        "a non-premium user must not be able to confirm"
    );

    let cancel_response = post_json(
        router,
        &format!("/api/agent/actions/{}/cancel", row.id),
        serde_json::json!({ "nonce": row.nonce }),
    )
    .await;
    assert_ne!(
        cancel_response.status(),
        StatusCode::OK,
        "a non-premium user must not be able to cancel"
    );

    let reread = db
        .get_pending_action(&row.id, "local")
        .expect("row is still there");
    assert_eq!(reread.status, "pending", "neither refusal ran the action");
}
