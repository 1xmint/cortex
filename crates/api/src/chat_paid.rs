//! Chat replies paid for by Cortex, on Cortex's own Anthropic key, charged to
//! the user at observed cost.
//!
//! This is the path that keeps chat alive once BYOK and BYOS are gone: no
//! stored credential, no CLI subprocess, just the private provider gateway
//! (`provider_gateway.rs`) called in-process with a capability scoped to one
//! reply. The gateway reserves a spend cap before it calls the supplier and
//! settles on what the supplier actually reports; this module never guesses
//! that number, only reads it back and hands the exact micro-USD total to
//! [`crate::db::Database::charge_settled_cost`], which turns it into whole
//! credits against the user's balance and carry.
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
pub(crate) fn turn_cap_per_minute() -> u32 {
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

/// One paid reply per user at a time. Without this, two concurrent replies
/// for the same user each read the balance before either has charged
/// anything, each get authorized against the full balance, and each run
/// real (billable-to-Cortex) supplier turns — only for the second one's
/// final charge to be clamped down to whatever the first left behind (see
/// `charge_settled_cost`), handing out real work for free. Serializing per
/// user means the second reply's turn-by-turn balance checks see what the
/// first actually spent.
///
/// In-process only, same as `TURN_WINDOWS` above: `CORTEX_SINGLE_NODE=1` is
/// required for this server (see `AGENTS.md`), so one process holds every
/// in-flight reply for a given user and this lock is never bypassed by a
/// sibling process.
static USER_REPLY_LOCKS: Lazy<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Holds the per-user reply slot until dropped, then removes the map entry
/// if nothing else is waiting on it — otherwise a long-lived server
/// accumulates one entry per distinct user it has ever replied to.
struct UserReplyPermit {
    user_id: String,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl Drop for UserReplyPermit {
    fn drop(&mut self) {
        // Release the tokio lock itself before checking whether anyone
        // else still holds a clone of the `Arc` — otherwise our own guard
        // would always make the strong count look like more than one.
        self.guard.take();
        let mut locks = USER_REPLY_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
        if locks
            .get(&self.user_id)
            .is_some_and(|arc| Arc::strong_count(arc) == 1)
        {
            locks.remove(&self.user_id);
        }
    }
}

/// Wait for, and hold, this user's reply slot. A second concurrent call for
/// the same `user_id` waits here until the first one's `UserReplyPermit` is
/// dropped.
async fn acquire_user_reply_permit(user_id: &str) -> UserReplyPermit {
    let arc = {
        let mut locks = USER_REPLY_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
        locks
            .entry(user_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    let guard = arc.lock_owned().await;
    UserReplyPermit {
        user_id: user_id.to_string(),
        guard: Some(guard),
    }
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
                "Cortex can't reach the model right now. Try again in a few minutes.".into()
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
    // Anthropic rejects a request whose history contains `tool_use`/
    // `tool_result` blocks unless `tools` is also present on that request,
    // so the last turn still gets the full tool list — it just also gets
    // `tool_choice: {"type": "none"}` so the model has to answer in text
    // instead of asking for a call it will never see the result of.
    last_turn: bool,
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
        if last_turn {
            body["tool_choice"] = serde_json::json!({"type": "none"});
        }
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
            // A reservation is refused for one of several reasons, and only
            // one of them is actually the user's balance: "authorization
            // exhausted: ..." (db/provider_gateway.rs), meaning this turn's
            // reservation would exceed the spend cap this module itself
            // computed from the user's credits, is the sole genuine
            // insufficient-credit case. Every other `Reservation` string --
            // "supplier capacity is not funded" / "supplier capacity
            // exhausted: ...", a DB lock or IO failure, an amount overflow, a
            // missing/expired/revoked authorization, a capability mismatch, a
            // request-key replay conflict -- is a Cortex-side problem the
            // user did nothing to cause, and telling them they're out of
            // credits would be false; those get the same outage message as a
            // gateway that's off or has no price row for this model.
            GatewayError::Reservation(detail) if detail.starts_with("authorization exhausted") => {
                tracing::info!(user_id, %detail, turn, "chat: spend reservation refused");
                PaidReplyError::NotEnoughCredits
            }
            GatewayError::Reservation(detail) => {
                tracing::warn!(user_id, %detail, turn, "chat: reservation unavailable");
                PaidReplyError::Unavailable
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

/// Whether a reply is allowed to offer, and act on, `Risk::Confirm` agent
/// tools (`open_pr`, `cancel_run`).
///
/// - `Off`: today's chat behavior — the full tool list, confirm tools
///   included, confirmed the usual way (a tap on `POST
///   /api/agent/actions/{id}/confirm`).
/// - `Spoken`: a live-voice turn that allows `Risk::Confirm` proposals once
///   the reply has an owned `conversation_id` to write the pending row
///   against — approved by a tap, same as `Off`. Spoken approval (a matched
///   spoken "yes") comes in part 2. Without an owned conversation,
///   `send_paid_reply` still withholds `Risk::Confirm` tools entirely (there
///   would be nowhere to attach the confirmation), the same as the old
///   `voice_turn: true` behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VoiceConfirm {
    Off,
    Spoken,
}

/// Run the tool loop: reserve, call the supplier, settle, and charge for up
/// to [`MAX_AGENT_TURNS`] turns, executing any `tool_use` blocks the model
/// asks for between turns. Independent of the SSE plumbing so it can be
/// tested directly against a stub transport and an in-memory database.
///
/// One charge per reply, not one per turn: every turn's observed cost is
/// summed and settled in a single ledger line at the end (see the
/// `charge_settled_cost` call below), even for a reply that took three
/// turns to answer. Before each turn this checks that the user's balance
/// still covers a reservation and that the conversation has not spent its
/// per-minute turn allowance (`CORTEX_CHAT_AGENT_TURN_CAP_PER_MINUTE`,
/// `turn_cap_per_minute`); either stops the loop and returns whatever text
/// the last turn produced, with the turns that did run still charged once
/// at the end.
///
/// `reply_id` is the caller's uuid for this one reply; each turn's
/// reservation key is derived from it (`{reply_id}:turn{n}`) and the final
/// charge's idempotency key is derived from it directly, so replaying the
/// whole call charges the reply at most once.
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
    // See [`VoiceConfirm`]. Text chat always passes `VoiceConfirm::Off`.
    voice_confirm: VoiceConfirm,
    // Cooperative stop: checked once per turn, right next to the balance
    // check below, rather than the caller hard-aborting our task. An abort
    // could land mid supplier call (leaving a `chat-reply:*` reservation
    // stuck `reserved` forever) or after the supplier answered but before
    // the final charge (Cortex pays the supplier, the user is never
    // charged) — see `voice_session`'s `delegation_cancel` for the caller
    // that needs this. `None` (every chat call site) means never cancel.
    cancel: Option<&std::sync::atomic::AtomicBool>,
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

    // Serialize the whole loop-plus-final-charge per user (see
    // `UserReplyPermit`'s doc comment). Held for the rest of this function.
    let _user_reply_permit = acquire_user_reply_permit(user_id).await;

    let run_id = conversation_id
        .map(|c| format!("chat:{c}"))
        .unwrap_or_else(|| format!("chat:{reply_id}"));
    // The per-minute cap is per (user, conversation): two users in the same
    // conversation-less state (falling back to their own reply id) must
    // never share a window, and a bare conversation id must not let one
    // user's turns count against another's cap.
    let turn_cap_key = format!("{user_id}:{}", conversation_id.unwrap_or(reply_id));
    let turn_cap = turn_cap_per_minute;

    // `Spoken` still withholds `Risk::Confirm` tools when there is no
    // `conversation_id` this user owns to write a pending row against — the
    // same as the old `voice_turn: true` behavior. `Off` (chat, and a
    // `Spoken` turn with an owned conversation) offers the full list.
    let confirm_withheld = voice_confirm == VoiceConfirm::Spoken
        && !conversation_id
            .map(|c| db.get_conversation(c, user_id).is_some())
            .unwrap_or(false);
    let tools = if confirm_withheld {
        agent_tools::tool_definitions_excluding_confirm()
    } else {
        agent_tools::tool_definitions()
    };
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
    // One proposal per reply: once a `Risk::Confirm` tool has been proposed
    // (a pending row written and `ConfirmRequired` streamed), the next turn
    // is forced to be the last one — tool_choice: none — so the loop ends
    // with the model's text instead of proposing (or re-proposing) another
    // action it can't get a result back for this reply.
    let mut confirm_proposed = false;

    for turn in 1..=MAX_AGENT_TURNS {
        // Stamped fresh every turn: a slow, multi-turn reply must not let
        // capability expiry, reservation timestamps, or the per-minute
        // window all pin to the moment the reply started.
        let now_ms = chrono::Utc::now().timestamp_millis();

        // Read the balance unfiltered. It is not decremented until the
        // single end-of-reply charge (see below), so what earlier turns in
        // *this* loop already cost is subtracted here at its exact
        // micro-USD value (`total_observed_micro_usd`, not rounded to whole
        // credits), so "credits used so far" mid-loop always matches what
        // the final charge will actually settle. Nothing left on turn 1 is
        // a hard refusal — nothing
        // has been reserved or charged yet, so there is nothing to
        // preserve. Nothing left on turn 2+ is a graceful stop: earlier
        // turns already did billable work that must still be charged once,
        // at the end, and returned as a partial answer.
        let balance = match db.get_credit_balance_row(user_id) {
            Some(balance) => balance,
            None if turn == 1 => return Err(PaidReplyError::NoCredits),
            None => {
                tracing::info!(
                    user_id,
                    reply_id,
                    turn,
                    "chat: credit balance row disappeared mid-loop; stopping gracefully"
                );
                stopped_reason = Some(
                    "\n\n(This answer may be incomplete: your credit balance could not be \
                     read, so I stopped before finishing.)",
                );
                break;
            }
        };

        // Checked right next to the balance so a cancelled request stops at
        // the same turn boundary a graceful "ran out mid-loop" stop would:
        // whatever turns already ran are still charged once, below, and
        // nothing here forces an early return that would skip that charge.
        if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::SeqCst)) {
            tracing::info!(user_id, reply_id, turn, "chat: cancelled; stopping");
            stopped_reason = Some(
                "\n\n(This answer may be incomplete: this request was cancelled, so I stopped \
                 before finishing.)",
            );
            break;
        }

        // Exact-billing, carry-aware gate. What this reply can still spend is
        // the user's whole balance converted to micro-USD, minus the carry
        // already owed from previous replies (a fractional debt that never
        // shows up in `subscription_remaining`/`pack_remaining` until it
        // crosses a whole credit, but is real and must be paid first), minus
        // whatever this reply has already spent in earlier turns of this
        // same loop (not yet deducted from the balance -- the whole reply is
        // one charge at the end; see `send_one_turn`'s doc comment):
        //
        //   payable = balance_credits * micros_per_credit - carry - spent_so_far
        //
        // `max_micro_usd`, the cap handed to this turn's reservation, is the
        // smaller of the operator's own spend limit and this payable amount.
        let carry_micro_usd = db.get_credit_carry_micro_usd(user_id) as i64;
        let payable_micro_usd = (balance.subscription_remaining + balance.pack_remaining)
            .saturating_mul(price_list.micros_per_credit)
            .saturating_sub(carry_micro_usd)
            .saturating_sub(total_observed_micro_usd);
        if payable_micro_usd <= 0 {
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
        let max_micro_usd = limits.max_micro_usd.min(payable_micro_usd);

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
        // on, so the model must answer in text with whatever it already
        // knows instead of asking for a tool call it will never see the
        // result of — but the request still has to carry `tools` (see
        // `send_one_turn`'s doc comment on `last_turn`), just with
        // `tool_choice: none` forcing text instead of dropping the list.
        let is_last_turn = turn == MAX_AGENT_TURNS || confirm_proposed;

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
            &tools,
            is_last_turn,
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
            // A voice turn never gets a `Risk::Confirm` tool in its `tools`
            // list (see `tools` above), but the model can still name one
            // anyway — refuse it here, before the `open_pr` proposal path or
            // `execute`'s own (chat-worded) `ConfirmRequired` message, with
            // the voice-appropriate wording. Nothing is proposed or run.
            if confirm_withheld && agent_tools::is_confirm_risk(&name) {
                tool_results.push(serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": tool_use_id,
                    "content": "That needs confirmation in the chat; voice confirmation is not available yet.",
                    "is_error": true,
                }));
                continue;
            }
            // `open_pr` is the one `Risk::Confirm` tool wired up so far
            // (`agent_tools::validate_confirm_tool`/`execute_confirmed`):
            // instead of running it, or just refusing it, propose it — write
            // a row to `agent_pending_actions` and tell the model (and, via
            // `StepEvent::ConfirmRequired`, the client) that it's waiting on
            // the user's own tap on `POST /api/agent/actions/{id}/confirm`.
            // Every other tool, including the still-refused `cancel_run`,
            // falls through to the unchanged path below.
            if name == "open_pr" {
                // A pending row's `conversation_id` is a `NOT NULL` foreign
                // key that `agent_confirm::confirm_action` later writes an
                // assistant message against (`Database::add_message`).
                // Without this check a missing, unknown, or another user's
                // `conversation_id` (`.unwrap_or_default()` used to paper
                // over the missing case with `""`) would sail through here
                // and only blow up later, at confirm time, as a foreign-key
                // panic — so check ownership up front and refuse before any
                // row is written, exactly like an invalid-argument tool call.
                let owned_conversation =
                    conversation_id.filter(|c| db.get_conversation(c, user_id).is_some());
                let validated = match owned_conversation {
                    Some(_) => agent_tools::validate_confirm_tool(db, user_id, &name, &input),
                    None => Err(agent_tools::ToolError::InvalidArguments(
                        "confirmable actions need a saved conversation".to_string(),
                    )),
                };
                match validated {
                    Ok(_summary) if confirm_proposed => {
                        // One proposal per reply: a proposal already went
                        // out this reply (the next turn was forced to be
                        // the last one, see `is_last_turn` above), so a
                        // second `open_pr` — a fake ignoring
                        // `tool_choice: none`, in practice — does not get a
                        // second pending row or a second
                        // `StepEvent::ConfirmRequired`. Tell the model
                        // plainly that this one was not proposed; the loop
                        // ends after this turn either way.
                        tool_results.push(serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": tool_use_id,
                            "content": "Not proposed: only one action can wait for confirmation per reply. Ask the user to confirm or cancel the pending one first.",
                            "is_error": true,
                        }));
                    }
                    Ok(summary) => {
                        let now = chrono::Utc::now().timestamp();
                        let action = db.insert_pending_action(
                            user_id,
                            owned_conversation.expect("checked above"),
                            &name,
                            &input,
                            &summary,
                            now,
                        );
                        if let Some(tx) = tool_events {
                            let _ = tx
                                .send(StepEvent::ConfirmRequired {
                                    action_id: action.id.clone(),
                                    nonce: action.nonce.clone(),
                                    summary: action.summary.clone(),
                                    expires_at: action.expires_at,
                                })
                                .await;
                        }
                        // Proposing is not running: no `ToolActivity` entry
                        // (and no "Checked open_pr." line via
                        // `tool_activity_summary`) — the client's only signal
                        // for a proposal is `StepEvent::ConfirmRequired`
                        // above.
                        tool_results.push(serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": tool_use_id,
                            "content": "Waiting for the user's confirmation. This has not run yet.",
                            "is_error": false,
                        }));
                        confirm_proposed = true;
                    }
                    Err(e) => {
                        // No row written — refuse exactly like an
                        // unconfirmable tool call would today.
                        tool_activity.push(ToolActivity {
                            tool_name: name.clone(),
                            ok: false,
                        });
                        if let Some(tx) = tool_events {
                            let _ = tx
                                .send(StepEvent::ToolActivity {
                                    step_id: "chat".into(),
                                    tool_name: name.clone(),
                                    ok: false,
                                })
                                .await;
                        }
                        tool_results.push(serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": tool_use_id,
                            "content": e.message(),
                            "is_error": true,
                        }));
                    }
                }
                continue;
            }
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
        // The last turn (whether forced by a proposal above or by the turn
        // cap) never gets a next turn to send this tool_result back on —
        // there's nothing more for the loop to do.
        if is_last_turn {
            break;
        }
    }

    if let Some(reason) = stopped_reason {
        last_text.push_str(reason);
    }

    // One ledger line per reply, for the sum of what every turn actually
    // cost — including a reply that stopped early, so the turns that did
    // run are never given away for free.
    let charged_credits_total = if total_observed_micro_usd > 0 {
        // Never discard the reply over the final charge: `?` here would turn
        // a billing hiccup (or a balance that shrank mid-loop, e.g. another
        // reply spending concurrently) into a lost answer the user already
        // paid the supplier for. `charge_settled_cost` clamps the deduction
        // to whatever is actually left instead of erroring — the full
        // nominal cost still lands in `cost_micro_usd`, but any whole
        // credits it could not collect are not collected from anyone:
        // Cortex absorbs them, and that absorption is recorded in the
        // ledger row's own description rather than hidden — so this only
        // fails on a real database error, which we log and still answer
        // past.
        match db.charge_settled_cost(
            user_id,
            total_observed_micro_usd as u64,
            price_list.micros_per_credit,
            "Cortex-paid chat reply",
            &ChargeKey::for_chat_reply(reply_id),
        ) {
            Ok(settled) => settled.credits_charged,
            Err(e) => {
                tracing::error!(user_id, reply_id, error = %e, "chat: failed to charge for reply");
                0
            }
        }
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
pub(crate) fn tool_activity_summary(activity: &[ToolActivity]) -> Option<String> {
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
        VoiceConfirm::Off,
        None,
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
    async fn a_reply_charges_exactly_its_observed_cost_and_carries_the_remainder() {
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 100).unwrap();
        let price_list = db.active_price_list().unwrap();
        let rate = price_list.model("claude", MODEL).unwrap();
        // 10 input, 10 output tokens at the seeded haiku rate: 60 micro-USD,
        // far under one credit (100_000 micro-USD) -- exact billing charges
        // 0 whole credits and carries the full cost forward instead of
        // rounding it up to 1.
        let observed_micros = rate.cost_micros(10, 0, 10);
        assert!(observed_micros > 0, "fixture must actually cost something");
        assert!(
            observed_micros < price_list.micros_per_credit,
            "fixture must cost less than one credit for this test to be meaningful"
        );

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
            VoiceConfirm::Off,
            None,
        )
        .await
        .expect("reply should succeed");

        assert_eq!(reply.text, "hello there");
        assert_eq!(reply.charged_credits, 0);
        let balance = db.get_credit_balance_row("user-1").unwrap();
        assert_eq!(balance.subscription_remaining, 100);
        assert_eq!(
            db.get_credit_carry_micro_usd("user-1"),
            observed_micros as u64
        );
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
            VoiceConfirm::Off,
            None,
        )
        .await
        .expect_err("supplier failure must not succeed");

        assert!(matches!(error, PaidReplyError::Provider(_)));
        let balance = db.get_credit_balance_row("user-1").unwrap();
        assert_eq!(balance.subscription_remaining, 100, "nothing was charged");
        assert_eq!(
            db.get_credit_carry_micro_usd("user-1"),
            0,
            "no cost was observed, so the carry must not move either"
        );
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
            VoiceConfirm::Off,
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
                VoiceConfirm::Off,
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
        let observed_micros = rate.cost_micros(10, 0, 10);
        let balance = db.get_credit_balance_row("user-1").unwrap();
        assert_eq!(
            balance.subscription_remaining, 100,
            "the tiny fixture cost is under one credit either way"
        );
        assert_eq!(
            db.get_credit_carry_micro_usd("user-1"),
            observed_micros as u64,
            "the carry must have moved only once, not twice"
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
            VoiceConfirm::Off,
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
        assert_eq!(reply.charged_credits, 0, "6 micro-USD is under one credit");
        assert_eq!(db.get_credit_carry_micro_usd("user-1"), 6);
    }

    #[tokio::test]
    async fn turn_ones_authorization_cap_subtracts_the_outstanding_carry() {
        // The gate that gets turned into `max_micro_usd` is `balance_credits *
        // micros_per_credit - carry - spent_so_far`: a fractional debt from
        // earlier replies is real money owed and must come off the top
        // before this turn gets to spend anything, or the same micro-USD
        // would effectively get spent twice.
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 1).unwrap();
        db.conn()
            .execute(
                "UPDATE credit_balances SET carry_micro_usd = 40000 WHERE clerk_user_id = ?1",
                rusqlite::params!["user-1"],
            )
            .unwrap();

        send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            FixedTransport::ok(1, 1, "tiny reply"),
            big_limits(),
            "user-1",
            Some("conv-gate-1"),
            MODEL,
            "system",
            "hi",
            "reply-gate-1",
            NOW,
            20,
            None,
            VoiceConfirm::Off,
            None,
        )
        .await
        .expect("reply should succeed");

        let max_micro_usd: i64 = db
            .conn()
            .query_row(
                "SELECT max_micro_usd FROM provider_spend_authorizations WHERE id = ?1",
                rusqlite::params!["gateway-auth:chat-reply:reply-gate-1:turn1"],
                |r| r.get(0),
            )
            .expect("turn 1 authorization row");
        // 1 credit (100_000 micro-USD) minus the 40_000 carry already owed.
        assert_eq!(max_micro_usd, 60_000);
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
        // Every request body this transport has been asked to forward, in
        // call order, so a test can inspect exactly what was sent on a
        // given turn (e.g. whether `tools`/`tool_choice` were set) without
        // hand-replicating the gateway's own byte-cost math.
        request_bodies: Arc<Mutex<Vec<Value>>>,
    }

    impl SequenceTransport {
        fn new(responses: Vec<Result<TransportResponse, TransportFailure>>) -> Self {
            assert!(!responses.is_empty());
            Self {
                calls: Arc::new(AtomicUsize::new(0)),
                responses: Arc::new(responses),
                request_bodies: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn last_request_body(&self) -> Value {
            self.request_bodies
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .last()
                .cloned()
                .expect("at least one request must have been recorded")
        }

        fn request_body_at(&self, index: usize) -> Value {
            self.request_bodies
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(index)
                .cloned()
                .unwrap_or_else(|| panic!("no request recorded at index {index}"))
        }
    }

    impl ProviderTransport for SequenceTransport {
        fn forward(
            &self,
            supplier_key: &str,
            request: &GatewayRequest,
        ) -> impl Future<Output = Result<TransportResponse, TransportFailure>> + Send {
            assert_eq!(supplier_key, SUPPLIER_KEY);
            self.request_bodies
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(request.body.clone());
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
        // One ledger line for the whole reply, for the exact sum of every
        // turn's observed cost — not one line (and one separate rounding
        // step) per turn.
        let total_observed_micros = per_turn_micro * 2;

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
            VoiceConfirm::Off,
            None,
        )
        .await
        .expect("reply should succeed");

        assert_eq!(transport.call_count(), 2, "one turn per gateway call");
        assert_eq!(reply.text, "here is your answer");
        assert_eq!(
            reply.charged_credits, 0,
            "120 micro-USD is under one credit"
        );
        let balance = db.get_credit_balance_row("user-1").unwrap();
        assert_eq!(
            balance.subscription_remaining, 1000,
            "the ledger reflects one deduction for the whole reply, not one per turn"
        );
        assert_eq!(
            db.get_credit_carry_micro_usd("user-1"),
            total_observed_micros as u64
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
    async fn turn_twos_authorization_cap_subtracts_turn_ones_observed_cost() {
        // `spent_so_far` in the payable gate is this loop's own running
        // total, not yet deducted from the balance -- so turn 2's cap must
        // be smaller than turn 1's by exactly what turn 1 was observed to
        // cost, on top of the carry already owed.
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 1).unwrap();
        db.conn()
            .execute(
                "UPDATE credit_balances SET carry_micro_usd = 40000 WHERE clerk_user_id = ?1",
                rusqlite::params!["user-1"],
            )
            .unwrap();

        let price_list = db.active_price_list().unwrap();
        let rate = price_list.model("claude", MODEL).unwrap();
        let turn_one_observed = rate.cost_micros(10, 0, 10);
        assert_eq!(
            turn_one_observed, 60,
            "haiku fixture cost must be 60 micro-USD for this test's numbers to hold"
        );

        let transport = SequenceTransport::new(vec![
            tool_use_response(10, 10, "toolu_1", "list_runs", serde_json::json!({})),
            text_response(10, 10, "here is your answer"),
        ]);

        send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            transport,
            big_limits(),
            "user-1",
            Some("conv-gate-2"),
            MODEL,
            "system",
            "how are my runs?",
            "reply-gate-2",
            NOW,
            20,
            None,
            VoiceConfirm::Off,
            None,
        )
        .await
        .expect("reply should succeed");

        let turn1_max: i64 = db
            .conn()
            .query_row(
                "SELECT max_micro_usd FROM provider_spend_authorizations WHERE id = ?1",
                rusqlite::params!["gateway-auth:chat-reply:reply-gate-2:turn1"],
                |r| r.get(0),
            )
            .expect("turn 1 authorization row");
        assert_eq!(turn1_max, 60_000);

        let turn2_max: i64 = db
            .conn()
            .query_row(
                "SELECT max_micro_usd FROM provider_spend_authorizations WHERE id = ?1",
                rusqlite::params!["gateway-auth:chat-reply:reply-gate-2:turn2"],
                |r| r.get(0),
            )
            .expect("turn 2 authorization row");
        assert_eq!(
            turn2_max,
            60_000 - turn_one_observed,
            "turn 2's payable cap must subtract what turn 1 already spent this loop"
        );
    }

    #[tokio::test]
    async fn loop_stops_at_turn_cap() {
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 1_000_000).unwrap();
        // The model calls a tool every turn but the last: the last turn is
        // sent with `tool_choice: none` (see `send_paid_reply`), so a real
        // model still sees the tools but cannot call one and must answer in
        // text instead.
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
            VoiceConfirm::Off,
            None,
        )
        .await
        .expect("reply should succeed even though it never got a final answer on its own");

        assert_eq!(transport.call_count(), MAX_AGENT_TURNS as usize);
        assert_eq!(reply.tool_activity.len(), (MAX_AGENT_TURNS - 1) as usize);
        assert!(
            !reply.text.is_empty(),
            "the final turn, which cannot call a tool, must produce a text answer"
        );

        // The last turn's history contains tool_use/tool_result blocks, so
        // Anthropic requires `tools` to still be present — it's
        // `tool_choice: none` that stops the model from calling one, not an
        // absent tool list.
        let last_body = transport.last_request_body();
        assert_eq!(
            last_body.get("tool_choice"),
            Some(&serde_json::json!({"type": "none"})),
            "the last turn must set tool_choice: none: {last_body}"
        );
        assert!(
            last_body
                .get("tools")
                .and_then(Value::as_array)
                .is_some_and(|tools| !tools.is_empty()),
            "the last turn must still carry a non-empty tools list: {last_body}"
        );
    }

    /// The `Risk::Confirm` `open_pr` tool never actually opens a PR from the
    /// tool loop itself: asking for it only proposes an
    /// `agent_pending_actions` row and streams `StepEvent::ConfirmRequired`.
    /// The only place `agent_tools::execute_confirmed`
    /// (`crate::routes::create_pr_core`'s caller) is ever invoked is
    /// `agent_confirm::confirm_action`, which this test never calls — so the
    /// pending row still reading back as `pending` (not `confirmed`) here is
    /// direct proof the PR side effect did not run.
    #[tokio::test]
    async fn risky_tool_never_executes_without_confirm() {
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 1000).unwrap();
        let conversation = db.create_conversation("user-1", None);

        let run_id = db
            .create_run_with_steps_and_resource_leases(
                "user-1",
                "Ship a feature",
                "auto",
                &["src/lib.rs".to_string()],
                None,
                None,
                Some(&conversation.id),
                &[crate::db::ResourceLeaseRequest {
                    resource_type: "path".to_string(),
                    repo_key: "github:test/repo".to_string(),
                    resource_key: "src/lib.rs".to_string(),
                    mode: "write".to_string(),
                    reason: Some("test".to_string()),
                    metadata: serde_json::json!({}),
                }],
                &[],
                &[],
            )
            .expect("run created with a write lease");
        db.record_run_branch(&run_id, "cortex/run-open-pr");

        // The fake repeats its last queued response forever (see
        // `SequenceTransport`'s doc comment), so a one-item script would
        // have the model ask for `open_pr` on every remaining turn. Give it
        // a second, distinct response so the forced last turn (see
        // `confirm_proposed` in `send_paid_reply`) can answer in text
        // instead.
        let transport = SequenceTransport::new(vec![
            tool_use_response(
                10,
                10,
                "toolu_1",
                "open_pr",
                serde_json::json!({"run_id": run_id}),
            ),
            text_response(10, 10, "waiting on you"),
        ]);
        let (tx, mut rx) = mpsc::channel(8);

        let reply = send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            transport.clone(),
            big_limits(),
            "user-1",
            Some(&conversation.id),
            MODEL,
            "system",
            "open a pr for my run",
            "reply-open-pr",
            NOW,
            20,
            Some(&tx),
            VoiceConfirm::Off,
            None,
        )
        .await
        .expect("reply should succeed even though the tool never ran");
        drop(tx);

        assert_eq!(
            transport.call_count(),
            2,
            "the loop makes one more, forced-last-turn call after proposing"
        );
        assert!(
            reply.tool_activity.is_empty(),
            "proposing is not running: no ToolActivity entry (and no \"Checked open_pr.\" \
             line), only the ConfirmRequired event below"
        );

        let mut confirm_action_id = None;
        while let Some(event) = rx.recv().await {
            if let StepEvent::ConfirmRequired { action_id, .. } = event {
                confirm_action_id = Some(action_id);
            }
        }
        let action_id =
            confirm_action_id.expect("a ConfirmRequired event should have been streamed");

        let pending = db
            .get_pending_action(&action_id, "user-1")
            .expect("the proposal was written to agent_pending_actions");
        assert_eq!(pending.tool_name, "open_pr");
        assert_eq!(
            pending.status, "pending",
            "still pending — nothing confirmed it, so execute_confirmed/create_pr_core never ran"
        );
    }

    /// A `Spoken` turn with no owned conversation never even offers
    /// `Risk::Confirm` tools to the model — but if the model names one
    /// anyway (a stale tool_use from before the delegation, or a model that
    /// hallucinates one), it must be refused as a plain tool error rather
    /// than proposed or run: no `agent_pending_actions` row, no
    /// `ConfirmRequired` event.
    #[tokio::test]
    async fn voice_turn_refuses_a_forced_confirm_tool_without_executing() {
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 1000).unwrap();

        let transport = SequenceTransport::new(vec![
            tool_use_response(
                10,
                10,
                "toolu_1",
                "open_pr",
                serde_json::json!({"run_id": "run-1"}),
            ),
            text_response(10, 10, "done"),
        ]);
        let (tx, mut rx) = mpsc::channel(8);

        let reply = send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            transport.clone(),
            big_limits(),
            "user-1",
            None,
            MODEL,
            "system",
            "open a pr for my run",
            "reply-voice-confirm",
            NOW,
            20,
            Some(&tx),
            VoiceConfirm::Spoken,
            None,
        )
        .await
        .expect("reply should succeed — the tool is refused, not the whole turn");
        drop(tx);

        assert!(
            reply.tool_activity.is_empty(),
            "a refused tool call is not \"activity\""
        );

        let mut saw_confirm_required = false;
        while let Some(event) = rx.recv().await {
            if matches!(event, StepEvent::ConfirmRequired { .. }) {
                saw_confirm_required = true;
            }
        }
        assert!(
            !saw_confirm_required,
            "a voice turn must never stream ConfirmRequired for a withheld tool \
             (the only path that would write an agent_pending_actions row)"
        );
    }

    /// `open_pr` with no `conversation_id` (or one that isn't the caller's
    /// own, e.g. another user's) is refused before any row is written —
    /// otherwise `agent_confirm::confirm_action` would later panic on the
    /// `messages.conversation_id` foreign key when it tried to save the
    /// tool's result.
    #[tokio::test]
    async fn open_pr_without_owned_conversation_is_refused() {
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 1000).unwrap();

        // See the comment in `risky_tool_never_executes_without_confirm`:
        // the fake repeats its last response forever, so a one-item script
        // would have the model retry the refused `open_pr` on every turn.
        let transport = SequenceTransport::new(vec![
            tool_use_response(
                10,
                10,
                "toolu_1",
                "open_pr",
                serde_json::json!({"run_id": "does-not-matter"}),
            ),
            text_response(10, 10, "waiting on you"),
        ]);
        let (tx, mut rx) = mpsc::channel(8);

        let reply = send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            transport.clone(),
            big_limits(),
            "user-1",
            None, // no saved conversation
            MODEL,
            "system",
            "open a pr for my run",
            "reply-no-conv",
            NOW,
            20,
            Some(&tx),
            VoiceConfirm::Off,
            None,
        )
        .await
        .expect("reply should still succeed — the tool call is refused, not the whole reply");
        drop(tx);

        assert_eq!(
            reply.tool_activity,
            vec![ToolActivity {
                tool_name: "open_pr".into(),
                ok: false
            }]
        );
        while let Some(event) = rx.recv().await {
            assert!(
                !matches!(event, StepEvent::ConfirmRequired { .. }),
                "a refused proposal must not stream ConfirmRequired"
            );
        }
    }

    /// A `Spoken` turn whose `conversation_id` names a conversation the
    /// caller does not own (it belongs to another user) must behave exactly
    /// like the no-conversation case: `Risk::Confirm` tools are withheld
    /// from the very first request, and a model that names one anyway gets
    /// the same voice-appropriate refusal, not the `open_pr`-specific one.
    #[tokio::test]
    async fn spoken_with_another_users_conversation_withholds_confirm_tools() {
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 1000).unwrap();
        db.init_credit_balance("user-2", 1000).unwrap();
        let other_conversation = db.create_conversation("user-2", None);

        let transport = SequenceTransport::new(vec![
            tool_use_response(
                10,
                10,
                "toolu_1",
                "open_pr",
                serde_json::json!({"run_id": "does-not-matter"}),
            ),
            text_response(10, 10, "waiting on you"),
        ]);
        let (tx, mut rx) = mpsc::channel(8);

        let reply = send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            transport.clone(),
            big_limits(),
            "user-1",
            Some(&other_conversation.id),
            MODEL,
            "system",
            "open a pr for my run",
            "reply-other-users-conv",
            NOW,
            20,
            Some(&tx),
            VoiceConfirm::Spoken,
            None,
        )
        .await
        .expect("reply should succeed — the tool is refused, not the whole turn");
        drop(tx);

        let first_body = transport.request_body_at(0);
        let tool_names: Vec<String> = first_body
            .get("tools")
            .and_then(Value::as_array)
            .expect("first request must carry a tools list")
            .iter()
            .filter_map(|t| t.get("name").and_then(Value::as_str))
            .map(String::from)
            .collect();
        assert!(
            !tool_names.contains(&"open_pr".to_string()),
            "tools must not offer open_pr for an unowned conversation: {tool_names:?}"
        );
        assert!(
            !tool_names.contains(&"cancel_run".to_string()),
            "tools must not offer cancel_run for an unowned conversation: {tool_names:?}"
        );

        assert!(
            reply.tool_activity.is_empty(),
            "a refused tool call is not \"activity\""
        );

        let second_body = transport.request_body_at(1);
        let refusal_text = second_body
            .get("messages")
            .and_then(Value::as_array)
            .and_then(|messages| messages.last())
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
            .and_then(|blocks| blocks.first())
            .and_then(|b| b.get("content"))
            .and_then(Value::as_str)
            .expect("the tool_result content must be a plain refusal string");
        assert_eq!(
            refusal_text,
            "That needs confirmation in the chat; voice confirmation is not available yet.",
        );

        while let Some(event) = rx.recv().await {
            assert!(
                !matches!(event, StepEvent::ConfirmRequired { .. }),
                "a refused proposal must not stream ConfirmRequired"
            );
        }
    }

    /// Same as `spoken_with_another_users_conversation_withholds_confirm_tools`,
    /// but for a conversation that did belong to the caller and was then
    /// deleted — `get_conversation` no longer finds it, so it must be
    /// treated the same as never having owned one at all.
    #[tokio::test]
    async fn spoken_with_deleted_conversation_withholds_confirm_tools() {
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 1000).unwrap();
        let conversation = db.create_conversation("user-1", None);
        assert!(db.delete_conversation(&conversation.id, "user-1"));

        let transport = SequenceTransport::new(vec![
            tool_use_response(
                10,
                10,
                "toolu_1",
                "open_pr",
                serde_json::json!({"run_id": "does-not-matter"}),
            ),
            text_response(10, 10, "waiting on you"),
        ]);
        let (tx, mut rx) = mpsc::channel(8);

        let reply = send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            transport.clone(),
            big_limits(),
            "user-1",
            Some(&conversation.id),
            MODEL,
            "system",
            "open a pr for my run",
            "reply-deleted-conv",
            NOW,
            20,
            Some(&tx),
            VoiceConfirm::Spoken,
            None,
        )
        .await
        .expect("reply should succeed — the tool is refused, not the whole turn");
        drop(tx);

        let first_body = transport.request_body_at(0);
        let tool_names: Vec<String> = first_body
            .get("tools")
            .and_then(Value::as_array)
            .expect("first request must carry a tools list")
            .iter()
            .filter_map(|t| t.get("name").and_then(Value::as_str))
            .map(String::from)
            .collect();
        assert!(
            !tool_names.contains(&"open_pr".to_string()),
            "tools must not offer open_pr for a deleted conversation: {tool_names:?}"
        );
        assert!(
            !tool_names.contains(&"cancel_run".to_string()),
            "tools must not offer cancel_run for a deleted conversation: {tool_names:?}"
        );

        assert!(
            reply.tool_activity.is_empty(),
            "a refused tool call is not \"activity\""
        );

        let second_body = transport.request_body_at(1);
        let refusal_text = second_body
            .get("messages")
            .and_then(Value::as_array)
            .and_then(|messages| messages.last())
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
            .and_then(|blocks| blocks.first())
            .and_then(|b| b.get("content"))
            .and_then(Value::as_str)
            .expect("the tool_result content must be a plain refusal string");
        assert_eq!(
            refusal_text,
            "That needs confirmation in the chat; voice confirmation is not available yet.",
        );

        while let Some(event) = rx.recv().await {
            assert!(
                !matches!(event, StepEvent::ConfirmRequired { .. }),
                "a refused proposal must not stream ConfirmRequired"
            );
        }
    }

    /// One proposal per reply: even if the model keeps asking for `open_pr`
    /// on every turn (a fake ignoring `tool_choice: none`, standing in for
    /// a model that just won't take no for an answer), only the first ask
    /// gets a pending row and a `ConfirmRequired` event — the turn right
    /// after it is forced to be the last one, so the loop stops there.
    #[tokio::test]
    async fn one_proposal_per_reply() {
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 1000).unwrap();
        let conversation = db.create_conversation("user-1", None);

        let run_id = db
            .create_run_with_steps_and_resource_leases(
                "user-1",
                "Ship a feature",
                "auto",
                &["src/lib.rs".to_string()],
                None,
                None,
                Some(&conversation.id),
                &[crate::db::ResourceLeaseRequest {
                    resource_type: "path".to_string(),
                    repo_key: "github:test/repo".to_string(),
                    resource_key: "src/lib.rs".to_string(),
                    mode: "write".to_string(),
                    reason: Some("test".to_string()),
                    metadata: serde_json::json!({}),
                }],
                &[],
                &[],
            )
            .expect("run created with a write lease");
        db.record_run_branch(&run_id, "cortex/run-open-pr");

        // Every turn asks for `open_pr` — `SequenceTransport` repeats its
        // last queued response forever (see its doc comment), and this test
        // only queues one, on purpose, to simulate a model that keeps
        // calling the tool even on the forced last turn.
        let transport = SequenceTransport::new(vec![tool_use_response(
            10,
            10,
            "toolu_1",
            "open_pr",
            serde_json::json!({"run_id": run_id}),
        )]);
        let (tx, mut rx) = mpsc::channel(8);

        let reply = send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            transport.clone(),
            big_limits(),
            "user-1",
            Some(&conversation.id),
            MODEL,
            "system",
            "open a pr for my run",
            "reply-one-proposal",
            NOW,
            20,
            Some(&tx),
            VoiceConfirm::Off,
            None,
        )
        .await
        .expect("reply should succeed even though the tool never ran");
        drop(tx);

        assert_eq!(
            transport.call_count(),
            2,
            "the loop stops after the forced last turn, even though it also asked for open_pr"
        );
        assert!(
            reply.tool_activity.is_empty(),
            "asking for open_pr, proposed or not, is never a ToolActivity entry"
        );

        let mut confirm_events = 0;
        while let Some(event) = rx.recv().await {
            if matches!(event, StepEvent::ConfirmRequired { .. }) {
                confirm_events += 1;
            }
        }
        assert_eq!(
            confirm_events, 1,
            "only the first ask writes a pending row and streams ConfirmRequired"
        );

        let pending_count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM agent_pending_actions WHERE user_id = ?1 AND status = \
                 'pending'",
                rusqlite::params!["user-1"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending_count, 1, "exactly one pending row");

        let second_body = transport.last_request_body();
        assert_eq!(
            second_body.get("tool_choice"),
            Some(&serde_json::json!({"type": "none"})),
            "the 2nd recorded request must set tool_choice: none: {second_body}"
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

        // Compute each turn's reservation with the gateway's own function
        // rather than hand-replicating its byte-cost math, so this test
        // tracks `upper_bound_cost` instead of silently drifting from it.
        fn reserved_cost(body: &Value, rate: &crate::pricing::ModelPrice) -> i64 {
            let bytes = serde_json::to_vec(body).unwrap().len() as i64;
            crate::provider_gateway::upper_bound_cost(
                bytes,
                MAX_OUTPUT_TOKENS,
                rate.input_micros_per_1k,
                rate.output_micros_per_1k,
            )
            .unwrap()
        }

        let tools = agent_tools::tool_definitions();
        let user_message = "keep checking";
        let tool_use_id = "toolu_1";
        let tool_name = "not_a_real_tool";

        // Recorded from the actual turn-1 request rather than constructed
        // by hand, so this test's notion of "turn 1's body" cannot drift
        // from what `send_one_turn` really sends. Uses its own user/balance
        // so probing does not spend any of "user-1"'s credits below.
        db.init_credit_balance("probe-user", 1_000_000).unwrap();
        let probe = SequenceTransport::new(vec![tool_use_response(
            10,
            10,
            tool_use_id,
            tool_name,
            serde_json::json!({}),
        )]);
        send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            probe.clone(),
            big_limits(),
            "probe-user",
            Some("conv-tool-5-probe"),
            MODEL,
            "system",
            user_message,
            "reply-tool-5-probe",
            NOW,
            20,
            None,
            VoiceConfirm::Off,
            None,
        )
        .await
        .expect("probe reply should succeed");
        let turn1_body = probe.request_body_at(0);
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
            VoiceConfirm::Off,
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
        let observed_micros = rate.cost_micros(10, 0, 10);
        assert_eq!(reply.charged_credits, 0, "60 micro-USD is under one credit");
        let balance = db.get_credit_balance_row("user-1").unwrap();
        assert_eq!(balance.subscription_remaining, 1_000_000);
        assert_eq!(
            db.get_credit_carry_micro_usd("user-1"),
            observed_micros as u64
        );
    }

    /// Drains a user's balance to zero from a second connection right after
    /// answering a turn, while still asking for a tool call so the loop
    /// actually attempts a second turn — standing in for a concurrent spend
    /// (another reply, another device) landing between turn 1 and the
    /// in-loop balance check before turn 2.
    ///
    /// Exact billing makes a small per-turn cost (60 micro-USD against a
    /// 100_000-micro-USD credit) essentially unable to exhaust a balance on
    /// its own within `MAX_AGENT_TURNS`, since the gateway's own worst-case
    /// reservation check would refuse turn 1 long before that many small
    /// charges could add up — so a real concurrent drain is what actually
    /// exercises the in-loop `payable <= 0` stop.
    #[derive(Clone)]
    struct DrainBalanceAndCallToolTransport {
        db_path: std::path::PathBuf,
        user_id: &'static str,
        calls: Arc<AtomicUsize>,
    }

    impl ProviderTransport for DrainBalanceAndCallToolTransport {
        fn forward(
            &self,
            _supplier_key: &str,
            _request: &GatewayRequest,
        ) -> impl Future<Output = Result<TransportResponse, TransportFailure>> + Send {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let concurrent_db = Database::open(&self.db_path);
            let balance = concurrent_db
                .get_credit_balance_row(self.user_id)
                .expect("balance row");
            let available = balance.subscription_remaining + balance.pack_remaining;
            if available > 0 {
                concurrent_db
                    .deduct_credits(
                        self.user_id,
                        available,
                        "concurrent drain",
                        &ChargeKey::for_verification("concurrent-drain-mid-loop"),
                    )
                    .expect("concurrent drain should succeed");
            }
            std::future::ready(Ok(TransportResponse {
                body: serde_json::json!({
                    "id": "msg_test",
                    "type": "message",
                    "role": "assistant",
                    "model": MODEL,
                    "content": [{"type": "tool_use", "id": "toolu_1", "name": "list_runs", "input": {}}],
                    "stop_reason": "tool_use",
                    "stop_sequence": null,
                    "usage": {"input_tokens": 10, "output_tokens": 10}
                }),
                upstream_request_id: Some("upstream-1".into()),
                usage: Some(ObservedUsage {
                    input_tokens: 10,
                    cached_input_tokens: 0,
                    output_tokens: 10,
                }),
            }))
        }
    }

    #[tokio::test]
    async fn insufficient_credits_mid_loop_stops_without_charge() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("chat-paid.sqlite");
        let db = Database::open(&db_path);
        // Plenty of headroom for turn 1's reservation (well above the
        // worst-case floor); the transport drains it to zero right after
        // answering, so turn 2's in-loop balance check sees nothing left.
        db.init_credit_balance("user-1", 5).unwrap();

        let calls = Arc::new(AtomicUsize::new(0));
        let transport = DrainBalanceAndCallToolTransport {
            db_path: db_path.clone(),
            user_id: "user-1",
            calls: calls.clone(),
        };

        let reply = send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            transport,
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
            VoiceConfirm::Off,
            None,
        )
        .await
        .expect("a partial answer, not an error, once at least one turn ran");

        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "no second gateway call once the concurrent drain leaves nothing payable"
        );
        assert_eq!(
            reply.charged_credits, 0,
            "60 micro-USD from the one turn that ran is under one credit"
        );
        assert!(
            reply.text.contains("out of credits"),
            "partial answer must say why it stopped: {}",
            reply.text
        );
        let balance = db.get_credit_balance_row("user-1").unwrap();
        assert_eq!(
            balance.subscription_remaining + balance.pack_remaining,
            0,
            "the concurrent drain left nothing"
        );
        assert_eq!(
            db.get_credit_carry_micro_usd("user-1"),
            60,
            "the one turn that ran still owes its cost"
        );
    }

    /// Wraps another transport and flips a shared flag right after that
    /// transport's call returns — the deterministic hook the cancel tests
    /// below use to flip `cancel` exactly once the first turn's supplier
    /// call has actually happened, instead of racing a timer against it.
    #[derive(Clone)]
    struct CancelAfterCallTransport<T> {
        inner: T,
        cancel: Arc<AtomicBool>,
    }

    impl<T: ProviderTransport + Clone + Send + Sync> ProviderTransport for CancelAfterCallTransport<T> {
        fn forward(
            &self,
            supplier_key: &str,
            request: &GatewayRequest,
        ) -> impl Future<Output = Result<TransportResponse, TransportFailure>> + Send {
            let fut = self.inner.forward(supplier_key, request);
            let cancel = self.cancel.clone();
            async move {
                let result = fut.await;
                cancel.store(true, Ordering::SeqCst);
                result
            }
        }
    }

    #[tokio::test]
    async fn cancel_set_after_the_first_turn_stops_before_a_second_and_charges_only_the_first() {
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 1_000_000).unwrap();

        let cancel = Arc::new(AtomicBool::new(false));
        // A model that never stops calling tools on its own — without the
        // cancel, this loop would keep going well past turn 1.
        let inner = SequenceTransport::new(vec![tool_use_response(
            10,
            10,
            "toolu_1",
            "list_runs",
            serde_json::json!({}),
        )]);
        let transport = CancelAfterCallTransport {
            inner: inner.clone(),
            cancel: cancel.clone(),
        };

        let reply = send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            transport,
            big_limits(),
            "user-1",
            Some("conv-cancel-1"),
            MODEL,
            "system",
            "keep checking",
            "reply-cancel-1",
            NOW,
            20,
            None,
            VoiceConfirm::Off,
            Some(&cancel),
        )
        .await
        .expect("turn 1's work must still come back as a partial answer, not an error");

        assert_eq!(
            inner.call_count(),
            1,
            "the loop must stop before a second supplier call once cancelled"
        );
        assert!(
            reply.text.contains("cancelled"),
            "partial answer must say why it stopped: {}",
            reply.text
        );

        let price_list = db.active_price_list().unwrap();
        let rate = price_list.model("claude", MODEL).unwrap();
        let observed_micros = rate.cost_micros(10, 0, 10);
        assert_eq!(
            reply.charged_credits, 0,
            "60 micro-USD from the one turn that ran is under one credit"
        );
        assert_eq!(
            db.get_credit_carry_micro_usd("user-1"),
            observed_micros as u64,
            "only the turn that actually ran must be charged, into the carry"
        );

        let reservation = db
            .get_provider_reservation("chat:chat-reply:reply-cancel-1:turn1")
            .unwrap();
        assert_eq!(
            reservation.status, "settled",
            "the completed turn's reservation must be settled, not left dangling as 'reserved'"
        );
    }

    #[tokio::test]
    async fn cancel_set_before_the_call_makes_no_supplier_call_and_leaves_balance_unchanged() {
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 100).unwrap();
        let before = db.get_credit_balance_row("user-1").unwrap();

        // Already cancelled by the time send_paid_reply is called at all.
        let cancel = Arc::new(AtomicBool::new(true));
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
            Some("conv-cancel-2"),
            MODEL,
            "system",
            "hi",
            "reply-cancel-2",
            NOW,
            20,
            None,
            VoiceConfirm::Off,
            Some(&cancel),
        )
        .await
        .expect("a cancel before turn 1 must return cleanly, not error or panic");

        assert_eq!(
            transport.call_count(),
            0,
            "no supplier call must happen once cancelled before turn 1"
        );
        assert_eq!(
            reply.charged_credits, 0,
            "nothing was reserved or charged for zero turns"
        );
        assert!(
            reply.text.contains("cancelled"),
            "text must explain why: {}",
            reply.text
        );

        let after = db.get_credit_balance_row("user-1").unwrap();
        assert_eq!(after.subscription_remaining, before.subscription_remaining);
        assert_eq!(after.pack_remaining, before.pack_remaining);
    }

    /// A generic (non-reservation, non-`NotEnoughCredits`) transport failure
    /// on turn 2 — a timeout, a malformed response, anything the `Err(other)`
    /// arm catches — must still return turn 1's work as a partial answer and
    /// charge for it, the same as the other mid-loop stop reasons.
    #[tokio::test]
    async fn other_error_on_turn_two_returns_partial() {
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 1_000_000).unwrap();
        let price_list = db.active_price_list().unwrap();
        let rate = price_list.model("claude", MODEL).unwrap();

        let transport = SequenceTransport::new(vec![
            tool_use_response(10, 10, "toolu_1", "list_runs", serde_json::json!({})),
            Err(TransportFailure {
                kind: TransportFailureKind::Rejected,
                upstream_request_id: None,
                message: "supplier had a bad day".into(),
            }),
        ]);

        let reply = send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            transport.clone(),
            big_limits(),
            "user-1",
            Some("conv-tool-other-err"),
            MODEL,
            "system",
            "keep checking",
            "reply-tool-other-err",
            NOW,
            20,
            None,
            VoiceConfirm::Off,
            None,
        )
        .await
        .expect("turn 1's work must still come back as a partial answer");

        assert_eq!(transport.call_count(), 2, "turn 2 was attempted and failed");
        assert!(
            reply.text.contains("something went wrong partway"),
            "partial answer must say why it stopped: {}",
            reply.text
        );
        let observed_micros = rate.cost_micros(10, 0, 10);
        assert_eq!(
            reply.charged_credits, 0,
            "60 micro-USD from turn 1 is under one credit"
        );
        assert_eq!(
            db.get_credit_carry_micro_usd("user-1"),
            observed_micros as u64,
            "turn 1's work is still charged (into the carry) even though turn 2 failed"
        );
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
            VoiceConfirm::Off,
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

    /// A transport that, on its first call, opens a second connection to the
    /// same sqlite file and spends most of the user's balance out from under
    /// the reply in progress — standing in for a concurrent charge (another
    /// reply, another device) between when this loop reserved its spend and
    /// when it settles the final charge.
    #[derive(Clone)]
    struct ShrinkingBalanceTransport {
        db_path: std::path::PathBuf,
        user_id: &'static str,
        leave_remaining: i64,
        response_text: &'static str,
        input_tokens: i64,
        output_tokens: i64,
    }

    impl ProviderTransport for ShrinkingBalanceTransport {
        fn forward(
            &self,
            _supplier_key: &str,
            _request: &GatewayRequest,
        ) -> impl Future<Output = Result<TransportResponse, TransportFailure>> + Send {
            let concurrent_db = Database::open(&self.db_path);
            let balance = concurrent_db
                .get_credit_balance_row(self.user_id)
                .expect("balance row");
            let available = balance.subscription_remaining + balance.pack_remaining;
            let drain = (available - self.leave_remaining).max(0);
            if drain > 0 {
                concurrent_db
                    .deduct_credits(
                        self.user_id,
                        drain,
                        "concurrent spend",
                        &ChargeKey::for_verification("concurrent-drain"),
                    )
                    .expect("concurrent deduction should succeed");
            }
            let input_tokens = self.input_tokens;
            let output_tokens = self.output_tokens;
            let text = self.response_text;
            std::future::ready(Ok(TransportResponse {
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
            }))
        }
    }

    #[tokio::test]
    async fn final_charge_shortfall_still_returns_reply() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("chat-paid.sqlite");
        let db = Database::open(&db_path);
        db.init_credit_balance("user-1", 100).unwrap();

        // Usage must stay inside the per-turn reservation (MAX_OUTPUT_TOKENS
        // of output) or the gateway refuses it before the loop sees it. A
        // haiku turn that size costs under one credit, so this test uses a
        // pricier model to make one in-bounds turn cost more than the 1
        // credit left after the concurrent drain.
        const PRICEY_MODEL: &str = "claude-opus-5";
        let input_tokens = 10;
        let output_tokens = MAX_OUTPUT_TOKENS - 96;
        let price_list = db.active_price_list().unwrap();
        let rate = price_list.model("claude", PRICEY_MODEL).unwrap();
        let observed_micros = rate.cost_micros(input_tokens, 0, output_tokens);
        assert!(
            observed_micros > price_list.micros_per_credit,
            "fixture must cost more than the 1 credit (100_000 micro-USD) left after the \
             concurrent drain, for the clamp below to be exercised"
        );

        let transport = ShrinkingBalanceTransport {
            db_path: db_path.clone(),
            user_id: "user-1",
            leave_remaining: 1,
            response_text: "hello there",
            input_tokens,
            output_tokens,
        };

        // The turn-1 reservation cap is based on the balance at loop start
        // (100 credits), so it is happy to authorize a turn that ends up
        // costing more than what's left once the concurrent drain (inside
        // the transport call) has run.
        let limits = SpendLimits {
            max_micro_usd: 1_000_000_000_000,
            funded_micro_usd: 1_000_000_000_000,
        };

        let reply = send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            transport,
            limits,
            "user-1",
            Some("conv-1"),
            PRICEY_MODEL,
            "system",
            "hi",
            "reply-shortfall",
            NOW,
            20,
            None,
            VoiceConfirm::Off,
            None,
        )
        .await
        .expect("a shortfall at final-charge time must not discard the reply");

        assert_eq!(reply.text, "hello there");
        assert_eq!(
            reply.charged_credits, 1,
            "clamped to the 1 credit left, not the full amount owed"
        );
        let balance = db.get_credit_balance_row("user-1").unwrap();
        assert_eq!(
            balance.subscription_remaining + balance.pack_remaining,
            0,
            "the clamped charge must take exactly what was left, not go negative"
        );
        let recorded_cost: i64 = db
            .conn()
            .query_row(
                "SELECT cost_micro_usd FROM credit_transactions \
                 WHERE idempotency_key = ?1",
                rusqlite::params![ChargeKey::for_chat_reply("reply-shortfall").as_str()],
                |row| row.get(0),
            )
            .expect("settled charge row");
        assert_eq!(
            recorded_cost, observed_micros,
            "cost_micro_usd must record the full nominal cost, not the clamped amount \
             actually charged"
        );

        // The carry must still advance by the pricing math's full total
        // (starting carry, here 0, plus the observed cost), mod
        // micros_per_credit -- not clamped down to what was actually
        // collected. This is a fresh balance, so starting carry is 0.
        let expected_carry =
            (observed_micros as u64) % (price_list.micros_per_credit as u64);
        assert_eq!(
            db.get_credit_carry_micro_usd("user-1"),
            expected_carry,
            "carry must equal (starting carry + observed) % micros_per_credit"
        );

        // The whole credits this charge could not collect (owed - charged)
        // must be recorded, not hidden: they are absorbed by Cortex, and
        // that absorption lives in the ledger row's own description.
        let credits_owed = (observed_micros as u64) / (price_list.micros_per_credit as u64);
        assert!(
            credits_owed > reply.charged_credits as u64,
            "this test only proves anything if the shortfall is nonzero"
        );
        let description: String = db
            .conn()
            .query_row(
                "SELECT description FROM credit_transactions WHERE idempotency_key = ?1",
                rusqlite::params![ChargeKey::for_chat_reply("reply-shortfall").as_str()],
                |row| row.get(0),
            )
            .expect("settled charge row");
        assert!(
            description.contains(&format!(
                "owed {credits_owed}, charged {}",
                reply.charged_credits
            )) && description.contains("shortfall absorbed by Cortex"),
            "description must record the shortfall instead of hiding it: {description}"
        );
    }

    #[tokio::test]
    async fn a_non_exhaustion_reservation_error_is_unavailable_not_no_credits() {
        // Only "authorization exhausted: ..." (db/provider_gateway.rs) is
        // genuinely the user's balance. Every other `GatewayError::Reservation`
        // -- a DB/IO failure, an amount overflow, a missing/expired/revoked
        // authorization, or (exercised here) a request-key replay conflict --
        // is Cortex's own problem, and telling the user they're out of
        // credits over it would be false.
        let (_dir, db) = test_db();
        db.init_credit_balance("user-1", 100).unwrap();

        let messages_1 = vec![serde_json::json!({"role": "user", "content": "hi"})];
        send_one_turn(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            FixedTransport::ok(10, 10, "first"),
            1_000_000,
            1_000_000,
            "user-1",
            "reply-conflict",
            "claude",
            MODEL,
            "system",
            &messages_1,
            &[],
            true,
            "reply-conflict",
            1,
            NOW,
        )
        .await
        .expect("first call reserves and settles turn 1");

        // Same reply id and turn number (same request key), a different
        // message body (different request digest): the reservation this
        // replays onto was settled for a different digest, so the gateway
        // refuses it as a replay conflict rather than an authorization-cap
        // refusal.
        let messages_2 =
            vec![serde_json::json!({"role": "user", "content": "a different message"})];
        let result = send_one_turn(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            FixedTransport::ok(10, 10, "second"),
            1_000_000,
            1_000_000,
            "user-1",
            "reply-conflict",
            "claude",
            MODEL,
            "system",
            &messages_2,
            &[],
            true,
            "reply-conflict",
            1,
            NOW,
        )
        .await;

        assert!(
            matches!(result, Err(PaidReplyError::Unavailable)),
            "a request-key replay conflict is Cortex's problem, not the user's balance: {result:?}"
        );
    }

    /// A transport that tracks how many calls are in flight at once (and the
    /// high-water mark), and sleeps briefly so two concurrent callers would
    /// overlap if nothing serialized them.
    #[derive(Clone)]
    struct ConcurrencyTrackingTransport {
        in_flight: Arc<AtomicUsize>,
        max_in_flight: Arc<AtomicUsize>,
        input_tokens: i64,
        output_tokens: i64,
    }

    impl ProviderTransport for ConcurrencyTrackingTransport {
        fn forward(
            &self,
            _supplier_key: &str,
            _request: &GatewayRequest,
        ) -> impl Future<Output = Result<TransportResponse, TransportFailure>> + Send {
            let in_flight = self.in_flight.clone();
            let max_in_flight = self.max_in_flight.clone();
            let input_tokens = self.input_tokens;
            let output_tokens = self.output_tokens;
            async move {
                let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                max_in_flight.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                in_flight.fetch_sub(1, Ordering::SeqCst);
                Ok(TransportResponse {
                    body: serde_json::json!({
                        "id": "msg_test",
                        "type": "message",
                        "role": "assistant",
                        "model": MODEL,
                        "content": [{"type": "text", "text": "hello there"}],
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
                })
            }
        }
    }

    #[tokio::test]
    async fn concurrent_replies_same_user_do_not_overspend() {
        let (_dir, db) = test_db();
        let price_list = db.active_price_list().unwrap();
        let rate = price_list.model("claude", MODEL).unwrap();
        // One credit is comfortably more than either reply's ~60 micro-USD
        // observed cost, or the gateway's own worst-case per-call reservation
        // ceiling -- this test is about the per-user lock serializing the
        // ledger writes, not about exhausting a tight balance.
        let initial_balance = 1;
        db.init_credit_balance("user-1", initial_balance).unwrap();

        let limits = SpendLimits {
            max_micro_usd: 1_000_000_000_000,
            funded_micro_usd: 1_000_000_000_000,
        };
        let transport = ConcurrencyTrackingTransport {
            in_flight: Arc::new(AtomicUsize::new(0)),
            max_in_flight: Arc::new(AtomicUsize::new(0)),
            input_tokens: 10,
            output_tokens: 10,
        };

        let fut_a = send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            transport.clone(),
            limits,
            "user-1",
            Some("conv-a"),
            MODEL,
            "system",
            "hi",
            "reply-a",
            NOW,
            20,
            None,
            VoiceConfirm::Off,
            None,
        );
        let fut_b = send_paid_reply(
            &db,
            SIGNING_KEY,
            SUPPLIER_KEY,
            transport.clone(),
            limits,
            "user-1",
            Some("conv-b"),
            MODEL,
            "system",
            "hi",
            "reply-b",
            NOW,
            20,
            None,
            VoiceConfirm::Off,
            None,
        );

        let (result_a, result_b) = tokio::join!(fut_a, fut_b);

        let reply_a = result_a.expect("first reply should succeed");
        let reply_b = result_b.expect("second reply should succeed");
        assert_eq!(
            transport.max_in_flight.load(Ordering::SeqCst),
            1,
            "the per-user lock must keep the two replies' supplier calls from overlapping"
        );

        let balance = db.get_credit_balance_row("user-1").unwrap();
        let total_charged =
            initial_balance - (balance.subscription_remaining + balance.pack_remaining);
        assert_eq!(
            total_charged, 0,
            "120 micro-USD total is well under one credit, so nothing should be deducted \
             from the balance"
        );
        assert_eq!(
            reply_a.charged_credits + reply_b.charged_credits,
            total_charged,
            "the sum of what each reply reports charging must match the actual balance delta"
        );
        let observed_micros = rate.cost_micros(10, 0, 10);
        assert_eq!(
            db.get_credit_carry_micro_usd("user-1"),
            (observed_micros * 2) as u64,
            "both replies' costs must land in the carry exactly once each, with nothing lost \
             to a race"
        );
    }
}
