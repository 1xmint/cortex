//! Route-level tests for `/api/provider-keys*`, driven through the real
//! `build_cortex_router` so auth extraction runs as in production.
//!
//! Lives in its own file because it sets the process-wide
//! `CORTEX_AUTH_DISABLED` env var: every file under `tests/` runs as its own
//! process, so this cannot leak into the library's unit tests or into other
//! integration test binaries.

use std::sync::Mutex;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use http_body_util::BodyExt;
use tower::ServiceExt;

use cortex_api::state::AppState;

fn test_unlock() -> String {
    URL_SAFE_NO_PAD.encode([7u8; 32])
}

/// Every case runs as the same test user, and the "10 key saves per hour"
/// limit is process-wide, so cases take this lock and start from a clean
/// counter instead of spending each other's saves.
static SERIAL: once_cell::sync::Lazy<tokio::sync::Mutex<()>> =
    once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(()));

async fn router() -> (tokio::sync::MutexGuard<'static, ()>, axum::Router) {
    let serial = SERIAL.lock().await;
    cortex_api::reset_provider_key_save_limits();
    std::env::set_var("CORTEX_AUTH_DISABLED", "1");
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".cortex")).unwrap();
    let state = AppState::new(
        dir.path().join(".cortex/ledger.jsonl"),
        dir.path().to_path_buf(),
        None,
    )
    .await;
    (serial, cortex_api::build_cortex_router(state))
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

/// Save then list: the list response carries only `last4` and `device_id`,
/// never the submitted key or the unlock secret.
#[tokio::test]
async fn put_then_get_returns_only_last4() {
    let (_serial, app) = router().await;

    let put = send(
        &app,
        Method::PUT,
        "/api/provider-keys/zen",
        Some(serde_json::json!({
            "api_key": "zen-test-SECRETSECRET1234",
            "device_id": "device-1",
            "unlock": test_unlock(),
        })),
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
    assert_eq!(rows[0]["device_id"], "device-1");
    assert!(rows[0].get("nonce").is_none());
    assert!(rows[0].get("ciphertext").is_none());
    assert!(rows[0].get("api_key").is_none());
    assert!(rows[0].get("unlock").is_none());
}

/// PUT of an unsupported provider is 400. An over-long key is 400. A bad
/// device id is 400. An unlock that isn't 32 bytes is 400.
#[tokio::test]
async fn put_rejects_bad_provider_and_bad_key_and_bad_device_and_bad_unlock() {
    let (_serial, app) = router().await;

    let bad_provider = send(
        &app,
        Method::PUT,
        "/api/provider-keys/anthropic",
        Some(serde_json::json!({
            "api_key": "some-long-enough-key-value",
            "device_id": "device-1",
            "unlock": test_unlock(),
        })),
    )
    .await;
    assert_eq!(bad_provider.status(), StatusCode::BAD_REQUEST);

    let too_long = send(
        &app,
        Method::PUT,
        "/api/provider-keys/zen",
        Some(serde_json::json!({
            "api_key": "a".repeat(600),
            "device_id": "device-1",
            "unlock": test_unlock(),
        })),
    )
    .await;
    assert_eq!(too_long.status(), StatusCode::BAD_REQUEST);

    let bad_device = send(
        &app,
        Method::PUT,
        "/api/provider-keys/zen",
        Some(serde_json::json!({
            "api_key": "zen-test-SECRETSECRET1234",
            "device_id": "not a valid device id!!",
            "unlock": test_unlock(),
        })),
    )
    .await;
    assert_eq!(bad_device.status(), StatusCode::BAD_REQUEST);

    let bad_unlock = send(
        &app,
        Method::PUT,
        "/api/provider-keys/zen",
        Some(serde_json::json!({
            "api_key": "zen-test-SECRETSECRET1234",
            "device_id": "device-1",
            "unlock": URL_SAFE_NO_PAD.encode([7u8; 16]),
        })),
    )
    .await;
    assert_eq!(bad_unlock.status(), StatusCode::BAD_REQUEST);

    let get = send(&app, Method::GET, "/api/provider-keys", None).await;
    let value = json_body(get).await;
    assert_eq!(
        value.as_array().unwrap().len(),
        0,
        "no invalid request should have written a row"
    );
}

/// DELETE one device then GET is empty; DELETE again is 404.
#[tokio::test]
async fn delete_one_device_then_get_is_empty_and_repeat_delete_is_404() {
    let (_serial, app) = router().await;

    let put = send(
        &app,
        Method::PUT,
        "/api/provider-keys/zen",
        Some(serde_json::json!({
            "api_key": "zen-test-SECRETSECRET1234",
            "device_id": "device-1",
            "unlock": test_unlock(),
        })),
    )
    .await;
    assert_eq!(put.status(), StatusCode::NO_CONTENT);

    let delete = send(
        &app,
        Method::DELETE,
        "/api/provider-keys/zen/device-1",
        None,
    )
    .await;
    assert_eq!(delete.status(), StatusCode::NO_CONTENT);

    let get = send(&app, Method::GET, "/api/provider-keys", None).await;
    let value = json_body(get).await;
    assert_eq!(value.as_array().unwrap().len(), 0);

    let delete_again = send(
        &app,
        Method::DELETE,
        "/api/provider-keys/zen/device-1",
        None,
    )
    .await;
    assert_eq!(delete_again.status(), StatusCode::NOT_FOUND);
}

/// DELETE without a device id removes every device's row for the provider.
#[tokio::test]
async fn delete_all_devices_removes_every_row() {
    let (_serial, app) = router().await;

    for device in ["device-1", "device-2"] {
        let put = send(
            &app,
            Method::PUT,
            "/api/provider-keys/zen",
            Some(serde_json::json!({
                "api_key": "zen-test-SECRETSECRET1234",
                "device_id": device,
                "unlock": test_unlock(),
            })),
        )
        .await;
        assert_eq!(put.status(), StatusCode::NO_CONTENT);
    }

    let get = send(&app, Method::GET, "/api/provider-keys", None).await;
    let value = json_body(get).await;
    assert_eq!(value.as_array().unwrap().len(), 2);

    let delete_all = send(&app, Method::DELETE, "/api/provider-keys/zen", None).await;
    assert_eq!(delete_all.status(), StatusCode::NO_CONTENT);

    let get = send(&app, Method::GET, "/api/provider-keys", None).await;
    let value = json_body(get).await;
    assert_eq!(value.as_array().unwrap().len(), 0);

    let delete_again = send(&app, Method::DELETE, "/api/provider-keys/zen", None).await;
    assert_eq!(delete_again.status(), StatusCode::NOT_FOUND);
}

/// An 11th device for the same (user, provider) is refused with 409, and
/// writes no row; the 10 existing rows are untouched.
#[tokio::test]
async fn an_eleventh_device_is_refused_with_409() {
    let (_serial, app) = router().await;

    for i in 0..10 {
        let put = send(
            &app,
            Method::PUT,
            "/api/provider-keys/zen",
            Some(serde_json::json!({
                "api_key": "zen-test-SECRETSECRET1234",
                "device_id": format!("device-{i}"),
                "unlock": test_unlock(),
            })),
        )
        .await;
        assert_eq!(put.status(), StatusCode::NO_CONTENT);
    }

    // Ten saves used the hourly allowance; clear it so the 11th save is
    // judged by the device cap alone.
    cortex_api::reset_provider_key_save_limits();
    let eleventh = send(
        &app,
        Method::PUT,
        "/api/provider-keys/zen",
        Some(serde_json::json!({
            "api_key": "zen-test-SECRETSECRET1234",
            "device_id": "device-10",
            "unlock": test_unlock(),
        })),
    )
    .await;
    assert_eq!(eleventh.status(), StatusCode::CONFLICT);

    let get = send(&app, Method::GET, "/api/provider-keys", None).await;
    let value = json_body(get).await;
    assert_eq!(value.as_array().unwrap().len(), 10);

    // Replacing an existing device's key never counts against the cap.
    cortex_api::reset_provider_key_save_limits();
    let replace = send(
        &app,
        Method::PUT,
        "/api/provider-keys/zen",
        Some(serde_json::json!({
            "api_key": "zen-test-SECRETSECRET5678",
            "device_id": "device-0",
            "unlock": test_unlock(),
        })),
    )
    .await;
    assert_eq!(replace.status(), StatusCode::NO_CONTENT);
}

/// The submitted key and unlock secret must never reach a log line, in any
/// of: a successful PUT, a PUT with a malformed body that still contains
/// the key text (axum's default JSON rejection can otherwise echo the raw
/// body), and a PUT whose key fails format validation (contains
/// whitespace). Captures every `tracing` event at TRACE and below into a
/// buffer and asserts the marker text is absent from all of it.
#[tokio::test]
async fn logs_never_contain_a_submitted_key_or_unlock() {
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

    let (_serial, app) = router().await;
    let unlock = test_unlock();

    // A valid key.
    let put_ok = send(
        &app,
        Method::PUT,
        "/api/provider-keys/zen",
        Some(serde_json::json!({
            "api_key": "zen-test-SECRETSECRET1234",
            "device_id": "device-1",
            "unlock": unlock,
        })),
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
                .body(Body::from(format!(
                    "{{ this is not valid json but has SECRETSECRET and {unlock} in it"
                )))
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
        Some(serde_json::json!({
            "api_key": "has a space SECRETSECRET",
            "device_id": "device-1",
            "unlock": unlock,
        })),
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
    assert!(
        !captured.contains(&unlock),
        "unlock secret leaked into logs ({} bytes captured)",
        captured.len()
    );
}

/// User B's requests can never read or delete user A's key. `CORTEX_AUTH_
/// DISABLED` makes every request authenticate as the same fixed user in
/// this harness, so cross-user isolation is exercised at the DB layer's own
/// unit tests (`db/provider_keys.rs`) instead of here.
#[tokio::test]
async fn a_saved_key_is_scoped_to_this_process_test_user_only() {
    let (_serial, app) = router().await;
    let put = send(
        &app,
        Method::PUT,
        "/api/provider-keys/zen",
        Some(serde_json::json!({
            "api_key": "zen-test-SECRETSECRET1234",
            "device_id": "device-1",
            "unlock": test_unlock(),
        })),
    )
    .await;
    assert_eq!(put.status(), StatusCode::NO_CONTENT);

    let get = send(&app, Method::GET, "/api/provider-keys", None).await;
    let value = json_body(get).await;
    assert_eq!(value.as_array().unwrap().len(), 1);
}
