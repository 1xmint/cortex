//! Zen chat on the customer's own OpenCode Zen key (BYOK).
//!
//! Cortex never pays for Zen and never charges credits for it (D1/A1/A5 in
//! the BYOK plan): this path never opens a `ProviderGateway` reservation,
//! never calls `deduct_credits*`, and writes exactly one `provider_spend` row
//! per successful reply, at `cost_micro_usd = 0`, for analytics only. When
//! the customer has no usable key, this refuses -- it never falls through to
//! `ProviderPath::Cortex` and spends Cortex's own money instead (D3, D5).
//!
//! The refusal happens entirely inside [`chat`], before any SSE stream is
//! opened and before the key is decrypted, so a 409 here never touches the
//! transport, the KEK, or (for the "no subscription" case, checked by the
//! caller before this module runs at all) the database.

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use once_cell::sync::Lazy;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use crate::byok::{self, EncryptedKey, ZenApiKey};
use crate::chat::{step_event_to_sse, BoxedSseStream, ChatRequest};
use crate::clerk::ClerkUser;
use crate::provider_gateway::{
    GatewayRequest, ObservedUsage, ProviderTransport, SignedCapability, TransportFailureKind,
};
use crate::state::{AppState, StepEvent};
use crate::supplier_zen::{self, ZenTransport};

/// Fixed per D3/M-D-0003: this slice is one non-streamed call, text-only, no
/// agent tools. A higher cap is a follow-up (Josh-5 in the plan), not a
/// per-request choice.
pub(crate) const MAX_OUTPUT_TOKENS: i64 = 4096;

#[derive(serde::Serialize)]
struct ZenErrorBody {
    error: &'static str,
    message: String,
}

fn zen_key_required(message: String) -> Response {
    (
        StatusCode::CONFLICT,
        Json(ZenErrorBody {
            error: "zen_key_required",
            message,
        }),
    )
        .into_response()
}

fn bad_model(model: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ZenErrorBody {
            error: "zen_model_not_allowed",
            message: format!("{model:?} is not an OpenCode Zen model Cortex offers."),
        }),
    )
        .into_response()
}

fn needs_key_message(model: &str) -> String {
    format!(
        "{model} runs on your own OpenCode Zen key. Add it in Settings \u{2192} Model keys. \
         Cortex does not pay for Zen."
    )
}

/// `POST /api/chat` when `model` is `"zen:<model>"`. Returns the same SSE
/// contract as `chat::chat`'s other paths (`Started`/`Output`/`Completed`/
/// `Failed`), except that every refusal in this function returns a plain
/// HTTP error *before* that stream is ever created -- D3 requires the 409
/// to land before any SSE opens.
pub(crate) async fn chat(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    req: ChatRequest,
    zen_model: String,
    system_prompt: String,
) -> Result<Sse<BoxedSseStream>, Response> {
    if !supplier_zen::allowed_models().contains(&zen_model.as_str()) {
        return Err(bad_model(&zen_model));
    }

    if !byok::KekRing::enabled() {
        return Err(zen_key_required(
            "Zen is not available on this server".into(),
        ));
    }

    let db = state.db.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ZenErrorBody {
                error: "database_unavailable",
                message: "Cortex chat is temporarily unavailable. Please try again in a moment."
                    .into(),
            }),
        )
            .into_response()
    })?;

    let row = db.get_provider_key_row(&user.user_id, "zen");
    let row = match row {
        Some(row) if row.status == "active" => row,
        _ => return Err(zen_key_required(needs_key_message(&zen_model))),
    };

    let encrypted = EncryptedKey {
        key_version: row.key_version,
        nonce: row.nonce.clone(),
        ciphertext: row.ciphertext.clone(),
    };
    let key: ZenApiKey = match byok::decrypt(&user.user_id, "zen", &encrypted) {
        Ok(key) => key,
        // Unreadable under any KEK this process has -- the same recoverable
        // case `rewrap_provider_keys` documents: the customer re-enters
        // their key. Never a 500 for this.
        Err(_) => return Err(zen_key_required(needs_key_message(&zen_model))),
    };

    let (tx, rx) = mpsc::channel::<StepEvent>(64);

    state.vera_tracker.record_conversation(&user.user_id);

    let user_id = user.user_id.clone();
    let conversation_id = req.conversation_id.clone();
    let user_message = req.message.clone();
    if let Some(cid) = &conversation_id {
        db.add_message(cid, "user", &user_message, None, None);
    }

    let state_clone = state.clone();
    tokio::spawn(run(
        state_clone,
        user_id,
        conversation_id,
        zen_model,
        system_prompt,
        user_message,
        key,
        tx,
    ));

    let stream = ReceiverStream::new(rx).map(step_event_to_sse);
    let boxed: std::pin::Pin<
        Box<dyn futures_core::Stream<Item = Result<axum::response::sse::Event, Infallible>> + Send>,
    > = Box::pin(stream);
    Ok(Sse::new(boxed).keep_alive(KeepAlive::default()))
}

/// One in-flight Zen BYOK reply per user, and a rolling one-minute cap per
/// conversation -- the same shapes `chat_paid.rs` uses for its own turn
/// limiter and per-user permit, kept as separate statics here because those
/// are private to `chat_paid` (BYOK money is the customer's, not Cortex's,
/// so this exists to protect Cortex's server from a runaway client, not to
/// protect a shared balance).
static ZEN_TURN_WINDOWS: Lazy<Mutex<HashMap<String, VecDeque<i64>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static ZEN_USER_LOCKS: Lazy<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

const ZEN_TURN_WINDOW_MS: i64 = 60_000;

fn zen_turn_cap_per_minute() -> u32 {
    std::env::var("CORTEX_CHAT_AGENT_TURN_CAP_PER_MINUTE")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(20)
}

fn try_acquire_zen_turn_slot(key: &str, cap: u32, now_ms: i64) -> bool {
    let mut windows = ZEN_TURN_WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
    let mut window = windows.remove(key).unwrap_or_default();
    while window
        .front()
        .is_some_and(|t| now_ms - *t >= ZEN_TURN_WINDOW_MS)
    {
        window.pop_front();
    }
    let allowed = (window.len() as u32) < cap;
    if allowed {
        window.push_back(now_ms);
    }
    if !window.is_empty() {
        windows.insert(key.to_string(), window);
    }
    allowed
}

struct ZenUserPermit {
    user_id: String,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl Drop for ZenUserPermit {
    fn drop(&mut self) {
        self.guard.take();
        let mut locks = ZEN_USER_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
        if locks
            .get(&self.user_id)
            .is_some_and(|arc| Arc::strong_count(arc) == 1)
        {
            locks.remove(&self.user_id);
        }
    }
}

async fn acquire_zen_user_permit(user_id: &str) -> ZenUserPermit {
    let arc = {
        let mut locks = ZEN_USER_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
        locks
            .entry(user_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    let guard = arc.lock_owned().await;
    ZenUserPermit {
        user_id: user_id.to_string(),
        guard: Some(guard),
    }
}

/// Parses the status code out of `supplier_zen.rs`'s fixed
/// `"zen returned {status}: ..."` message shape. Returns `None` for any
/// other message (a `NotSent`/`Timeout` failure never reached that format),
/// which is treated as the generic "did not answer" case below.
fn status_from_message(message: &str) -> Option<u16> {
    message
        .strip_prefix("zen returned ")?
        .split(':')
        .next()?
        .trim()
        .parse()
        .ok()
}

fn user_message_for_failure(kind: TransportFailureKind, message: &str) -> (String, bool) {
    let status = status_from_message(message);
    match (kind, status) {
        (TransportFailureKind::Rejected, Some(401 | 403)) => (
            "Zen rejected your key. Replace it in Settings.".into(),
            true,
        ),
        (TransportFailureKind::Rejected, _) => {
            ("Zen could not process this request.".into(), false)
        }
        (TransportFailureKind::Unknown, Some(402)) => (
            "Your Zen account is out of balance. Top up at opencode.ai.".into(),
            false,
        ),
        (TransportFailureKind::Unknown, Some(429)) => {
            ("Zen is rate-limiting your key.".into(), false)
        }
        _ => (
            "Zen did not answer. Nothing was charged by Cortex.".into(),
            false,
        ),
    }
}

/// Extracts the assistant's reply text from a Zen chat-completions response
/// body: `choices[0].message.content`.
fn reply_text(body: &serde_json::Value) -> String {
    body.pointer("/choices/0/message/content")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[allow(clippy::too_many_arguments)]
async fn run(
    state: Arc<AppState>,
    user_id: String,
    conversation_id: Option<String>,
    model: String,
    system_prompt: String,
    user_message: String,
    key: ZenApiKey,
    tx: mpsc::Sender<StepEvent>,
) {
    let _ = tx
        .send(StepEvent::Started {
            step_id: "chat".into(),
            provider: "zen".into(),
            model: model.clone(),
        })
        .await;

    let Some(db) = state.db.as_ref() else {
        let _ = tx
            .send(StepEvent::Failed {
                step_id: "chat".into(),
                error: "Cortex chat is temporarily unavailable. Please try again in a moment."
                    .into(),
            })
            .await;
        return;
    };

    let _user_permit = acquire_zen_user_permit(&user_id).await;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let reply_id = uuid::Uuid::new_v4().to_string();
    let turn_cap_key = format!(
        "{user_id}:{}",
        conversation_id.as_deref().unwrap_or(&reply_id)
    );
    if !try_acquire_zen_turn_slot(&turn_cap_key, zen_turn_cap_per_minute(), now_ms) {
        let _ = tx
            .send(StepEvent::Failed {
                step_id: "chat".into(),
                error: "This conversation is using tools too quickly right now. Please wait a moment and try again.".into(),
            })
            .await;
        return;
    }

    let request = GatewayRequest {
        request_key: reply_id.clone(),
        tenant_id: user_id.clone(),
        run_id: conversation_id.clone().unwrap_or_else(|| reply_id.clone()),
        attempt_id: reply_id.clone(),
        provider: "zen".into(),
        model: model.clone(),
        max_output_tokens: MAX_OUTPUT_TOKENS,
        body: serde_json::json!({
            "model": model,
            "stream": false,
            "n": 1,
            "max_completion_tokens": MAX_OUTPUT_TOKENS,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_message},
            ],
        }),
        capability: SignedCapability::from_exposed("zen-byok"),
    };

    let transport = ZenTransport::new();
    match transport.forward(key.expose_secret(), &request).await {
        Ok(response) => {
            let text = reply_text(&response.body);
            let _ = tx
                .send(StepEvent::Output {
                    step_id: "chat".into(),
                    line: text.clone(),
                })
                .await;
            let _ = tx
                .send(StepEvent::Completed {
                    step_id: "chat".into(),
                    exit_code: 0,
                })
                .await;

            if let Some(cid) = &conversation_id {
                if !text.is_empty() {
                    db.add_message(cid, "assistant", &text, Some("zen"), Some(&model));
                }
            }

            db.touch_provider_key_last_used(&user_id, "zen", now_ms);
            let ObservedUsage {
                input_tokens,
                cached_input_tokens,
                output_tokens,
            } = response.usage.unwrap_or(ObservedUsage {
                input_tokens: 0,
                cached_input_tokens: 0,
                output_tokens: 0,
            });
            db.insert_byok_usage(
                &reply_id,
                &user_id,
                conversation_id.as_deref(),
                &model,
                input_tokens,
                output_tokens,
                cached_input_tokens,
                now_ms,
            );
        }
        Err(failure) => {
            // The raw upstream body/message is sanitized by
            // `supplier_zen::error_message` already (300 chars, JSON
            // `error.message` only), but it is still never shown to the
            // user -- only logged, and only after this failure has been
            // through that sanitizer.
            tracing::warn!(
                kind = ?failure.kind,
                %reply_id,
                "zen byok call failed"
            );
            if matches!(
                (failure.kind, status_from_message(&failure.message)),
                (TransportFailureKind::Rejected, Some(401 | 403))
            ) {
                db.mark_provider_key_rejected(&user_id, "zen", now_ms);
            }
            let (user_text, _marked_rejected) =
                user_message_for_failure(failure.kind, &failure.message);
            let _ = tx
                .send(StepEvent::Failed {
                    step_id: "chat".into(),
                    error: user_text,
                })
                .await;
        }
    }
}
