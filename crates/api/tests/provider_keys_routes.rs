//! Route-level tests for `/api/provider-keys*`, driven through the real
//! `build_cortex_router` so auth extraction runs as in production.
//!
//! Lives in its own file because it sets process-wide env vars
//! (`CORTEX_AUTH_DISABLED`, `CORTEX_BYOK_KEK_*`): every file under `tests/`
//! runs as its own process, so these cannot leak into the library's unit
//! tests or into other integration test binaries.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use base64::{engine::general_purpose::STANDARD, Engine};
use http_body_util::BodyExt;
use tower::ServiceExt;

use cortex_api::state::AppState;

fn set_test_kek() {
    std::env::set_var("CORTEX_BYOK_KEK_V1", STANDARD.encode([11u8; 32]));
    std::env::set_var("CORTEX_BYOK_KEK_CURRENT", "1");
}

fn clear_kek() {
    std::env::remove_var("CORTEX_BYOK_KEK_V1");
    std::env::remove_var("CORTEX_BYOK_KEK_CURRENT");
}

async fn router() -> axum::Router {
    std::env::set_var("CORTEX_AUTH_DISABLED", "1");
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".cortex")).unwrap();
    let state = AppState::new(
        dir.path().join(".cortex/ledger.jsonl"),
        dir.path().to_path_buf(),
        None,
    )
    .await;
    cortex_api::build_cortex_router(state)
}

async fn send(
    app: &axum::Router,
    method: Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> axum::response::Response {
    let body = match body {
        Some(v) => Body::from(v.to_string()),
        None => Body::empty(),
    };
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn json_body(response: axum::response::Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    }
}

/// Save then list: the list response carries only `last4`, never the
/// submitted key, and the key value appears nowhere in the response bytes.
#[tokio::test]
async fn put_then_get_returns_only_last4() {
    set_test_kek();
    let app = router().await;

    let put = send(
        &app,
        Method::PUT,
        "/api/provider-keys/zen",
        Some(serde_json::json!({ "api_key": "zen-test-SECRETSECRET1234" })),
    )
    .await;
    assert_eq!(put.status(), StatusCode::NO_CONTENT);

    let get = send(&app, Method::GET, "/api/provider-keys", None).await;
    assert_eq!(get.status(), StatusCode::OK);
    let bytes = get.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains("SECRETSECRET"), "full key leaked: {text}");
    assert!(text.contains("1234"), "response should show last4: {text}");

    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let rows = value.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["last4"], "1234");
    assert_eq!(rows[0]["status"], "active");
    assert!(rows[0].get("nonce").is_none());
    assert!(rows[0].get("ciphertext").is_none());
    assert!(rows[0].get("api_key").is_none());

    clear_kek();
}

/// PUT of an unsupported provider is 400. An over-long key is 400.
#[tokio::test]
async fn put_rejects_bad_provider_and_bad_key() {
    set_test_kek();
    let app = router().await;

    let bad_provider = send(
        &app,
        Method::PUT,
        "/api/provider-keys/anthropic",
        Some(serde_json::json!({ "api_key": "some-long-enough-key-value" })),
    )
    .await;
    assert_eq!(bad_provider.status(), StatusCode::BAD_REQUEST);

    let too_long = send(
        &app,
        Method::PUT,
        "/api/provider-keys/zen",
        Some(serde_json::json!({ "api_key": "a".repeat(600) })),
    )
    .await;
    assert_eq!(too_long.status(), StatusCode::BAD_REQUEST);

    clear_kek();
}

/// DELETE then GET is empty; DELETE again is 404.
#[tokio::test]
async fn delete_then_get_is_empty_and_repeat_delete_is_404() {
    set_test_kek();
    let app = router().await;

    let put = send(
        &app,
        Method::PUT,
        "/api/provider-keys/zen",
        Some(serde_json::json!({ "api_key": "zen-test-SECRETSECRET1234" })),
    )
    .await;
    assert_eq!(put.status(), StatusCode::NO_CONTENT);

    let delete = send(&app, Method::DELETE, "/api/provider-keys/zen", None).await;
    assert_eq!(delete.status(), StatusCode::NO_CONTENT);

    let get = send(&app, Method::GET, "/api/provider-keys", None).await;
    let value = json_body(get).await;
    assert_eq!(value.as_array().unwrap().len(), 0);

    let delete_again = send(&app, Method::DELETE, "/api/provider-keys/zen", None).await;
    assert_eq!(delete_again.status(), StatusCode::NOT_FOUND);

    clear_kek();
}

/// With no KEK configured, the routes report the feature off (503) and no
/// row is written — confirmed by a follow-up GET that still shows nothing
/// once a KEK is later configured for that check.
#[tokio::test]
async fn missing_master_key_reports_feature_off_and_writes_nothing() {
    clear_kek();
    let app = router().await;

    let put = send(
        &app,
        Method::PUT,
        "/api/provider-keys/zen",
        Some(serde_json::json!({ "api_key": "zen-test-SECRETSECRET1234" })),
    )
    .await;
    assert_eq!(put.status(), StatusCode::SERVICE_UNAVAILABLE);

    // GET is unauthenticated by the feature flag (it just lists whatever is
    // there, which as of D2/D4 is fine even with BYOK off) — assert the PUT
    // above wrote no row by checking under a KEK that would otherwise read it.
    set_test_kek();
    let get = send(&app, Method::GET, "/api/provider-keys", None).await;
    let value = json_body(get).await;
    assert_eq!(
        value.as_array().unwrap().len(),
        0,
        "PUT must not have written a row while BYOK was off"
    );
    clear_kek();
}
