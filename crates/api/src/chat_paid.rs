//! Chat replies paid for by Cortex, on Cortex's own Anthropic key, charged to
//! the user at observed cost.
//!
//! This is the path that keeps chat alive once BYOK and BYOS are gone: no
//! stored credential, no CLI subprocess, just the private provider gateway
//! (`provider_gateway.rs`) called in-process with a capability scoped to one
//! reply. The gateway reserves a spend cap before it calls the supplier and
//! settles on what the supplier actually reports; this module never guesses
//! that number, only reads it back and turns it into whole credits.
//!
//! # What decides the price
//!
//! Nothing here does. `provider_gateway_http::create_authorization_and_capability`
//! refuses to authorize a model with no row in the active price list, and
//! `[send_paid_reply]` refuses up front for the same reason with a message a
//! user can read, rather than letting the refusal surface as an opaque
//! gateway error after a reservation was already attempted.
//!
//! # What decides the spend cap
//!
//! `min(the operator's env cap, the user's own credit balance)`. A user with
//! zero credits gets a plain refusal before anything is reserved — the
//! gateway is never even asked.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use cortex_core::billing_binding::ChargeKey;
use once_cell::sync::Lazy;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::agent_tools;
use crate::db::Database;
use crate::provider_gateway::{GatewayError, GatewayRequest, ProviderGateway, ProviderTransport};
use crate::provider_gateway_http::{self, GatewayUsable, SpendLimits};
use crate::state::{AppState, StepEvent};

/// Anthropic's cap on one reply. Fixed rather than user-supplied: chat has no
/// notion of "how long an answer should be" yet, and a bounded, known value is
/// what lets the gateway's context-window check (`provider_gateway.rs`) run
/// before anything is reserved.
const MAX_OUTPUT_TOKENS: i64 = 4096;

/// How long a chat reply's authorization stays valid. One reply, one round
/// trip — this only needs to outlive the supplier call.
const LEASE_MS: i64 = 120_000;

/// How many model turns one user message may drive. A turn is one billed
/// supplier call; a turn that comes back with `tool_use` blocks costs
/// another turn to send the tool results back. Fixed rather than
/// user-supplied for the same reason as `MAX_OUTPUT_TOKENS`: an unbounded
/// loop is an unbounded bill.
const MAX_AGENT_TURNS: u32 = 8;

/// How many billed agent turns one conversation may spend per rolling
/// minute, across every user message in it. Read once per reply rather than
/// per turn so a single reply sees one consistent cap. Documented here next
/// to the other `CORTEX_` variables this module reads
/// (`CORTEX_PROVIDER_GATEWAY_MAX_MICRO_USD` and
/// `CORTEX_PROVIDER_GATEWAY_FUNDED_MICRO_USD` live in
/// `provider_gateway_http.rs`).
///
/// `CORTEX_CHAT_AGENT_TURN_CAP_PER_MINUTE` — default 20 if unset or
/// unparseable.
fn turn_cap_per_minute() -> u32 {
    std::env::var("CORTEX_CHAT_AGENT_TURN_CAP_PER_MINUTE")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(20)
}

/// Rolling one-minute windows of billed-turn timestamps, keyed by
/// conversation (or by reply id for a conversation-less reply, which then
/// only limits itself). In-process only: `CORTEX_SINGLE_NODE=1` is required
/// for this server (see `AGENTS.md`), so there is exactly one process to
/// hold this state.
static TURN_WINDOWS: Lazy<Mutex<HashMap<String, VecDeque<i64>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

const TURN_WINDOW_MS: i64 = 60_000;

/// Record a billed turn's timestamp against `key` and refuse if that pushes
/// the rolling one-minute count over `cap`. The timestamp is recorded only
/// when the turn is allowed to proceed — a refused attempt does not count
/// against the window it was refused from.
fn try_acquire_turn_slot(key: &str, cap: u32, now_ms: i64) -> bool {
    let mut windows = TURN_WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
    let window = windows.entry(key.to_string()).or_default();
    while window.front().is_some_and(|t| now_ms - *t >= TURN_WINDOW_MS) {
        window.pop_front();
    }
    if window.len() as u32 >= cap {
        return false;
    }
    window.push_back(now_ms);
    true
}

#[cfg(test)]
fn reset_turn_windows_for_test() {
    TURN_WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Round a micro-USD cost up to the nearest whole credit. `i64::div_ceil` is
/// still unstable on this toolchain, so this spells out the same arithmetic
/// by hand rather than reaching for a nightly feature.
fn ceil_div(numerator: i64, denominator: i64) -> i64 {
    (numerator + denominator - 1) / denominator
}

/// The chat model tiers, mapped to Claude model ids that are already priced
/// on every seeded list, so picking them here never needs a second place to
/// keep in sync with what the price list actually publishes.
pub(crate) fn model_for_tier(tier: Option<&str>) -> &'static str {
    match tier.unwrap_or("fast") {
        "powerful" => "claude-opus-4-6",
        "balanced" => "claude-sonnet-4-6",
        _ => "claude-haiku-4-5",
    }
}

/// A refusal a user should be told about in plain words. Never a wrapped
/// gateway error — those are logged, not shown, because they describe the
/// gateway's internals rather than anything the user can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PaidReplyError {
    /// The gateway is off, misconfigured, or the model has no price. Cortex's
    /// problem, not the user's.
    Unavailable,
    /// The user has nothing left to spend.
    NoCredits,
    /// The user has some credits, but not enough to cover the longest reply
    /// this model could send.
    NotEnoughCredits,
    /// The supplier call itself failed after a reservation was attempted.
    Provider(String),
    /// This conversation has spent its per-minute allowance of billed agent
    /// turns. Distinct from `NotEnoughCredits`: this is a rate limit, not a
    /// balance problem, and it resets on its own a minute later.
    TurnCapExceeded,
}

impl PaidReplyError {
    fn user_message(&self) -> String {
        match self {
            PaidReplyError::Unavailable => {
                "Cortex chat is temporarily unavailable. Please try again in a moment.".into()
            }
            PaidReplyError::NoCredits => {
                "You're out of credits. Go to Settings → Billing to buy more or subscribe.".into()
            }
            PaidReplyError::NotEnoughCredits => {
                "You don't have enough credits left for a reply this long. Go to Settings → Billing to buy more or subscribe.".into()
            }
            PaidReplyError::Provider(_) => {
                "The model provider could not complete this reply. Please try again in a moment.".into()
            }
            PaidReplyError::TurnCapExceeded => {
                "This conversation is using tools too quickly right now. Please wait a moment and try again.".into()
            }
        }
    }
}

/// One tool call and its outcome, recorded so the caller can show the user
/// what the agent looked at. Never the raw tool output — see
/// `agent_tools::render_tool_output` for why that stays out of anything a
/// human reads directly, and out of this summary too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolActivity {
    pub tool_name: String,
    pub ok: bool,
}

#[derive(Debug)]
pub(crate) struct PaidReply {
    /// The assistant's final text, concatenated from every text block the
    /// last turn returned. May be non-empty even when the turn cap was hit
    /// mid-loop — see `send_paid_reply`'s doc comment.
    pub text: String,
    /// Whole credits charged, summed across every billed turn.
    pub charged_credits: i64,
    /// Every tool call this reply made, in order, across every turn.
    pub tool_activity: Vec<ToolActivity>,
}

/// One billed model turn: reserve, call the supplier, settle, and charge.
/// Returns the raw response body and the credits this one turn cost.
///
/// `turn` is folded into the gateway's attempt id and the ledger's charge
/// key (`{reply_id}:turn{n}`), so each turn is its own idempotency unit —
/// replaying turn 2 never re-charges turn 1, and never re-charges turn 2
/// either once it has settled.
#[allow(clippy::too_many_arguments)]
async fn send_one_turn<T: ProviderTransport + Clone>(
    db: &Database,
    signing_key: &str,
    supplier_key: &str,
    transport: T,
    max_micro_usd: i64,
    funded_micro_usd: i64,
    micros_per_credit: i64,
    user_id: &str,
    run_id: &str,
    provider: &str,
    model: &str,
    system_prompt: &str,
    messages: &[Value],
    tools: &[Value],
    reply_id: &str,
    turn: u32,
    now_ms: i64,
) -> Result<(Value, i64), PaidReplyError> {
    let attempt_id = format!("chat-reply:{reply_id}:turn{turn}");
    let expires_at_ms = now_ms + LEASE_MS;

    let (_authorization_id, signed) = provider_gateway_http::create_authorization_and_capability(
        db,
        signing_key,
        user_id,
        run_id,
        &attempt_id,
        provider,
        model,
        max_micro_usd,
        funded_micro_usd,
        expires_at_ms,
        now_ms,
    )
    .ok_or(PaidReplyError::Unavailable)?;

    let mut body = serde_json::json!({
        "model": model,
        "max_tokens": MAX_OUTPUT_TOKENS,
        "system": system_prompt,
        "messages": messages,
        "stream": false,
    });
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools.to_vec());
    }
    let request = GatewayRequest {
        request_key: format!("chat:{attempt_id}"),
        tenant_id: user_id.to_string(),
        run_id: run_id.to_string(),
        attempt_id,
        provider: provider.to_string(),
        model: model.to_string(),
        max_output_tokens: MAX_OUTPUT_TOKENS,
        body,
        capability: signed,
    };

    let gateway = ProviderGateway::new(db, signing_key.as_bytes(), supplier_key, transport);
    let outcome = gateway
        .forward(request, now_ms)
        .await
        .map_err(|e| match e {
            // The reservation is refused when the cap (at most the user's
            // balance) cannot cover the worst-case cost of this turn.
            GatewayError::Reservation(detail) => {
                tracing::info!(user_id, %detail, turn, "chat: spend reservation refused");
                PaidReplyError::NotEnoughCredits
            }
            other => {
                tracing::error!(user_id, error = %other, turn, "chat: gateway call failed");
                PaidReplyError::Provider(other.to_string())
            }
        })?;

    let Some(body) = outcome.body else {
        // A replay of a turn id that already resolved. Turn ids are minted
        // once per (reply, turn) pair, so this should not happen in
        // practice; treat it as the caller asking twice rather than as a
        // supplier failure.
        return Err(PaidReplyError::Provider(
            "this turn was already sent".into(),
        ));
    };

    let charged_credits = match outcome.reservation.observed_micro_usd {
        Some(observed) if observed > 0 => {
            let credits = ceil_div(observed, micros_per_credit);
            if let Err(e) = db.deduct_credits(
                user_id,
                credits,
                "Cortex-paid chat reply",
                &ChargeKey::per_unit(format!("{reply_id}:turn{turn}")),
            ) {
                // The turn already succeeded and the user already has its
                // output; failing the reply over a ledger write would
                // double-punish them for Cortex's bug. The reservation is
                // the source of truth for what was actually spent, so
                // nothing is lost.
                tracing::error!(user_id, reply_id, turn, error = %e, "chat: turn succeeded but charging credits failed");
            }
            credits
        }
        Some(_) => 0,
        None => {
            // No observed usage: never invent a price. The reservation
            // stays exactly as the gateway left it (reserved or
            // unresolved) for reconciliation; this module does not touch
            // it further.
            tracing::error!(
                user_id,
                reply_id,
                turn,
                "chat: turn succeeded with no observed usage; not charging"
            );
            0
        }
    };

    Ok((body, charged_credits))
}

/// Pull every `tool_use` block out of an assistant turn's content array.
/// Returns `(id, name, input)` triples, in the order Anthropic sent them.
fn tool_uses(body: &Value) -> Vec<(String, String, Value)> {
    body.get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
                .filter_map(|b| {
                    let id = b.get("id")?.as_str()?.to_string();
                    let name = b.get("name")?.as_str()?.to_string();
                    let input = b.get("input").cloned().unwrap_or(serde_json::json!({}));
                    Some((id, name, input))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Run the tool loop: reserve, call the supplier, settle, and charge for up
/// to [`MAX_AGENT_TURNS`] turns, executing any `tool_use` blocks the model
/// asks for between turns. Independent of the SSE plumbing so it can be
/// tested directly against a stub transport and an in-memory database.
///
/// Every turn is its own charge (see `send_one_turn`), so a reply that used
/// three turns to answer shows up as three ledger lines, not one. Before
/// each turn this checks that the user's balance still covers a reservation
/// and that the conversation has not spent its per-minute turn allowance
/// (`CORTEX_CHAT_AGENT_TURN_CAP_PER_MINUTE`, `turn_cap_per_minute`); either
/// stops the loop and returns whatever text the last turn produced, with no
/// charge for the turn that did not run.
///
/// `reply_id` is the caller's uuid for this one reply; each turn's charge
/// key is derived from it (`{reply_id}:turn{n}`), so replaying the whole
/// call charges each turn at most once.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn send_paid_reply<T: ProviderTransport + Clone>(
    db: &Database,
    signing_key: &str,
    supplier_key: &str,
    transport: T,
    limits: SpendLimits,
    user_id: &str,
    conversation_id: Option<&str>,
    model: &str,
    system_prompt: &str,
    user_message: &str,
    reply_id: &str,
    now_ms: i64,
) -> Result<PaidReply, PaidReplyError> {
    // Chat is paid by Cortex on the Anthropic path only (`ProviderPath::Cortex`);
    // a run that wants a different supplier goes through the HTTP gateway
    // instead, where the provider comes from the caller's own capability.
    const PROVIDER: &str = "claude";
    let price_list = db.active_price_list().ok_or(PaidReplyError::Unavailable)?;
    if price_list.model(PROVIDER, model).is_none() {
        tracing::error!(model, "chat: no price for this model tier; refusing");
        return Err(PaidReplyError::Unavailable);
    }

    let run_id = conversation_id
        .map(|c| format!("chat:{c}"))
        .unwrap_or_else(|| format!("chat:{reply_id}"));
    // The per-minute cap is per conversation, not per reply: a conversation
    // with no id (should not happen from the SSE entry point, but this
    // function is also called directly in tests) falls back to the reply's
    // own id, which only limits itself.
    let turn_cap_key = conversation_id.unwrap_or(reply_id).to_string();
    let turn_cap = turn_cap_per_minute();

    let tools = agent_tools::tool_definitions();
    let mut messages = vec![serde_json::json!({"role": "user", "content": user_message})];
    let mut charged_credits_total: i64 = 0;
    let mut tool_activity = Vec::new();
    let mut last_text = String::new();
    // Set when the loop stops early (turn > 1) instead of finishing on its
    // own, so we can tell the user their answer may be incomplete instead of
    // silently handing back whatever partial text the last turn produced.
    let mut stopped_reason: Option<&'static str> = None;

    for turn in 1..=MAX_AGENT_TURNS {
        // Re-read the balance every turn: a prior turn in this same loop
        // already spent some of it.
        let balance = db
            .get_credit_balance_row(user_id)
            .filter(|b| b.subscription_remaining + b.pack_remaining > 0)
            .ok_or(PaidReplyError::NoCredits)?;
        let balance_micro_usd = (balance.subscription_remaining + balance.pack_remaining)
            .saturating_mul(price_list.micros_per_credit);
        let max_micro_usd = limits.max_micro_usd.min(balance_micro_usd);
        if max_micro_usd <= 0 {
            if turn == 1 {
                return Err(PaidReplyError::NoCredits);
            }
            tracing::info!(user_id, reply_id, turn, "chat: out of credits mid-loop; stopping");
            stopped_reason = Some(
                "\n\n(This answer may be incomplete: you're out of credits, so I stopped \
                 before finishing.)",
            );
            break;
        }

        if !try_acquire_turn_slot(&turn_cap_key, turn_cap, now_ms) {
            if turn == 1 {
                return Err(PaidReplyError::TurnCapExceeded);
            }
            tracing::info!(user_id, reply_id, turn, "chat: per-minute turn cap hit; stopping");
            stopped_reason = Some(
                "\n\n(This answer may be incomplete: this conversation is using tools too \
                 quickly right now, so I stopped before finishing. Please wait a moment and \
                 try again.)",
            );
            break;
        }

        let (body, charged) = send_one_turn(
            db,
            signing_key,
            supplier_key,
            transport.clone(),
            max_micro_usd,
            limits.funded_micro_usd,
            price_list.micros_per_credit,
            user_id,
            &run_id,
            PROVIDER,
            model,
            system_prompt,
            &messages,
            &tools,
            reply_id,
            turn,
            now_ms,
        )
        .await?;
        charged_credits_total += charged;
        last_text = extract_text(&body);

        let uses = tool_uses(&body);
        if uses.is_empty() {
            break;
        }

        // The assistant's own turn (including its tool_use blocks) has to
        // go back verbatim, or the supplier rejects the next turn as
        // missing the calls its tool_results answer.
        messages.push(serde_json::json!({
            "role": "assistant",
            "content": body.get("content").cloned().unwrap_or(Value::Array(vec![])),
        }));

        let mut tool_results = Vec::with_capacity(uses.len());
        for (tool_use_id, name, input) in uses {
            let (content, is_error) = match agent_tools::execute(db, user_id, &name, &input) {
                Ok(rendered) => {
                    tool_activity.push(ToolActivity {
                        tool_name: name.clone(),
                        ok: true,
                    });
                    (rendered, false)
                }
                Err(e) => {
                    tool_activity.push(ToolActivity {
                        tool_name: name.clone(),
                        ok: false,
                    });
                    (e.message(), true)
                }
            };
            tool_results.push(serde_json::json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": content,
                "is_error": is_error,
            }));
        }
        messages.push(serde_json::json!({
            "role": "user",
            "content": tool_results,
        }));

        if turn == MAX_AGENT_TURNS {
            tracing::info!(user_id, reply_id, "chat: turn cap reached with an open tool call");
        }
    }

    if let Some(reason) = stopped_reason {
        last_text.push_str(reason);
    }

    Ok(PaidReply {
        text: last_text,
        charged_credits: charged_credits_total,
        tool_activity,
    })
}

/// A minimal, human-readable line summarizing what the agent looked at this
/// reply, e.g. "Checked your runs, credit balance." `None` when no tool was
/// called, so a plain reply gets no extra message.
fn tool_activity_summary(activity: &[ToolActivity]) -> Option<String> {
    if activity.is_empty() {
        return None;
    }
    fn phrase(name: &str) -> &str {
        match name {
            "list_runs" => "your runs",
            "run_status" => "a run's status",
            "list_projects" => "your projects",
            "credit_balance" => "your credit balance",
            "run_estimate" => "a cost estimate",
            other => other,
        }
    }
    let mut seen: Vec<&str> = Vec::new();
    for a in activity {
        let p = phrase(&a.tool_name);
        if !seen.contains(&p) {
            seen.push(p);
        }
    }
    Some(format!("Checked {}.", seen.join(", ")))
}

fn extract_text(body: &Value) -> String {
    body.get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

/// The SSE-facing entry point: resolve whether the gateway is usable, run the
/// paid-reply pipeline, and turn the outcome into the standard `StepEvent`
/// sequence — `Started`, then either `Output` + `Completed` or `Failed` —
/// plus the conversation writes.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run(
    state: Arc<AppState>,
    user_id: String,
    conversation_id: Option<String>,
    model: String,
    system_prompt: String,
    user_message: String,
    tx: mpsc::Sender<StepEvent>,
) {
    let _ = tx
        .send(StepEvent::Started {
            step_id: "chat".into(),
            provider: "cortex".into(),
            model: model.clone(),
        })
        .await;

    let Some(db) = state.db.as_ref() else {
        let _ = tx
            .send(StepEvent::Failed {
                step_id: "chat".into(),
                error: PaidReplyError::Unavailable.user_message(),
            })
            .await;
        return;
    };
    let Some(GatewayUsable {
        signing_key,
        supplier_key,
        transport,
    }) = provider_gateway_http::gateway_usable()
    else {
        let _ = tx
            .send(StepEvent::Failed {
                step_id: "chat".into(),
                error: PaidReplyError::Unavailable.user_message(),
            })
            .await;
        return;
    };

    let Some(limits) = SpendLimits::from_env() else {
        let _ = tx
            .send(StepEvent::Failed {
                step_id: "chat".into(),
                error: PaidReplyError::Unavailable.user_message(),
            })
            .await;
        return;
    };

    let reply_id = uuid::Uuid::new_v4().to_string();
    let now_ms = chrono::Utc::now().timestamp_millis();

    match send_paid_reply(
        db,
        &signing_key,
        &supplier_key,
        transport,
        limits,
        &user_id,
        conversation_id.as_deref(),
        &model,
        &system_prompt,
        &user_message,
        &reply_id,
        now_ms,
    )
    .await
    {
        Ok(reply) => {
            tracing::info!(
                credits = reply.charged_credits,
                tool_calls = reply.tool_activity.len(),
                %reply_id,
                "cortex-paid chat reply charged"
            );
            for activity in &reply.tool_activity {
                let _ = tx
                    .send(StepEvent::ToolActivity {
                        step_id: "chat".into(),
                        tool_name: activity.tool_name.clone(),
                        ok: activity.ok,
                    })
                    .await;
            }
            let _ = tx
                .send(StepEvent::Output {
                    step_id: "chat".into(),
                    line: reply.text.clone(),
                })
                .await;
            let _ = tx
                .send(StepEvent::Completed {
                    step_id: "chat".into(),
                    exit_code: 0,
                })
                .await;
            if let Some(cid) = &conversation_id {
                if let Some(summary) = tool_activity_summary(&reply.tool_activity) {
                    db.add_message(cid, "tool", &summary, Some("cortex"), None);
                }
                if !reply.text.is_empty() {
                    db.add_message(cid, "assistant", &reply.text, Some("cortex"), None);
                }
            }
        }
        Err(error) => {
            let _ = tx
                .send(StepEvent::Failed {
                    step_id: "chat".into(),
                    error: error.user_message(),
                })
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use crate::provider_gateway::{
        ObservedUsage, TransportFailure, TransportFailureKind, TransportResponse,
    };

    use super::*;

    const NOW: i64 = 1_800_000_000_000;
    const MODEL: &str = "claude-haiku-4-5";
    const SIGNING_KEY: &str = "chat-paid-test-signing-key-32-bytes!!";
    const SUPPLIER_KEY: &str = "supplier-secret";

    fn test_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("chat-paid.sqlite"));
        (dir, db)
    }

    #[derive(Clone)]
    struct FixedTransport {
        calls: Arc<AtomicUsize>,
        response: Result<TransportResponse, TransportFailure>,
    }

    impl FixedTransport {
        fn ok(input_tokens: i64, output_tokens: i64, text: &str) -> Self {
            Self {
                calls: Arc::new(AtomicUsize::new(0)),
                response: Ok(TransportResponse {
                    body: serde_json::json!({
                        "id": "msg_test",
                        "type": "message",
                        "role": "assistant",
                        "model": MODEL,
                        "content": [{"type": "text", "text": text}],
                        "stop_reason": "end_turn",
                        "stop_sequence": null,
                        "usage": {"input_tokens": input_tokens, "output_tokens": output_tokens}
                    }),
                    upstream_request_id: Some("upstream-1".into()),
                    usage: Some(ObservedUsage {
                        input_tokens,
                        cached_input_tokens: 0,
                        output_tokens,
                    }),
                }),
            }
        }

        fn failing() -> Self {
            Self {
                calls: Arc::new(AtomicUsize::new(0)),
                response: Err(TransportFailure {
                    kind: TransportFailureKind::Rejected,
                    upstream_request_id: None,
                    message: "supplier refused the request".into(),
                }),
            }
        }
    }

    impl ProviderTransport for FixedTransport {
        fn forward(
            &self,
            supplier_key: &str,
            _request: &GatewayRequest,
        ) -> impl Future<Output = Result<TransportResponse, TransportFailure>> + Send {
            assert_eq!(supplier_key, SUPPLIER_KEY);
            self.calls.fetch_add(1, Ordering::SeqCst);
            std::future::ready(self.response.clone())
        }
    }

    #[tokio::test]
    async fn a_reply_charges_exactly_the_ceiling_of_observed_cost() {
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 100).unwrap();
        let price_list = db.active_price_list().unwrap();
        let rate = price_list.model("claude", MODEL).unwrap();
        // 10 input, 10 output tokens at the seeded haiku rate.
        let observed_micros = rate.cost_micros(10, 0, 10);
        assert!(observed_micros > 0, "fixture must actually cost something");
        let expected_credits = ceil_div(observed_micros, price_list.micros_per_credit);

        let limits = SpendLimits {
            max_micro_usd: 1000000000,
            funded_micro_usd: 1000000000,
        };
        let reply = send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            FixedTransport::ok(10, 10, "hello there"),
            limits,
            "user-1",
            Some("conv-1"),
            MODEL,
            "system",
            "hi",
            "reply-1",
            NOW,
        )
        .await
        .expect("reply should succeed");

        assert_eq!(reply.text, "hello there");
        assert_eq!(reply.charged_credits, expected_credits);
        let balance = db.get_credit_balance_row("user-1").unwrap();
        assert_eq!(balance.subscription_remaining, 100 - expected_credits);
    }

    #[tokio::test]
    async fn a_supplier_failure_charges_nothing() {
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 100).unwrap();

        let limits = SpendLimits {
            max_micro_usd: 1000000000,
            funded_micro_usd: 1000000000,
        };
        let error = send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            FixedTransport::failing(),
            limits,
            "user-1",
            Some("conv-1"),
            MODEL,
            "system",
            "hi",
            "reply-2",
            NOW,
        )
        .await
        .expect_err("supplier failure must not succeed");

        assert!(matches!(error, PaidReplyError::Provider(_)));
        let balance = db.get_credit_balance_row("user-1").unwrap();
        assert_eq!(balance.subscription_remaining, 100, "nothing was charged");
    }

    #[tokio::test]
    async fn zero_balance_refuses_before_any_reservation_exists() {
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 0).unwrap();

        let limits = SpendLimits {
            max_micro_usd: 1000000000,
            funded_micro_usd: 1000000000,
        };
        let error = send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            FixedTransport::ok(10, 10, "should not be called"),
            limits,
            "user-1",
            Some("conv-1"),
            MODEL,
            "system",
            "hi",
            "reply-3",
            NOW,
        )
        .await
        .expect_err("zero balance must refuse");

        assert_eq!(error, PaidReplyError::NoCredits);
        assert!(
            db.get_provider_reservation("chat:chat-reply:reply-3:turn1")
                .is_none(),
            "nothing should have been reserved"
        );
    }

    #[tokio::test]
    async fn the_same_reply_id_charged_twice_charges_once() {
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 100).unwrap();

        let limits = SpendLimits {
            max_micro_usd: 1000000000,
            funded_micro_usd: 1000000000,
        };
        // The first call succeeds; the repeat is refused (its authorization
        // already exists) — either way it must not charge a second time.
        for attempt in 0..2 {
            let result = send_paid_reply(
                &db,
                SIGNING_KEY,
                SUPPLIER_KEY,
                FixedTransport::ok(10, 10, "hello there"),
                limits,
                "user-1",
                Some("conv-1"),
                MODEL,
                "system",
                "hi",
                "reply-4",
                NOW,
            )
            .await;
            if attempt == 0 {
                result.expect("first reply should succeed");
            } else {
                assert!(result.is_err(), "a replayed reply id must be refused");
            }
        }

        let price_list = db.active_price_list().unwrap();
        let rate = price_list.model("claude", MODEL).unwrap();
        let expected_credits = ceil_div(rate.cost_micros(10, 0, 10), price_list.micros_per_credit);
        let balance = db.get_credit_balance_row("user-1").unwrap();
        assert_eq!(
            balance.subscription_remaining,
            100 - expected_credits,
            "the replayed reply must not charge twice"
        );
    }

    #[tokio::test]
    async fn the_spend_cap_never_exceeds_the_users_balance() {
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 1).unwrap();
        // A deliberately huge env cap: the user's own balance must still win.
        let limits = SpendLimits {
            max_micro_usd: 1000000000000,
            funded_micro_usd: 1000000000000,
        };

        let reply = send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            FixedTransport::ok(1, 1, "tiny reply"),
            limits,
            "user-1",
            Some("conv-1"),
            MODEL,
            "system",
            "hi",
            "reply-5",
            NOW,
        )
        .await
        .expect("reply should succeed");

        let price_list = db.active_price_list().unwrap();
        let authorization_id = "gateway-auth:chat-reply:reply-5:turn1";
        let reservation = db
            .get_provider_reservation("chat:chat-reply:reply-5:turn1")
            .unwrap();
        assert_eq!(reservation.authorization_id, authorization_id);
        // The authorization itself is capped at the user's one credit, in
        // micros — never the (much larger) env value.
        assert!(
            reservation.reserved_micro_usd <= price_list.micros_per_credit,
            "reservation {} must not exceed the user's one-credit balance ({})",
            reservation.reserved_micro_usd,
            price_list.micros_per_credit
        );
        assert_eq!(reply.charged_credits, 1);
    }
}
