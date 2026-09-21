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

use std::sync::Arc;

use cortex_core::billing_binding::ChargeKey;
use serde_json::Value;
use tokio::sync::mpsc;

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
        }
    }
}

#[derive(Debug)]
pub(crate) struct PaidReply {
    /// The assistant's text, concatenated from every text block Anthropic
    /// returned.
    pub text: String,
    /// Whole credits charged. `0` when usage was observed but rounded down to
    /// nothing, or when usage was never observed at all (see below).
    pub charged_credits: i64,
}

/// Reserve, call the supplier, settle, and charge — the whole paid-reply
/// pipeline, independent of the SSE plumbing so it can be tested directly
/// against a stub transport and an in-memory database.
///
/// `reply_id` is the caller's uuid for this one reply; it is folded into both
/// the gateway's attempt id and the ledger's charge key, so calling this
/// twice with the same id charges at most once.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn send_paid_reply<T: ProviderTransport>(
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
    let price_list = db.active_price_list().ok_or(PaidReplyError::Unavailable)?;
    if price_list.model("claude", model).is_none() {
        tracing::error!(model, "chat: no price for this model tier; refusing");
        return Err(PaidReplyError::Unavailable);
    }

    let balance = db
        .get_credit_balance_row(user_id)
        .filter(|b| b.subscription_remaining + b.pack_remaining > 0)
        .ok_or(PaidReplyError::NoCredits)?;
    let balance_micro_usd = (balance.subscription_remaining + balance.pack_remaining)
        .saturating_mul(price_list.micros_per_credit);

    let max_micro_usd = limits.max_micro_usd.min(balance_micro_usd);
    if max_micro_usd <= 0 {
        return Err(PaidReplyError::NoCredits);
    }

    let attempt_id = format!("chat-reply:{reply_id}");
    let run_id = conversation_id
        .map(|c| format!("chat:{c}"))
        .unwrap_or_else(|| format!("chat:{reply_id}"));
    let expires_at_ms = now_ms + LEASE_MS;

    let (_authorization_id, signed) = provider_gateway_http::create_authorization_and_capability(
        db,
        signing_key,
        user_id,
        &run_id,
        &attempt_id,
        model,
        max_micro_usd,
        limits.funded_micro_usd,
        expires_at_ms,
        now_ms,
    )
    .ok_or(PaidReplyError::Unavailable)?;

    let body = serde_json::json!({
        "model": model,
        "max_tokens": MAX_OUTPUT_TOKENS,
        "system": system_prompt,
        "messages": [{"role": "user", "content": user_message}],
        "stream": false,
    });
    let request = GatewayRequest {
        request_key: format!("chat:{attempt_id}"),
        tenant_id: user_id.to_string(),
        run_id,
        attempt_id,
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
            // balance) cannot cover the worst-case cost of this reply.
            GatewayError::Reservation(detail) => {
                tracing::info!(user_id, %detail, "chat: spend reservation refused");
                PaidReplyError::NotEnoughCredits
            }
            other => {
                tracing::error!(user_id, error = %other, "chat: gateway call failed");
                PaidReplyError::Provider(other.to_string())
            }
        })?;

    let Some(body) = outcome.body else {
        // A replay of a reply id that already resolved. `reply_id` is minted
        // once per reply, so this should not happen in practice; treat it as
        // the caller asking twice rather than as a supplier failure.
        return Err(PaidReplyError::Provider(
            "this reply was already sent".into(),
        ));
    };
    let text = extract_text(&body);

    let charged_credits = match outcome.reservation.observed_micro_usd {
        Some(observed) if observed > 0 => {
            let credits = ceil_div(observed, price_list.micros_per_credit);
            if let Err(e) = db.deduct_credits(
                user_id,
                credits,
                "Cortex-paid chat reply",
                &ChargeKey::for_chat_reply(reply_id),
            ) {
                // The reply already succeeded and the user already has it;
                // failing the reply over a ledger write would double-punish
                // them for Cortex's bug. The reservation is the source of
                // truth for what was actually spent, so nothing is lost.
                tracing::error!(user_id, reply_id, error = %e, "chat: reply succeeded but charging credits failed");
            }
            credits
        }
        Some(_) => 0,
        None => {
            // No observed usage: never invent a price. The reservation stays
            // exactly as the gateway left it (reserved or unresolved) for
            // reconciliation; this module does not touch it further.
            tracing::error!(
                user_id,
                reply_id,
                "chat: reply succeeded with no observed usage; not charging"
            );
            0
        }
    };

    Ok(PaidReply {
        text,
        charged_credits,
    })
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
                %reply_id,
                "cortex-paid chat reply charged"
            );
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
            db.get_provider_reservation("chat:chat-reply:reply-3")
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
        let authorization_id = "gateway-auth:chat-reply:reply-5";
        let reservation = db
            .get_provider_reservation("chat:chat-reply:reply-5")
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
