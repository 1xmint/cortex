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
        ZenTransport::new(),
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
/// `"zen returned {status}: ..."` message shape, where `{status}` is an
/// `http::StatusCode` and so displays as e.g. `"401 Unauthorized"` -- only
/// the first whitespace-separated token is the numeric code. Returns `None`
/// for any other message (a `NotSent`/`Timeout` failure never reached that
/// format), which is treated as the generic "did not answer" case below.
fn status_from_message(message: &str) -> Option<u16> {
    message
        .strip_prefix("zen returned ")?
        .split_whitespace()
        .next()?
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

/// Generic over `T: ProviderTransport` purely so tests can substitute a
/// mock Zen server (see `tests::run` below, mirroring `chat_paid::run`'s own
/// `send_paid_reply<T: ProviderTransport>`); the only production caller
/// (`chat` above) always passes a real `ZenTransport`.
#[allow(clippy::too_many_arguments)]
async fn run<T: ProviderTransport + Clone>(
    state: Arc<AppState>,
    user_id: String,
    conversation_id: Option<String>,
    model: String,
    system_prompt: String,
    user_message: String,
    key: ZenApiKey,
    tx: mpsc::Sender<StepEvent>,
    transport: T,
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

            if let Some(cid) = &conversation_id {
                if !text.is_empty() {
                    if let Err(error) =
                        db.try_add_message(cid, "assistant", &text, Some("zen"), Some(&model))
                    {
                        // The conversation may have been deleted mid-reply;
                        // usage is already recorded above, so this is only
                        // a lost transcript entry, never a panicked task.
                        tracing::error!(
                            %error,
                            %reply_id,
                            conversation_id = %cid,
                            "failed to store zen byok assistant reply"
                        );
                    }
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::byok::BYOK_ENV_LOCK;
    use axum::http::HeaderMap;
    use axum::routing::post;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use rusqlite::params;
    use std::sync::Mutex as StdMutex;

    const MODEL: &str = "deepseek-v4-flash";

    async fn test_state() -> (tempfile::TempDir, Arc<AppState>) {
        test_state_with_clerk_secret(None).await
    }

    async fn test_state_with_clerk_secret(
        clerk_secret_key: Option<String>,
    ) -> (tempfile::TempDir, Arc<AppState>) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".cortex")).unwrap();
        let state = AppState::new(
            dir.path().join(".cortex/ledger.jsonl"),
            dir.path().to_path_buf(),
            clerk_secret_key,
        )
        .await;
        (dir, state)
    }

    /// A stub key row good enough to exercise `get_provider_key_row` /
    /// `mark_provider_key_rejected` -- these tests never decrypt it, so the
    /// ciphertext does not need to be real.
    fn stub_encrypted_key() -> EncryptedKey {
        EncryptedKey {
            key_version: 1,
            nonce: vec![0u8; 12],
            ciphertext: vec![1, 2, 3, 4],
        }
    }

    type Seen = Arc<StdMutex<Vec<(HeaderMap, serde_json::Value)>>>;

    /// A stand-in Zen on a local port, mirroring `supplier_zen::tests::fake_zen`.
    async fn fake_zen(status: StatusCode, reply: serde_json::Value) -> (String, Seen) {
        let seen: Seen = Arc::default();
        let log = seen.clone();
        let app = axum::Router::new().route(
            "/chat/completions",
            post(
                move |headers: HeaderMap, Json(body): Json<serde_json::Value>| {
                    let log = log.clone();
                    let reply = reply.clone();
                    async move {
                        log.lock().unwrap().push((headers, body));
                        (status, [("x-request-id", "req_fake_1")], Json(reply))
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}"), seen)
    }

    async fn collect(mut rx: mpsc::Receiver<StepEvent>) -> Vec<StepEvent> {
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    }

    fn output_text(events: &[StepEvent]) -> Option<String> {
        events.iter().find_map(|e| match e {
            StepEvent::Output { line, .. } => Some(line.clone()),
            _ => None,
        })
    }

    fn failed_text(events: &[StepEvent]) -> Option<String> {
        events.iter().find_map(|e| match e {
            StepEvent::Failed { error, .. } => Some(error.clone()),
            _ => None,
        })
    }

    fn table_row_count(db: &crate::db::Database, table: &str) -> i64 {
        db.conn()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    // --- 1/2: a saved key reaches Zen with the customer's own key, and
    // settles as an unbilled `provider_spend` row only. ---

    #[tokio::test]
    async fn a_saved_key_reaches_zen_with_bearer_auth_and_the_reply_streams() {
        let (url, seen) = fake_zen(
            StatusCode::OK,
            serde_json::json!({
                "id": "chatcmpl-1",
                "choices": [{"message": {"role": "assistant", "content": "hello from zen"}}],
                "usage": {
                    "prompt_tokens": 5,
                    "completion_tokens": 3,
                    "prompt_tokens_details": {"cached_tokens": 0}
                }
            }),
        )
        .await;
        let (_dir, state) = test_state().await;
        let db = state.db.as_ref().unwrap();
        let conversation = db.create_conversation("user-1", None);
        // A real subscriber balance, the way db/ledger.rs's own tests seed
        // one -- with no balance row at all, `deduct_credits_up_to` rolls
        // back without writing anything, which would make the
        // zero-credit-rows assertions below pass even if this path wrongly
        // called it. Seeding a real balance means those assertions only
        // pass because this path never calls `deduct_credits_up_to`, not
        // because there was nothing to deduct from.
        db.init_credit_balance("user-1", 100).expect("balance");
        let (tx, rx) = mpsc::channel::<StepEvent>(64);
        let key = ZenApiKey::new("zen-secret-CUSTKEY1234".into());

        run(
            state.clone(),
            "user-1".into(),
            Some(conversation.id.clone()),
            MODEL.into(),
            "system".into(),
            "hi".into(),
            key,
            tx,
            ZenTransport::with_base_url(url),
        )
        .await;

        let events = collect(rx).await;
        assert_eq!(output_text(&events).as_deref(), Some("hello from zen"));

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "zen must be called exactly once");
        assert_eq!(
            seen[0].0["authorization"], "Bearer zen-secret-CUSTKEY1234",
            "the mock must receive the customer's own key as a bearer token"
        );

        // #2: nothing billed to Cortex -- no reservation, no authorization,
        // no credit transaction, no balance row at all; exactly one
        // `provider_spend` row, tagged `byok` at zero cost, for analytics
        // only. NOTE (reasoned, not run): if `chat_zen::run` ever grew a
        // `db.deduct_credits_up_to(...)` call on this path, this test would
        // still pass unless that call also wrote a `credit_transactions`
        // row and moved `credit_balances` -- which `deduct_credits_up_to`
        // always does on success (see `db/ledger.rs`). So a stray deduct
        // call here would flip the `credit_transactions` and
        // `credit_balances` assertions below and fail this test.
        let db = state.db.as_ref().unwrap();
        assert_eq!(table_row_count(db, "credit_transactions"), 0);
        assert_eq!(table_row_count(db, "provider_request_reservations"), 0);
        assert_eq!(table_row_count(db, "provider_spend_authorizations"), 0);
        assert_eq!(table_row_count(db, "provider_spend"), 1);
        let balance = db
            .get_credit_balance_row("user-1")
            .expect("balance row was seeded above");
        assert_eq!(
            balance.subscription_remaining, 100,
            "byok never touches credit_balances -- the seeded balance must be untouched"
        );
        let (cost_type, cost_micro_usd): (String, i64) = db
            .conn()
            .query_row(
                "SELECT cost_type, cost_micro_usd FROM provider_spend",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(cost_type, "byok");
        assert_eq!(cost_micro_usd, 0);

        let stored: String = db
            .conn()
            .query_row(
                "SELECT content FROM messages WHERE conversation_id = ?1 AND role = 'assistant'",
                params![conversation.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, "hello from zen");
    }

    // --- 3: no key -> 409 zen_key_required, before any transport call.
    // Driven through `crate::chat::chat` (not `chat_zen::chat` directly) so
    // that deleting the "zen:" dispatch in `chat::chat` would fail this
    // test, and with a subscribed user so the subscription check above the
    // dispatch never intercepts it first. ---

    #[tokio::test]
    async fn no_key_refuses_with_409_before_any_call() {
        let _guard = BYOK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CORTEX_BYOK_KEK_V1", STANDARD.encode([11u8; 32]));
        std::env::set_var("CORTEX_BYOK_KEK_CURRENT", "1");

        let (_dir, state) = test_state_with_clerk_secret(Some("test-clerk-secret".into())).await;
        state
            .db
            .as_ref()
            .unwrap()
            .upsert_subscription(&crate::db::SubscriptionRecord {
                clerk_user_id: "user-3".into(),
                stripe_customer_id: "cus_test".into(),
                stripe_subscription_id: None,
                plan_type: "pro".into(),
                status: "active".into(),
                trial_end: None,
                current_period_start: None,
                current_period_end: None,
            });

        let user = ClerkUser {
            user_id: "user-3".into(),
        };
        let req = ChatRequest {
            message: "hi".into(),
            file_paths: vec![],
            user_id: None,
            conversation_id: None,
            routing_preferences: None,
            model: Some(format!("zen:{MODEL}")),
        };

        let result = crate::chat::chat(State(state.clone()), user, Json(req)).await;

        std::env::remove_var("CORTEX_BYOK_KEK_V1");
        std::env::remove_var("CORTEX_BYOK_KEK_CURRENT");

        let response = result.expect_err("no key must refuse, not stream");
        assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"], "zen_key_required");

        let db = state.db.as_ref().unwrap();
        assert_eq!(table_row_count(db, "provider_request_reservations"), 0);
        assert_eq!(table_row_count(db, "provider_spend_authorizations"), 0);
        assert_eq!(table_row_count(db, "credit_transactions"), 0);
    }

    // --- 4: a rejected key -> the same 409, never falls through. ---

    #[tokio::test]
    async fn rejected_key_refuses_with_409() {
        let _guard = BYOK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CORTEX_BYOK_KEK_V1", STANDARD.encode([11u8; 32]));
        std::env::set_var("CORTEX_BYOK_KEK_CURRENT", "1");

        let (_dir, state) = test_state().await;
        state.db.as_ref().unwrap().upsert_provider_key(
            "user-4",
            "zen",
            &stub_encrypted_key(),
            "1234",
            1_800_000_000_000,
        );
        state
            .db
            .as_ref()
            .unwrap()
            .mark_provider_key_rejected("user-4", "zen", 1_800_000_000_000);

        let user = ClerkUser {
            user_id: "user-4".into(),
        };
        let req = ChatRequest {
            message: "hi".into(),
            file_paths: vec![],
            user_id: None,
            conversation_id: None,
            routing_preferences: None,
            model: Some(format!("zen:{MODEL}")),
        };

        let result = chat(
            State(state.clone()),
            user,
            req,
            MODEL.into(),
            "system".into(),
        )
        .await;

        std::env::remove_var("CORTEX_BYOK_KEK_V1");
        std::env::remove_var("CORTEX_BYOK_KEK_CURRENT");

        let response = result.expect_err("a rejected key must refuse, not stream");
        assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"], "zen_key_required");
    }

    // --- 5: no subscription -> 402, before this module (and its decrypt
    // call) ever runs -- the check lives in `chat::chat`, not here. ---

    #[tokio::test]
    async fn no_subscription_refuses_with_402_before_chat_zen_runs() {
        let (_dir, state) = test_state_with_clerk_secret(Some("test-clerk-secret".into())).await;
        state
            .db
            .as_ref()
            .unwrap()
            .upsert_subscription(&crate::db::SubscriptionRecord {
                clerk_user_id: "user-5".into(),
                stripe_customer_id: "cus_test".into(),
                stripe_subscription_id: None,
                plan_type: "pro".into(),
                status: "past_due".into(),
                trial_end: None,
                current_period_start: None,
                current_period_end: None,
            });

        let user = ClerkUser {
            user_id: "user-5".into(),
        };
        let req = ChatRequest {
            message: "hi".into(),
            file_paths: vec![],
            user_id: None,
            conversation_id: None,
            routing_preferences: None,
            model: Some(format!("zen:{MODEL}")),
        };

        let result = crate::chat::chat(State(state.clone()), user, Json(req)).await;

        let response = result.expect_err("no active subscription must refuse before zen runs");
        assert_eq!(response.status(), axum::http::StatusCode::PAYMENT_REQUIRED);
        // Never touched a provider key: no row exists, and none was created.
        assert!(state
            .db
            .as_ref()
            .unwrap()
            .get_provider_key_row("user-5", "zen")
            .is_none());
    }

    // --- 6: a 401 from Zen marks the key rejected, and never shows the
    // customer their key or the raw upstream body. ---

    #[tokio::test]
    async fn a_401_from_zen_marks_the_key_rejected_and_hides_the_body() {
        let (url, _seen) = fake_zen(
            StatusCode::UNAUTHORIZED,
            serde_json::json!({"error": {"message": "raw-upstream-body-marker-should-not-leak"}}),
        )
        .await;
        let (_dir, state) = test_state().await;
        state.db.as_ref().unwrap().upsert_provider_key(
            "user-6",
            "zen",
            &stub_encrypted_key(),
            "1234",
            1_800_000_000_000,
        );

        let (tx, rx) = mpsc::channel::<StepEvent>(64);
        let key = ZenApiKey::new("zen-secret-CUSTKEY1234".into());
        run(
            state.clone(),
            "user-6".into(),
            None,
            MODEL.into(),
            "system".into(),
            "hi".into(),
            key,
            tx,
            ZenTransport::with_base_url(url),
        )
        .await;

        let events = collect(rx).await;
        let failed = failed_text(&events).expect("a 401 must fail the turn");
        assert!(!failed.contains("zen-secret-CUSTKEY1234"));
        assert!(!failed.contains("raw-upstream-body-marker-should-not-leak"));

        let row = state
            .db
            .as_ref()
            .unwrap()
            .get_provider_key_row("user-6", "zen")
            .unwrap();
        assert_eq!(row.status, "rejected");
    }

    // --- status_from_message must parse `StatusCode`'s Display form
    // ("401 Unauthorized", not a bare number), or every status-specific
    // message below silently falls back to the generic one. ---

    async fn failure_message_for(
        user_id: &str,
        status: StatusCode,
        body: serde_json::Value,
    ) -> String {
        let (url, _seen) = fake_zen(status, body).await;
        let (_dir, state) = test_state().await;
        state.db.as_ref().unwrap().upsert_provider_key(
            user_id,
            "zen",
            &stub_encrypted_key(),
            "1234",
            1_800_000_000_000,
        );

        let (tx, rx) = mpsc::channel::<StepEvent>(64);
        let key = ZenApiKey::new("zen-secret-CUSTKEY1234".into());
        run(
            state.clone(),
            user_id.into(),
            None,
            MODEL.into(),
            "system".into(),
            "hi".into(),
            key,
            tx,
            ZenTransport::with_base_url(url),
        )
        .await;

        let events = collect(rx).await;
        failed_text(&events).expect("failure must fail the turn")
    }

    #[tokio::test]
    async fn a_402_from_zen_reports_the_customer_is_out_of_balance() {
        let message = failure_message_for(
            "user-402",
            StatusCode::PAYMENT_REQUIRED,
            serde_json::json!({"error": {"message": "insufficient balance"}}),
        )
        .await;
        assert_eq!(
            message,
            "Your Zen account is out of balance. Top up at opencode.ai."
        );
    }

    #[tokio::test]
    async fn a_429_from_zen_reports_rate_limiting() {
        let message = failure_message_for(
            "user-429",
            StatusCode::TOO_MANY_REQUESTS,
            serde_json::json!({"error": {"message": "slow down"}}),
        )
        .await;
        assert_eq!(message, "Zen is rate-limiting your key.");
    }

    #[tokio::test]
    async fn a_500_from_zen_reports_the_generic_no_answer_message() {
        let message = failure_message_for(
            "user-500",
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": {"message": "boom"}}),
        )
        .await;
        assert_eq!(
            message,
            "Zen did not answer. Nothing was charged by Cortex."
        );
    }

    // --- 7: a model outside the allowlist -> 400, before any KEK/db work. ---

    #[tokio::test]
    async fn a_model_outside_the_allowlist_refuses_with_400() {
        let (_dir, state) = test_state().await;
        let user = ClerkUser {
            user_id: "user-7".into(),
        };
        let req = ChatRequest {
            message: "hi".into(),
            file_paths: vec![],
            user_id: None,
            conversation_id: None,
            routing_preferences: None,
            model: Some("zen:big-pickle".into()),
        };

        let result = chat(
            State(state),
            user,
            req,
            "big-pickle".into(),
            "system".into(),
        )
        .await;

        let response = result.expect_err("an unlisted model must refuse, not stream");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"], "zen_model_not_allowed");
    }

    // --- 8: GET /api/chat/models reflects real key state for Zen entries,
    // and never changes the Claude entries. ---

    #[tokio::test]
    async fn chat_models_reflects_zen_key_state_and_leaves_claude_unchanged() {
        let _guard = BYOK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CORTEX_BYOK_KEK_V1", STANDARD.encode([11u8; 32]));
        std::env::set_var("CORTEX_BYOK_KEK_CURRENT", "1");

        let (_dir, state) = test_state().await;
        let user = ClerkUser {
            user_id: "user-8".into(),
        };

        let before = crate::chat::chat_models(State(state.clone()), user.clone())
            .await
            .0;
        let claude_before: Vec<serde_json::Value> = before
            .models
            .iter()
            .filter(|m| m.provider == "claude")
            .map(|m| serde_json::to_value(m).unwrap())
            .collect();
        let zen_before: Vec<_> = before
            .models
            .iter()
            .filter(|m| m.provider == "zen")
            .collect();
        assert!(zen_before
            .iter()
            .all(|m| !m.available && m.unavailable_reason.as_deref() == Some("needs_key")));

        state.db.as_ref().unwrap().upsert_provider_key(
            "user-8",
            "zen",
            &stub_encrypted_key(),
            "1234",
            1_800_000_000_000,
        );

        let after = crate::chat::chat_models(State(state.clone()), user).await.0;
        let zen_after: Vec<_> = after
            .models
            .iter()
            .filter(|m| m.provider == "zen")
            .collect();
        assert!(zen_after
            .iter()
            .all(|m| m.available && m.unavailable_reason.is_none()));

        let claude_after: Vec<serde_json::Value> = after
            .models
            .iter()
            .filter(|m| m.provider == "claude")
            .map(|m| serde_json::to_value(m).unwrap())
            .collect();
        assert_eq!(claude_before, claude_after, "claude entries never change");

        std::env::remove_var("CORTEX_BYOK_KEK_V1");
        std::env::remove_var("CORTEX_BYOK_KEK_CURRENT");
    }

    // --- 9: no `model` field behaves exactly as before -- it never reaches
    // chat_zen at all. ---

    #[tokio::test]
    async fn no_model_field_takes_the_pre_existing_path_not_zen() {
        let (_dir, state) = test_state().await;
        let user = ClerkUser {
            user_id: "user-9".into(),
        };
        let req = ChatRequest {
            message: "hi".into(),
            file_paths: vec![],
            user_id: None,
            conversation_id: None,
            routing_preferences: None,
            model: None,
        };

        let result = crate::chat::chat(State(state.clone()), user, Json(req)).await;
        // With no `model` field, `chat::chat`'s own guard never calls into
        // `chat_zen` at all -- this must succeed exactly as it did before
        // the Zen path existed, and its first event must be the pre-existing
        // `Started` event tagged with the non-zen provider (`ProviderPath::
        // None` in this test environment, no gateway configured), never a
        // zen-tagged one.
        let response = result
            .expect("no model field must take the pre-existing path, never error")
            .into_response();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        let first_data_line = text
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .expect("an SSE stream must send at least one data event");
        let first_event: serde_json::Value = serde_json::from_str(first_data_line).unwrap();
        assert_eq!(first_event["type"], "started");
        assert_ne!(first_event["provider"], "zen");
    }

    // --- billing hole: `model` values that are neither `"zen:"`-prefixed
    // nor an exact Claude tier value must refuse with 400 before any
    // provider work, never fall through to the Claude-tier path and get
    // billed. ---

    #[tokio::test]
    async fn an_unknown_model_value_refuses_with_400_before_any_provider_work() {
        let (_dir, state) = test_state().await;
        let user = ClerkUser {
            user_id: "user-unknown-model".into(),
        };
        let req = ChatRequest {
            message: "hi".into(),
            file_paths: vec![],
            user_id: None,
            conversation_id: None,
            routing_preferences: None,
            model: Some("glm-5.2".into()),
        };

        let result = crate::chat::chat(State(state.clone()), user, Json(req)).await;

        let response = result.expect_err("an unknown model value must refuse, not stream");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"], "unknown_model");

        let db = state.db.as_ref().unwrap();
        assert_eq!(table_row_count(db, "credit_transactions"), 0);
    }

    #[tokio::test]
    async fn chat_models_zen_entries_carry_the_zen_prefix() {
        let (_dir, state) = test_state().await;
        let user = ClerkUser {
            user_id: "user-models-prefix".into(),
        };

        let response = crate::chat::chat_models(State(state), user).await.0;
        let zen_entries: Vec<_> = response
            .models
            .iter()
            .filter(|m| m.provider == "zen")
            .collect();
        assert!(!zen_entries.is_empty(), "at least one zen model exists");
        assert!(
            zen_entries.iter().all(|m| m.model.starts_with("zen:")),
            "every zen model entry must be echoable back to chat() unchanged"
        );
    }
}
