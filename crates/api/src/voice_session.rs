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

/// The session's shape sent to OpenAI, held in one function so a later PR
/// adding client delegation and tools has exactly one place to change. No
/// delegation and no tools yet — gpt-live-1 only listens and speaks.
pub(crate) fn live_session_config() -> Value {
    serde_json::json!({ "model": LIVE_MODEL })
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
        let mut sessions = state.voice_sessions.lock().unwrap();
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

    let result = start_live_session(
        state,
        &supplier_key,
        http_base,
        ws_base,
        signing_key,
        limits,
        user_id,
        sdp,
        now_ms,
        local_id.clone(),
    )
    .await;

    if result.is_err() {
        state.voice_sessions.lock().unwrap().remove(&local_id);
    }
    result
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
    let expires_at_ms = now_ms + SESSION_LEASE_MS;
    let Some((authorization_id, _signed)) =
        provider_gateway_http::create_authorization_and_capability(
            db,
            signing_key,
            user_id,
            &run_id,
            "live",
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
        "live",
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

    let sideband = match WsSideband::attach(ws_base, &session_id, supplier_key).await {
        Ok(sideband) => sideband,
        Err(detail) => {
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
            return Err(LiveSessionError::SupplierFailed);
        }
    };

    // The placeholder inserted under `local_id` in `start_session` is
    // replaced here, now that the real session id is known, with the same
    // close channel — nothing that could race the one-per-user check is
    // still pending.
    let (close_tx, close_rx) = mpsc::channel(1);
    {
        let mut sessions = state.voice_sessions.lock().unwrap();
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
    let micros_per_credit = price_list.micros_per_credit;
    tokio::spawn(async move {
        run_billing_loop(
            loop_state,
            sideband,
            claims,
            loop_session_id,
            loop_local_id,
            loop_user_id,
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
        let sessions = state.voice_sessions.lock().unwrap();
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
    rate: crate::pricing::ModelPrice,
    micros_per_credit: i64,
    max_micro_usd: i64,
    segment0_micro: i64,
    mut close_rx: mpsc::Receiver<()>,
) {
    let Some(db) = state.db.as_ref() else {
        state.voice_sessions.lock().unwrap().remove(&session_id);
        return;
    };

    let mut segments: std::collections::VecDeque<Segment> = std::collections::VecDeque::new();
    segments.push_back(Segment {
        index: 0,
        reserved_micro: segment0_micro,
    });
    let mut next_segment_index: i64 = 0;
    let mut reserved_so_far_micro = segment0_micro;
    let mut settled_so_far_micro: i64 = 0;
    let segment_micro = rate.cost_micros(SEGMENT_SECONDS, 0, 0);
    let mut warning_deadline: Option<tokio::time::Instant> = None;
    let mut closed_cleanly = false;
    let now_ms = || chrono::Utc::now().timestamp_millis();

    loop {
        let warning_timer = async {
            match warning_deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending::<()>().await,
            }
        };

        tokio::select! {
            event = sideband.recv() => {
                let Some(event) = event else { break };
                match event.get("type").and_then(Value::as_str) {
                    Some("session.usage.updated") => {
                        let seconds = event
                            .pointer("/usage/seconds")
                            .and_then(Value::as_i64)
                            .unwrap_or(0);
                        let observed_total = rate.cost_micros(seconds, 0, 0);
                        let budget_exhausted = settle_up_to(
                            db,
                            &user_id,
                            &session_id,
                            &local_id,
                            &mut segments,
                            &mut settled_so_far_micro,
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
                                        segments.push_back(Segment {
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
                        settle_up_to(
                            db,
                            &user_id,
                            &session_id,
                            &local_id,
                            &mut segments,
                            &mut settled_so_far_micro,
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
        for seg in segments.drain(..) {
            let key = format!("voice:{local_id}:{}", seg.index);
            let _ = db.mark_provider_request_unresolved(
                &key,
                None,
                "sideband dropped without session.closed",
                now_ms(),
            );
        }
        tracing::error!(
            session_id = %session_id,
            "voice: sideband dropped without session.closed; open reservation(s) left unresolved for reconciliation"
        );
    }

    state.voice_sessions.lock().unwrap().remove(&session_id);
}

/// Settles every fully-consumed segment at the front of `segments` against
/// `observed_total_micro`, in order: a segment only ever settles for
/// exactly its own `reserved_micro` (never more — that is what used to let
/// a late-arriving usage jump land as a `mismatch`), except the final
/// segment on `is_final`, which settles for whatever is left, including
/// zero. Usage that still exceeds every reservation once `is_final` is true
/// is charged directly as an overrun rather than forced through settle.
///
/// Returns whether a credit deduction failed during this call — budget
/// exhaustion the caller must stop renewing against (item D).
#[allow(clippy::too_many_arguments)]
fn settle_up_to(
    db: &crate::db::Database,
    user_id: &str,
    session_id: &str,
    local_id: &str,
    segments: &mut std::collections::VecDeque<Segment>,
    settled_so_far_micro: &mut i64,
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
                let credits = ceil_div(amount, micros_per_credit);
                if credits > 0 {
                    if let Err(error) = db.deduct_credits(
                        user_id,
                        credits,
                        "Cortex live voice",
                        &ChargeKey::per_unit(&key),
                    ) {
                        tracing::error!(
                            user_id,
                            session_id,
                            segment_index = seg.index,
                            %error,
                            "voice: segment settled but charging credits failed"
                        );
                        budget_exhausted = true;
                    }
                }
            }
            Err(error) => {
                tracing::error!(
                    session_id,
                    segment_index = seg.index,
                    %error,
                    "voice: segment settle failed"
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
            let credits = ceil_div(remaining, micros_per_credit);
            if credits > 0 {
                if let Err(error) = db.deduct_credits(
                    user_id,
                    credits,
                    "Cortex live voice overrun",
                    &ChargeKey::per_unit(&key),
                ) {
                    tracing::error!(user_id, session_id, %error, "voice: overrun charge failed");
                }
            }
            *settled_so_far_micro = observed_total_micro;
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
        let expected_credits_floor =
            expected_total_micro / db.active_price_list().unwrap().micros_per_credit;
        // Charged per-segment (three ceil_div rounds), so it can be at most
        // two credits above the whole-session floor and never below it.
        assert!(
            spent_credits >= expected_credits_floor && spent_credits <= expected_credits_floor + 3,
            "spent {spent_credits} credits, expected close to {expected_credits_floor}"
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
    async fn a_dropped_sideband_leaves_one_unresolved_reservation_and_charges_nothing_extra() {
        let (_dir, state) = test_state().await;
        let (http_base, ws_base) = spawn(FakeLiveScript {
            usage_events: vec![50, 100],
            send_closed_after_script: false,
            respond_to_client_close: false,
            refuse_attach: false,
            pause_after_first_event: None,
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
        let reservation = db
            .get_provider_reservation(&format!("voice:{local_id}:0"))
            .expect("segment 0 exists");
        assert_eq!(reservation.status, "unresolved");
        assert_eq!(
            db.provider_spend_row_count(&format!("voice:{local_id}:0")),
            0
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

        // The failed deduction stops renewal and heads for the goodbye
        // path, which needs the full warning grace before it closes.
        wait_until_session_gone_with_timeout(&state, &session_id, StdDuration::from_secs(40)).await;
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
}
