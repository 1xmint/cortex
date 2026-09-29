//! `POST /api/voice/dictation` — speech-to-text for the chat composer's mic,
//! brokered through Cortex and metered.
//!
//! The browser records audio and uploads it here; Cortex sends it to OpenAI's
//! transcription endpoint with its own supplier key and returns the text. The
//! browser never sees a provider key or token.
//!
//! Billing is pass-through, like every other Cortex metered feature: the
//! customer pays exactly what the call cost, measured from the usage OpenAI
//! reports on the response, deducted from credits through the same
//! `charge_observed_cost` path live voice uses (whole credits off the balance,
//! the sub-credit remainder kept as a per-account carry). There is no markup,
//! no hold and no invented price.
//!
//! - Balance at or below zero (whole credits less the carry already owed):
//!   402 `{need_credits: 1, available_credits: n}`, and OpenAI is not called.
//! - OpenAI answers 5xx or 429, times out, or cannot be reached: 503 "try
//!   again later", and nothing is charged.
//! - OpenAI rejects the recording itself (4xx): 422, nothing charged (OpenAI
//!   does not bill a request it refused).
//! - Success: the observed cost is charged once under `dictation:{request_id}`.
//!   A second request carrying the same id is refused with 409 before OpenAI
//!   is called, so a replay is neither double-charged nor an unmetered call.
//! - A failure of Cortex's own machinery after OpenAI answered (the ledger
//!   write fails) is absorbed: the customer still gets their text.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Serialize;
use serde_json::{json, Value};

use crate::clerk::ClerkUser;
use crate::state::AppState;
use crate::voice_session::{charge_observed_cost, live_voice_mode, payable_micro, LiveVoiceMode};

const DICTATION_PROVIDER: &str = "openai";
/// The transcription model. Its price row lives in `pricing::seed_models`.
const DICTATION_MODEL: &str = "gpt-4o-mini-transcribe";
const OPENAI_HTTP_BASE: &str = "https://api.openai.com";
/// The largest recording accepted. The router's body limit is 12 MB; this sits
/// under it so the refusal is ours (a JSON 413), and two minutes of browser
/// audio is a small fraction of it.
pub(crate) const MAX_AUDIO_BYTES: usize = 10 * 1024 * 1024;
/// The longest recording accepted, as declared by the browser.
pub(crate) const MAX_AUDIO_SECONDS: i64 = 120;
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(60);
const REQUEST_ID_HEADER: &str = "idempotency-key";
const DURATION_HEADER: &str = "x-audio-duration-ms";

#[derive(Debug, Serialize, PartialEq)]
pub struct DictationResponse {
    pub text: String,
}

type Reject = (StatusCode, Json<Value>);

fn reject(status: StatusCode, message: &str) -> Reject {
    (status, Json(json!({ "error": message })))
}

fn try_again_later() -> Reject {
    reject(
        StatusCode::SERVICE_UNAVAILABLE,
        "Dictation unavailable, try again later",
    )
}

/// Request ids in flight right now, so two simultaneous requests carrying the
/// same id cannot both reach OpenAI before either has charged.
fn in_flight() -> &'static Mutex<HashSet<String>> {
    static SET: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

struct InFlight(String);

impl InFlight {
    fn claim(key: &str) -> Option<Self> {
        let mut set = in_flight().lock().unwrap_or_else(|e| e.into_inner());
        set.insert(key.to_string()).then(|| Self(key.to_string()))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        in_flight()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// What OpenAI reported it processed.
#[derive(Debug, PartialEq)]
enum ObservedUsage {
    Tokens { input: i64, output: i64 },
    Seconds(f64),
    Absent,
}

fn parse_usage(response: &Value) -> ObservedUsage {
    let Some(usage) = response.get("usage") else {
        return ObservedUsage::Absent;
    };
    if let Some(input) = usage.get("input_tokens").and_then(Value::as_i64) {
        let output = usage
            .get("output_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        return ObservedUsage::Tokens {
            input: input.max(0),
            output: output.max(0),
        };
    }
    if let Some(seconds) = usage.get("seconds").and_then(Value::as_f64) {
        if seconds.is_finite() && seconds >= 0.0 {
            return ObservedUsage::Seconds(seconds);
        }
    }
    ObservedUsage::Absent
}

/// Micro-USD cost of one transcription. Observed tokens are priced at the
/// model's per-token rates; a duration-billed response (or one with no usage
/// at all) is priced from the audio length at OpenAI's published per-minute
/// estimate.
fn observed_cost_micros(
    rate: &crate::pricing::ModelPrice,
    usage: &ObservedUsage,
    declared_ms: i64,
) -> i64 {
    match usage {
        ObservedUsage::Tokens { input, output } => rate.cost_micros(*input, 0, *output),
        ObservedUsage::Seconds(seconds) => {
            crate::pricing::transcribe_duration_cost_micros((seconds * 1000.0) as i64)
        }
        ObservedUsage::Absent => crate::pricing::transcribe_duration_cost_micros(declared_ms),
    }
}

enum ProviderFailure {
    /// 5xx, 429, timeout, unreachable, or a body that is not a transcription.
    Unavailable(String),
    /// OpenAI refused this recording (other 4xx).
    Rejected(String),
}

struct Transcription {
    text: String,
    usage: ObservedUsage,
}

/// The one call this route makes: `POST /v1/audio/transcriptions`. The base
/// URL is injectable so tests can point it at a loopback fake.
struct OpenAiTranscriber {
    client: reqwest::Client,
    base_url: String,
}

impl OpenAiTranscriber {
    fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(PROVIDER_TIMEOUT)
                .build()
                .unwrap_or_default(),
            base_url: base_url.into(),
        }
    }

    async fn transcribe(
        &self,
        supplier_key: &str,
        audio: &[u8],
        extension: &str,
        mime: &str,
    ) -> Result<Transcription, ProviderFailure> {
        let boundary = format!("cortex-{}", uuid::Uuid::new_v4().simple());
        let body = multipart_body(&boundary, extension, mime, audio);
        let url = format!(
            "{}/v1/audio/transcriptions",
            self.base_url.trim_end_matches('/')
        );
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {supplier_key}"))
            .header(
                "Content-Type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(body)
            .send()
            .await
            .map_err(|error| {
                ProviderFailure::Unavailable(format!("transcription request failed: {error}"))
            })?;
        let status = response.status();
        let text = response.text().await.map_err(|error| {
            ProviderFailure::Unavailable(format!("transcription response was cut off: {error}"))
        })?;
        // 401/403 mean Cortex's own key is bad, not the customer's recording.
        if status.is_server_error() || matches!(status.as_u16(), 401 | 403 | 429) {
            return Err(ProviderFailure::Unavailable(format!(
                "openai returned {status}"
            )));
        }
        if !status.is_success() {
            return Err(ProviderFailure::Rejected(format!(
                "openai returned {status}"
            )));
        }
        let parsed: Value = serde_json::from_str(&text).map_err(|_| {
            ProviderFailure::Unavailable("openai returned a body that is not JSON".into())
        })?;
        let transcript = parsed
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                ProviderFailure::Unavailable("openai response is missing text".into())
            })?;
        Ok(Transcription {
            text: transcript,
            usage: parse_usage(&parsed),
        })
    }
}

/// A `multipart/form-data` body with the three fields the endpoint needs.
/// Built by hand: it is a fixed shape, and the `multipart` feature of
/// `reqwest` is not otherwise compiled in.
fn multipart_body(boundary: &str, extension: &str, mime: &str, audio: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(audio.len() + 512);
    let mut text_field = |name: &str, value: &str| {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    };
    text_field("model", DICTATION_MODEL);
    text_field("response_format", "json");
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"dictation.{extension}\"\r\nContent-Type: {mime}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(audio);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

/// The upload's content type, reduced to the file extension OpenAI sniffs the
/// format from. `None` for anything that is not audio we know how to name.
fn audio_kind(headers: &HeaderMap) -> Option<(&'static str, String)> {
    let raw = headers.get("content-type")?.to_str().ok()?;
    let mime = raw.split(';').next()?.trim().to_ascii_lowercase();
    let extension = match mime.as_str() {
        "audio/webm" | "video/webm" => "webm",
        "audio/ogg" => "ogg",
        "audio/mp4" | "audio/x-m4a" | "audio/m4a" | "audio/aac" => "m4a",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/mpeg" | "audio/mp3" => "mp3",
        _ => return None,
    };
    Some((extension, mime))
}

fn request_id(headers: &HeaderMap) -> Option<String> {
    let id = headers.get(REQUEST_ID_HEADER)?.to_str().ok()?.trim();
    let valid = (8..=128).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    valid.then(|| id.to_string())
}

fn declared_duration_ms(headers: &HeaderMap) -> Option<i64> {
    headers
        .get(DURATION_HEADER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|ms| *ms > 0)
}

pub async fn dictation(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<DictationResponse>, Reject> {
    if let Some(blocked) = crate::billing::check_chat_access(&state, &user.user_id) {
        return Err(reject(
            StatusCode::PAYMENT_REQUIRED,
            &format!(
                "Subscription required to access chat. Status: {blocked:?}. Go to Settings → Billing to subscribe."
            ),
        ));
    }
    let Some(mode) = live_voice_mode() else {
        return Err(try_again_later());
    };
    dictation_with(
        &state,
        mode,
        OPENAI_HTTP_BASE,
        &user.user_id,
        &headers,
        body,
    )
    .await
}

/// The env-independent core of [`dictation`]: validates the upload, checks
/// the balance, calls OpenAI, and charges what it reports. Split out so tests
/// can point it at a loopback fake without touching process-wide env vars.
pub(crate) async fn dictation_with(
    state: &Arc<AppState>,
    mode: LiveVoiceMode,
    http_base: &str,
    user_id: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Json<DictationResponse>, Reject> {
    if body.len() > MAX_AUDIO_BYTES {
        return Err(reject(
            StatusCode::PAYLOAD_TOO_LARGE,
            "That recording is too large. Keep dictation under two minutes.",
        ));
    }
    let Some((extension, mime)) = audio_kind(headers) else {
        return Err(reject(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Unsupported audio format.",
        ));
    };
    let Some(request_id) = request_id(headers) else {
        return Err(reject(
            StatusCode::BAD_REQUEST,
            "Missing or invalid Idempotency-Key.",
        ));
    };
    let Some(declared_ms) = declared_duration_ms(headers) else {
        return Err(reject(
            StatusCode::BAD_REQUEST,
            "Missing or invalid X-Audio-Duration-Ms.",
        ));
    };
    if declared_ms > MAX_AUDIO_SECONDS * 1000 {
        return Err(reject(
            StatusCode::PAYLOAD_TOO_LARGE,
            "That recording is too long. Keep dictation under two minutes.",
        ));
    }
    if body.is_empty() {
        return Err(reject(StatusCode::BAD_REQUEST, "The recording is empty."));
    }

    let supplier_key = match mode {
        // Stub never leaves the machine and never spends, so it never charges.
        LiveVoiceMode::Stub => {
            return Ok(Json(DictationResponse {
                text: "Stub dictation.".into(),
            }));
        }
        LiveVoiceMode::Live(key) => key,
    };

    let db = state.db.as_ref().ok_or_else(try_again_later)?;
    let price_list = db.active_price_list().ok_or_else(try_again_later)?;
    let rate = price_list
        .model(DICTATION_PROVIDER, DICTATION_MODEL)
        .cloned()
        .ok_or_else(|| {
            tracing::error!("dictation: no price for {DICTATION_MODEL}; refusing");
            try_again_later()
        })?;
    let micros_per_credit = price_list.micros_per_credit;

    if payable_micro(db, user_id, micros_per_credit) <= 0 {
        let available = db
            .get_credit_balance_row(user_id)
            .map(|balance| (balance.subscription_remaining + balance.pack_remaining).max(0))
            .unwrap_or(0);
        return Err((
            StatusCode::PAYMENT_REQUIRED,
            Json(json!({
                "error": "Out of credits. Top up to keep dictating.",
                "need_credits": 1,
                "available_credits": available,
            })),
        ));
    }

    let charge_label = format!("dictation:{request_id}");
    // Claim first, then read the ledger: a concurrent request with the same
    // id releases its claim only after its charge commits, so this read
    // sees that charge and the replay is refused instead of served free.
    let Some(claim) = InFlight::claim(&charge_label) else {
        return Err(reject(
            StatusCode::CONFLICT,
            "That recording was already transcribed.",
        ));
    };
    let already_charged: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM credit_transactions WHERE idempotency_key = ?1",
            [&charge_label],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if already_charged > 0 {
        return Err(reject(
            StatusCode::CONFLICT,
            "That recording was already transcribed.",
        ));
    }

    // Run the call and the charge on their own task: a client that
    // disconnects while OpenAI is answering must not leave a completed,
    // uncharged transcription behind.
    let task_state = state.clone();
    let task_user = user_id.to_string();
    let task_base = http_base.to_string();
    let task = tokio::spawn(async move {
        let _claim = claim;
        let transcriber = OpenAiTranscriber::with_base_url(task_base);
        let transcription = match transcriber
            .transcribe(&supplier_key, &body, extension, &mime)
            .await
        {
            Ok(transcription) => transcription,
            Err(ProviderFailure::Unavailable(detail)) => {
                tracing::error!(user_id = %task_user, %detail, "dictation: provider unavailable; not charging");
                return Err(try_again_later());
            }
            Err(ProviderFailure::Rejected(detail)) => {
                tracing::warn!(user_id = %task_user, %detail, "dictation: provider rejected the recording; not charging");
                return Err(reject(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Could not read that recording. Try again.",
                ));
            }
        };
        if let Some(db) = task_state.db.as_ref() {
            let cost = observed_cost_micros(&rate, &transcription.usage, declared_ms);
            // A ledger failure here is Cortex's own fault: logged inside
            // `charge_observed_cost`, absorbed, and the text is still returned.
            let _ = charge_observed_cost(
                db,
                &task_user,
                "dictation",
                &charge_label,
                "Cortex dictation",
                cost,
                micros_per_credit,
            );
        }
        Ok(Json(DictationResponse {
            text: transcription.text,
        }))
    });
    match task.await {
        Ok(result) => result,
        Err(join_error) => {
            tracing::error!(%join_error, "dictation: task panicked");
            Err(try_again_later())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::post;
    use axum::Router;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const USER: &str = "user-1";
    const SUPPLIER_KEY: &str = "sk-test-supplier-0123456789";

    struct FakeOpenAi {
        status: StatusCode,
        body: Value,
        calls: AtomicUsize,
        last_auth: Mutex<Option<String>>,
        last_body: Mutex<Vec<u8>>,
    }

    async fn fake_transcribe(
        State(fake): State<Arc<FakeOpenAi>>,
        headers: HeaderMap,
        body: Bytes,
    ) -> (StatusCode, Json<Value>) {
        fake.calls.fetch_add(1, Ordering::SeqCst);
        *fake.last_auth.lock().unwrap() = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        *fake.last_body.lock().unwrap() = body.to_vec();
        (fake.status, Json(fake.body.clone()))
    }

    async fn spawn_fake(status: StatusCode, body: Value) -> (String, Arc<FakeOpenAi>) {
        let fake = Arc::new(FakeOpenAi {
            status,
            body,
            calls: AtomicUsize::new(0),
            last_auth: Mutex::new(None),
            last_body: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .route("/v1/audio/transcriptions", post(fake_transcribe))
            .with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}"), fake)
    }

    async fn test_state(credits: i64) -> (tempfile::TempDir, Arc<AppState>) {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(
            dir.path().join(".cortex/ledger.jsonl"),
            dir.path().to_path_buf(),
            None,
        )
        .await;
        state
            .db
            .as_ref()
            .unwrap()
            .init_credit_balance(USER, credits)
            .unwrap();
        (dir, state)
    }

    fn upload_headers(request_id: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", "audio/webm;codecs=opus".parse().unwrap());
        headers.insert(REQUEST_ID_HEADER, request_id.parse().unwrap());
        headers.insert(DURATION_HEADER, "4200".parse().unwrap());
        headers
    }

    fn live() -> LiveVoiceMode {
        LiveVoiceMode::Live(SUPPLIER_KEY.into())
    }

    fn ledger_rows(state: &AppState) -> i64 {
        state
            .db
            .as_ref()
            .unwrap()
            .conn()
            .query_row("SELECT COUNT(*) FROM credit_transactions", [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    async fn call(
        state: &Arc<AppState>,
        base: &str,
        request_id: &str,
        audio: Vec<u8>,
    ) -> Result<Json<DictationResponse>, Reject> {
        dictation_with(
            state,
            live(),
            base,
            USER,
            &upload_headers(request_id),
            Bytes::from(audio),
        )
        .await
    }

    #[test]
    fn multipart_body_carries_model_format_and_the_audio_bytes() {
        let body = multipart_body("B", "webm", "audio/webm", &[1, 2, 3]);
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("name=\"model\"\r\n\r\ngpt-4o-mini-transcribe\r\n"));
        assert!(text.contains("name=\"response_format\"\r\n\r\njson\r\n"));
        assert!(text.contains("filename=\"dictation.webm\""));
        assert!(body.ends_with(b"\r\n--B--\r\n"));
    }

    #[test]
    fn usage_parses_tokens_seconds_and_absence() {
        assert_eq!(
            parse_usage(
                &json!({"text": "x", "usage": {"type": "tokens", "input_tokens": 14, "output_tokens": 45}})
            ),
            ObservedUsage::Tokens {
                input: 14,
                output: 45
            }
        );
        assert_eq!(
            parse_usage(&json!({"text": "x", "usage": {"type": "duration", "seconds": 3.5}})),
            ObservedUsage::Seconds(3.5)
        );
        assert_eq!(parse_usage(&json!({"text": "x"})), ObservedUsage::Absent);
    }

    #[tokio::test]
    async fn observed_token_usage_is_charged_exactly_with_the_remainder_carried() {
        let (_dir, state) = test_state(10).await;
        let (base, fake) = spawn_fake(
            StatusCode::OK,
            json!({"text": "hello there", "usage": {
                "type": "tokens", "input_tokens": 48_000, "output_tokens": 9_000,
                "total_tokens": 57_000,
                "input_token_details": {"audio_tokens": 48_000, "text_tokens": 0}}}),
        )
        .await;

        let Json(reply) = call(&state, &base, "request-0001", vec![7; 2048])
            .await
            .expect("dictation succeeds");
        assert_eq!(reply.text, "hello there");
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            fake.last_auth.lock().unwrap().as_deref(),
            Some(format!("Bearer {SUPPLIER_KEY}").as_str())
        );
        let sent = fake.last_body.lock().unwrap().clone();
        assert!(String::from_utf8_lossy(&sent).contains("gpt-4o-mini-transcribe"));

        let db = state.db.as_ref().unwrap();
        let price_list = db.active_price_list().unwrap();
        let rate = price_list
            .model("openai", "gpt-4o-mini-transcribe")
            .cloned()
            .unwrap();
        // $1.25 and $5.00 per 1M tokens: 48_000 in = 60_000, 9_000 out =
        // 45_000, so 105_000 micro-USD exactly.
        let cost = rate.cost_micros(48_000, 0, 9_000);
        assert_eq!(cost, 105_000);
        let mpc = price_list.micros_per_credit;
        let balance = db.get_credit_balance_row(USER).unwrap();
        assert_eq!(10 - balance.subscription_remaining, cost / mpc);
        assert_eq!(db.get_credit_carry_micro_usd(USER), (cost % mpc) as u64);

        let rows: Vec<(String, i64, Option<i64>)> = {
            let conn = db.conn();
            let mut stmt = conn
                .prepare(
                    "SELECT idempotency_key, amount, cost_micro_usd FROM credit_transactions \
                     WHERE idempotency_key LIKE 'dictation:%'",
                )
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        assert_eq!(
            rows,
            vec![(
                "dictation:request-0001".to_string(),
                -(cost / mpc),
                Some(cost)
            )]
        );
    }

    #[tokio::test]
    async fn a_duplicate_request_id_is_not_charged_or_sent_to_the_provider_again() {
        let (_dir, state) = test_state(10).await;
        let (base, fake) = spawn_fake(
            StatusCode::OK,
            json!({"text": "once", "usage": {"type": "tokens", "input_tokens": 48_000, "output_tokens": 9_000}}),
        )
        .await;

        let _ = call(&state, &base, "request-dup1", vec![1; 512])
            .await
            .expect("first request succeeds");
        let db = state.db.as_ref().unwrap();
        let balance_after_first = db.get_credit_balance_row(USER).unwrap();
        let carry_after_first = db.get_credit_carry_micro_usd(USER);
        let rows_after_first = ledger_rows(&state);

        let (status, _) = call(&state, &base, "request-dup1", vec![1; 512])
            .await
            .expect_err("a replayed id is refused");
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            fake.calls.load(Ordering::SeqCst),
            1,
            "no second provider call"
        );
        assert_eq!(
            db.get_credit_balance_row(USER).unwrap(),
            balance_after_first
        );
        assert_eq!(db.get_credit_carry_micro_usd(USER), carry_after_first);
        assert_eq!(ledger_rows(&state), rows_after_first);
    }

    #[tokio::test]
    async fn a_zero_balance_gets_402_and_the_provider_is_not_called() {
        let (_dir, state) = test_state(0).await;
        let (base, fake) = spawn_fake(StatusCode::OK, json!({"text": "never"})).await;

        let (status, Json(body)) = call(&state, &base, "request-zero", vec![1; 512])
            .await
            .expect_err("no credits, no dictation");
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
        assert_eq!(body["need_credits"], 1);
        assert_eq!(body["available_credits"], 0);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
        assert_eq!(ledger_rows(&state), 0);
    }

    #[tokio::test]
    async fn a_provider_503_is_try_again_later_with_no_charge() {
        let (_dir, state) = test_state(10).await;
        let (base, fake) = spawn_fake(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error": {"message": "overloaded"}}),
        )
        .await;

        let (status, Json(body)) = call(&state, &base, "request-503a", vec![1; 512])
            .await
            .expect_err("provider outage");
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"], "Dictation unavailable, try again later");
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
        let db = state.db.as_ref().unwrap();
        assert_eq!(
            db.get_credit_balance_row(USER)
                .unwrap()
                .subscription_remaining,
            10
        );
        assert_eq!(db.get_credit_carry_micro_usd(USER), 0);
        assert_eq!(ledger_rows(&state), 0);

        // The same id can be tried again once the provider is back.
        let (status, _) = call(&state, &base, "request-503a", vec![1; 512])
            .await
            .expect_err("still down");
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn oversize_audio_is_413_with_no_provider_call() {
        let (_dir, state) = test_state(10).await;
        let (base, fake) = spawn_fake(StatusCode::OK, json!({"text": "never"})).await;

        let (status, _) = call(&state, &base, "request-big1", vec![0; MAX_AUDIO_BYTES + 1])
            .await
            .expect_err("too large");
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
        assert_eq!(ledger_rows(&state), 0);
    }

    #[tokio::test]
    async fn a_declared_duration_over_two_minutes_is_413_with_no_provider_call() {
        let (_dir, state) = test_state(10).await;
        let (base, fake) = spawn_fake(StatusCode::OK, json!({"text": "never"})).await;
        let mut headers = upload_headers("request-long");
        headers.insert(DURATION_HEADER, "120001".parse().unwrap());

        let (status, _) = dictation_with(
            &state,
            live(),
            &base,
            USER,
            &headers,
            Bytes::from(vec![1; 512]),
        )
        .await
        .expect_err("too long");
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_response_without_usage_is_charged_from_the_declared_duration() {
        let (_dir, state) = test_state(10).await;
        let (base, _fake) = spawn_fake(StatusCode::OK, json!({"text": "no usage"})).await;

        let _ = call(&state, &base, "request-dur1", vec![1; 512])
            .await
            .expect("dictation succeeds");
        // 4.2 s at $0.003/min = 50 micro-USD/s = 210 micro-USD, all carry.
        let db = state.db.as_ref().unwrap();
        assert_eq!(
            db.get_credit_balance_row(USER)
                .unwrap()
                .subscription_remaining,
            10
        );
        assert_eq!(db.get_credit_carry_micro_usd(USER), 210);
    }
}
