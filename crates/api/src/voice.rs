//! `POST /api/voice/dictation/token` — a short-lived token the browser uses
//! to speak directly to OpenAI's realtime transcription endpoint.
//!
//! Dictation has no server-observable usage the way a chat reply does: the
//! browser holds the token and streams audio straight to OpenAI, so nothing
//! comes back through Cortex to settle against. The whole lifetime of the
//! token (120 s) is charged up front, before OpenAI is ever called, on the
//! `("openai", "gpt-4o-mini-transcribe")` price-list row. A supplier failure
//! after the charge is refunded; a client retry (same `Idempotency-Key`)
//! charges once.
//!
//! Stub/live is the same switch the provider gateway uses
//! (`CORTEX_PROVIDER_GATEWAY_MODE`, `CORTEX_OPENAI_SUPPLIER_KEY`), read
//! directly here rather than through `provider_gateway_http` because this
//! route never goes through the signed-capability gateway: there is no
//! reservation to settle, only one up-front charge.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Serialize;
use serde_json::Value;

use cortex_core::billing_binding::{ChargeKey, RefundKey};

use crate::clerk::ClerkUser;
use crate::db::Database;
use crate::routes::ErrorResponse;
use crate::state::AppState;

/// The model dictation is always billed and called against.
const DICTATION_MODEL: &str = "gpt-4o-mini-transcribe";
/// How long a minted token is good for. Fixed, not caller-supplied: this is
/// exactly what gets charged up front, so it cannot be something the client
/// picks.
const DICTATION_SECONDS: i64 = 120;
const STUB_TOKEN: &str = "STUB-EK";
const OPENAI_BASE_URL: &str = "https://api.openai.com";
/// Anthropic/OpenAI keys are far longer than this; anything shorter is a typo
/// or a placeholder, and a placeholder must not switch real spending on.
/// Mirrors `provider_gateway_http::MIN_SUPPLIER_KEY_LEN`.
const MIN_SUPPLIER_KEY_LEN: usize = 20;
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(30);

fn ceil_div(numerator: i64, denominator: i64) -> i64 {
    (numerator + denominator - 1) / denominator
}

#[derive(Debug, Serialize, PartialEq)]
pub struct DictationTokenResponse {
    pub token: String,
    pub expires_at: i64,
    pub seconds: i64,
}

/// A refusal a user should be told about in plain words, same shape as
/// `chat_paid::PaidReplyError`.
#[derive(Debug, PartialEq)]
pub(crate) enum DictationError {
    /// The route is off, misconfigured, or the model has no price.
    Unavailable,
    /// The user cannot cover the token's full up-front charge.
    NotEnoughCredits,
    /// OpenAI refused or failed to mint the token after the charge; the
    /// charge has already been refunded by the time this is returned.
    SupplierFailed,
}

impl DictationError {
    fn into_response(self) -> (StatusCode, Json<ErrorResponse>) {
        let (status, message) = match self {
            DictationError::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "Voice dictation is temporarily unavailable. Please try again in a moment.",
            ),
            DictationError::NotEnoughCredits => (
                StatusCode::PAYMENT_REQUIRED,
                "You don't have enough credits for a dictation session. Go to Settings → Billing to buy more or subscribe.",
            ),
            DictationError::SupplierFailed => (
                StatusCode::BAD_GATEWAY,
                "Dictation is temporarily unavailable. Please try again in a moment.",
            ),
        };
        (
            status,
            Json(ErrorResponse {
                error: message.into(),
            }),
        )
    }
}

/// The same two states the provider gateway can be in
/// (`provider_gateway_http::gateway_mode`), narrowed to the one supplier this
/// route ever calls.
pub(crate) enum DictationMode {
    Stub,
    Live(String),
}

fn dictation_mode() -> Option<DictationMode> {
    match std::env::var("CORTEX_PROVIDER_GATEWAY_MODE").as_deref() {
        Ok("stub") => Some(DictationMode::Stub),
        Ok("live") => std::env::var("CORTEX_OPENAI_SUPPLIER_KEY")
            .ok()
            .filter(|key| key.trim().len() >= MIN_SUPPLIER_KEY_LEN)
            .map(DictationMode::Live),
        _ => None,
    }
}

pub(crate) struct ClientSecret {
    value: String,
    expires_at: i64,
}

/// Whatever can mint an ephemeral transcription token. One implementation
/// calls OpenAI for real; tests use a fake so the billing/refund logic can be
/// proven without a network call.
pub(crate) trait ClientSecretMinter {
    fn mint(&self, supplier_key: &str)
        -> impl Future<Output = Result<ClientSecret, String>> + Send;
}

/// The one call this route ever makes upstream: mint an ephemeral token for
/// a transcription session. Base URL is injectable so tests can point it at
/// a loopback fake instead of `api.openai.com`, mirroring
/// `supplier_openai::OpenAiTransport`.
pub(crate) struct OpenAiClientSecrets {
    client: reqwest::Client,
    base_url: String,
}

impl OpenAiClientSecrets {
    fn new() -> Self {
        Self::with_base_url(OPENAI_BASE_URL)
    }

    pub(crate) fn with_base_url(base_url: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(UPSTREAM_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            client,
            base_url: base_url.into(),
        }
    }
}

impl ClientSecretMinter for OpenAiClientSecrets {
    async fn mint(&self, supplier_key: &str) -> Result<ClientSecret, String> {
        let url = format!(
            "{}/v1/realtime/client_secrets",
            self.base_url.trim_end_matches('/')
        );
        let body = serde_json::json!({
            "expires_after": {"anchor": "created_at", "seconds": DICTATION_SECONDS},
            "session": {
                "type": "transcription",
                "audio": {"input": {"transcription": {"model": DICTATION_MODEL}}}
            }
        });

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {supplier_key}"))
            .json(&body)
            .send()
            .await
            .map_err(|error| format!("openai client_secrets request failed: {error}"))?;

        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|error| format!("openai client_secrets response was cut off: {error}"))?;

        if !status.is_success() {
            return Err(format!(
                "openai returned {status}: {}",
                error_message(&text)
            ));
        }

        let parsed: Value = serde_json::from_str(&text)
            .map_err(|_| "openai returned a body that is not JSON".to_string())?;
        let value = parsed
            .get("value")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "openai response is missing value".to_string())?;
        let expires_at = parsed
            .get("expires_at")
            .and_then(Value::as_i64)
            .ok_or_else(|| "openai response is missing expires_at".to_string())?;

        Ok(ClientSecret { value, expires_at })
    }
}

fn error_message(text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|body| {
            body.pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| text.chars().take(300).collect())
}

/// Charge, mint, and (on supplier failure) refund — independent of the HTTP
/// plumbing so it can be tested directly against a fake minter and an
/// in-memory database, the same shape as `chat_paid::send_paid_reply`.
///
/// `token_id` is the caller's idempotency label (the client's
/// `Idempotency-Key` header, or a fresh uuid); calling this twice with the
/// same id charges at most once.
pub(crate) async fn mint_dictation_token<M: ClientSecretMinter>(
    db: &Database,
    user_id: &str,
    mode: DictationMode,
    minter: &M,
    token_id: &str,
) -> Result<DictationTokenResponse, DictationError> {
    let price_list = db.active_price_list().ok_or(DictationError::Unavailable)?;
    let rate = price_list.model("openai", DICTATION_MODEL).ok_or_else(|| {
        tracing::error!("dictation: no price for gpt-4o-mini-transcribe; refusing");
        DictationError::Unavailable
    })?;

    let cost_micros = rate.cost_micros(DICTATION_SECONDS, 0, 0);
    let credits = ceil_div(cost_micros, price_list.micros_per_credit);
    let charge_key = ChargeKey::per_unit(format!("voice-dictation:{token_id}"));

    if let Err(detail) = db.deduct_credits(
        user_id,
        credits,
        "Cortex voice dictation token",
        &charge_key,
    ) {
        tracing::info!(user_id, %detail, "dictation: credit deduction refused");
        return Err(DictationError::NotEnoughCredits);
    }

    match mode {
        DictationMode::Stub => Ok(DictationTokenResponse {
            token: STUB_TOKEN.into(),
            expires_at: chrono::Utc::now().timestamp() + DICTATION_SECONDS,
            seconds: DICTATION_SECONDS,
        }),
        DictationMode::Live(supplier_key) => match minter.mint(&supplier_key).await {
            Ok(secret) => Ok(DictationTokenResponse {
                token: secret.value,
                expires_at: secret.expires_at,
                seconds: DICTATION_SECONDS,
            }),
            Err(detail) => {
                tracing::error!(
                    user_id,
                    %detail,
                    "dictation: openai client_secrets call failed; refunding"
                );
                let refund_key = RefundKey::per_unit(format!("voice-dictation-refund:{token_id}"));
                if let Err(e) = db.refund_credits(
                    user_id,
                    &charge_key,
                    &refund_key,
                    "Cortex voice dictation token refund",
                ) {
                    tracing::error!(
                        user_id,
                        error = %e,
                        "dictation: refund after supplier failure also failed"
                    );
                }
                Err(DictationError::SupplierFailed)
            }
        },
    }
}

pub async fn dictation_token(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    headers: HeaderMap,
) -> Result<Json<DictationTokenResponse>, (StatusCode, Json<ErrorResponse>)> {
    if let Some(blocked) = crate::billing::check_chat_access(&state, &user.user_id) {
        return Err((
            StatusCode::PAYMENT_REQUIRED,
            Json(ErrorResponse {
                error: format!("Subscription required to access chat. Status: {blocked:?}. Go to Settings → Billing to subscribe."),
            }),
        ));
    }

    let Some(db) = state.db.as_ref() else {
        return Err(DictationError::Unavailable.into_response());
    };
    let Some(mode) = dictation_mode() else {
        return Err(DictationError::Unavailable.into_response());
    };

    // A client's own `Idempotency-Key` makes a retried request charge once;
    // otherwise every mint is its own charge.
    let token_id = headers
        .get("Idempotency-Key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    mint_dictation_token(
        db,
        &user.user_id,
        mode,
        &OpenAiClientSecrets::new(),
        &token_id,
    )
    .await
    .map(Json)
    .map_err(DictationError::into_response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap as AxumHeaderMap, StatusCode as AxumStatusCode};
    use axum::routing::post;
    use axum::Json as AxumJson;
    use std::sync::{Arc as StdArc, Mutex};

    const USER: &str = "user-1";

    fn test_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("voice.sqlite"));
        db.init_credit_balance(USER, 1_000_000).unwrap();
        (dir, db)
    }

    /// A minter that must never be called: stub mode makes no network call.
    struct UnreachableMinter;
    impl ClientSecretMinter for UnreachableMinter {
        async fn mint(&self, _supplier_key: &str) -> Result<ClientSecret, String> {
            panic!("stub mode must never call the minter")
        }
    }

    fn dictation_price_micros(db: &Database) -> (i64, i64) {
        let price_list = db.active_price_list().unwrap();
        let rate = price_list.model("openai", DICTATION_MODEL).unwrap();
        let micros = rate.cost_micros(DICTATION_SECONDS, 0, 0);
        (micros, price_list.micros_per_credit)
    }

    #[test]
    fn dictation_price_is_6000_micros_for_120_seconds() {
        let (_dir, db) = test_db();
        let (micros, _) = dictation_price_micros(&db);
        assert_eq!(micros, 6_000);
    }

    #[tokio::test]
    async fn stub_returns_200_and_charges_exactly_one_ledger_row() {
        let (_dir, db) = test_db();
        let (micros, micros_per_credit) = dictation_price_micros(&db);
        let expected_credits = ceil_div(micros, micros_per_credit);

        let response = mint_dictation_token(
            &db,
            USER,
            DictationMode::Stub,
            &UnreachableMinter,
            "token-1",
        )
        .await
        .expect("stub mint should succeed");

        assert_eq!(response.token, STUB_TOKEN);
        assert_eq!(response.seconds, DICTATION_SECONDS);

        let balance = db.get_credit_balance_row(USER).unwrap();
        assert_eq!(balance.subscription_remaining, 1_000_000 - expected_credits);
    }

    #[tokio::test]
    async fn the_same_idempotency_key_charges_once() {
        let (_dir, db) = test_db();
        let (micros, micros_per_credit) = dictation_price_micros(&db);
        let expected_credits = ceil_div(micros, micros_per_credit);

        for _ in 0..2 {
            mint_dictation_token(
                &db,
                USER,
                DictationMode::Stub,
                &UnreachableMinter,
                "same-key",
            )
            .await
            .expect("a replayed idempotency key must still succeed");
        }

        let balance = db.get_credit_balance_row(USER).unwrap();
        assert_eq!(
            balance.subscription_remaining,
            1_000_000 - expected_credits,
            "the replayed idempotency key must not charge twice"
        );
    }

    #[tokio::test]
    async fn zero_balance_is_refused_and_the_supplier_is_never_called() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("voice-empty.sqlite"));
        db.init_credit_balance(USER, 0).unwrap();

        // `UnreachableMinter` panics if called at all, which is the proof
        // that a zero balance never reaches the supplier — whether the mode
        // is stub (which never calls it anyway) or live.
        let error = mint_dictation_token(
            &db,
            USER,
            DictationMode::Live("sk-test-should-not-be-used".into()),
            &UnreachableMinter,
            "no-balance",
        )
        .await
        .expect_err("zero balance must be refused");

        assert_eq!(error, DictationError::NotEnoughCredits);
        let balance = db.get_credit_balance_row(USER).unwrap();
        assert_eq!(balance.subscription_remaining, 0, "nothing was charged");

        let (status, Json(body)) = error.into_response();
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
        assert!(body.error.contains("credits"));
    }

    #[test]
    fn serialized_stub_response_never_contains_a_key() {
        let response = DictationTokenResponse {
            token: STUB_TOKEN.into(),
            expires_at: 1_800_000_000,
            seconds: DICTATION_SECONDS,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(!json.contains("sk-"));
        assert!(json.contains("STUB-EK"));
    }

    type Seen = StdArc<Mutex<Vec<(AxumHeaderMap, Value)>>>;

    /// A stand-in `client_secrets` endpoint on a local port.
    async fn fake_client_secrets(status: AxumStatusCode, reply: Value) -> (String, Seen) {
        let seen: Seen = StdArc::default();
        let log = seen.clone();
        let app = axum::Router::new().route(
            "/v1/realtime/client_secrets",
            post(
                move |headers: AxumHeaderMap, AxumJson(body): AxumJson<Value>| {
                    let log = log.clone();
                    let reply = reply.clone();
                    async move {
                        log.lock().unwrap().push((headers, body));
                        (status, AxumJson(reply))
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}"), seen)
    }

    #[tokio::test]
    async fn the_live_path_against_a_loopback_fake_returns_the_fakes_value() {
        let (_dir, db) = test_db();
        let (url, seen) = fake_client_secrets(
            AxumStatusCode::OK,
            serde_json::json!({"value": "ek_fake_123", "expires_at": 1_800_000_120}),
        )
        .await;
        let minter = OpenAiClientSecrets::with_base_url(url);

        let response = mint_dictation_token(
            &db,
            USER,
            DictationMode::Live("sk-test-supplier".into()),
            &minter,
            "live-1",
        )
        .await
        .expect("live mint should succeed");

        assert_eq!(response.token, "ek_fake_123");
        assert_eq!(response.expires_at, 1_800_000_120);

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "exactly one request reached the fake");
        let (headers, body) = &seen[0];
        assert_eq!(headers["authorization"], "Bearer sk-test-supplier");
        assert_eq!(body["session"]["type"], "transcription");
        assert_eq!(
            body["session"]["audio"]["input"]["transcription"]["model"],
            DICTATION_MODEL
        );
        assert_eq!(body["expires_after"]["seconds"], DICTATION_SECONDS);
    }

    #[tokio::test]
    async fn a_supplier_failure_refunds_the_charge_and_never_leaks_the_key() {
        let (_dir, db) = test_db();
        let (url, _seen) = fake_client_secrets(
            AxumStatusCode::UNAUTHORIZED,
            serde_json::json!({"error": {"message": "invalid api key sk-test-supplier"}}),
        )
        .await;
        let minter = OpenAiClientSecrets::with_base_url(url);

        let error = mint_dictation_token(
            &db,
            USER,
            DictationMode::Live("sk-test-supplier".into()),
            &minter,
            "live-fail",
        )
        .await
        .expect_err("a supplier rejection must surface as an error");

        assert_eq!(error, DictationError::SupplierFailed);
        let balance = db.get_credit_balance_row(USER).unwrap();
        assert_eq!(
            balance.subscription_remaining, 1_000_000,
            "the charge must be refunded after a supplier failure"
        );

        let (status, Json(body)) = error.into_response();
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert!(!body.error.contains("sk-test-supplier"));
    }
}
