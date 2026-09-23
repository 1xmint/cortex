//! Routes for a customer's own provider API keys ("bring your own key").
//! Write-only: nothing here ever returns more than the last 4 characters of
//! a saved key, and the request body is never logged or echoed back.
//!
//! See `crates/api/src/byok.rs` for the split-key cipher scheme, and
//! `crates/api/src/db/provider_keys.rs` for the row shape.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use once_cell::sync::Lazy;
use serde::Deserialize;

use crate::byok::{self, ByokError};
use crate::clerk::ClerkUser;
use crate::db::ProviderKeySummary;
use crate::db::MAX_DEVICES_PER_PROVIDER;
use crate::routes::{db_ref, ApiResult, ErrorResponse};
use crate::state::AppState;

/// Providers this endpoint accepts. Only `zen` today -- see plan D2/D5.
const SUPPORTED_PROVIDERS: &[&str] = &["zen"];

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn bad_provider() -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: "unsupported provider".into(),
        }),
    )
}

/// `GET /api/provider-keys`
pub async fn list_keys(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> ApiResult<Json<Vec<ProviderKeySummary>>> {
    let db = db_ref(&state)?;
    Ok(Json(db.list_provider_keys(&user.user_id)))
}

#[derive(Deserialize)]
pub struct SaveKeyRequest {
    api_key: String,
    device_id: String,
    /// Base64url (no padding) of the 32-byte AES-256-GCM secret this browser
    /// generated for this device. Never logged, never stored -- used once to
    /// encrypt `api_key`, then dropped.
    unlock: String,
}

/// `PUT /api/provider-keys/{provider}`
///
/// The body is read into a typed struct up front (`SaveKeyRequest`) rather
/// than logged or echoed anywhere: axum's default JSON rejection message can
/// include the offending body, so a malformed request here still must not
/// put the submitted key or unlock secret in a response or a log line.
/// Nothing in this handler passes `body`, `request.api_key`, or
/// `request.unlock` to `tracing`.
pub async fn save_key(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(provider): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult<StatusCode> {
    if !SUPPORTED_PROVIDERS.contains(&provider.as_str()) {
        return Err(bad_provider());
    }
    if !check_rate_limit(&user.user_id) {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(ErrorResponse {
                error: "too many key changes; try again later".into(),
            }),
        ));
    }

    // Parsed manually (not via the `Json<T>` extractor) so a malformed body
    // never risks axum's rejection message echoing the raw bytes back to the
    // caller or into a log: the fixed error text below is all that is ever
    // returned or recorded.
    let request: SaveKeyRequest = serde_json::from_slice(&body).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid request body".into(),
            }),
        )
    })?;

    byok::validate_key_format(&request.api_key).map_err(|msg| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse { error: msg.into() }),
        )
    })?;

    if !byok::valid_device_id(&request.device_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid device id".into(),
            }),
        ));
    }

    let unlock = byok::decode_unlock(&request.unlock).map_err(map_byok_error)?;

    let db = db_ref(&state)?;

    // Device cap: an existing row for this exact device is a replace, not a
    // new device, so it never counts against the cap.
    let existing = db.get_provider_key_row(&user.user_id, &provider, &request.device_id);
    if existing.is_none()
        && db.count_provider_key_devices(&user.user_id, &provider) >= MAX_DEVICES_PER_PROVIDER
    {
        return Err((
            StatusCode::CONFLICT,
            Json(ErrorResponse {
                error: format!(
                    "you already have {MAX_DEVICES_PER_PROVIDER} devices saved for this provider; remove one before adding another"
                ),
            }),
        ));
    }

    let last4 = byok::last4_of(&request.api_key);
    let encrypted = byok::encrypt(
        &user.user_id,
        &provider,
        &request.device_id,
        &unlock,
        &request.api_key,
    )
    .map_err(map_byok_error)?;

    db.upsert_provider_key(
        &user.user_id,
        &provider,
        &request.device_id,
        &encrypted,
        &last4,
        now_ms(),
    );

    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/provider-keys/{provider}/{device_id}` -- removes one device.
pub async fn delete_key_device(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path((provider, device_id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    if !SUPPORTED_PROVIDERS.contains(&provider.as_str()) {
        return Err(bad_provider());
    }
    let db = db_ref(&state)?;
    if db.delete_provider_key_device(&user.user_id, &provider, &device_id) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "no key saved for this provider on this device".into(),
            }),
        ))
    }
}

/// `DELETE /api/provider-keys/{provider}` -- removes every device's key for
/// this provider.
pub async fn delete_key_all(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(provider): Path<String>,
) -> ApiResult<StatusCode> {
    if !SUPPORTED_PROVIDERS.contains(&provider.as_str()) {
        return Err(bad_provider());
    }
    let db = db_ref(&state)?;
    if db.delete_provider_keys_all_devices(&user.user_id, &provider) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "no key saved for this provider".into(),
            }),
        ))
    }
}

fn map_byok_error(err: ByokError) -> (StatusCode, Json<ErrorResponse>) {
    match err {
        ByokError::MalformedUnlock => (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid unlock secret".into(),
            }),
        ),
        ByokError::DecryptFailed => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "failed to store key".into(),
            }),
        ),
    }
}

// ---------------------------------------------------------------------------
// Rate limit: 10 saves per user per hour, in-process.
//
// In-process only, same reasoning as `chat_paid::TURN_WINDOWS`:
// `CORTEX_SINGLE_NODE=1` is required for this server, so one process holds
// every save attempt for a given user.
// ---------------------------------------------------------------------------

static SAVE_WINDOWS: Lazy<Mutex<HashMap<String, VecDeque<i64>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

const SAVE_WINDOW_MS: i64 = 60 * 60 * 1000;
const SAVE_WINDOW_CAP: usize = 10;

fn check_rate_limit(user_id: &str) -> bool {
    let now = now_ms();
    let mut windows = SAVE_WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
    let mut window = windows.remove(user_id).unwrap_or_default();
    while window.front().is_some_and(|t| now - *t >= SAVE_WINDOW_MS) {
        window.pop_front();
    }
    let allowed = window.len() < SAVE_WINDOW_CAP;
    if allowed {
        window.push_back(now);
    }
    if !window.is_empty() {
        windows.insert(user_id.to_string(), window);
    }
    allowed
}
