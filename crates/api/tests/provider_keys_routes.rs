//! Route-level tests for `/api/provider-keys*`, driven through the real
//! `build_cortex_router` so auth extraction runs as in production.
//!
//! Lives in its own file because it sets process-wide env vars
//! (`CORTEX_AUTH_DISABLED`, `CORTEX_BYOK_KEK_*`): every file under `tests/`
//! runs as its own process, so these cannot leak into the library's unit
//! tests or into other integration test binaries.

use std::sync::Mutex;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use base64::{engine::general_purpose::STANDARD, Engine};
use http_body_util::BodyExt;
use tower::ServiceExt;

use cortex_api::state::AppState;

// These 4 tests run as separate `#[tokio::test]` tasks and all set or clear
// process-global `CORTEX_BYOK_KEK_*` / `CORTEX_AUTH_DISABLED` env vars —
// racing without a shared lock would be flaky. Each `#[tokio::test]` is its
// own current-thread runtime, so holding a std `Mutex` guard across `.await`
// here cannot deadlock another task's executor.
static ENV_LOCK: Mutex<()> = Mutex::new(());

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
#[allow(clippy::await_holding_lock)] // guard held across .await is fine: each test is its own current-thread runtime
async fn put_then_get_returns_only_last4() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
#[allow(clippy::await_holding_lock)] // guard held across .await is fine: each test is its own current-thread runtime
async fn put_rejects_bad_provider_and_bad_key() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
#[allow(clippy::await_holding_lock)] // guard held across .await is fine: each test is its own current-thread runtime
async fn delete_then_get_is_empty_and_repeat_delete_is_404() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
#[allow(clippy::await_holding_lock)] // guard held across .await is fine: each test is its own current-thread runtime
async fn missing_master_key_reports_feature_off_and_writes_nothing() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

/// The submitted key must never reach a log line, in any of: a successful
/// PUT, a PUT with a malformed body that still contains the key text (axum's
/// default JSON rejection can otherwise echo the raw body), and a PUT whose
/// key fails format validation (contains whitespace). Captures every
/// `tracing` event at TRACE and below into a buffer and asserts the marker
/// text is absent from all of it.
#[tokio::test]
#[allow(clippy::await_holding_lock)] // guard held across .await is fine: each test is its own current-thread runtime
async fn logs_never_contain_a_submitted_key() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_test_kek();

    let buf = std::sync::Arc::new(Mutex::new(Vec::<u8>::new()));

    #[derive(Clone)]
    struct BufWriter(std::sync::Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for BufWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let writer_buf = buf.clone();

    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || BufWriter(writer_buf.clone()))
        .finish();

    let _dispatch_guard = tracing::subscriber::set_default(subscriber);

    let app = router().await;

    // A valid key.
    let put_ok = send(
        &app,
        Method::PUT,
        "/api/provider-keys/zen",
        Some(serde_json::json!({ "api_key": "zen-test-SECRETSECRET1234" })),
    )
    .await;
    assert_eq!(put_ok.status(), StatusCode::NO_CONTENT);

    // Malformed JSON body that still contains the marker text.
    let bad_json = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/api/provider-keys/zen")
                .header("content-type", "application/json")
                .body(Body::from(
                    "{ this is not valid json but has SECRETSECRET in it",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad_json.status(), StatusCode::BAD_REQUEST);

    // A key that fails format validation (contains a space).
    let put_bad_format = send(
        &app,
        Method::PUT,
        "/api/provider-keys/zen",
        Some(serde_json::json!({ "api_key": "has a space SECRETSECRET" })),
    )
    .await;
    assert_eq!(put_bad_format.status(), StatusCode::BAD_REQUEST);

    drop(_dispatch_guard);
    let captured =
        String::from_utf8_lossy(&buf.lock().unwrap_or_else(|e| e.into_inner())).into_owned();
    // The request-id middleware logs every response, so an empty capture
    // means the subscriber saw nothing and the check below would prove nothing.
    assert!(
        captured.contains("request completed"),
        "log capture saw nothing"
    );
    assert!(
        !captured.contains("SECRETSECRET"),
        "submitted key leaked into logs ({} bytes captured)",
        captured.len()
    );

    clear_kek();
}
