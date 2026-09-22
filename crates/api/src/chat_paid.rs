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
    // Remove-then-reinsert instead of `entry().or_default()`, so a key with
    // an empty window (every timestamp aged out) does not sit in the map
    // forever — a long-lived server otherwise accumulates one entry per
    // distinct conversation/reply id it has ever seen.
    let mut window = windows.remove(key).unwrap_or_default();
    while window
        .front()
        .is_some_and(|t| now_ms - *t >= TURN_WINDOW_MS)
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

#[cfg(test)]
fn reset_turn_windows_for_test() {
    TURN_WINDOWS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
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

/// One model turn: reserve, call the supplier, and settle. Returns the raw
/// response body and the observed micro-USD cost of this one turn — never
/// charged here. Charging is one ledger write per whole reply (see
/// `send_paid_reply`), because Josh's rule is "charge actual cost": a reply
/// that took three turns to answer is still one line on the user's ledger,
/// for the sum of what those three turns actually cost.
///
/// `turn` is folded into the gateway's attempt id (`{reply_id}:turn{n}`), so
/// each turn is its own reservation and idempotency unit — replaying turn 2
/// never re-reserves turn 1, and never re-reserves turn 2 either once it has
/// settled.
#[allow(clippy::too_many_arguments)]
async fn send_one_turn<T: ProviderTransport + Clone>(
    db: &Database,
    signing_key: &str,
    supplier_key: &str,
    transport: T,
    max_micro_usd: i64,
    funded_micro_usd: i64,
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

    let observed_micro_usd = match outcome.reservation.observed_micro_usd {
        Some(observed) if observed > 0 => observed,
        Some(_) => 0,
        None => {
            // No observed usage: never invent a price. The reservation
            // stays exactly as the gateway left it (reserved or
            // unresolved) for reconciliation; this module does not touch
            // it further, and this turn contributes nothing to the
            // reply's eventual single charge.
            tracing::error!(
                user_id,
                reply_id,
                turn,
                "chat: turn succeeded with no observed usage; contributes nothing to the charge"
            );
            0
        }
    };

    Ok((body, observed_micro_usd))
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
    // Unused now that every turn stamps its own `chrono::Utc::now()` for
    // capability expiry and rate-limit windows (a fixed value shared across
    // every turn of a slow, multi-turn reply would let later turns'
    // capabilities and DB timestamps drift from real wall-clock time).
    // Kept so every existing call site — production and test — still
    // compiles unchanged; a future cleanup can drop it from the signature.
    _now_ms: i64,
    turn_cap_per_minute: u32,
    // Live tool-activity events, sent as each tool call finishes rather
    // than batched after the whole reply completes, so the UI can show
    // "Checked your runs" while a slow multi-turn reply is still running.
    // `None` in tests that don't care about the stream.
    tool_events: Option<&mpsc::Sender<StepEvent>>,
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
    // The per-minute cap is per (user, conversation): two users in the same
    // conversation-less state (falling back to their own reply id) must
    // never share a window, and a bare conversation id must not let one
    // user's turns count against another's cap.
    let turn_cap_key = format!("{user_id}:{}", conversation_id.unwrap_or(reply_id));
    let turn_cap = turn_cap_per_minute;

    let tools = agent_tools::tool_definitions();
    let no_tools: Vec<Value> = Vec::new();
    let mut messages = vec![serde_json::json!({"role": "user", "content": user_message})];
    // Summed across every turn and charged once at the very end (or once at
    // the point of an early, partial stop) — see `send_one_turn`'s doc
    // comment for why a reply is one ledger line, not one per turn.
    let mut total_observed_micro_usd: i64 = 0;
    let mut tool_activity = Vec::new();
    let mut last_text = String::new();
    // Set when the loop stops early (turn > 1) instead of finishing on its
    // own, so we can tell the user their answer may be incomplete instead of
    // silently handing back whatever partial text the last turn produced.
    let mut stopped_reason: Option<&'static str> = None;

    for turn in 1..=MAX_AGENT_TURNS {
        // Stamped fresh every turn: a slow, multi-turn reply must not let
        // capability expiry, reservation timestamps, or the per-minute
        // window all pin to the moment the reply started.
        let now_ms = chrono::Utc::now().timestamp_millis();

        // Read the balance unfiltered. It is not decremented until the
        // single end-of-reply charge (see below), so what earlier turns in
        // *this* loop already cost is subtracted here — as whole credits,
        // rounded up the same way the final charge will round, so "credits
        // used so far" mid-loop always matches what actually gets deducted
        // at the end. Nothing left on turn 1 is a hard refusal — nothing
        // has been reserved or charged yet, so there is nothing to
        // preserve. Nothing left on turn 2+ is a graceful stop: earlier
        // turns already did billable work that must still be charged once,
        // at the end, and returned as a partial answer.
        let balance = db
            .get_credit_balance_row(user_id)
            .ok_or(PaidReplyError::NoCredits)?;
        let credits_used_so_far = if total_observed_micro_usd > 0 {
            ceil_div(total_observed_micro_usd, price_list.micros_per_credit)
        } else {
            0
        };
        let remaining_credits =
            balance.subscription_remaining + balance.pack_remaining - credits_used_so_far;
        if remaining_credits <= 0 {
            if turn == 1 {
                return Err(PaidReplyError::NoCredits);
            }
            tracing::info!(
                user_id,
                reply_id,
                turn,
                "chat: out of credits mid-loop; stopping"
            );
            stopped_reason = Some(
                "\n\n(This answer may be incomplete: you're out of credits, so I stopped \
                 before finishing.)",
            );
            break;
        }
        let max_micro_usd = limits
            .max_micro_usd
            .min(remaining_credits.saturating_mul(price_list.micros_per_credit));

        if !try_acquire_turn_slot(&turn_cap_key, turn_cap, now_ms) {
            if turn == 1 {
                return Err(PaidReplyError::TurnCapExceeded);
            }
            tracing::info!(
                user_id,
                reply_id,
                turn,
                "chat: per-minute turn cap hit; stopping"
            );
            stopped_reason = Some(
                "\n\n(This answer may be incomplete: this conversation is using tools too \
                 quickly right now, so I stopped before finishing. Please wait a moment and \
                 try again.)",
            );
            break;
        }

        // On the last turn there is no next turn to send tool results back
        // on, so don't offer tools at all: the model has to answer in text
        // with whatever it already knows, instead of asking for a tool call
        // it will never see the result of.
        let turn_tools: &[Value] = if turn == MAX_AGENT_TURNS {
            &no_tools
        } else {
            &tools
        };

        let turn_outcome = send_one_turn(
            db,
            signing_key,
            supplier_key,
            transport.clone(),
            max_micro_usd,
            limits.funded_micro_usd,
            user_id,
            &run_id,
            PROVIDER,
            model,
            system_prompt,
            &messages,
            turn_tools,
            reply_id,
            turn,
            now_ms,
        )
        .await;

        let (body, observed_micro_usd) = match turn_outcome {
            Ok(pair) => pair,
            Err(e) if turn == 1 => return Err(e),
            Err(PaidReplyError::NotEnoughCredits) => {
                tracing::info!(
                    user_id,
                    reply_id,
                    turn,
                    "chat: reservation refused mid-loop; stopping"
                );
                stopped_reason = Some(
                    "\n\n(This answer may be incomplete: you don't have enough credits left \
                     for another turn, so I stopped before finishing.)",
                );
                break;
            }
            Err(other) => {
                tracing::error!(
                    user_id,
                    reply_id,
                    turn,
                    error = ?other,
                    "chat: turn failed mid-loop; stopping with a partial answer"
                );
                stopped_reason = Some(
                    "\n\n(This answer may be incomplete: something went wrong partway \
                     through, so I stopped early.)",
                );
                break;
            }
        };
        total_observed_micro_usd += observed_micro_usd;
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
            let (activity, content, is_error) =
                match agent_tools::execute(db, user_id, &name, &input) {
                    Ok(rendered) => (
                        ToolActivity {
                            tool_name: name.clone(),
                            ok: true,
                        },
                        rendered,
                        false,
                    ),
                    Err(e) => (
                        ToolActivity {
                            tool_name: name.clone(),
                            ok: false,
                        },
                        e.message(),
                        true,
                    ),
                };
            if let Some(tx) = tool_events {
                let _ = tx
                    .send(StepEvent::ToolActivity {
                        step_id: "chat".into(),
                        tool_name: activity.tool_name.clone(),
                        ok: activity.ok,
                    })
                    .await;
            }
            tool_activity.push(activity);
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
            tracing::info!(
                user_id,
                reply_id,
                "chat: turn cap reached with an open tool call"
            );
        }
    }

    if let Some(reason) = stopped_reason {
        last_text.push_str(reason);
    }

    // One ledger line per reply, for the sum of what every turn actually
    // cost — including a reply that stopped early, so the turns that did
    // run are never given away for free.
    let charged_credits_total = if total_observed_micro_usd > 0 {
        let credits = ceil_div(total_observed_micro_usd, price_list.micros_per_credit);
        db.deduct_credits(
            user_id,
            credits,
            "Cortex-paid chat reply",
            &ChargeKey::for_chat_reply(reply_id),
        )
        .map_err(|e| {
            tracing::error!(user_id, reply_id, error = %e, "chat: failed to charge for reply");
            PaidReplyError::Unavailable
        })?;
        credits
    } else {
        0
    };

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
    let turn_cap = turn_cap_per_minute();

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
        turn_cap,
        Some(&tx),
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
            // Tool activity was already streamed live, turn by turn, inside
            // `send_paid_reply` (via the `tool_events` sender above) — not
            // replayed here, so the UI sees "Checked your runs" while a
            // slow multi-turn reply is still in progress rather than only
            // once it finishes.
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
                    // Saved as "assistant", not "tool": the frontend has no
                    // renderer for a "tool" role and shows it as if the
                    // user had said it after a page reload.
                    db.add_message(cid, "assistant", &summary, Some("cortex"), None);
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
            20,
            None,
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
            20,
            None,
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
            20,
            None,
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
                20,
                None,
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
            20,
            None,
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

    fn text_response(
        input_tokens: i64,
        output_tokens: i64,
        text: &str,
    ) -> Result<TransportResponse, TransportFailure> {
        FixedTransport::ok(input_tokens, output_tokens, text).response
    }

    fn tool_use_response(
        input_tokens: i64,
        output_tokens: i64,
        tool_use_id: &str,
        name: &str,
        input: Value,
    ) -> Result<TransportResponse, TransportFailure> {
        Ok(TransportResponse {
            body: serde_json::json!({
                "id": "msg_test",
                "type": "message",
                "role": "assistant",
                "model": MODEL,
                "content": [{"type": "tool_use", "id": tool_use_id, "name": name, "input": input}],
                "stop_reason": "tool_use",
                "stop_sequence": null,
                "usage": {"input_tokens": input_tokens, "output_tokens": output_tokens}
            }),
            upstream_request_id: Some("upstream-1".into()),
            usage: Some(ObservedUsage {
                input_tokens,
                cached_input_tokens: 0,
                output_tokens,
            }),
        })
    }

    /// Returns one queued response per call; repeats the last one forever
    /// once the queue is exhausted, so a test can hand it e.g.
    /// [tool_use, tool_use, ..., final_text] or just [tool_use] to simulate
    /// a model that never stops calling tools.
    #[derive(Clone)]
    struct SequenceTransport {
        calls: Arc<AtomicUsize>,
        responses: Arc<Vec<Result<TransportResponse, TransportFailure>>>,
    }

    impl SequenceTransport {
        fn new(responses: Vec<Result<TransportResponse, TransportFailure>>) -> Self {
            assert!(!responses.is_empty());
            Self {
                calls: Arc::new(AtomicUsize::new(0)),
                responses: Arc::new(responses),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl ProviderTransport for SequenceTransport {
        fn forward(
            &self,
            supplier_key: &str,
            _request: &GatewayRequest,
        ) -> impl Future<Output = Result<TransportResponse, TransportFailure>> + Send {
            assert_eq!(supplier_key, SUPPLIER_KEY);
            let i = self.calls.fetch_add(1, Ordering::SeqCst);
            let idx = i.min(self.responses.len() - 1);
            std::future::ready(self.responses[idx].clone())
        }
    }

    fn big_limits() -> SpendLimits {
        SpendLimits {
            max_micro_usd: 1_000_000_000,
            funded_micro_usd: 1_000_000_000,
        }
    }

    #[tokio::test]
    async fn tool_use_turn_executes_and_bills_each_turn() {
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 1000).unwrap();
        let price_list = db.active_price_list().unwrap();
        let rate = price_list.model("claude", MODEL).unwrap();
        let per_turn_micro = rate.cost_micros(10, 0, 10);
        // One ledger line for the whole reply, for the ceiling of the sum of
        // every turn's observed cost — not one line (and one separate
        // rounding-up) per turn.
        let expected_credits = ceil_div(per_turn_micro * 2, price_list.micros_per_credit);

        let transport = SequenceTransport::new(vec![
            tool_use_response(10, 10, "toolu_1", "list_runs", serde_json::json!({})),
            text_response(10, 10, "here is your answer"),
        ]);

        let reply = send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            transport.clone(),
            big_limits(),
            "user-1",
            Some("conv-tool-1"),
            MODEL,
            "system",
            "how are my runs?",
            "reply-tool-1",
            NOW,
            20,
            None,
        )
        .await
        .expect("reply should succeed");

        assert_eq!(transport.call_count(), 2, "one turn per gateway call");
        assert_eq!(reply.text, "here is your answer");
        assert_eq!(reply.charged_credits, expected_credits);
        let balance = db.get_credit_balance_row("user-1").unwrap();
        assert_eq!(
            balance.subscription_remaining,
            1000 - expected_credits,
            "the ledger reflects one deduction for the whole reply"
        );
        assert_eq!(
            reply.tool_activity,
            vec![ToolActivity {
                tool_name: "list_runs".into(),
                ok: true
            }]
        );
    }

    #[tokio::test]
    async fn loop_stops_at_turn_cap() {
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 1_000_000).unwrap();
        // The model calls a tool every turn but the last: the last turn is
        // sent with no tools at all (see `send_paid_reply`), so a real model
        // has nothing to call and must answer in text instead.
        let mut responses: Vec<_> = (1..MAX_AGENT_TURNS)
            .map(|_| tool_use_response(10, 10, "toolu_1", "list_runs", serde_json::json!({})))
            .collect();
        responses.push(text_response(
            10,
            10,
            "here's what I found before running out of turns",
        ));
        let transport = SequenceTransport::new(responses);

        let reply = send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            transport.clone(),
            big_limits(),
            "user-1",
            Some("conv-tool-2"),
            MODEL,
            "system",
            "keep checking",
            "reply-tool-2",
            NOW,
            20,
            None,
        )
        .await
        .expect("reply should succeed even though it never got a final answer on its own");

        assert_eq!(transport.call_count(), MAX_AGENT_TURNS as usize);
        assert_eq!(reply.tool_activity.len(), (MAX_AGENT_TURNS - 1) as usize);
        assert!(
            !reply.text.is_empty(),
            "the final, tool-less turn must produce a text answer"
        );
    }

    /// A turn's reservation is refused when its worst-case cost would push
    /// this turn's own authorization over `SpendLimits.max_micro_usd` — an
    /// operator cap, independent of the user's balance. Tunes that cap to
    /// sit exactly at turn 1's reserved cost: turn 1 (a short user message)
    /// fits, turn 2 (the same messages plus the appended tool_use/
    /// tool_result turn) does not, since its serialized body is strictly
    /// larger.
    #[tokio::test]
    async fn reservation_refused_on_turn_two_returns_partial() {
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 1_000_000).unwrap();
        let price_list = db.active_price_list().unwrap();
        let rate = price_list.model("claude", MODEL).unwrap();

        fn ceiling_per_thousand(n: i64, rate: i64) -> i64 {
            (n * rate + 999) / 1_000
        }
        fn reserved_cost(body: &Value, rate: &crate::pricing::ModelPrice) -> i64 {
            let bytes = serde_json::to_vec(body).unwrap().len() as i64;
            ceiling_per_thousand(bytes, rate.input_micros_per_1k)
                + ceiling_per_thousand(MAX_OUTPUT_TOKENS, rate.output_micros_per_1k)
        }

        let tools = agent_tools::tool_definitions();
        let user_message = "keep checking";
        let tool_use_id = "toolu_1";
        let tool_name = "not_a_real_tool";

        let turn1_messages = serde_json::json!([{"role": "user", "content": user_message}]);
        let turn1_body = serde_json::json!({
            "model": MODEL, "max_tokens": MAX_OUTPUT_TOKENS, "system": "system",
            "messages": turn1_messages, "stream": false, "tools": tools,
        });
        let turn1_reserved = reserved_cost(&turn1_body, rate);

        let assistant_content = serde_json::json!([{"type": "tool_use", "id": tool_use_id, "name": tool_name, "input": {}}]);
        let tool_results = serde_json::json!([{
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": "unknown tool: not_a_real_tool",
            "is_error": true,
        }]);
        let turn2_messages = serde_json::json!([
            {"role": "user", "content": user_message},
            {"role": "assistant", "content": assistant_content},
            {"role": "user", "content": tool_results},
        ]);
        let turn2_body = serde_json::json!({
            "model": MODEL, "max_tokens": MAX_OUTPUT_TOKENS, "system": "system",
            "messages": turn2_messages, "stream": false, "tools": tools,
        });
        let turn2_reserved = reserved_cost(&turn2_body, rate);
        assert!(
            turn2_reserved > turn1_reserved,
            "turn 2's body must cost more to reserve than turn 1's for this test to prove anything"
        );

        let limits = SpendLimits {
            max_micro_usd: turn1_reserved,
            funded_micro_usd: 1_000_000_000,
        };
        let transport = SequenceTransport::new(vec![tool_use_response(
            10,
            10,
            tool_use_id,
            tool_name,
            serde_json::json!({}),
        )]);

        let reply = send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            transport.clone(),
            limits,
            "user-1",
            Some("conv-tool-5"),
            MODEL,
            "system",
            user_message,
            "reply-tool-5",
            NOW,
            20,
            None,
        )
        .await
        .expect("turn 1's work must still come back as a partial answer");

        assert_eq!(
            transport.call_count(),
            1,
            "turn 2's reservation is refused before any second gateway call"
        );
        assert!(
            reply.text.contains("enough credits left for another turn"),
            "partial answer must say why it stopped: {}",
            reply.text
        );
        let expected_credits = ceil_div(rate.cost_micros(10, 0, 10), price_list.micros_per_credit);
        assert_eq!(reply.charged_credits, expected_credits);
        let balance = db.get_credit_balance_row("user-1").unwrap();
        assert_eq!(balance.subscription_remaining, 1_000_000 - expected_credits);
    }

    #[tokio::test]
    async fn insufficient_credits_mid_loop_stops_without_charge() {
        let (_dir, db) = test_db();
        let price_list = db.active_price_list().unwrap();
        let rate = price_list.model("claude", MODEL).unwrap();
        let per_turn_credits = ceil_div(rate.cost_micros(10, 0, 10), price_list.micros_per_credit);
        // Exactly enough for one turn; the balance check before turn 2 must
        // see zero remaining and stop without a second charge.
        db.init_credit_balance("user-1", per_turn_credits).unwrap();

        let transport = SequenceTransport::new(vec![tool_use_response(
            10,
            10,
            "toolu_1",
            "list_runs",
            serde_json::json!({}),
        )]);

        let reply = send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            transport.clone(),
            big_limits(),
            "user-1",
            Some("conv-tool-3"),
            MODEL,
            "system",
            "keep checking",
            "reply-tool-3",
            NOW,
            20,
            None,
        )
        .await
        .expect("a partial answer, not an error, once at least one turn ran");

        assert_eq!(
            transport.call_count(),
            1,
            "no gateway call once credits are gone"
        );
        assert_eq!(
            reply.charged_credits, per_turn_credits,
            "only the turn that ran is charged"
        );
        assert!(
            reply.text.contains("out of credits"),
            "partial answer must say why it stopped: {}",
            reply.text
        );
        let balance = db.get_credit_balance_row("user-1").unwrap();
        assert_eq!(balance.subscription_remaining + balance.pack_remaining, 0);
    }

    #[tokio::test]
    async fn per_minute_cap_stops_loop() {
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 1_000_000).unwrap();
        reset_turn_windows_for_test();

        let transport = SequenceTransport::new(vec![tool_use_response(
            10,
            10,
            "toolu_1",
            "list_runs",
            serde_json::json!({}),
        )]);

        let reply = send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            transport.clone(),
            big_limits(),
            "user-1",
            Some("conv-tool-4"),
            MODEL,
            "system",
            "keep checking",
            "reply-tool-4",
            NOW,
            1,
            None,
        )
        .await;

        let reply = reply.expect("a partial answer once the cap is hit mid-loop");

        assert_eq!(
            transport.call_count(),
            1,
            "the second turn never reaches the gateway"
        );
        assert!(
            reply.text.contains("too quickly"),
            "partial answer must explain the rate limit: {}",
            reply.text
        );
    }
}
