//! `POST /api/voice/live/sessions` / `DELETE /api/voice/live/sessions/{id}` —
//! broker a GPT-Live session from a browser's SDP offer and hold credits
//! against it for as long as it runs.
//!
//! The browser never talks to OpenAI directly and never sees a key: it hands
//! its SDP offer to Cortex, Cortex forwards it to `POST /v1/live/sessions` on
//! the real key and returns the answer SDP plus the session id OpenAI
//! minted. Media then flows browser <-> OpenAI directly, over the addresses
//! the SDP negotiated — Cortex is out of that path.
//!
//! What Cortex stays on the hook for is billing, and the only input it
//! trusts for that is `usage.seconds` carried on a server-side "sideband"
//! WebSocket this module attaches to the session the moment it starts
//! (`wss://.../v1/live/sessions/{id}/attach`, bearer-authenticated with
//! Cortex's own key — nothing the browser reports is billing input). A
//! background task owns that socket for the session's whole life and runs
//! the credit hold: reserve a 300s-worth segment, settle it and reserve the
//! next when usage crosses 80% of what has been reserved so far, and settle
//! the final open segment when `session.closed` arrives. A reservation that
//! cannot be renewed (the authorization is spent) gets one spoken warning
//! and 20s before the server closes the call itself; a sideband that drops
//! without a `session.closed` leaves its open segment `unresolved` rather
//! than inventing a final cost.
//!
//! Stub/live is the same switch `voice.rs` and the provider gateway use.
//! Tests never flip that switch — they call [`start_session`] directly with
//! [`LiveVoiceMode::Live`] pointed at a loopback `FakeLive` (below), the same
//! pattern `voice.rs`'s dictation tests use against a loopback
//! `client_secrets` fake.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

use cortex_core::billing_binding::ChargeKey;

use crate::clerk::ClerkUser;
use crate::provider_gateway::GatewayCapability;
use crate::provider_gateway_http::{self, SpendLimits};
use crate::routes::ErrorResponse;
use crate::state::AppState;
use crate::voice::ceil_div;

/// The model every live voice session runs. Held in one place so a
/// capability's model and the config sent to OpenAI can never drift apart.
const LIVE_MODEL: &str = "gpt-live-1";
const LIVE_PROVIDER: &str = "openai";
const OPENAI_HTTP_BASE: &str = "https://api.openai.com";
const OPENAI_WS_BASE: &str = "wss://api.openai.com";
/// One authorization's outside lifetime. A voice call longer than this needs
/// a new session; nothing today asks for one.
const SESSION_LEASE_MS: i64 = 4 * 60 * 60 * 1000;
/// Each credit segment covers up to this many seconds of gpt-live-1 time.
const SEGMENT_SECONDS: i64 = 300;
/// A segment settles, and the next one reserves, once usage has burned
/// through this fraction of what has been reserved so far (as a `/5`
/// fraction so the comparison stays integer: `observed*5 >= reserved*4`).
const SETTLE_THRESHOLD_NUM: i64 = 4;
const SETTLE_THRESHOLD_DEN: i64 = 5;
/// How long a warned session gets to wrap up before the server closes it.
const WARNING_GRACE: Duration = Duration::from_secs(20);
const WARNING_TEXT: &str = "Credits are nearly out. Say goodbye briefly.";
/// Anthropic/OpenAI keys are far longer than this; anything shorter is a typo
/// or a placeholder, and a placeholder must not switch real spending on.
const MIN_SUPPLIER_KEY_LEN: usize = 20;
/// How long the sideband attach (initial, and the one re-attach attempted
/// after an unexpected drop) is given before it counts as failed. A client
/// disconnect or a panic must not leave this hanging forever.
const SIDEBAND_ATTACH_TIMEOUT: Duration = Duration::from_secs(15);

/// The session's shape sent to OpenAI, held in one function so client
/// delegation and tools have exactly one place to change. `delegation.type:
/// "client"` hands requests the model can't answer on its own to this
/// server (see `handle_delegation_created` in the billing sideband), which
/// runs the same paid agent loop text chat uses and answers back with
/// `session.commentary.append`.
pub(crate) fn live_session_config() -> Value {
    serde_json::json!({
        "model": LIVE_MODEL,
        "delegation": { "type": "client" },
    })
}

/// The same two states the provider gateway and dictation can be in,
/// narrowed to the one supplier this route ever calls.
pub(crate) enum LiveVoiceMode {
    Stub,
    Live(String),
}

pub(crate) fn live_voice_mode() -> Option<LiveVoiceMode> {
    match std::env::var("CORTEX_PROVIDER_GATEWAY_MODE").as_deref() {
        Ok("stub") => Some(LiveVoiceMode::Stub),
        Ok("live") => std::env::var("CORTEX_OPENAI_SUPPLIER_KEY")
            .ok()
            .filter(|key| key.trim().len() >= MIN_SUPPLIER_KEY_LEN)
            .map(LiveVoiceMode::Live),
        _ => None,
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum LiveSessionError {
    /// The route is off, misconfigured, or the model has no price.
    Unavailable,
    /// The user's balance cannot fund even the first segment.
    NotEnoughCredits,
    /// OpenAI refused or failed to start the session.
    SupplierFailed,
    /// The session id does not exist (already closed, or never was ours).
    NotFound,
    /// The caller does not own this session.
    Forbidden,
    /// The user already has a live voice session open.
    AlreadyOpen,
}

impl LiveSessionError {
    fn into_response(self) -> (StatusCode, Json<ErrorResponse>) {
        let (status, message) = match self {
            LiveSessionError::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "Live voice is temporarily unavailable. Please try again in a moment.",
            ),
            LiveSessionError::NotEnoughCredits => (
                StatusCode::PAYMENT_REQUIRED,
                "You don't have enough credits for a live voice session. Go to Settings → Billing to buy more or subscribe.",
            ),
            LiveSessionError::SupplierFailed => (
                StatusCode::BAD_GATEWAY,
                "Live voice is temporarily unavailable. Please try again in a moment.",
            ),
            LiveSessionError::NotFound => (StatusCode::NOT_FOUND, "No such voice session."),
            LiveSessionError::Forbidden => {
                (StatusCode::FORBIDDEN, "That is not your voice session.")
            }
            LiveSessionError::AlreadyOpen => (
                StatusCode::CONFLICT,
                "You already have a live voice session open. End it first.",
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

/// What `AppState` keeps for a session this server is brokering: who owns
/// it, and how to ask its billing task to close it.
pub struct VoiceSessionHandle {
    pub user_id: String,
    close_tx: mpsc::Sender<()>,
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

/// The one call this route ever makes to mint a session: `POST
/// /v1/live/sessions`. Base URL is injectable so tests can point it at a
/// loopback `FakeLive` instead of `api.openai.com`.
pub(crate) struct OpenAiLiveSessions {
    client: reqwest::Client,
    base_url: String,
}

impl OpenAiLiveSessions {
    pub(crate) fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            base_url: base_url.into(),
        }
    }

    async fn start(&self, supplier_key: &str, sdp: &str) -> Result<(String, String), String> {
        let url = format!("{}/v1/live/sessions", self.base_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "session": live_session_config(),
            "transport": {"type": "webrtc", "sdp": sdp},
        });
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {supplier_key}"))
            .json(&body)
            .send()
            .await
            .map_err(|error| format!("openai live session request failed: {error}"))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|error| format!("openai live session response was cut off: {error}"))?;
        if !status.is_success() {
            return Err(format!(
                "openai returned {status}: {}",
                error_message(&text)
            ));
        }
        let parsed: Value = serde_json::from_str(&text)
            .map_err(|_| "openai returned a body that is not JSON".to_string())?;
        let session_id = parsed
            .pointer("/session/id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "openai response is missing session.id".to_string())?;
        let answer_sdp = parsed
            .pointer("/transport/sdp")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "openai response is missing transport.sdp".to_string())?;
        Ok((session_id, answer_sdp))
    }
}

/// The sideband: one WebSocket, attached right after the session starts,
/// that lives for the session's whole life. Every event this module bills
/// on arrives here; nothing the browser reports is billing input.
struct WsSideband {
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
}

impl WsSideband {
    async fn attach(ws_base: &str, session_id: &str, supplier_key: &str) -> Result<Self, String> {
        let url = format!(
            "{}/v1/live/sessions/{session_id}/attach",
            ws_base.trim_end_matches('/')
        );
        let mut request = url
            .into_client_request()
            .map_err(|error| format!("bad sideband url: {error}"))?;
        let header_value = format!("Bearer {supplier_key}")
            .parse()
            .map_err(|_| "supplier key is not a valid header value".to_string())?;
        request.headers_mut().insert("Authorization", header_value);
        let (socket, _response) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|error| format!("sideband attach failed: {error}"))?;
        Ok(Self { socket })
    }

    async fn send(&mut self, value: Value) {
        let _ = self
            .socket
            .send(TungsteniteMessage::Text(value.to_string().into()))
            .await;
    }

    /// The next event, or `None` once the sideband is gone for good (a
    /// clean close frame, a transport error, or end of stream).
    async fn recv(&mut self) -> Option<Value> {
        loop {
            match self.socket.next().await {
                Some(Ok(TungsteniteMessage::Text(text))) => {
                    if let Ok(value) = serde_json::from_str::<Value>(&text) {
                        return Some(value);
                    }
                }
                Some(Ok(TungsteniteMessage::Close(_))) | None => return None,
                Some(Ok(_)) => continue,
                Some(Err(_)) => return None,
            }
        }
    }
}

/// Removes the one-session-per-user placeholder at `local_id` from
/// `state.voice_sessions` on drop, unless [`disarm`](Self::disarm) has run.
/// Guards the whole start pipeline (spawned so a client disconnect cannot
/// cancel it — see [`start_session`]) against leaking the placeholder on
/// any exit, including a panic, not just an `Err` return.
struct PlaceholderGuard {
    state: Arc<AppState>,
    local_id: String,
    armed: bool,
}

impl PlaceholderGuard {
    fn new(state: Arc<AppState>, local_id: String) -> Self {
        Self {
            state,
            local_id,
            armed: true,
        }
    }

    /// The pipeline succeeded and replaced (or otherwise owns) the
    /// placeholder itself; the guard must not remove it on drop.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PlaceholderGuard {
    fn drop(&mut self) {
        if self.armed {
            self.state
                .voice_sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&self.local_id);
        }
    }
}

/// Reserve, mint, attach, and start the billing task — the whole live-voice
/// pipeline, independent of the HTTP plumbing so it can be tested directly
/// against a loopback `FakeLive` and an in-memory database, the same shape
/// as `chat_paid::send_paid_reply` and `voice::mint_dictation_token`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn start_session(
    state: &Arc<AppState>,
    mode: LiveVoiceMode,
    http_base: &str,
    ws_base: &str,
    signing_key: &str,
    limits: SpendLimits,
    user_id: &str,
    sdp: &str,
    now_ms: i64,
) -> Result<(String, String, String), LiveSessionError> {
    let supplier_key = match mode {
        // Stub never leaves the machine and never spends: no session is
        // actually running, so there is no usage to hold credits against.
        LiveVoiceMode::Stub => {
            let stub_id = format!("stub-live-{}", uuid::Uuid::new_v4());
            return Ok((stub_id.clone(), "stub-answer-sdp".to_string(), stub_id));
        }
        LiveVoiceMode::Live(key) => key,
    };

    // A server-generated id, minted before anything else touches the
    // ledger or OpenAI, so a reservation key never has to wait on a
    // supplier response that may never come.
    let local_id = uuid::Uuid::new_v4().to_string();

    // One live session per user. The check and the placeholder insert
    // happen under the same lock acquisition so two concurrent starts for
    // the same user cannot both pass the check before either inserts.
    {
        let mut sessions = state
            .voice_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if sessions.values().any(|handle| handle.user_id == user_id) {
            return Err(LiveSessionError::AlreadyOpen);
        }
        let (close_tx, _close_rx) = mpsc::channel(1);
        sessions.insert(
            local_id.clone(),
            VoiceSessionHandle {
                user_id: user_id.to_string(),
                close_tx,
            },
        );
    }

    // Run the pipeline on its own task and await the `JoinHandle` rather
    // than the pipeline future directly: if the caller (the HTTP handler)
    // is dropped — a client disconnect while OpenAI or the sideband attach
    // is still in flight — awaiting a `JoinHandle` drops cleanly but does
    // NOT cancel the spawned task, so the reservation this already made
    // still gets released/marked and the per-user placeholder still gets
    // removed instead of leaking until restart.
    let spawn_state = state.clone();
    let spawn_http_base = http_base.to_string();
    let spawn_ws_base = ws_base.to_string();
    let spawn_signing_key = signing_key.to_string();
    let spawn_user_id = user_id.to_string();
    let spawn_sdp = sdp.to_string();
    let spawn_local_id = local_id.clone();
    let join_handle = tokio::spawn(async move {
        let mut guard = PlaceholderGuard::new(spawn_state.clone(), spawn_local_id.clone());
        let result = start_live_session(
            &spawn_state,
            &supplier_key,
            &spawn_http_base,
            &spawn_ws_base,
            &spawn_signing_key,
            limits,
            &spawn_user_id,
            &spawn_sdp,
            now_ms,
            spawn_local_id,
        )
        .await;
        if result.is_ok() {
            guard.disarm();
        }
        result
    });

    match join_handle.await {
        Ok(result) => result,
        Err(join_error) => {
            tracing::error!(%join_error, "voice: start pipeline task panicked");
            Err(LiveSessionError::SupplierFailed)
        }
    }
}

/// The reserve-mint-attach-and-launch pipeline for a real (non-stub) live
/// session, split out of [`start_session`] so the one-session-per-user
/// placeholder in the caller has a single, simple cleanup point: remove
/// `local_id` from `state.voice_sessions` on any `Err` this returns.
#[allow(clippy::too_many_arguments)]
async fn start_live_session(
    state: &Arc<AppState>,
    supplier_key: &str,
    http_base: &str,
    ws_base: &str,
    signing_key: &str,
    limits: SpendLimits,
    user_id: &str,
    sdp: &str,
    now_ms: i64,
    local_id: String,
) -> Result<(String, String, String), LiveSessionError> {
    let db = state.db.as_ref().ok_or(LiveSessionError::Unavailable)?;
    let price_list = db
        .active_price_list()
        .ok_or(LiveSessionError::Unavailable)?;
    let rate = price_list
        .model(LIVE_PROVIDER, LIVE_MODEL)
        .cloned()
        .ok_or_else(|| {
            tracing::error!("voice: no price for gpt-live-1; refusing");
            LiveSessionError::Unavailable
        })?;

    let balance = db
        .get_credit_balance_row(user_id)
        .filter(|b| b.subscription_remaining + b.pack_remaining > 0)
        .ok_or(LiveSessionError::NotEnoughCredits)?;
    let balance_micro_usd = (balance.subscription_remaining + balance.pack_remaining)
        .saturating_mul(price_list.micros_per_credit);
    let max_micro_usd = limits.max_micro_usd.min(balance_micro_usd);

    let segment_micro = rate.cost_micros(SEGMENT_SECONDS, 0, 0);
    let segment0_micro = max_micro_usd.min(segment_micro);
    if segment0_micro <= 0 {
        return Err(LiveSessionError::NotEnoughCredits);
    }

    let run_id = format!("voice:{local_id}");
    // The authorization row's primary key is `gateway-auth:{attempt_id}` —
    // a literal `"live"` here collided on every second voice session ever
    // started (by anyone), since `create_spend_authorization` would then
    // try to insert the exact same row id twice and fail. `local_id` is a
    // fresh UUID per call, so this keeps every session's attempt id unique
    // the way `chat_paid`'s `chat-reply:{reply_id}` already does.
    let attempt_id = format!("live:{local_id}");
    let expires_at_ms = now_ms + SESSION_LEASE_MS;
    let Some((authorization_id, _signed)) =
        provider_gateway_http::create_authorization_and_capability(
            db,
            signing_key,
            user_id,
            &run_id,
            &attempt_id,
            LIVE_PROVIDER,
            LIVE_MODEL,
            max_micro_usd,
            limits.funded_micro_usd,
            expires_at_ms,
            now_ms,
        )
    else {
        return Err(LiveSessionError::Unavailable);
    };
    let claims = GatewayCapability::new(
        authorization_id,
        user_id,
        &run_id,
        &attempt_id,
        LIVE_PROVIDER,
        LIVE_MODEL,
        expires_at_ms,
    );

    // Reserve the first segment before OpenAI is ever asked to start a
    // session: the hold has to exist before any supplier cost can be run
    // up, not after.
    let segment_key = format!("voice:{local_id}:0");
    if db
        .reserve_provider_request(
            &claims,
            &segment_key,
            "voice-segment:0",
            segment0_micro,
            now_ms,
        )
        .is_err()
    {
        tracing::error!(
            user_id,
            "voice: first segment reservation failed before the session could start"
        );
        return Err(LiveSessionError::NotEnoughCredits);
    }

    let starter = OpenAiLiveSessions::with_base_url(http_base);
    let (session_id, answer_sdp) = match starter.start(supplier_key, sdp).await {
        Ok(started) => started,
        Err(detail) => {
            tracing::error!(user_id, %detail, "voice: openai live session POST failed");
            let _ =
                db.release_provider_request(&segment_key, "openai session start failed", now_ms);
            return Err(LiveSessionError::SupplierFailed);
        }
    };

    let sideband = match tokio::time::timeout(
        SIDEBAND_ATTACH_TIMEOUT,
        WsSideband::attach(ws_base, &session_id, supplier_key),
    )
    .await
    {
        Ok(Ok(sideband)) => sideband,
        Ok(Err(detail)) => {
            tracing::error!(
                user_id,
                session_id = %session_id,
                %detail,
                "voice: sideband attach failed after the openai session already started"
            );
            let _ = db.mark_provider_request_unresolved(
                &segment_key,
                None,
                "sideband attach failed after session start",
                now_ms,
            );
            // The OpenAI session started but we will never bill on it now;
            // best effort, ask it to close rather than leave it running
            // unmetered.
            reattach_and_close(ws_base, &session_id, supplier_key).await;
            return Err(LiveSessionError::SupplierFailed);
        }
        Err(_elapsed) => {
            tracing::error!(
                user_id,
                session_id = %session_id,
                "voice: sideband attach timed out after the openai session already started"
            );
            let _ = db.mark_provider_request_unresolved(
                &segment_key,
                None,
                "sideband attach timed out after session start",
                now_ms,
            );
            reattach_and_close(ws_base, &session_id, supplier_key).await;
            return Err(LiveSessionError::SupplierFailed);
        }
    };

    // The placeholder inserted under `local_id` in `start_session` is
    // replaced here, now that the real session id is known, with the same
    // close channel — nothing that could race the one-per-user check is
    // still pending.
    let (close_tx, close_rx) = mpsc::channel(1);
    {
        let mut sessions = state
            .voice_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        sessions.remove(&local_id);
        sessions.insert(
            session_id.clone(),
            VoiceSessionHandle {
                user_id: user_id.to_string(),
                close_tx,
            },
        );
    }

    let loop_state = state.clone();
    let loop_session_id = session_id.clone();
    let loop_local_id = local_id.clone();
    let loop_user_id = user_id.to_string();
    let loop_ws_base = ws_base.to_string();
    let loop_supplier_key = supplier_key.to_string();
    let micros_per_credit = price_list.micros_per_credit;
    tokio::spawn(async move {
        run_billing_loop(
            loop_state,
            sideband,
            claims,
            loop_session_id,
            loop_local_id,
            loop_user_id,
            loop_ws_base,
            loop_supplier_key,
            rate,
            micros_per_credit,
            max_micro_usd,
            segment0_micro,
            close_rx,
        )
        .await;
    });

    Ok((session_id, answer_sdp, local_id))
}

/// Ask the session owned at `session_id` to close. Only the owner may do
/// this; the actual `session.close` is sent by the billing task, which also
/// removes the session from `state.voice_sessions` once it is gone.
pub(crate) async fn close_session(
    state: &Arc<AppState>,
    user_id: &str,
    session_id: &str,
) -> Result<(), LiveSessionError> {
    let close_tx = {
        let sessions = state
            .voice_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = sessions.get(session_id).ok_or(LiveSessionError::NotFound)?;
        if handle.user_id != user_id {
            return Err(LiveSessionError::Forbidden);
        }
        handle.close_tx.clone()
    };
    let _ = close_tx.send(()).await;
    Ok(())
}

/// One credit hold still open (or partly consumed) against the durable
/// reservation table, keyed by `voice:{local_id}:{index}`.
#[derive(Clone, Copy)]
struct Segment {
    index: i64,
    reserved_micro: i64,
}

/// Guards `run_billing_loop`'s whole life against leaking the session on
/// any exit that is not the loop's own normal cleanup — a panic in event
/// handling, most concretely. On drop while still armed it removes
/// `session_id` from `state.voice_sessions` and marks every segment still
/// in `segments` `unresolved`, the same shape the sideband-drop path uses
/// deliberately, so a panicked billing task cannot leave the user's
/// placeholder stuck or a reservation silently `reserved` forever. The
/// normal exit path disarms it right before doing this itself.
struct BillingLoopGuard {
    state: Arc<AppState>,
    session_id: String,
    local_id: String,
    segments: Arc<std::sync::Mutex<std::collections::VecDeque<Segment>>>,
    armed: bool,
}

impl BillingLoopGuard {
    fn new(
        state: Arc<AppState>,
        session_id: String,
        local_id: String,
        segments: Arc<std::sync::Mutex<std::collections::VecDeque<Segment>>>,
    ) -> Self {
        Self {
            state,
            session_id,
            local_id,
            segments,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for BillingLoopGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.state
            .voice_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.session_id);
        if let Some(db) = self.state.db.as_ref() {
            let now_ms = chrono::Utc::now().timestamp_millis();
            let mut segments = self.segments.lock().unwrap_or_else(|e| e.into_inner());
            for seg in segments.drain(..) {
                let key = format!("voice:{}:{}", self.local_id, seg.index);
                let _ = db.mark_provider_request_unresolved(
                    &key,
                    None,
                    "voice billing task exited abnormally without settling",
                    now_ms,
                );
            }
        }
        tracing::error!(
            session_id = %self.session_id,
            "voice: billing loop task exited abnormally; session removed and open segments left unresolved"
        );
    }
}

/// Owns the sideband for one session's whole life: settles and renews
/// credit segments off `session.usage.updated`, settles whatever is left
/// off `session.closed`, and — if the socket disappears without a
/// `session.closed` — leaves every still-open segment `unresolved` for
/// reconciliation instead of inventing a final cost.
#[allow(clippy::too_many_arguments)]
async fn run_billing_loop(
    state: Arc<AppState>,
    mut sideband: WsSideband,
    claims: GatewayCapability,
    session_id: String,
    local_id: String,
    user_id: String,
    ws_base: String,
    supplier_key: String,
    rate: crate::pricing::ModelPrice,
    micros_per_credit: i64,
    max_micro_usd: i64,
    segment0_micro: i64,
    mut close_rx: mpsc::Receiver<()>,
) {
    let Some(db) = state.db.as_ref() else {
        state
            .voice_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&session_id);
        return;
    };

    let segments: Arc<std::sync::Mutex<std::collections::VecDeque<Segment>>> =
        Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
    segments
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_back(Segment {
            index: 0,
            reserved_micro: segment0_micro,
        });
    let mut guard = BillingLoopGuard::new(
        state.clone(),
        session_id.clone(),
        local_id.clone(),
        segments.clone(),
    );
    let mut next_segment_index: i64 = 0;
    let mut reserved_so_far_micro = segment0_micro;
    let mut settled_so_far_micro: i64 = 0;
    // Cumulative credits already deducted across every settle/overrun charge
    // in this session. Each charge deducts only the delta between
    // `ceil_div(settled_so_far_micro, micros_per_credit)` and this — so
    // rounding only ever happens once, on the outstanding remainder, instead
    // of once per segment (which could overcharge by up to a credit per
    // segment and, worse, make the final segment's all-or-nothing deduction
    // fail even though the user had enough for the *session's* real cost).
    let mut charged_credits_so_far: i64 = 0;
    let segment_micro = rate.cost_micros(SEGMENT_SECONDS, 0, 0);
    let mut warning_deadline: Option<tokio::time::Instant> = None;
    let mut closed_cleanly = false;
    // Once a credit deduction fails, this session must never reserve
    // another segment — a loop-lifetime flag, not a per-event local, so a
    // later `session.usage.updated` cannot start renewing again.
    let mut budget_exhausted = false;
    // The most recent `usage.seconds` this session ever reported, kept so a
    // sideband that drops mid-session (no final `session.closed`) can still
    // settle and charge for everything actually observed instead of
    // treating a mid-flight drop as zero usage since the last renewal.
    let mut last_observed_total_micro: i64 = 0;
    let now_ms = || chrono::Utc::now().timestamp_millis();

    // Delegation state, local to this session's billing task. The task text
    // for the next delegation is whatever speech text has accumulated since
    // the last one was answered — `session.delegation.created` itself
    // carries no request text or tool arguments (confirmed against
    // https://developers.openai.com/api/docs/guides/live-migration).
    let mut pending_transcript = String::new();
    let delegation_busy = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // The currently-running delegation's `JoinHandle`, if any — aborted once
    // this loop exits so a delegation's agent turn never keeps running (and
    // billing) after the voice session it was answering for is gone.
    let delegation_task: Arc<std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>> =
        Arc::new(std::sync::Mutex::new(None));
    // The delegation task runs on its own spawned task (an agent turn can
    // take much longer than this loop should ever block for) and answers
    // through this channel rather than touching `sideband` directly, so the
    // loop below stays the single owner of the socket.
    let (commentary_tx, mut commentary_rx) = mpsc::channel::<Value>(8);

    loop {
        let warning_timer = async {
            match warning_deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending::<()>().await,
            }
        };

        tokio::select! {
            Some(commentary) = commentary_rx.recv() => {
                sideband.send(commentary).await;
            }
            event = sideband.recv() => {
                let Some(event) = event else { break };
                match event.get("type").and_then(Value::as_str) {
                    Some("session.input_transcript.delta") => {
                        accumulate_transcript(&mut pending_transcript, &event);
                    }
                    Some("session.delegation.created") => {
                        handle_delegation_created(
                            &event,
                            &state,
                            &user_id,
                            &delegation_busy,
                            &delegation_task,
                            &mut pending_transcript,
                            &commentary_tx,
                        );
                    }
                    Some("session.usage.updated") => {
                        let seconds = event
                            .pointer("/usage/seconds")
                            .and_then(Value::as_i64)
                            .unwrap_or(0);
                        let observed_total = rate.cost_micros(seconds, 0, 0);
                        last_observed_total_micro = observed_total;
                        budget_exhausted |= settle_up_to(
                            db,
                            &user_id,
                            &session_id,
                            &local_id,
                            &mut segments.lock().unwrap_or_else(|e| e.into_inner()),
                            &mut settled_so_far_micro,
                            &mut charged_credits_so_far,
                            observed_total,
                            false,
                            micros_per_credit,
                            now_ms(),
                        );

                        let mut cap_exhausted = false;
                        let mut reserve_refused = false;
                        if !budget_exhausted {
                            loop {
                                // observed/reserved_so_far >= 4/5 (80%), kept
                                // integer as observed*5 >= reserved*4: DEN
                                // multiplies observed, NUM multiplies
                                // reserved_so_far.
                                let threshold_crossed = observed_total.saturating_mul(SETTLE_THRESHOLD_DEN)
                                    >= reserved_so_far_micro.saturating_mul(SETTLE_THRESHOLD_NUM);
                                if !threshold_crossed {
                                    break;
                                }
                                let remaining_cap = max_micro_usd - reserved_so_far_micro;
                                if remaining_cap <= 0 {
                                    cap_exhausted = true;
                                    break;
                                }
                                let next_micro = remaining_cap.min(segment_micro);
                                let candidate_index = next_segment_index + 1;
                                let key = format!("voice:{local_id}:{candidate_index}");
                                match db.reserve_provider_request(
                                    &claims,
                                    &key,
                                    &format!("voice-segment:{candidate_index}"),
                                    next_micro,
                                    now_ms(),
                                ) {
                                    Ok(_) => {
                                        next_segment_index = candidate_index;
                                        segments
                                            .lock()
                                            .unwrap_or_else(|e| e.into_inner())
                                            .push_back(Segment {
                                                index: candidate_index,
                                                reserved_micro: next_micro,
                                            });
                                        reserved_so_far_micro += next_micro;
                                    }
                                    Err(_) => {
                                        reserve_refused = true;
                                        break;
                                    }
                                }
                            }
                        }

                        if budget_exhausted {
                            // Item D: a settle's credit deduction failing is
                            // budget exhaustion, exactly like a refused
                            // renewal — stop renewing and head for the
                            // goodbye path.
                            if warning_deadline.is_none() {
                                sideband
                                    .send(serde_json::json!({
                                        "type": "session.instructions.append",
                                        "instructions": WARNING_TEXT,
                                    }))
                                    .await;
                                warning_deadline = Some(tokio::time::Instant::now() + WARNING_GRACE);
                            }
                        } else if cap_exhausted || reserve_refused {
                            let overrun = observed_total >= reserved_so_far_micro;
                            if overrun {
                                sideband
                                    .send(serde_json::json!({"type": "session.close"}))
                                    .await;
                                warning_deadline = None;
                            } else if warning_deadline.is_none() {
                                sideband
                                    .send(serde_json::json!({
                                        "type": "session.instructions.append",
                                        "instructions": WARNING_TEXT,
                                    }))
                                    .await;
                                warning_deadline = Some(tokio::time::Instant::now() + WARNING_GRACE);
                            }
                        }
                    }
                    Some("session.closed") => {
                        let seconds = event
                            .pointer("/usage/seconds")
                            .and_then(Value::as_i64)
                            .unwrap_or(0);
                        let observed_total = rate.cost_micros(seconds, 0, 0);
                        last_observed_total_micro = observed_total;
                        settle_up_to(
                            db,
                            &user_id,
                            &session_id,
                            &local_id,
                            &mut segments.lock().unwrap_or_else(|e| e.into_inner()),
                            &mut settled_so_far_micro,
                            &mut charged_credits_so_far,
                            observed_total,
                            true,
                            micros_per_credit,
                            now_ms(),
                        );
                        closed_cleanly = true;
                        break;
                    }
                    _ => {}
                }
            }
            _ = close_rx.recv() => {
                sideband.send(serde_json::json!({"type": "session.close"})).await;
            }
            _ = warning_timer => {
                sideband.send(serde_json::json!({"type": "session.close"})).await;
                warning_deadline = None;
            }
        }
    }

    if !closed_cleanly {
        // Settle whatever was fully consumed against the last usage this
        // session ever reported — a drop is not zero usage since the last
        // renewal, it is exactly `last_observed_total_micro`. `is_final` is
        // false here on purpose: a segment that is only partly consumed
        // must stay `unresolved` below rather than being force-settled for
        // less than it reserved.
        let _ = settle_up_to(
            db,
            &user_id,
            &session_id,
            &local_id,
            &mut segments.lock().unwrap_or_else(|e| e.into_inner()),
            &mut settled_so_far_micro,
            &mut charged_credits_so_far,
            last_observed_total_micro,
            false,
            micros_per_credit,
            now_ms(),
        );

        // Usage observed but not yet reflected in charged credits is still
        // real spend, whether it is a mid-segment drop (usage past what
        // fully settled but not past every reservation, so the loop above
        // left it `None`) or the debt from an earlier non-final charge
        // that failed and was only ever recorded in
        // `charged_credits_so_far` — with no further `session.usage.updated`
        // coming, that debt would otherwise never be retried. Charge the
        // whole outstanding remainder now, capped to balance since this is
        // the last chance (`is_final = true`).
        let basis_micro = last_observed_total_micro.max(settled_so_far_micro);
        let credits_due = ceil_div(basis_micro, micros_per_credit);
        let delta = credits_due - charged_credits_so_far;
        if delta > 0 {
            let drop_key = format!("voice:{local_id}:drop");
            tracing::error!(
                user_id = %user_id,
                session_id = %session_id,
                delta,
                "voice: sideband dropped with usage not yet reflected in charged credits; charging the remainder directly"
            );
            let _ = charge_incremental(
                db,
                &user_id,
                &session_id,
                &drop_key,
                "Cortex live voice (dropped session)",
                delta,
                true,
            );
        }

        for seg in segments.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
            let key = format!("voice:{local_id}:{}", seg.index);
            let _ = db.mark_provider_request_unresolved(
                &key,
                Some(session_id.as_str()),
                &format!(
                    "sideband dropped without session.closed; last observed session total {last_observed_total_micro} micro-USD"
                ),
                now_ms(),
            );
        }
        tracing::error!(
            session_id = %session_id,
            "voice: sideband dropped without session.closed; open reservation(s) left unresolved for reconciliation"
        );

        // Best effort: the sideband is gone, but the OpenAI session may
        // still be running and metering. Try once to re-attach and ask it
        // to close rather than leaving it running unmetered.
        reattach_and_close(&ws_base, &session_id, &supplier_key).await;
    }

    // The loop is done with this session; a delegation still running its
    // agent turn must not outlive it, so abort it here rather than let it
    // keep going (and keep billing) unattended.
    if let Some(handle) = delegation_task
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    {
        handle.abort();
    }

    guard.disarm();
    state
        .voice_sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&session_id);
}

/// A safe character cap for `session.commentary.append`'s 500-token limit
/// (https://developers.openai.com/api/docs/guides/live-migration). English
/// text tokenizes at roughly 4 characters/token, but token-dense text
/// (UUID-heavy strings, non-Latin scripts) can run as low as 1-3
/// characters/token — capping at 1200 characters was not safe against that
/// case. 450 characters stays under the 500-token limit even at 1
/// character/token, with the system prompt also asking the model to answer
/// in under 60 words as a second line of defense.
const COMMENTARY_CHAR_CAP: usize = 450;

/// The system prompt the delegated agent turn runs under. Short and
/// spoken-first: the text comes back as `session.commentary.append`, which
/// GPT-Live paraphrases aloud rather than reading verbatim, so it does not
/// need to be conversational itself — just accurate and short.
const VOICE_DELEGATION_SYSTEM_PROMPT: &str = "You are Cortex's coding agent, answering a request that came in over a live voice call. Keep answers short and to the point; they will be read aloud. Answer in under 60 words.";

/// Append a `session.input_transcript.delta`'s text to the accumulator that
/// becomes the next delegation's task text. The event's own shape is not
/// pinned down by the docs beyond "append to captions"; this reads the
/// common `delta`/`text` fields defensively and drops the event if neither
/// is present rather than guessing.
/// Caption text accumulates here between delegations; a caller who never
/// stops talking (or a delegation that never arrives) must not let this grow
/// without bound, so it is kept a sliding window of the most recent
/// [`PENDING_TRANSCRIPT_CHAR_CAP`] characters — the task text a delegation
/// actually needs is what was said most recently, not everything said since
/// the session started. Trimming always lands on a char boundary (never
/// splits a multi-byte UTF-8 character) by walking back from the cut point.
const PENDING_TRANSCRIPT_CHAR_CAP: usize = 2000;

fn accumulate_transcript(pending_transcript: &mut String, event: &Value) {
    let delta = event
        .get("delta")
        .and_then(Value::as_str)
        .or_else(|| event.get("text").and_then(Value::as_str));
    if let Some(delta) = delta {
        pending_transcript.push_str(delta);
    }
    if pending_transcript.len() > PENDING_TRANSCRIPT_CHAR_CAP {
        let excess = pending_transcript.len() - PENDING_TRANSCRIPT_CHAR_CAP;
        let mut cut = excess;
        while !pending_transcript.is_char_boundary(cut) {
            cut += 1;
        }
        pending_transcript.drain(..cut);
    }
}

/// Build one `session.commentary.append` event, trimmed to
/// [`COMMENTARY_CHAR_CAP`] characters.
fn commentary_event(delegation_id: &str, content: &str) -> Value {
    let content: String = content.chars().take(COMMENTARY_CHAR_CAP).collect();
    serde_json::json!({
        "type": "session.commentary.append",
        "delegation_id": delegation_id,
        "content": content,
    })
}

/// Answer a `session.delegation.created` event. `session.delegation.created`
/// itself carries no request text or tool arguments (confirmed against
/// https://developers.openai.com/api/docs/guides/live-migration) — the task
/// text is whatever has accumulated in `pending_transcript` since the last
/// delegation was answered, drained here.
///
/// Releases `delegation_busy` when dropped — including when the spawned
/// delegation task panics, since dropping still runs during the unwind. The
/// old code stored `false` as the last line of the spawned task's async
/// block, which never ran on panic and would wedge every later
/// `session.delegation.created` behind "still working on the last request."
/// for the rest of the session.
struct BusyGuard(Arc<std::sync::atomic::AtomicBool>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// One delegation runs at a time per session: `delegation_busy` is a
/// session-lifetime flag checked (and set) synchronously, right here in the
/// billing loop's single task, so there is no race between two delegations
/// arriving back to back. A second one while the first is still running
/// gets a short "still working" commentary instead of being queued or
/// dropped silently.
///
/// The spawned delegation task's handle is stashed in `delegation_task` so
/// the billing loop can abort it when the session ends — without this, a
/// delegation still running an agent turn would keep going (and keep
/// billing) after the session it was answering for is gone.
fn handle_delegation_created(
    event: &Value,
    state: &Arc<AppState>,
    user_id: &str,
    delegation_busy: &Arc<std::sync::atomic::AtomicBool>,
    delegation_task: &Arc<std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    pending_transcript: &mut String,
    commentary_tx: &mpsc::Sender<Value>,
) {
    let Some(delegation_id) = event
        .pointer("/delegation/id")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        tracing::warn!("voice: session.delegation.created had no delegation.id; cannot answer it");
        return;
    };

    if delegation_busy.swap(true, std::sync::atomic::Ordering::SeqCst) {
        let tx = commentary_tx.clone();
        tokio::spawn(async move {
            let _ = tx
                .send(commentary_event(
                    &delegation_id,
                    "Still working on the last request.",
                ))
                .await;
        });
        return;
    }

    let task_text = std::mem::take(pending_transcript);
    let state = state.clone();
    let user_id = user_id.to_string();
    let busy = delegation_busy.clone();
    let tx = commentary_tx.clone();
    let handle = tokio::spawn(async move {
        let _busy_guard = BusyGuard(busy);
        let answer = run_voice_delegation(&state, &user_id, &task_text).await;
        let _ = tx.send(commentary_event(&delegation_id, &answer)).await;
    });
    *delegation_task.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
}

/// Run the same paid agent loop text chat uses (`chat_paid::send_paid_reply`,
/// `voice_turn: true` so `Risk::Confirm` tools are withheld) for one
/// delegated voice request, charged in credits exactly like chat reuses that
/// same reservation/charge path. Never panics and never hangs the
/// delegation: every failure becomes a short spoken-friendly string instead
/// of being propagated.
///
/// Resolves its transport/signing key/spend limits from `state`/env, then
/// delegates to [`run_voice_delegation_with`] for the actual logic — kept
/// separate so tests can exercise that logic with an injected fake
/// transport instead of process-global env vars, matching `chat_paid.rs`'s
/// own test conventions.
async fn run_voice_delegation(state: &Arc<AppState>, user_id: &str, task_text: &str) -> String {
    let Some(db) = state.db.as_ref() else {
        return "Cortex is temporarily unavailable.".to_string();
    };
    let Some(provider_gateway_http::GatewayUsable {
        signing_key,
        supplier_key,
        transport,
    }) = provider_gateway_http::gateway_usable()
    else {
        return "Cortex is temporarily unavailable.".to_string();
    };
    let Some(limits) = SpendLimits::from_env() else {
        return "Cortex is temporarily unavailable.".to_string();
    };

    run_voice_delegation_with(
        db,
        &signing_key,
        &supplier_key,
        transport,
        limits,
        user_id,
        task_text,
    )
    .await
}

/// The transport-injectable core of [`run_voice_delegation`]. An empty (or
/// whitespace-only) `task_text` — the caption never accumulated anything
/// usable before the delegation arrived — is answered without ever calling
/// the agent loop, so there is no charge for it.
async fn run_voice_delegation_with<T: crate::provider_gateway::ProviderTransport + Clone>(
    db: &crate::db::Database,
    signing_key: &str,
    supplier_key: &str,
    transport: T,
    limits: SpendLimits,
    user_id: &str,
    task_text: &str,
) -> String {
    let task_text = task_text.trim();
    if task_text.is_empty() {
        return "I didn't catch that.".to_string();
    }

    let reply_id = uuid::Uuid::new_v4().to_string();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let turn_cap = crate::chat_paid::turn_cap_per_minute();

    match crate::chat_paid::send_paid_reply(
        db,
        signing_key,
        supplier_key,
        transport,
        limits,
        user_id,
        // Live voice sessions do not carry a conversation id today, so
        // there is nothing to save the delegated turn's messages against;
        // see the module notes on `VoiceSessionHandle`.
        None,
        crate::chat_paid::model_for_tier(None),
        VOICE_DELEGATION_SYSTEM_PROMPT,
        task_text,
        &reply_id,
        now_ms,
        turn_cap,
        None,
        true,
    )
    .await
    {
        Ok(reply) if reply.text.trim().is_empty() => "Done.".to_string(),
        Ok(reply) => reply.text,
        Err(crate::chat_paid::PaidReplyError::NoCredits)
        | Err(crate::chat_paid::PaidReplyError::NotEnoughCredits) => {
            "You're out of credits for that request.".to_string()
        }
        Err(crate::chat_paid::PaidReplyError::TurnCapExceeded) => {
            "This conversation is using tools too quickly right now. Please try again shortly."
                .to_string()
        }
        Err(_) => "Sorry, I couldn't complete that just now.".to_string(),
    }
}

/// Best effort: re-attach the sideband once and immediately ask the session
/// to close. Used both when the sideband drops mid-session (nothing left to
/// bill on, but the OpenAI session may still be running and metering) and
/// when the initial attach itself fails or times out after the OpenAI
/// session already started (same problem, at the very start of the call).
async fn reattach_and_close(ws_base: &str, session_id: &str, supplier_key: &str) {
    match tokio::time::timeout(
        SIDEBAND_ATTACH_TIMEOUT,
        WsSideband::attach(ws_base, session_id, supplier_key),
    )
    .await
    {
        Ok(Ok(mut reattached)) => {
            reattached
                .send(serde_json::json!({"type": "session.close"}))
                .await;
        }
        Ok(Err(detail)) => {
            tracing::error!(
                session_id,
                %detail,
                "voice: re-attach failed; could not send session.close"
            );
        }
        Err(_elapsed) => {
            tracing::error!(
                session_id,
                "voice: re-attach timed out; could not send session.close"
            );
        }
    }
}

/// Deducts the next `delta_credits` credits owed under `key`, on top of
/// whatever this session has already charged. `delta_credits` is a
/// cumulative-rounding delta (see [`settle_up_to`]), never a per-segment
/// `ceil_div`, so a multi-segment session is never charged more than
/// `ceil_div(total_micro, micros_per_credit)` in total.
///
/// On the session's final charge (`is_final`), `deduct_credits`'s
/// all-or-nothing behavior would otherwise let the very last, smallest
/// segment fail outright even though the user funded the whole session's
/// real cost — so the final charge instead reads the live balance and caps
/// itself to what remains, logging the shortfall rather than leaving that
/// last segment unresolved for free. Earlier, non-final charges keep the
/// strict all-or-nothing behavior: a shortfall there is real budget
/// exhaustion the caller must stop renewing against.
///
/// Returns `(credits actually charged, budget exhausted)`.
fn charge_incremental(
    db: &crate::db::Database,
    user_id: &str,
    session_id: &str,
    key: &str,
    description: &str,
    delta_credits: i64,
    is_final: bool,
) -> (i64, bool) {
    if delta_credits <= 0 {
        return (0, false);
    }
    let to_charge = if is_final {
        let available = db
            .get_credit_balance_row(user_id)
            .map(|balance| (balance.subscription_remaining + balance.pack_remaining).max(0))
            .unwrap_or(0);
        if delta_credits > available {
            tracing::error!(
                user_id,
                session_id,
                key,
                requested_credits = delta_credits,
                available_credits = available,
                shortfall_credits = delta_credits - available,
                "voice: final charge exceeds the user's remaining balance; charging what is left"
            );
        }
        delta_credits.min(available)
    } else {
        delta_credits
    };
    if to_charge <= 0 {
        return (0, !is_final);
    }
    match db.deduct_credits(user_id, to_charge, description, &ChargeKey::per_unit(key)) {
        Ok(_) => (to_charge, false),
        Err(error) => {
            tracing::error!(user_id, session_id, key, %error, "voice: credit deduction failed");
            (0, !is_final)
        }
    }
}

/// Settles every fully-consumed segment at the front of `segments` against
/// `observed_total_micro`, in order: a segment only ever settles for
/// exactly its own `reserved_micro` (never more — that is what used to let
/// a late-arriving usage jump land as a `mismatch`), except the final
/// segment on `is_final`, which settles for whatever is left, including
/// zero. Usage that still exceeds every reservation once `is_final` is true
/// is charged directly as an overrun rather than forced through settle.
///
/// Every charge (segment or overrun) is a delta against
/// `charged_credits_so_far`: `ceil_div(settled_so_far_micro,
/// micros_per_credit) - charged_credits_so_far`. That means rounding only
/// ever happens once, against the outstanding remainder, so a multi-segment
/// session's total charge is always exactly `ceil_div(total_micro,
/// micros_per_credit)` rather than the sum of several per-segment
/// `ceil_div`s (which could both overcharge and make the final,
/// often-partial segment's all-or-nothing deduction fail on its own
/// rounded-up credit even though the user funded the session's real cost).
///
/// Returns whether a non-final credit deduction failed during this call —
/// budget exhaustion the caller must stop renewing against (item D).
#[allow(clippy::too_many_arguments)]
fn settle_up_to(
    db: &crate::db::Database,
    user_id: &str,
    session_id: &str,
    local_id: &str,
    segments: &mut std::collections::VecDeque<Segment>,
    settled_so_far_micro: &mut i64,
    charged_credits_so_far: &mut i64,
    observed_total_micro: i64,
    is_final: bool,
    micros_per_credit: i64,
    now_ms: i64,
) -> bool {
    let mut budget_exhausted = false;
    while let Some(seg) = segments.front().copied() {
        let remaining = observed_total_micro - *settled_so_far_micro;
        let settle_amount = if remaining >= seg.reserved_micro {
            Some(seg.reserved_micro)
        } else if is_final {
            Some(remaining.max(0))
        } else {
            None
        };
        let Some(amount) = settle_amount else {
            break;
        };
        segments.pop_front();
        let key = format!("voice:{local_id}:{}", seg.index);
        match db.settle_provider_request(&key, amount, None, now_ms) {
            Ok(_) => {
                *settled_so_far_micro += amount;
                let credits_due = ceil_div(*settled_so_far_micro, micros_per_credit);
                let delta = credits_due - *charged_credits_so_far;
                let (charged, exhausted) = charge_incremental(
                    db,
                    user_id,
                    session_id,
                    &key,
                    "Cortex live voice",
                    delta,
                    is_final,
                );
                *charged_credits_so_far += charged;
                if exhausted {
                    budget_exhausted = true;
                }
            }
            Err(error) => {
                tracing::error!(
                    session_id,
                    segment_index = seg.index,
                    %error,
                    "voice: segment settle failed"
                );
                let _ = db.mark_provider_request_unresolved(
                    &key,
                    None,
                    "settle failed after usage update",
                    now_ms,
                );
            }
        }
    }

    if is_final {
        let remaining = observed_total_micro - *settled_so_far_micro;
        if remaining > 0 {
            let key = format!("voice:{local_id}:overrun");
            tracing::error!(
                user_id,
                session_id,
                remaining,
                "voice: usage exceeded every reservation; charging the excess directly rather than settling it"
            );
            *settled_so_far_micro = observed_total_micro;
            let credits_due = ceil_div(*settled_so_far_micro, micros_per_credit);
            let delta = credits_due - *charged_credits_so_far;
            let (charged, _exhausted) = charge_incremental(
                db,
                user_id,
                session_id,
                &key,
                "Cortex live voice overrun",
                delta,
                true,
            );
            *charged_credits_so_far += charged;
        }

        // A non-final charge earlier in this session can fail
        // (`charge_incremental` returns `charged: 0` on a deduction
        // error) while `settled_so_far_micro` already reflects the
        // settle that triggered it — the debt then lives only in the gap
        // between `ceil_div(settled_so_far_micro, micros_per_credit)` and
        // `charged_credits_so_far`. If nothing above just charged it (the
        // `remaining > 0` branch did not fire, because observed usage
        // never grew past what was already settled), this is the last
        // chance to retry it before the segment goes out of scope.
        let credits_due = ceil_div(*settled_so_far_micro, micros_per_credit);
        let delta = credits_due - *charged_credits_so_far;
        if delta > 0 {
            let key = format!("voice:{local_id}:final");
            let (charged, _exhausted) = charge_incremental(
                db,
                user_id,
                session_id,
                &key,
                "Cortex live voice",
                delta,
                true,
            );
            *charged_credits_so_far += charged;
        }
    }

    budget_exhausted
}

#[derive(Debug, Serialize, PartialEq)]
pub struct LiveSessionStartResponse {
    pub session_id: String,
    pub sdp: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct LiveSessionStartRequest {
    pub sdp: String,
}

pub async fn live_session_start(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    _headers: HeaderMap,
    Json(body): Json<LiveSessionStartRequest>,
) -> Result<Json<LiveSessionStartResponse>, (StatusCode, Json<ErrorResponse>)> {
    if let Some(blocked) = crate::billing::check_chat_access(&state, &user.user_id) {
        return Err((
            StatusCode::PAYMENT_REQUIRED,
            Json(ErrorResponse {
                error: format!("Subscription required to access chat. Status: {blocked:?}. Go to Settings → Billing to subscribe."),
            }),
        ));
    }
    let Some(mode) = live_voice_mode() else {
        return Err(LiveSessionError::Unavailable.into_response());
    };
    let Some(signing_key) = std::env::var("CORTEX_PROVIDER_GATEWAY_SIGNING_KEY")
        .ok()
        .filter(|key| key.len() >= 32)
    else {
        return Err(LiveSessionError::Unavailable.into_response());
    };
    let Some(limits) = SpendLimits::from_env() else {
        return Err(LiveSessionError::Unavailable.into_response());
    };
    let now_ms = chrono::Utc::now().timestamp_millis();

    start_session(
        &state,
        mode,
        OPENAI_HTTP_BASE,
        OPENAI_WS_BASE,
        &signing_key,
        limits,
        &user.user_id,
        &body.sdp,
        now_ms,
    )
    .await
    .map(|(session_id, sdp, _local_id)| Json(LiveSessionStartResponse { session_id, sdp }))
    .map_err(LiveSessionError::into_response)
}

pub async fn live_session_close(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(session_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    close_session(&state, &user.user_id, &session_id)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(LiveSessionError::into_response)
}

/// Registers the fake OpenAI Live endpoints this module's tests run against
/// — a real network loopback, not a mock, so the exact same
/// `OpenAiLiveSessions`/`WsSideband` production code paths are exercised.
/// Unused outside tests.
#[cfg(test)]
mod fake_live {
    use super::*;
    use axum::extract::ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade as WsUpgrade};
    use axum::response::IntoResponse;
    use axum::routing::{get, post};
    use axum::Router;
    use std::sync::Arc as StdArc;

    /// What one fake connection plays back after the sideband attaches.
    #[derive(Clone)]
    pub(super) struct FakeLiveScript {
        /// `usage.seconds` values sent, in order, right after attach.
        pub usage_events: Vec<i64>,
        /// Send `session.closed` (with the last scripted second) once the
        /// script is exhausted — a call that ends on its own.
        pub send_closed_after_script: bool,
        /// After the script (and no self-close), wait for the client's own
        /// `session.close` and answer it with `session.closed`. `false`
        /// drops the connection instead — a sideband failure.
        pub respond_to_client_close: bool,
        /// Refuse the WebSocket upgrade on `/attach` outright — exercises
        /// the "sideband attach failed after session start" path.
        pub refuse_attach: bool,
        /// Sleep this long right after sending the first usage event,
        /// giving a test room to mutate the database mid-script (e.g.
        /// draining a balance to exercise a failed settle) before the rest
        /// of the script arrives.
        pub pause_after_first_event: Option<std::time::Duration>,
        /// Sleep this long *before* completing the WebSocket upgrade on
        /// `/attach` — exercises the sideband-attach timeout path. `None`
        /// (the default) upgrades immediately.
        pub hang_attach_for: Option<std::time::Duration>,
        /// Counts every `/attach` upgrade this script has completed, so a
        /// test can tell a re-attach after a drop actually happened.
        pub attach_count: Option<StdArc<std::sync::atomic::AtomicUsize>>,
    }

    impl Default for FakeLiveScript {
        fn default() -> Self {
            Self {
                usage_events: Vec::new(),
                send_closed_after_script: false,
                respond_to_client_close: true,
                refuse_attach: false,
                pause_after_first_event: None,
                hang_attach_for: None,
                attach_count: None,
            }
        }
    }

    async fn fake_start(
        State(_script): State<StdArc<FakeLiveScript>>,
        Json(_body): Json<Value>,
    ) -> Json<Value> {
        let id = format!("live-fake-{}", uuid::Uuid::new_v4());
        Json(serde_json::json!({
            "session": {"id": id},
            "transport": {"type": "webrtc", "sdp": "fake-answer-sdp"},
        }))
    }

    async fn fake_attach(
        State(script): State<StdArc<FakeLiveScript>>,
        ws: WsUpgrade,
    ) -> axum::response::Response {
        if script.refuse_attach {
            return (StatusCode::FORBIDDEN, "attach refused").into_response();
        }
        if let Some(hang) = script.hang_attach_for {
            tokio::time::sleep(hang).await;
        }
        if let Some(counter) = &script.attach_count {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        ws.on_upgrade(move |socket| drive_fake_session(socket, (*script).clone()))
            .into_response()
    }

    async fn drive_fake_session(mut socket: WebSocket, script: FakeLiveScript) {
        let mut last_seconds = 0i64;
        for (index, seconds) in script.usage_events.iter().enumerate() {
            last_seconds = *seconds;
            let message = serde_json::json!({
                "type": "session.usage.updated",
                "usage": {"seconds": seconds},
            });
            if socket
                .send(AxumMessage::Text(message.to_string().into()))
                .await
                .is_err()
            {
                return;
            }
            if index == 0 {
                if let Some(pause) = script.pause_after_first_event {
                    tokio::time::sleep(pause).await;
                }
            }
        }
        if script.send_closed_after_script {
            let message = serde_json::json!({
                "type": "session.closed",
                "usage": {"seconds": last_seconds},
                "reason": "done",
            });
            let _ = socket
                .send(AxumMessage::Text(message.to_string().into()))
                .await;
            return;
        }
        if script.respond_to_client_close {
            while let Some(Ok(AxumMessage::Text(text))) = socket.recv().await {
                if text.contains("session.close") {
                    let message = serde_json::json!({
                        "type": "session.closed",
                        "usage": {"seconds": last_seconds},
                        "reason": "client_close",
                    });
                    let _ = socket
                        .send(AxumMessage::Text(message.to_string().into()))
                        .await;
                    return;
                }
            }
        }
        // Otherwise: drop the connection with no `session.closed` at all.
    }

    /// Starts a fake OpenAI Live server on a loopback port and returns
    /// `(http_base, ws_base)` ready for [`super::start_session`].
    pub(super) async fn spawn(script: FakeLiveScript) -> (String, String) {
        let script = StdArc::new(script);
        let app = Router::new()
            .route("/v1/live/sessions", post(fake_start))
            .route("/v1/live/sessions/{id}/attach", get(fake_attach))
            .with_state(script);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}"), format!("ws://{address}"))
    }
}

#[cfg(test)]
mod tests {
    use super::fake_live::{spawn, FakeLiveScript};
    use super::*;
    use std::time::Duration as StdDuration;

    const USER: &str = "user-1";
    const SIGNING_KEY: &str = "0123456789abcdef0123456789abcdef";

    async fn test_state() -> (tempfile::TempDir, Arc<AppState>) {
        test_state_with_balance(1_000_000_000).await
    }

    /// A fresh `AppState` with exactly `credits` on the user's balance —
    /// `init_credit_balance` only ever inserts, so callers that need a
    /// specific balance must reach for this instead of `test_state()`.
    async fn test_state_with_balance(credits: i64) -> (tempfile::TempDir, Arc<AppState>) {
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

    fn ample_limits() -> SpendLimits {
        SpendLimits {
            max_micro_usd: 1_000_000_000,
            funded_micro_usd: 1_000_000_000,
        }
    }

    #[test]
    fn live_session_config_requests_client_delegation() {
        let config = live_session_config();
        assert_eq!(config["model"], LIVE_MODEL);
        assert_eq!(config["delegation"]["type"], "client");
    }

    #[tokio::test]
    async fn stub_mode_never_touches_the_ledger_or_the_map() {
        let (_dir, state) = test_state().await;
        let balance_before = state.db.as_ref().unwrap().get_credit_balance_row(USER);

        let (session_id, sdp, _local_id) = start_session(
            &state,
            LiveVoiceMode::Stub,
            OPENAI_HTTP_BASE,
            OPENAI_WS_BASE,
            SIGNING_KEY,
            ample_limits(),
            USER,
            "offer-sdp",
            0,
        )
        .await
        .expect("stub start must succeed");

        assert!(session_id.starts_with("stub-live-"));
        assert_eq!(sdp, "stub-answer-sdp");
        assert_eq!(
            state.db.as_ref().unwrap().get_credit_balance_row(USER),
            balance_before
        );
        assert!(state.voice_sessions.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_scripted_650_second_session_settles_three_segments_and_leaves_nothing_open() {
        let (_dir, state) = test_state().await;
        let (http_base, ws_base) = spawn(FakeLiveScript {
            usage_events: vec![100, 240, 300, 480, 600, 650],
            send_closed_after_script: true,
            respond_to_client_close: true,
            refuse_attach: false,
            pause_after_first_event: None,
            ..Default::default()
        })
        .await;

        // A real timestamp, not a fixed 0: `run_billing_loop`'s renewal
        // reservations check the authorization's expiry against the real
        // wall clock (`chrono::Utc::now()`), same as production does, so a
        // multi-segment test needs the authorization's `now_ms` on the same
        // clock or every renewal past segment 0 is rejected as "expired".
        let (session_id, _sdp, local_id) = start_session(
            &state,
            LiveVoiceMode::Live("sk-test-supplier-0123456789".into()),
            &http_base,
            &ws_base,
            SIGNING_KEY,
            ample_limits(),
            USER,
            "offer-sdp",
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .expect("live start against the fake must succeed");

        // The billing task runs in the background; give it a moment to drain
        // the whole scripted conversation and settle the final segment.
        wait_until_session_gone(&state, &session_id).await;

        let db = state.db.as_ref().unwrap();
        let rate = db
            .active_price_list()
            .unwrap()
            .model("openai", "gpt-live-1")
            .cloned()
            .unwrap();
        let expected_total_micro = rate.cost_micros(650, 0, 0);
        let balance = db.get_credit_balance_row(USER).unwrap();
        let spent_credits = 1_000_000_000 - balance.subscription_remaining;
        // Charged on the cumulative-rounding rule (a running
        // `ceil_div(settled_total, micros_per_credit) - charged_so_far`
        // delta), so a multi-segment session is charged exactly the
        // whole-session ceiling — never the sum of three separate
        // per-segment `ceil_div` rounds, which used to overcharge.
        let expected_credits = ceil_div(
            expected_total_micro,
            db.active_price_list().unwrap().micros_per_credit,
        );
        assert_eq!(
            spent_credits, expected_credits,
            "spent {spent_credits} credits, expected exactly {expected_credits}"
        );

        for n in 0..3 {
            let key = format!("voice:{local_id}:{n}");
            let reservation = db
                .get_provider_reservation(&key)
                .expect("reservation exists");
            assert_eq!(reservation.status, "settled", "segment {n} must be settled");
        }
        assert!(
            db.get_provider_reservation(&format!("voice:{local_id}:3"))
                .is_none(),
            "no fourth segment should have been opened"
        );
    }

    #[tokio::test]
    async fn a_balance_of_two_minutes_closes_the_session_well_before_240_scripted_seconds() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(
            dir.path().join(".cortex/ledger.jsonl"),
            dir.path().to_path_buf(),
            None,
        )
        .await;
        let db = state.db.as_ref().unwrap();
        let price_list = db.active_price_list().unwrap();
        let rate = price_list.model("openai", "gpt-live-1").cloned().unwrap();
        // ~2 minutes of gpt-live-1.
        let two_minutes_micro = rate.cost_micros(120, 0, 0);
        let two_minutes_credits = ceil_div(two_minutes_micro, price_list.micros_per_credit);
        db.init_credit_balance(USER, two_minutes_credits).unwrap();
        let _dir = dir;

        let (http_base, ws_base) = spawn(FakeLiveScript {
            usage_events: vec![80, 100, 120, 150, 200],
            send_closed_after_script: false,
            respond_to_client_close: true,
            refuse_attach: false,
            pause_after_first_event: None,
            ..Default::default()
        })
        .await;

        let (session_id, _sdp, local_id) = start_session(
            &state,
            LiveVoiceMode::Live("sk-test-supplier-0123456789".into()),
            &http_base,
            &ws_base,
            SIGNING_KEY,
            ample_limits(),
            USER,
            "offer-sdp",
            0,
        )
        .await
        .expect("live start must succeed even on a small balance");

        // The billing loop's warning grace is real wall-clock time; give it
        // room to fire and the fake time to answer session.closed.
        wait_until_session_gone_with_timeout(&state, &session_id, StdDuration::from_secs(30)).await;

        let db = state.db.as_ref().unwrap();
        let reservation = db
            .get_provider_reservation(&format!("voice:{local_id}:0"))
            .expect("segment 0 exists");
        assert_eq!(reservation.status, "settled");
        // segment 0's full reservation (120s) is exactly what it settles
        // for; usage that arrived after the session was told to close
        // (up to 200s) is charged as an overrun, not folded into the
        // segment settle.
        assert_eq!(
            reservation.observed_micro_usd.unwrap(),
            rate.cost_micros(120, 0, 0)
        );
        assert!(
            db.get_provider_reservation(&format!("voice:{local_id}:1"))
                .is_none(),
            "the balance could not fund a second segment"
        );
    }

    #[tokio::test]
    async fn a_dropped_sideband_charges_the_observed_remainder() {
        // Neither scripted event (50s, 100s) ever crosses the 80% renewal
        // threshold or the segment boundary, so segment 0 never settles —
        // it is only the drop-cleanup path's direct `:drop` charge that
        // must bill for the 100s actually observed.
        let (_dir, state) = test_state().await;
        let (http_base, ws_base) = spawn(FakeLiveScript {
            usage_events: vec![50, 100],
            send_closed_after_script: false,
            respond_to_client_close: false,
            refuse_attach: false,
            pause_after_first_event: None,
            ..Default::default()
        })
        .await;

        let (session_id, _sdp, local_id) = start_session(
            &state,
            LiveVoiceMode::Live("sk-test-supplier-0123456789".into()),
            &http_base,
            &ws_base,
            SIGNING_KEY,
            ample_limits(),
            USER,
            "offer-sdp",
            0,
        )
        .await
        .expect("live start must succeed");

        wait_until_session_gone(&state, &session_id).await;

        let db = state.db.as_ref().unwrap();
        let price_list = db.active_price_list().unwrap();
        let rate = price_list.model("openai", "gpt-live-1").cloned().unwrap();
        let reservation = db
            .get_provider_reservation(&format!("voice:{local_id}:0"))
            .expect("segment 0 exists");
        assert_eq!(reservation.status, "unresolved");
        assert_eq!(
            db.provider_spend_row_count(&format!("voice:{local_id}:0")),
            0
        );

        let expected_credits = ceil_div(rate.cost_micros(100, 0, 0), price_list.micros_per_credit);
        // 100s = 83_333 micro-USD at this test's rate, ceil-divided by
        // 100_000 micros/credit = 1 credit; pinned literally so a change to
        // `rate` or `ceil_div` that silently zeroed the delta cannot make
        // this test pass by agreeing with itself.
        assert_eq!(expected_credits, 1);
        let balance = db.get_credit_balance_row(USER).unwrap();
        let spent_credits = 1_000_000_000 - balance.subscription_remaining;
        assert_eq!(spent_credits, expected_credits);

        let drop_key_prefix = format!("voice:{local_id}:drop%");
        let ledger_rows: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM credit_transactions WHERE idempotency_key LIKE ?1",
                [&drop_key_prefix],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            ledger_rows >= 1,
            "the observed remainder must be charged directly on drop"
        );
    }

    #[tokio::test]
    async fn a_dropped_sideband_after_850_seconds_settles_two_segments_and_charges_the_rest() {
        // Crosses the 80% renewal threshold enough times to reserve four
        // 300s segments, but only ever reports usage through 850s: segments
        // 0 (0-300s) and 1 (300-600s) are fully consumed and settle inline;
        // segment 2 (600-900s) is only partially observed (250s of its
        // 300s) so it never settles, and segment 3 (900-1200s) is reserved
        // with no usage at all. The drop then charges the remainder beyond
        // what settled and leaves both open segments unresolved.
        let (_dir, state) = test_state().await;
        let (http_base, ws_base) = spawn(FakeLiveScript {
            usage_events: vec![240, 480, 720, 850],
            send_closed_after_script: false,
            respond_to_client_close: false,
            refuse_attach: false,
            pause_after_first_event: None,
            ..Default::default()
        })
        .await;

        let (session_id, _sdp, local_id) = start_session(
            &state,
            LiveVoiceMode::Live("sk-test-supplier-0123456789".into()),
            &http_base,
            &ws_base,
            SIGNING_KEY,
            ample_limits(),
            USER,
            "offer-sdp",
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .expect("live start must succeed");

        wait_until_session_gone(&state, &session_id).await;

        let db = state.db.as_ref().unwrap();
        let price_list = db.active_price_list().unwrap();
        let rate = price_list.model("openai", "gpt-live-1").cloned().unwrap();

        for n in 0..2 {
            let reservation = db
                .get_provider_reservation(&format!("voice:{local_id}:{n}"))
                .expect("reservation exists");
            assert_eq!(reservation.status, "settled", "segment {n} must be settled");
        }
        for n in 2..4 {
            let reservation = db
                .get_provider_reservation(&format!("voice:{local_id}:{n}"))
                .expect("reservation exists");
            assert_eq!(
                reservation.status, "unresolved",
                "segment {n} must be left unresolved by the drop"
            );
        }

        let expected_credits = ceil_div(rate.cost_micros(850, 0, 0), price_list.micros_per_credit);
        // 850s = 708_333 micro-USD at this test's rate, ceil-divided by
        // 100_000 micros/credit = 8 credits; pinned literally so a change to
        // `rate` or `ceil_div` that silently under-charges cannot make this
        // test pass by agreeing with itself.
        assert_eq!(expected_credits, 8);
        let balance = db.get_credit_balance_row(USER).unwrap();
        let spent_credits = 1_000_000_000 - balance.subscription_remaining;
        assert_eq!(spent_credits, expected_credits);

        let drop_key_prefix = format!("voice:{local_id}:drop%");
        let ledger_rows: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM credit_transactions WHERE idempotency_key LIKE ?1",
                [&drop_key_prefix],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            ledger_rows >= 1,
            "the observed remainder beyond the settled segments must be charged on drop"
        );
    }

    #[tokio::test]
    async fn cumulative_rounding_charges_exactly_the_ceiling_and_settles_the_last_segment() {
        // Same scripted session as the 650s multi-segment test, but the
        // balance is funded for *exactly* the session's real cost instead
        // of an ample amount. Under the old per-segment `ceil_div` rounding
        // each of the three segments could round up independently, so the
        // final (often partial) segment's all-or-nothing deduction failed
        // even though the user funded the whole session's real cost. The
        // cumulative-delta rule must charge exactly the ceiling and settle
        // every segment, including the last one.
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(
            dir.path().join(".cortex/ledger.jsonl"),
            dir.path().to_path_buf(),
            None,
        )
        .await;
        let db = state.db.as_ref().unwrap();
        let price_list = db.active_price_list().unwrap();
        let rate = price_list.model("openai", "gpt-live-1").cloned().unwrap();
        let expected_total_micro = rate.cost_micros(650, 0, 0);
        let expected_credits = ceil_div(expected_total_micro, price_list.micros_per_credit);
        db.init_credit_balance(USER, expected_credits).unwrap();
        let _dir = dir;

        let (http_base, ws_base) = spawn(FakeLiveScript {
            usage_events: vec![100, 240, 300, 480, 600, 650],
            send_closed_after_script: true,
            respond_to_client_close: true,
            refuse_attach: false,
            pause_after_first_event: None,
            ..Default::default()
        })
        .await;

        let (session_id, _sdp, local_id) = start_session(
            &state,
            LiveVoiceMode::Live("sk-test-supplier-0123456789".into()),
            &http_base,
            &ws_base,
            SIGNING_KEY,
            ample_limits(),
            USER,
            "offer-sdp",
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .expect("live start must succeed on a tightly-funded balance");

        wait_until_session_gone(&state, &session_id).await;

        let balance = db.get_credit_balance_row(USER).unwrap();
        let spent_credits = expected_credits - balance.subscription_remaining;
        assert_eq!(
            spent_credits, expected_credits,
            "must charge exactly the ceiling of the whole session's cost, not a per-segment overcharge"
        );

        for n in 0..3 {
            let key = format!("voice:{local_id}:{n}");
            let reservation = db
                .get_provider_reservation(&key)
                .expect("reservation exists");
            assert_eq!(
                reservation.status, "settled",
                "segment {n} must be settled, including the last one — it must not be skipped \
                 or left unresolved because an earlier segment's per-segment rounding ate the \
                 whole tightly-funded balance"
            );
        }
    }

    #[tokio::test]
    async fn a_hung_sideband_attach_times_out_frees_the_placeholder_and_does_not_wedge_later_starts(
    ) {
        let (_dir, state) = test_state().await;
        let (http_base, ws_base) = spawn(FakeLiveScript {
            usage_events: vec![],
            send_closed_after_script: false,
            respond_to_client_close: false,
            refuse_attach: false,
            pause_after_first_event: None,
            hang_attach_for: Some(StdDuration::from_secs(20)),
            ..Default::default()
        })
        .await;

        let error = start_session(
            &state,
            LiveVoiceMode::Live("sk-test-supplier-0123456789".into()),
            &http_base,
            &ws_base,
            SIGNING_KEY,
            ample_limits(),
            USER,
            "offer-sdp",
            0,
        )
        .await
        .expect_err("an attach that hangs past the timeout must fail start_session");
        assert_eq!(error, LiveSessionError::SupplierFailed);

        // The per-user placeholder must be gone — otherwise a disconnect
        // during a hung attach would wedge the user with a permanent 409.
        assert!(state.voice_sessions.lock().unwrap().is_empty());

        let db = state.db.as_ref().unwrap();
        let request_key: String = db
            .conn()
            .query_row(
                "SELECT request_key FROM provider_request_reservations \
                 ORDER BY created_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("the pre-start reservation was persisted");
        let reservation = db
            .get_provider_reservation(&request_key)
            .expect("reservation exists");
        assert_eq!(reservation.status, "unresolved");

        // A second start for the same user must not be refused with 409 —
        // the placeholder from the hung attach must already be gone.
        let (http_base2, ws_base2) = spawn(FakeLiveScript {
            usage_events: vec![],
            send_closed_after_script: false,
            respond_to_client_close: true,
            refuse_attach: false,
            pause_after_first_event: None,
            ..Default::default()
        })
        .await;
        let (session_id, _sdp, _local_id) = start_session(
            &state,
            LiveVoiceMode::Live("sk-test-supplier-0123456789".into()),
            &http_base2,
            &ws_base2,
            SIGNING_KEY,
            ample_limits(),
            USER,
            "offer-sdp",
            0,
        )
        .await
        .expect("a later start for the same user must not be 409'd by the hung attempt");

        close_session(&state, USER, &session_id).await.unwrap();
    }

    #[tokio::test]
    async fn a_client_disconnect_during_a_hung_attach_frees_the_placeholder_and_does_not_wedge_later_starts(
    ) {
        let (_dir, state) = test_state().await;
        let (http_base, ws_base) = spawn(FakeLiveScript {
            usage_events: vec![],
            send_closed_after_script: false,
            respond_to_client_close: false,
            refuse_attach: false,
            pause_after_first_event: None,
            hang_attach_for: Some(StdDuration::from_secs(20)),
            ..Default::default()
        })
        .await;

        // Simulate a real client disconnect while the sideband attach is
        // still hung: await `start_session` itself only briefly, then
        // drop that future. Per `start_session`'s own doc comment, the
        // pipeline runs on its own spawned task and is NOT cancelled by
        // this — it keeps running to its own internal
        // `SIDEBAND_ATTACH_TIMEOUT` and must clean up the per-user
        // placeholder and mark the reservation unresolved on its own,
        // with nothing left awaiting its result at all.
        let disconnected = tokio::time::timeout(
            StdDuration::from_millis(100),
            start_session(
                &state,
                LiveVoiceMode::Live("sk-test-supplier-0123456789".into()),
                &http_base,
                &ws_base,
                SIGNING_KEY,
                ample_limits(),
                USER,
                "offer-sdp",
                0,
            ),
        )
        .await;
        assert!(
            disconnected.is_err(),
            "the 100ms wrapper must itself time out before the ~15s internal attach timeout — \
             otherwise this test is not exercising a disconnect at all"
        );

        // Poll (bounded) for the background pipeline to finish cleaning
        // up on its own: the per-user placeholder gone from
        // `state.voice_sessions`.
        let deadline = tokio::time::Instant::now() + StdDuration::from_secs(30);
        loop {
            if state.voice_sessions.lock().unwrap().is_empty() {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "placeholder was never freed after the client disconnected during a hung attach"
            );
            tokio::time::sleep(StdDuration::from_millis(50)).await;
        }

        let db = state.db.as_ref().unwrap();
        let request_key: String = db
            .conn()
            .query_row(
                "SELECT request_key FROM provider_request_reservations \
                 ORDER BY created_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("the pre-start reservation was persisted");
        assert!(
            request_key.ends_with(":0"),
            "segment 0's own reservation must be the one left unresolved"
        );
        let reservation = db
            .get_provider_reservation(&request_key)
            .expect("reservation exists");
        assert_eq!(reservation.status, "unresolved");

        // A later start for the same user must not be refused with 409 —
        // the placeholder must already be gone even though nothing ever
        // awaited the disconnected pipeline's own result.
        let (http_base2, ws_base2) = spawn(FakeLiveScript {
            usage_events: vec![],
            send_closed_after_script: false,
            respond_to_client_close: true,
            refuse_attach: false,
            pause_after_first_event: None,
            ..Default::default()
        })
        .await;
        let (session_id, _sdp, _local_id) = start_session(
            &state,
            LiveVoiceMode::Live("sk-test-supplier-0123456789".into()),
            &http_base2,
            &ws_base2,
            SIGNING_KEY,
            ample_limits(),
            USER,
            "offer-sdp",
            0,
        )
        .await
        .expect("a later start for the same user must not be 409'd by the disconnected attempt");

        close_session(&state, USER, &session_id).await.unwrap();
    }

    #[tokio::test]
    async fn a_dropped_sideband_with_usage_past_every_reservation_charges_the_overrun_and_reattaches_to_close(
    ) {
        // A tight max_micro_usd budget of exactly one segment: the second
        // scripted event lands on the cap and gets no further renewal, and
        // the third event reports usage past every reservation this session
        // ever made, then the fake drops the connection without a
        // `session.closed`. The drop-cleanup path must charge that excess
        // directly as an overrun and attempt one re-attach to send
        // `session.close` rather than leaving the upstream session running
        // unmetered.
        let (_dir, state) = test_state().await;
        let db = state.db.as_ref().unwrap();
        let price_list = db.active_price_list().unwrap();
        let rate = price_list.model("openai", "gpt-live-1").cloned().unwrap();
        let one_segment_micro = rate.cost_micros(SEGMENT_SECONDS, 0, 0);
        let limits = SpendLimits {
            max_micro_usd: one_segment_micro,
            funded_micro_usd: one_segment_micro,
        };

        let attach_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (http_base, ws_base) = spawn(FakeLiveScript {
            // 450s = 375_000 micro-USD at this test's rate = 4 credits, so
            // the drop-cleanup charge is provably nonzero (300s and 330s
            // both round up to 3 credits and would make the delta 0).
            usage_events: vec![80, SEGMENT_SECONDS, SEGMENT_SECONDS + 150],
            send_closed_after_script: false,
            respond_to_client_close: false,
            refuse_attach: false,
            pause_after_first_event: None,
            attach_count: Some(attach_count.clone()),
            ..Default::default()
        })
        .await;

        let (session_id, _sdp, local_id) = start_session(
            &state,
            LiveVoiceMode::Live("sk-test-supplier-0123456789".into()),
            &http_base,
            &ws_base,
            SIGNING_KEY,
            limits,
            USER,
            "offer-sdp",
            0,
        )
        .await
        .expect("live start must succeed");

        wait_until_session_gone_with_timeout(&state, &session_id, StdDuration::from_secs(30)).await;

        // Segment 0 (the only reservation this cap could ever fund) settles
        // for its full reserved amount; the extra 150s reported on top of it
        // is charged directly as an overrun.
        let reservation = db
            .get_provider_reservation(&format!("voice:{local_id}:0"))
            .expect("segment 0 exists");
        assert_eq!(reservation.status, "settled");

        // The overrun charge goes straight through `deduct_credits` — it
        // never creates a `provider_request_reservations` row, so
        // `provider_spend_row_count` (which joins on that table) can only
        // ever read 0 for it. Count the ledger rows it actually writes
        // instead: `deduct_credits` fans one idempotency key out into a
        // `:subscription`/`:pack` suffixed row per bucket it draws from,
        // so match on the prefix.
        let drop_key_prefix = format!("voice:{local_id}:drop%");
        let ledger_rows: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM credit_transactions WHERE idempotency_key LIKE ?1",
                [&drop_key_prefix],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            ledger_rows >= 1,
            "usage past every reservation must be charged directly on drop"
        );

        let expected_total_micro = rate.cost_micros(SEGMENT_SECONDS + 150, 0, 0);
        let expected_credits = ceil_div(expected_total_micro, price_list.micros_per_credit);
        // 450s = 375_000 micro-USD at this test's price = 4 credits;
        // pinned literally so a change to `rate` or `ceil_div` that
        // silently zeroed the delta cannot make this test pass by
        // agreeing with itself.
        assert_eq!(expected_credits, 4);
        let balance = db.get_credit_balance_row(USER).unwrap();
        let spent_credits = 1_000_000_000 - balance.subscription_remaining;
        assert_eq!(spent_credits, expected_credits);

        // The drop-cleanup path must have tried a re-attach to send
        // `session.close` on top of the original attach.
        assert!(
            attach_count.load(std::sync::atomic::Ordering::SeqCst) >= 2,
            "must attempt a re-attach after the sideband drops to close the upstream session"
        );
    }

    #[tokio::test]
    async fn a_non_owner_close_is_refused() {
        let (_dir, state) = test_state().await;
        let (http_base, ws_base) = spawn(FakeLiveScript {
            usage_events: vec![10],
            send_closed_after_script: false,
            respond_to_client_close: true,
            refuse_attach: false,
            pause_after_first_event: None,
            ..Default::default()
        })
        .await;

        let (session_id, _sdp, _local_id) = start_session(
            &state,
            LiveVoiceMode::Live("sk-test-supplier-0123456789".into()),
            &http_base,
            &ws_base,
            SIGNING_KEY,
            ample_limits(),
            USER,
            "offer-sdp",
            0,
        )
        .await
        .expect("live start must succeed");

        let error = close_session(&state, "someone-else", &session_id)
            .await
            .expect_err("a non-owner close must be refused");
        assert_eq!(error, LiveSessionError::Forbidden);

        // Clean up: the owner can still close it.
        close_session(&state, USER, &session_id).await.unwrap();
    }

    #[tokio::test]
    async fn a_refused_sideband_attach_leaves_segment_zero_unresolved_not_reserved() {
        let (_dir, state) = test_state().await;
        let (http_base, ws_base) = spawn(FakeLiveScript {
            usage_events: vec![],
            send_closed_after_script: false,
            respond_to_client_close: false,
            refuse_attach: true,
            pause_after_first_event: None,
            ..Default::default()
        })
        .await;

        let error = start_session(
            &state,
            LiveVoiceMode::Live("sk-test-supplier-0123456789".into()),
            &http_base,
            &ws_base,
            SIGNING_KEY,
            ample_limits(),
            USER,
            "offer-sdp",
            0,
        )
        .await
        .expect_err("a refused attach must fail start_session");
        assert_eq!(error, LiveSessionError::SupplierFailed);

        // The failed attempt's own reservation is the only row in the
        // table; its key carries the server-generated local id, which the
        // caller never sees on an error path, so recover it from the row
        // itself.
        let db = state.db.as_ref().unwrap();
        let request_key: String = db
            .conn()
            .query_row(
                "SELECT request_key FROM provider_request_reservations \
                 ORDER BY created_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("the pre-start reservation was persisted");
        let reservation = db
            .get_provider_reservation(&request_key)
            .expect("reservation exists");
        assert_eq!(
            reservation.status, "unresolved",
            "segment 0 must be marked unresolved, not left dangling as reserved"
        );
        assert!(state.voice_sessions.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_second_start_for_the_same_user_is_refused_with_409() {
        let (_dir, state) = test_state().await;
        let (http_base, ws_base) = spawn(FakeLiveScript {
            usage_events: vec![],
            send_closed_after_script: false,
            respond_to_client_close: true,
            refuse_attach: false,
            pause_after_first_event: None,
            ..Default::default()
        })
        .await;

        let (session_id, _sdp, _local_id) = start_session(
            &state,
            LiveVoiceMode::Live("sk-test-supplier-0123456789".into()),
            &http_base,
            &ws_base,
            SIGNING_KEY,
            ample_limits(),
            USER,
            "offer-sdp",
            0,
        )
        .await
        .expect("first live start must succeed");

        let db = state.db.as_ref().unwrap();
        let reservations_before = db.conn().query_row(
            "SELECT COUNT(*) FROM provider_request_reservations",
            [],
            |row| row.get::<_, i64>(0),
        );

        let error = start_session(
            &state,
            LiveVoiceMode::Live("sk-test-supplier-0123456789".into()),
            &http_base,
            &ws_base,
            SIGNING_KEY,
            ample_limits(),
            USER,
            "offer-sdp",
            0,
        )
        .await
        .expect_err("a second start for the same user must be refused");
        assert_eq!(error, LiveSessionError::AlreadyOpen);

        let reservations_after = db.conn().query_row(
            "SELECT COUNT(*) FROM provider_request_reservations",
            [],
            |row| row.get::<_, i64>(0),
        );
        assert_eq!(
            reservations_before, reservations_after,
            "the refused second start must not have reserved anything"
        );
        assert_eq!(state.voice_sessions.lock().unwrap().len(), 1);

        close_session(&state, USER, &session_id).await.unwrap();
    }

    #[tokio::test]
    async fn a_settle_that_cannot_charge_credits_closes_the_session() {
        // Look up the segment cost against a throwaway state first, since
        // the real state's balance must be initialized to exactly that
        // amount in one `init_credit_balance` call — it only ever inserts,
        // so a second call for the same user is a no-op.
        let price_probe = test_state().await.1;
        let price_list = price_probe
            .db
            .as_ref()
            .unwrap()
            .active_price_list()
            .unwrap();
        let rate = price_list.model("openai", "gpt-live-1").cloned().unwrap();
        let segment_micro = rate.cost_micros(SEGMENT_SECONDS, 0, 0);
        let segment_credits = ceil_div(segment_micro, price_list.micros_per_credit);

        // Exactly enough to fund (and later settle) segment 0 — until the
        // test drains it mid-script.
        let (_dir, state) = test_state_with_balance(segment_credits).await;
        let db = state.db.as_ref().unwrap();

        // A harmless first event (below the 80% renewal threshold), a real
        // pause for the test to drain the balance, then a second event that
        // lands exactly on the segment boundary and forces a full settle.
        let (http_base, ws_base) = spawn(FakeLiveScript {
            usage_events: vec![10, SEGMENT_SECONDS],
            send_closed_after_script: false,
            respond_to_client_close: true,
            refuse_attach: false,
            pause_after_first_event: Some(std::time::Duration::from_millis(300)),
            ..Default::default()
        })
        .await;

        let (session_id, _sdp, local_id) = start_session(
            &state,
            LiveVoiceMode::Live("sk-test-supplier-0123456789".into()),
            &http_base,
            &ws_base,
            SIGNING_KEY,
            ample_limits(),
            USER,
            "offer-sdp",
            0,
        )
        .await
        .expect("live start must succeed with exactly enough balance for segment 0");

        // Give the fake time to send the first event, then drain the
        // balance to zero before it sends the segment-crossing one.
        tokio::time::sleep(StdDuration::from_millis(100)).await;
        let balance = db.get_credit_balance_row(USER).unwrap();
        let drainable = balance.subscription_remaining + balance.pack_remaining;
        db.deduct_credits(
            USER,
            drainable,
            "test drain",
            &cortex_core::billing_binding::ChargeKey::per_unit("test:drain"),
        )
        .expect("draining the balance directly must succeed");

        // Give the segment-crossing event time to arrive and the settle it
        // triggers time to fail against the drained balance (entering the
        // warning-grace/goodbye path), then top the balance back up — still
        // well inside the 20s `WARNING_GRACE` window — the same way the
        // ledger's own tests fund a balance (`add_pack_credits`). This
        // proves the goodbye path's `session.closed` retry (the `:final`
        // charge in `settle_up_to`) actually recovers a settle that failed
        // only because credits were briefly unavailable, not because the
        // session was over budget.
        tokio::time::sleep(StdDuration::from_millis(500)).await;
        db.add_pack_credits(USER, segment_credits)
            .expect("topping the balance back up must succeed");

        // The failed deduction stops renewal and heads for the goodbye
        // path, which needs the full warning grace before it closes.
        wait_until_session_gone_with_timeout(&state, &session_id, StdDuration::from_secs(40)).await;

        let final_key_prefix = format!("voice:{local_id}:final%");
        let final_rows: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM credit_transactions WHERE idempotency_key LIKE ?1",
                [&final_key_prefix],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            final_rows >= 1,
            "the retried settle must charge under the :final key once credits are available again"
        );

        // Total spent, excluding the test's own direct drain, must be
        // exactly the one segment's cost — the retried settle must not
        // double-charge on top of what `charged_credits_so_far` already
        // tracked as owed.
        let balance = db.get_credit_balance_row(USER).unwrap();
        let remaining = balance.subscription_remaining + balance.pack_remaining;
        // Funded in total: `segment_credits` at `test_state_with_balance`,
        // plus `segment_credits` from the top-up above.
        let funded = segment_credits + segment_credits;
        let spent_excluding_drain = funded - remaining - drainable;
        assert_eq!(spent_excluding_drain, segment_credits);
    }

    #[test]
    fn the_response_never_contains_a_key() {
        let response = LiveSessionStartResponse {
            session_id: "live_123".into(),
            sdp: "v=0...".into(),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(!json.contains("sk-"));
    }

    async fn wait_until_session_gone(state: &Arc<AppState>, session_id: &str) {
        wait_until_session_gone_with_timeout(state, session_id, StdDuration::from_secs(10)).await
    }

    async fn wait_until_session_gone_with_timeout(
        state: &Arc<AppState>,
        session_id: &str,
        timeout: StdDuration,
    ) {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if !state
                .voice_sessions
                .lock()
                .unwrap()
                .contains_key(session_id)
            {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("voice session {session_id} did not finish within {timeout:?}");
            }
            tokio::time::sleep(StdDuration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn startup_sweep_marks_a_reserved_voice_reservation_unresolved() {
        // Simulates a server restart while a live session's segment was
        // still `reserved`: nothing ever marks it terminal because the
        // billing task and its drop-cleanup both died with the old
        // process, so the startup sweep is the only thing that reconciles
        // it.
        let (_dir, state) = test_state().await;
        // `respond_to_client_close: true` here (unlike the drop tests) is
        // load-bearing: with `false` the fake has nothing left to do once
        // its empty `usage_events` script runs out and no self-close is
        // scripted, so it drops the connection immediately after attach —
        // the sideband-drop cleanup then races the test's own `before`
        // check and can mark segment 0 `unresolved` before the test ever
        // gets to sweep it, which is not what a live "reserved" segment at
        // restart looks like. Waiting on the client's own `session.close`
        // (never sent here) keeps the sideband open and the reservation
        // `reserved` until the test explicitly sweeps it.
        let (http_base, ws_base) = spawn(FakeLiveScript {
            usage_events: vec![],
            send_closed_after_script: false,
            respond_to_client_close: true,
            refuse_attach: false,
            pause_after_first_event: None,
            ..Default::default()
        })
        .await;

        let (_session_id, _sdp, local_id) = start_session(
            &state,
            LiveVoiceMode::Live("sk-test-supplier-0123456789".into()),
            &http_base,
            &ws_base,
            SIGNING_KEY,
            ample_limits(),
            USER,
            "offer-sdp",
            0,
        )
        .await
        .expect("live start must succeed");

        let db = state.db.as_ref().unwrap();
        let key = format!("voice:{local_id}:0");
        let before = db.get_provider_reservation(&key).expect("segment 0 exists");
        assert_eq!(before.status, "reserved");

        let swept = db
            .sweep_stale_voice_reservations(chrono::Utc::now().timestamp_millis())
            .expect("sweep must succeed");
        assert!(swept >= 1, "the sweep must have found at least segment 0");

        let after = db.get_provider_reservation(&key).expect("segment 0 exists");
        assert_eq!(after.status, "unresolved");
        assert_eq!(
            after.terminal_reason.as_deref(),
            Some("server restarted during live session")
        );
    }

    mod delegation {
        use std::future::Future;
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

        use crate::provider_gateway::{
            GatewayRequest, ObservedUsage, ProviderTransport, TransportFailure,
            TransportFailureKind, TransportResponse,
        };

        use super::*;

        const SUPPLIER_KEY: &str = "delegation-test-supplier-key";
        const DELEGATION_MODEL: &str = "claude-haiku-4-5";

        #[derive(Clone)]
        struct CountingTransport {
            calls: Arc<AtomicUsize>,
            response: Result<TransportResponse, TransportFailure>,
        }

        impl CountingTransport {
            fn ok(text: &str) -> Self {
                Self {
                    calls: Arc::new(AtomicUsize::new(0)),
                    response: Ok(TransportResponse {
                        body: serde_json::json!({
                            "id": "msg_test",
                            "type": "message",
                            "role": "assistant",
                            "model": DELEGATION_MODEL,
                            "content": [{"type": "text", "text": text}],
                            "stop_reason": "end_turn",
                            "stop_sequence": null,
                            "usage": {"input_tokens": 10, "output_tokens": 10}
                        }),
                        upstream_request_id: Some("upstream-1".into()),
                        usage: Some(ObservedUsage {
                            input_tokens: 10,
                            cached_input_tokens: 0,
                            output_tokens: 10,
                        }),
                    }),
                }
            }

            /// Never actually reached in the tests that use it — the point is
            /// to prove it was *not* called (empty-transcript, insufficient
            /// credits before any reservation).
            fn unreachable() -> Self {
                Self {
                    calls: Arc::new(AtomicUsize::new(0)),
                    response: Err(TransportFailure {
                        kind: TransportFailureKind::Rejected,
                        upstream_request_id: None,
                        message: "should not have been called".into(),
                    }),
                }
            }

            fn call_count(&self) -> usize {
                self.calls.load(AtomicOrdering::SeqCst)
            }
        }

        impl ProviderTransport for CountingTransport {
            fn forward(
                &self,
                supplier_key: &str,
                _request: &GatewayRequest,
            ) -> impl Future<Output = Result<TransportResponse, TransportFailure>> + Send
            {
                assert_eq!(supplier_key, SUPPLIER_KEY);
                self.calls.fetch_add(1, AtomicOrdering::SeqCst);
                std::future::ready(self.response.clone())
            }
        }

        #[test]
        fn accumulate_transcript_reads_delta_or_text() {
            let mut pending = String::new();
            accumulate_transcript(&mut pending, &serde_json::json!({"delta": "hello "}));
            accumulate_transcript(&mut pending, &serde_json::json!({"text": "world"}));
            accumulate_transcript(&mut pending, &serde_json::json!({"nothing_useful": true}));
            assert_eq!(pending, "hello world");
        }

        #[test]
        fn accumulate_transcript_keeps_only_a_sliding_window_and_stays_on_char_boundaries() {
            // A multi-byte character (3 bytes each in UTF-8) repeated well
            // past the cap, appended in chunks so the cut point does not
            // land on a chunk boundary by luck.
            let mut pending = String::new();
            for _ in 0..900 {
                accumulate_transcript(&mut pending, &serde_json::json!({"delta": "语言"}));
            }
            // 900 * "语言" is 5400 bytes, well past the 2000-byte cap.
            assert!(
                pending.len() <= PENDING_TRANSCRIPT_CHAR_CAP,
                "the transcript must be trimmed to the cap in bytes: was {}",
                pending.len()
            );
            // No panic above means every trim landed on a char boundary
            // (String::drain panics otherwise); also confirm the tail is
            // exactly what was most recently appended.
            assert!(
                pending.ends_with('言'),
                "the most recent text must survive trimming"
            );
        }

        #[tokio::test]
        async fn empty_transcript_skips_the_agent_and_is_not_charged() {
            let (_dir, state) = test_state_with_balance(1_000_000_000).await;
            let db = state.db.as_ref().unwrap();
            let before = db.get_credit_balance_row(USER);

            let transport = CountingTransport::unreachable();
            let answer = run_voice_delegation_with(
                db,
                SIGNING_KEY,
                SUPPLIER_KEY,
                transport.clone(),
                ample_limits(),
                USER,
                "   ",
            )
            .await;

            assert_eq!(answer, "I didn't catch that.");
            assert_eq!(transport.call_count(), 0, "the agent must never be called");
            assert_eq!(db.get_credit_balance_row(USER), before, "nothing charged");
        }

        #[tokio::test]
        async fn a_delegation_runs_the_agent_and_returns_its_answer() {
            let (_dir, state) = test_state_with_balance(1_000_000_000).await;
            let db = state.db.as_ref().unwrap();
            let before = db.get_credit_balance_row(USER).unwrap();

            let transport = CountingTransport::ok("the answer is four");
            let answer = run_voice_delegation_with(
                db,
                SIGNING_KEY,
                SUPPLIER_KEY,
                transport.clone(),
                ample_limits(),
                USER,
                "what is two plus two",
            )
            .await;

            assert_eq!(answer, "the answer is four");
            assert_eq!(transport.call_count(), 1);

            let price_list = db.active_price_list().unwrap();
            let rate = price_list.model("claude", DELEGATION_MODEL).unwrap();
            let expected_credits =
                ceil_div(rate.cost_micros(10, 0, 10), price_list.micros_per_credit);
            let after = db.get_credit_balance_row(USER).unwrap();
            assert_eq!(
                before.subscription_remaining - after.subscription_remaining,
                expected_credits,
                "the delegation must charge exactly the observed 10-in/10-out token cost"
            );
        }

        #[tokio::test]
        async fn insufficient_credits_yields_spoken_failure_and_no_charge() {
            let (_dir, state) = test_state_with_balance(0).await;
            let db = state.db.as_ref().unwrap();
            let before = db.get_credit_balance_row(USER);

            let transport = CountingTransport::unreachable();
            let answer = run_voice_delegation_with(
                db,
                SIGNING_KEY,
                SUPPLIER_KEY,
                transport.clone(),
                ample_limits(),
                USER,
                "do something",
            )
            .await;

            assert_eq!(answer, "You're out of credits for that request.");
            assert_eq!(
                transport.call_count(),
                0,
                "a zero balance must refuse before any provider call"
            );
            assert_eq!(db.get_credit_balance_row(USER), before, "nothing charged");
        }

        #[tokio::test]
        async fn a_delegation_drains_the_transcript_and_answers_with_matching_id() {
            // Exercises the real production path end to end: no gateway env
            // vars are set in this test process, so `gateway_usable()`
            // deterministically returns `None` and `run_voice_delegation`
            // answers "Cortex is temporarily unavailable." — the point of
            // this test is the plumbing around that call (transcript drain,
            // matching delegation_id, busy flag released), not the agent's
            // reply text, which `a_delegation_runs_the_agent_and_returns_its_answer`
            // above already covers directly against a fake transport.
            let (_dir, state) = test_state().await;
            let busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let task = Arc::new(std::sync::Mutex::new(None));
            let (tx, mut rx) = mpsc::channel::<Value>(8);
            let mut pending_transcript = "what is the weather".to_string();

            handle_delegation_created(
                &serde_json::json!({"delegation": {"id": "deleg-1"}}),
                &state,
                USER,
                &busy,
                &task,
                &mut pending_transcript,
                &tx,
            );

            assert_eq!(pending_transcript, "", "the transcript must be drained");

            let commentary = tokio::time::timeout(StdDuration::from_secs(5), rx.recv())
                .await
                .expect("commentary must arrive")
                .expect("channel must not close");
            assert_eq!(commentary["type"], "session.commentary.append");
            assert_eq!(commentary["delegation_id"], "deleg-1");
            assert_eq!(commentary["content"], "Cortex is temporarily unavailable.");
            assert!(
                !busy.load(std::sync::atomic::Ordering::SeqCst),
                "busy flag must be released once the delegation completes"
            );
        }

        #[tokio::test]
        async fn a_second_delegation_while_busy_is_refused() {
            let (_dir, state) = test_state().await;
            let busy = Arc::new(std::sync::atomic::AtomicBool::new(true));
            let task = Arc::new(std::sync::Mutex::new(None));
            let (tx, mut rx) = mpsc::channel::<Value>(8);
            let mut pending_transcript = "some accumulated speech".to_string();

            handle_delegation_created(
                &serde_json::json!({"delegation": {"id": "deleg-2"}}),
                &state,
                USER,
                &busy,
                &task,
                &mut pending_transcript,
                &tx,
            );

            let commentary = tokio::time::timeout(StdDuration::from_secs(5), rx.recv())
                .await
                .expect("commentary must arrive")
                .expect("channel must not close");
            assert_eq!(commentary["delegation_id"], "deleg-2");
            assert_eq!(commentary["content"], "Still working on the last request.");
            assert_eq!(
                pending_transcript, "some accumulated speech",
                "a refused-while-busy delegation must not drain the transcript"
            );
            assert!(
                busy.load(std::sync::atomic::Ordering::SeqCst),
                "the still-running first delegation's busy flag must stay set"
            );
        }

        #[test]
        fn busy_guard_releases_the_flag_even_when_dropped_during_a_panic_unwind() {
            let busy = Arc::new(std::sync::atomic::AtomicBool::new(true));
            let guard_busy = busy.clone();

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _guard = BusyGuard(guard_busy);
                panic!("simulated delegation task panic");
            }));

            assert!(result.is_err(), "the closure must have panicked");
            assert!(
                !busy.load(std::sync::atomic::Ordering::SeqCst),
                "BusyGuard must release the busy flag even when dropped while unwinding"
            );
        }

        #[tokio::test]
        async fn delegation_task_is_aborted_when_the_session_ends() {
            let (_dir, state) = test_state().await;
            let busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let task: Arc<std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>> =
                Arc::new(std::sync::Mutex::new(None));
            let (tx, _rx) = mpsc::channel::<Value>(8);
            let mut pending_transcript = "some long-running request".to_string();

            handle_delegation_created(
                &serde_json::json!({"delegation": {"id": "deleg-3"}}),
                &state,
                USER,
                &busy,
                &task,
                &mut pending_transcript,
                &tx,
            );

            // Simulate the billing loop ending: take and abort the handle,
            // exactly like the cleanup path after the loop breaks.
            let handle = task
                .lock()
                .unwrap()
                .take()
                .expect("a delegation task must have been spawned");
            handle.abort();

            let joined = handle.await;
            assert!(
                joined.is_err_and(|e| e.is_cancelled()),
                "the delegation task must report cancelled once aborted"
            );
        }
    }

    mod confirm_tools {
        use crate::agent_tools;

        #[test]
        fn confirm_risk_tools_are_excluded_from_voice_turns() {
            let names: Vec<String> = agent_tools::tool_definitions_excluding_confirm()
                .iter()
                .map(|t| t["name"].as_str().unwrap().to_string())
                .collect();
            for tool in agent_tools::tool_definitions() {
                let name = tool["name"].as_str().unwrap();
                if agent_tools::is_confirm_risk(name) {
                    assert!(
                        !names.contains(&name.to_string()),
                        "{name} is Risk::Confirm and must be withheld from voice turns"
                    );
                } else {
                    assert!(
                        names.contains(&name.to_string()),
                        "{name} is not Risk::Confirm and must stay available to voice turns"
                    );
                }
            }
            assert!(
                !agent_tools::tool_definitions().is_empty(),
                "fixture sanity: there must be at least one tool"
            );
        }
    }
}
