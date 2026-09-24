//! Capability-authenticated provider surface for the private gateway.
//!
//! Off unless `CORTEX_PROVIDER_GATEWAY_MODE` says otherwise, and it can say
//! exactly two things:
//!
//! - `stub`: answers every call itself with a fixed reply. Nothing leaves the
//!   machine and nothing is spent. This is what the proofs run against.
//! - `live`: calls the supplier named in the verified capability, on Cortex's
//!   own key for that supplier (`CORTEX_ANTHROPIC_SUPPLIER_KEY` for Claude,
//!   `CORTEX_OPENAI_SUPPLIER_KEY` for OpenAI). This spends real money, inside
//!   the same reservation and cap as the stub. Live mode is on as soon as at
//!   least one supplier key is present; a request for a provider without a
//!   funded key is refused the same way an unknown provider would be.
//!   OpenCode Zen is bring-your-own-key only and never reaches this gateway:
//!   Cortex never holds a Zen key of its own.
//!
//! Any other value, or none, leaves the listener unavailable.

use std::future::Future;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::Value;

use crate::provider_gateway::{
    sign_capability, GatewayCapability, GatewayError, GatewayRequest, ObservedUsage,
    ProviderGateway, ProviderTransport, SignedCapability, TransportFailure, TransportResponse,
};
use crate::state::AppState;

const STUB_SUPPLIER_KEY: &str = "STUB-PROVIDER-NOT-A-REAL-KEY";

/// Where the gateway itself is reachable, derived from the one host constant
/// so this and the sandbox's allowlist/policy checks can never drift apart.
static GATEWAY_BASE_URL: once_cell::sync::Lazy<String> = once_cell::sync::Lazy::new(|| {
    format!(
        "https://{}/internal/provider",
        cortex_core::egress::PROVIDER_GATEWAY_HOST
    )
});

/// Anthropic keys are far longer than this; anything shorter is a typo or a
/// placeholder, and a placeholder must not switch real spending on.
const MIN_SUPPLIER_KEY_LEN: usize = 20;

enum GatewayMode {
    Stub,
    /// One key per supplier the operator has funded. Live mode is on as soon
    /// as at least one supplier has a key; a request for a provider without
    /// one fails the same way an unknown provider would.
    Live {
        supplier_keys: std::collections::HashMap<String, String>,
    },
}

/// Every env var that can fund a supplier in live mode, paired with the
/// provider label it funds. Adding a supplier is one entry here.
const SUPPLIER_KEY_ENV_VARS: &[(&str, &str)] = &[
    ("claude", "CORTEX_ANTHROPIC_SUPPLIER_KEY"),
    ("openai", "CORTEX_OPENAI_SUPPLIER_KEY"),
];

fn gateway_mode() -> Option<GatewayMode> {
    match std::env::var("CORTEX_PROVIDER_GATEWAY_MODE").as_deref() {
        Ok("stub") => Some(GatewayMode::Stub),
        Ok("live") => {
            let supplier_keys: std::collections::HashMap<String, String> = SUPPLIER_KEY_ENV_VARS
                .iter()
                .filter_map(|(provider, env_var)| {
                    std::env::var(env_var)
                        .ok()
                        .filter(|key| key.trim().len() >= MIN_SUPPLIER_KEY_LEN)
                        .map(|key| (provider.to_string(), key))
                })
                .collect();
            if supplier_keys.is_empty() {
                tracing::error!(
                    "gateway mode is live but no supplier key is present; gateway stays off"
                );
                return None;
            }
            Some(GatewayMode::Live { supplier_keys })
        }
        _ => None,
    }
}

pub(crate) fn issue_access(
    db: &crate::db::Database,
    user_id: &str,
    run_id: &str,
    attempt_id: &str,
    provider: cortex_core::provider::ProviderId,
    model: &str,
    lease_deadline_ms: i64,
    now_ms: i64,
) -> Option<cortex_core::protocol::ProviderGatewayAccess> {
    gateway_mode()?;
    let allowed = match provider {
        cortex_core::provider::ProviderId::Claude | cortex_core::provider::ProviderId::Openai => {
            true
        }
        // Zen is BYOK; Cortex never funds it, so a run can never be issued a
        // Zen capability against Cortex's own money.
        cortex_core::provider::ProviderId::Zen => false,
        _ => false,
    };
    if !allowed {
        return None;
    }
    let provider_label = cortex_core::egress::provider_grant_name(provider);
    let signing_key = std::env::var("CORTEX_PROVIDER_GATEWAY_SIGNING_KEY").ok()?;
    let limits = SpendLimits::from_env()?;
    let expires_at_ms = lease_deadline_ms;
    let (authorization_id, signed) = create_authorization_and_capability(
        db,
        &signing_key,
        user_id,
        run_id,
        attempt_id,
        provider_label,
        model,
        limits.max_micro_usd,
        limits.funded_micro_usd,
        expires_at_ms,
        now_ms,
    )?;
    Some(cortex_core::protocol::ProviderGatewayAccess {
        authorization_id,
        run_id: run_id.to_string(),
        attempt_id: attempt_id.to_string(),
        provider: provider_label.into(),
        model: model.to_string(),
        base_url: GATEWAY_BASE_URL.clone(),
        expires_at_ms,
        bearer: cortex_core::protocol::GatewayBearer::new(signed.expose()),
    })
}

/// The reusable core of a spend authorization plus its signed capability.
///
/// `issue_access` (a run, handed the capability over HTTP) and chat (paid by
/// Cortex, calling the gateway in-process) are the same trust boundary — one
/// durable authorization row, funded supplier capacity, one signature over an
/// immutable rate — so this is the one place that boundary gets built. The
/// caller owns picking `max_micro_usd` (a run reads it from env; chat caps it
/// at the user's own balance) and the deadline; everything else is identical.
pub(crate) fn create_authorization_and_capability(
    db: &crate::db::Database,
    signing_key: &str,
    user_id: &str,
    run_id: &str,
    attempt_id: &str,
    provider: &str,
    model: &str,
    max_micro_usd: i64,
    funded_micro_usd: i64,
    expires_at_ms: i64,
    now_ms: i64,
) -> Option<(String, SignedCapability)> {
    if signing_key.len() < 32 {
        tracing::error!("gateway signing key must contain at least 32 bytes");
        return None;
    }
    if !crate::provider_gateway::KNOWN_PROVIDERS.contains(&provider) {
        tracing::error!(provider, "gateway does not know this supplier");
        return None;
    }
    let price_list = db.active_price_list()?;
    if price_list.model(provider, model).is_none() {
        tracing::error!(provider, model, "gateway has no immutable model rate");
        return None;
    }
    db.set_supplier_capacity(provider, funded_micro_usd, now_ms)
        .ok()?;
    if expires_at_ms <= now_ms {
        return None;
    }
    let authorization_id = format!("gateway-auth:{attempt_id}");
    db.create_spend_authorization(
        &crate::db::SpendAuthorization {
            id: authorization_id.clone(),
            user_id: user_id.to_string(),
            run_id: run_id.to_string(),
            attempt_id: attempt_id.to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
            price_list_id: price_list.id,
            max_micro_usd,
            expires_at_ms,
        },
        now_ms,
    )
    .ok()?;
    let claims = GatewayCapability::new(
        authorization_id.clone(),
        user_id,
        run_id,
        attempt_id,
        provider,
        model,
        expires_at_ms,
    );
    let signed = sign_capability(signing_key.as_bytes(), &claims).ok()?;
    Some((authorization_id, signed))
}

/// The operator's two spending numbers: the most one authorization may spend,
/// and how much Cortex has funded the supplier with. Read once from the
/// environment by the caller and passed down, so nothing below reads process
/// state that a test running alongside could change.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SpendLimits {
    pub max_micro_usd: i64,
    pub funded_micro_usd: i64,
}

impl SpendLimits {
    pub(crate) fn from_env() -> Option<Self> {
        Some(Self {
            max_micro_usd: positive_env("CORTEX_PROVIDER_GATEWAY_MAX_MICRO_USD")?,
            funded_micro_usd: positive_env("CORTEX_PROVIDER_GATEWAY_FUNDED_MICRO_USD")?,
        })
    }
}

pub(crate) fn positive_env(name: &str) -> Option<i64> {
    std::env::var(name)
        .ok()?
        .parse()
        .ok()
        .filter(|value| *value > 0)
}

#[derive(Clone, Copy)]
pub(crate) struct StubTransport;

impl ProviderTransport for StubTransport {
    fn forward(
        &self,
        _supplier_key: &str,
        request: &GatewayRequest,
    ) -> impl Future<Output = Result<TransportResponse, TransportFailure>> + Send {
        let model = request.model.clone();
        std::future::ready(Ok(TransportResponse {
            body: serde_json::json!({
                "id": "msg_cortex_stub",
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [{"type": "text", "text": "cortex gateway stub"}],
                "stop_reason": "end_turn",
                "stop_sequence": null,
                "usage": {"input_tokens": 1, "output_tokens": 1}
            }),
            upstream_request_id: Some("cortex-stub-upstream".into()),
            usage: Some(ObservedUsage {
                input_tokens: 1,
                cached_input_tokens: 0,
                output_tokens: 1,
            }),
        }))
    }
}

/// Either transport the gateway can run against, behind one type so a caller
/// that only knows "the gateway is usable" doesn't need to be generic.
#[derive(Clone)]
pub(crate) enum GatewayTransport {
    Stub(StubTransport),
    Live(crate::supplier_anthropic::AnthropicTransport),
    LiveOpenAi(crate::supplier_openai::OpenAiTransport),
}

impl ProviderTransport for GatewayTransport {
    async fn forward(
        &self,
        supplier_key: &str,
        request: &GatewayRequest,
    ) -> Result<TransportResponse, TransportFailure> {
        match self {
            GatewayTransport::Stub(t) => t.forward(supplier_key, request).await,
            GatewayTransport::Live(t) => t.forward(supplier_key, request).await,
            GatewayTransport::LiveOpenAi(t) => t.forward(supplier_key, request).await,
        }
    }
}

/// The live transport for a provider label, or `None` if the gateway does
/// not have a supplier file for it. One new supplier is one new match arm.
fn live_transport_for(provider: &str) -> Option<GatewayTransport> {
    match provider {
        "claude" => Some(GatewayTransport::Live(
            crate::supplier_anthropic::AnthropicTransport::new(),
        )),
        "openai" => Some(GatewayTransport::LiveOpenAi(
            crate::supplier_openai::OpenAiTransport::new(),
        )),
        // Zen is BYOK-only and never reaches the gateway; see supplier_zen.rs.
        _ => None,
    }
}

/// What chat needs to know to pay for its own reply: the gateway is on, and
/// here is what to sign with and who to call. Stub mode keeps this usable —
/// and free — in CI; live mode is only reachable with a real supplier key.
pub(crate) struct GatewayUsable {
    pub signing_key: String,
    pub supplier_key: String,
    pub transport: GatewayTransport,
}

/// Whether the gateway can be called right now, and with what. Returns
/// `None` for exactly the reasons `messages` above would answer
/// `SERVICE_UNAVAILABLE`: no mode configured, or a signing key that is
/// missing or too short to trust.
///
/// Chat is the Anthropic path only (`ProviderPath::Cortex` in `chat_paid.rs`),
/// so this always resolves the Claude supplier; a run reaching the gateway
/// over HTTP is the path that can ask for any known provider.
pub(crate) fn gateway_usable() -> Option<GatewayUsable> {
    let mode = gateway_mode()?;
    let signing_key = std::env::var("CORTEX_PROVIDER_GATEWAY_SIGNING_KEY").ok()?;
    if signing_key.len() < 32 {
        return None;
    }
    Some(match mode {
        GatewayMode::Stub => GatewayUsable {
            signing_key,
            supplier_key: STUB_SUPPLIER_KEY.into(),
            transport: GatewayTransport::Stub(StubTransport),
        },
        GatewayMode::Live { mut supplier_keys } => GatewayUsable {
            signing_key,
            supplier_key: supplier_keys.remove("claude")?,
            transport: GatewayTransport::Live(crate::supplier_anthropic::AnthropicTransport::new()),
        },
    })
}

pub async fn messages(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let Some(mode) = gateway_mode() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "provider gateway is disabled",
        )
            .into_response();
    };
    let Ok(signing_key) = std::env::var("CORTEX_PROVIDER_GATEWAY_SIGNING_KEY") else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "gateway signing key is absent",
        )
            .into_response();
    };
    if signing_key.len() < 32 {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "gateway signing key is invalid",
        )
            .into_response();
    }
    let Some(db) = state.db.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "gateway database is absent",
        )
            .into_response();
    };
    let now_ms = chrono::Utc::now().timestamp_millis();
    match mode {
        GatewayMode::Stub => {
            handle_stub_message(db, signing_key.as_bytes(), &headers, body, now_ms).await
        }
        GatewayMode::Live { supplier_keys } => {
            handle_live_message(
                db,
                signing_key.as_bytes(),
                &supplier_keys,
                &headers,
                body,
                now_ms,
            )
            .await
        }
    }
}

/// Live mode can hold keys for more than one supplier, so which transport and
/// key to use is decided from the verified capability's own provider claim,
/// never from anything the caller asserts unsigned.
async fn handle_live_message(
    db: &crate::db::Database,
    signing_key: &[u8],
    supplier_keys: &std::collections::HashMap<String, String>,
    headers: &HeaderMap,
    body: Value,
    now_ms: i64,
) -> Response {
    let Some(token) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return (StatusCode::UNAUTHORIZED, "missing gateway bearer").into_response();
    };
    let capability = SignedCapability::from_exposed(token);
    let claims = match crate::provider_gateway::verify_capability(signing_key, &capability) {
        Ok(claims) => claims,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid gateway bearer").into_response(),
    };
    let Some(supplier_key) = supplier_keys.get(claims.provider.as_str()) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "gateway has no supplier key for this provider",
        )
            .into_response();
    };
    let Some(transport) = live_transport_for(&claims.provider) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "gateway has no live transport for this provider",
        )
            .into_response();
    };
    handle_message(
        db,
        signing_key,
        supplier_key,
        transport,
        headers,
        body,
        now_ms,
    )
    .await
}

async fn handle_stub_message(
    db: &crate::db::Database,
    signing_key: &[u8],
    headers: &HeaderMap,
    body: Value,
    now_ms: i64,
) -> Response {
    handle_message(
        db,
        signing_key,
        STUB_SUPPLIER_KEY,
        StubTransport,
        headers,
        body,
        now_ms,
    )
    .await
}

async fn handle_message<T: ProviderTransport>(
    db: &crate::db::Database,
    signing_key: &[u8],
    supplier_key: &str,
    transport: T,
    headers: &HeaderMap,
    body: Value,
    now_ms: i64,
) -> Response {
    let wants_stream = body.get("stream").and_then(Value::as_bool) == Some(true);
    let Some(token) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return (StatusCode::UNAUTHORIZED, "missing gateway bearer").into_response();
    };
    let gateway = ProviderGateway::new(db, signing_key, supplier_key, transport);
    let capability = SignedCapability::from_exposed(token);
    let claims = match gateway.verified_claims(&capability) {
        Ok(claims) => claims,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid gateway bearer").into_response(),
    };
    let request_key = match headers
        .get("x-cortex-request-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
    {
        Some(explicit) => format!("{}:{}:{explicit}", claims.provider, claims.authorization_id),
        None => {
            let attempt = headers
                .get("x-cortex-attempt")
                .and_then(|value| value.to_str().ok());
            if attempt != Some(claims.attempt_id.as_str()) {
                return (StatusCode::BAD_REQUEST, "missing request identity").into_response();
            }
            use sha2::Digest as _;
            let encoded = serde_json::to_vec(&body).unwrap_or_default();
            format!(
                "{}:{}:sha256:{}",
                claims.provider,
                claims.authorization_id,
                hex::encode(sha2::Sha256::digest(encoded))
            )
        }
    };
    let Some(max_output_tokens) = body.get("max_tokens").and_then(Value::as_i64) else {
        return (StatusCode::BAD_REQUEST, "missing bounded max_tokens").into_response();
    };
    let request = GatewayRequest {
        request_key,
        tenant_id: claims.tenant_id,
        run_id: claims.run_id,
        attempt_id: claims.attempt_id,
        provider: claims.provider,
        model: claims.model,
        max_output_tokens,
        body,
        capability,
    };
    match gateway.forward(request, now_ms).await {
        Ok(outcome) => match outcome.body {
            Some(body) if wants_stream => message_as_sse(&body),
            Some(body) => Json(body).into_response(),
            None => (
                StatusCode::CONFLICT,
                "request already has a durable outcome",
            )
                .into_response(),
        },
        Err(error) => gateway_error_response(error),
    }
}

/// A finished message, replayed as the event stream Anthropic would have sent.
///
/// The gateway settles on the finished message (see `supplier_anthropic.rs`
/// for why), so a caller that asked for a stream gets it all at once. Each
/// block is sent whole in one delta rather than in pieces, which every client
/// that reads the stream accepts: text as a `text_delta`, a tool call's input
/// as one `input_json_delta`, thinking as a `thinking_delta` plus its
/// `signature_delta`. A block kind with no delta form is sent complete in its
/// `content_block_start`.
fn message_as_sse(message: &Value) -> Response {
    fn push_event(output: &mut String, name: &str, data: Value) {
        output.push_str("event: ");
        output.push_str(name);
        output.push_str("\ndata: ");
        output.push_str(&data.to_string());
        output.push_str("\n\n");
    }

    let id = message
        .get("id")
        .cloned()
        .unwrap_or_else(|| Value::from("msg_cortex_gateway"));
    let model = message.get("model").cloned().unwrap_or(Value::Null);
    let stop_reason = message
        .get("stop_reason")
        .cloned()
        .unwrap_or_else(|| Value::from("end_turn"));
    let stop_sequence = message.get("stop_sequence").cloned().unwrap_or(Value::Null);
    let blocks = message
        .get("content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let input_tokens = message
        .pointer("/usage/input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let output_tokens = message
        .pointer("/usage/output_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let mut stream = String::new();
    push_event(
        &mut stream,
        "message_start",
        serde_json::json!({
            "type": "message_start",
            "message": {
                "id": id,
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "usage": {"input_tokens": input_tokens, "output_tokens": 0}
            }
        }),
    );
    for (index, block) in blocks.iter().enumerate() {
        let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
        let field = |name: &str| block.get(name).cloned().unwrap_or(Value::Null);
        let (start, deltas) = match kind {
            "text" => (
                serde_json::json!({"type": "text", "text": ""}),
                vec![serde_json::json!({"type": "text_delta", "text": field("text")})],
            ),
            "tool_use" => (
                serde_json::json!({
                    "type": "tool_use",
                    "id": field("id"),
                    "name": field("name"),
                    "input": {}
                }),
                vec![serde_json::json!({
                    "type": "input_json_delta",
                    "partial_json": block
                        .get("input")
                        .map(Value::to_string)
                        .unwrap_or_else(|| "{}".into())
                })],
            ),
            "thinking" => (
                serde_json::json!({"type": "thinking", "thinking": ""}),
                vec![
                    serde_json::json!({"type": "thinking_delta", "thinking": field("thinking")}),
                    serde_json::json!({"type": "signature_delta", "signature": field("signature")}),
                ],
            ),
            _ => (block.clone(), Vec::new()),
        };
        push_event(
            &mut stream,
            "content_block_start",
            serde_json::json!({
                "type": "content_block_start",
                "index": index,
                "content_block": start
            }),
        );
        for delta in deltas {
            push_event(
                &mut stream,
                "content_block_delta",
                serde_json::json!({
                    "type": "content_block_delta",
                    "index": index,
                    "delta": delta
                }),
            );
        }
        push_event(
            &mut stream,
            "content_block_stop",
            serde_json::json!({"type": "content_block_stop", "index": index}),
        );
    }
    push_event(
        &mut stream,
        "message_delta",
        serde_json::json!({
            "type": "message_delta",
            "delta": {"stop_reason": stop_reason, "stop_sequence": stop_sequence},
            "usage": {"output_tokens": output_tokens}
        }),
    );
    push_event(
        &mut stream,
        "message_stop",
        serde_json::json!({"type": "message_stop"}),
    );
    (
        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
        stream,
    )
        .into_response()
}

#[cfg(feature = "gateway-cli-proof")]
#[derive(Clone)]
struct ProofState {
    db: std::sync::Arc<crate::db::Database>,
    signing_key: std::sync::Arc<Vec<u8>>,
    authorization_id: std::sync::Arc<String>,
}

#[cfg(feature = "gateway-cli-proof")]
pub(crate) fn proof_router(
    db: crate::db::Database,
    signing_key: Vec<u8>,
    authorization_id: String,
) -> axum::Router {
    use axum::routing::{get, post};

    let state = ProofState {
        db: std::sync::Arc::new(db),
        signing_key: std::sync::Arc::new(signing_key),
        authorization_id: std::sync::Arc::new(authorization_id),
    };
    axum::Router::new()
        .route("/internal/provider/v1/messages", post(proof_messages))
        .route("/health", get(|| async { StatusCode::NO_CONTENT }))
        .route("/proof/status", get(proof_status))
        .with_state(state)
}

#[cfg(feature = "gateway-cli-proof")]
async fn proof_messages(
    State(state): State<ProofState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let tool_names = body
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool.get("name").and_then(Value::as_str))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    eprintln!("gateway proof request tools: {tool_names:?}");
    handle_stub_message(
        &state.db,
        state.signing_key.as_slice(),
        &headers,
        body,
        chrono::Utc::now().timestamp_millis(),
    )
    .await
}

#[cfg(feature = "gateway-cli-proof")]
async fn proof_status(State(state): State<ProofState>) -> Json<Value> {
    let (rows, settled) = state
        .db
        .provider_authorization_spend_summary(&state.authorization_id);
    Json(serde_json::json!({"rows": rows, "settled": settled}))
}

fn gateway_error_response(error: GatewayError) -> Response {
    let status = match error {
        GatewayError::InvalidCapability | GatewayError::ExpiredCapability => {
            StatusCode::UNAUTHORIZED
        }
        GatewayError::UnsupportedProvider
        | GatewayError::ScopeMismatch
        | GatewayError::UnboundedRequest(_)
        | GatewayError::MissingRate => StatusCode::BAD_REQUEST,
        GatewayError::Reservation(_) => StatusCode::CONFLICT,
        GatewayError::CostOverflow
        | GatewayError::Transport(_)
        | GatewayError::CredentialExposure
        | GatewayError::Reconciliation(_) => StatusCode::BAD_GATEWAY,
    };
    (status, error.to_string()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::SpendAuthorization;

    const NOW: i64 = 1_800_000_000_000;
    const MODEL: &str = "claude-sonnet-4-6";
    const SIGNING_KEY: &[u8] = b"stub-http-signing-key-with-32-bytes";

    struct Fixture {
        _dir: tempfile::TempDir,
        db: crate::db::Database,
        token: SignedCapability,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db = crate::db::Database::open(&dir.path().join("gateway-http.sqlite"));
            let price_list_id = db.active_price_list().unwrap().id;
            db.set_supplier_capacity("claude", 1_000_000, NOW).unwrap();
            db.create_spend_authorization(
                &SpendAuthorization {
                    id: "auth-http".into(),
                    user_id: "tenant-http".into(),
                    run_id: "run-http".into(),
                    attempt_id: "attempt-http".into(),
                    provider: "claude".into(),
                    model: MODEL.into(),
                    price_list_id,
                    max_micro_usd: 1_000_000,
                    expires_at_ms: NOW + 60_000,
                },
                NOW,
            )
            .unwrap();
            let token = sign_capability(
                SIGNING_KEY,
                &GatewayCapability::new(
                    "auth-http",
                    "tenant-http",
                    "run-http",
                    "attempt-http",
                    "claude",
                    MODEL,
                    NOW + 60_000,
                ),
            )
            .unwrap();
            Self {
                _dir: dir,
                db,
                token,
            }
        }

        fn headers(&self, request_key: &str) -> HeaderMap {
            let mut headers = HeaderMap::new();
            headers.insert(
                axum::http::header::AUTHORIZATION,
                format!("Bearer {}", self.token.expose()).parse().unwrap(),
            );
            headers.insert("x-cortex-request-key", request_key.parse().unwrap());
            headers
        }
    }

    #[tokio::test]
    async fn stub_listener_authenticates_reserves_and_settles() {
        let fixture = Fixture::new();
        let response = handle_stub_message(
            &fixture.db,
            SIGNING_KEY,
            &fixture.headers("http-request-1"),
            serde_json::json!({
                "model": MODEL,
                "max_tokens": 100,
                "messages": [{"role": "user", "content": "stub only"}]
            }),
            NOW,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let reservation = fixture
            .db
            .get_provider_reservation("claude:auth-http:http-request-1")
            .unwrap();
        assert_eq!(reservation.status, "settled");
        assert_eq!(
            fixture
                .db
                .provider_spend_row_count("claude:auth-http:http-request-1"),
            1
        );
    }

    #[tokio::test]
    async fn measured_cli_shape_returns_anthropic_sse_after_settlement() {
        let fixture = Fixture::new();
        let headers = fixture.headers("http-stream-1");
        let request = serde_json::json!({
            "model": MODEL,
            "max_tokens": 32_000,
            "messages": [{"role": "user", "content": "stub only"}],
            "stream": true,
            "tools": []
        });
        let response =
            handle_stub_message(&fixture.db, SIGNING_KEY, &headers, request.clone(), NOW).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .unwrap(),
            "text/event-stream"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        for event in [
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop",
        ] {
            assert!(body.contains(&format!("event: {event}\n")));
        }
        let reservation = fixture
            .db
            .get_provider_reservation("claude:auth-http:http-stream-1")
            .unwrap();
        assert_eq!(reservation.status, "settled");
        assert_eq!(
            fixture
                .db
                .provider_spend_row_count("claude:auth-http:http-stream-1"),
            1
        );

        let replay = handle_stub_message(&fixture.db, SIGNING_KEY, &headers, request, NOW).await;
        assert_eq!(replay.status(), StatusCode::CONFLICT);
        assert_eq!(
            fixture
                .db
                .provider_spend_row_count("claude:auth-http:http-stream-1"),
            1
        );
    }

    #[tokio::test]
    async fn malformed_tools_remain_fail_closed_before_reserving() {
        let fixture = Fixture::new();
        let response = handle_stub_message(
            &fixture.db,
            SIGNING_KEY,
            &fixture.headers("http-tools-1"),
            serde_json::json!({
                "model": MODEL,
                "max_tokens": 100,
                "messages": [{"role": "user", "content": "stub only"}],
                "stream": true,
                "tools": [{"name": "Bash"}]
            }),
            NOW,
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(fixture
            .db
            .get_provider_reservation("claude:auth-http:http-tools-1")
            .is_none());
    }

    #[tokio::test]
    async fn listener_rejects_missing_auth_and_idempotency_before_reserving() {
        let fixture = Fixture::new();
        let body = serde_json::json!({"model": MODEL, "max_tokens": 100, "messages": []});
        let unauthenticated = handle_stub_message(
            &fixture.db,
            SIGNING_KEY,
            &HeaderMap::new(),
            body.clone(),
            NOW,
        )
        .await;
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let mut no_key = fixture.headers("temporary");
        no_key.remove("x-cortex-request-key");
        let no_key = handle_stub_message(&fixture.db, SIGNING_KEY, &no_key, body, NOW).await;
        assert_eq!(no_key.status(), StatusCode::BAD_REQUEST);
        assert!(fixture
            .db
            .get_provider_reservation("claude:auth-http:http-request-1")
            .is_none());
    }

    #[tokio::test]
    async fn a_replayed_stream_keeps_every_block_and_the_real_stop_reason() {
        let response = message_as_sse(&serde_json::json!({
            "id": "msg_real",
            "model": MODEL,
            "stop_reason": "tool_use",
            "content": [
                {"type": "thinking", "thinking": "plan", "signature": "sig"},
                {"type": "text", "text": "running it"},
                {"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "ls"}}
            ],
            "usage": {"input_tokens": 9, "output_tokens": 3}
        }));
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains(r#""id":"msg_real""#));
        assert!(body.contains(r#""signature_delta""#));
        assert!(body.contains(r#""text":"running it""#));
        assert!(body.contains(r#""name":"Bash""#));
        assert!(body.contains(r#""partial_json":"{\"command\":\"ls\"}""#));
        assert!(body.contains(r#""index":2"#));
        assert!(body.contains(r#""stop_reason":"tool_use""#));
        assert_eq!(body.matches("event: content_block_stop\n").count(), 3);
    }

    #[test]
    fn zen_never_gets_a_cortex_funded_authorization() {
        // Zen is BYOK-only: Cortex never funds it, so `KNOWN_PROVIDERS` must
        // not know it, even if an operator still has a Zen supplier key
        // lying around in `CORTEX_ZEN_SUPPLIER_KEY`. This fails on current
        // main, where `zen` is in `KNOWN_PROVIDERS`.
        let fixture = Fixture::new();
        let created = create_authorization_and_capability(
            &fixture.db,
            std::str::from_utf8(SIGNING_KEY).unwrap(),
            "tenant-http",
            "run-http",
            "attempt-zen",
            "zen",
            "glm-5.2",
            1_000_000,
            1_000_000,
            NOW + 60_000,
            NOW,
        );
        assert!(created.is_none());
    }

    #[tokio::test]
    async fn a_zen_supplier_key_still_finds_no_live_transport() {
        // Simulate an operator who never cleaned up their env: a "zen" entry
        // still sits in the live `supplier_keys` map (as it would if
        // `CORTEX_ZEN_SUPPLIER_KEY` were still set — this map is what
        // `gateway_mode()` would have built from it). Even so, the request
        // must be refused, because `live_transport_for` no longer has a
        // "zen" arm.
        //
        // This is the discriminating case: on the old code, `SUPPLIER_KEY_ENV_VARS`
        // had a `("zen", "CORTEX_ZEN_SUPPLIER_KEY")` entry and
        // `live_transport_for("zen")` returned `Some(LiveZen(..))`, so with a
        // "zen" entry present in `supplier_keys`, `handle_live_message` would
        // find both a supplier key *and* a transport and go on to call
        // `handle_message` — it would not stop here with this 503. An empty
        // `supplier_keys` map would reach the same 503 on both old and new
        // code (a false pass on revert), which is why this map is non-empty.
        assert!(!SUPPLIER_KEY_ENV_VARS.iter().any(|(p, _)| *p == "zen"));
        assert!(live_transport_for("zen").is_none());

        let token = sign_capability(
            SIGNING_KEY,
            &GatewayCapability::new(
                "auth-zen",
                "tenant-http",
                "run-zen",
                "attempt-zen",
                "zen",
                "glm-5.2",
                NOW + 60_000,
            ),
        )
        .unwrap();
        let fixture = Fixture::new();
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {}", token.expose()).parse().unwrap(),
        );
        headers.insert("x-cortex-request-key", "http-zen-1".parse().unwrap());
        let supplier_keys: std::collections::HashMap<String, String> =
            std::collections::HashMap::from([("zen".to_string(), "k".repeat(32))]);
        let response = handle_live_message(
            &fixture.db,
            SIGNING_KEY,
            &supplier_keys,
            &headers,
            serde_json::json!({
                "model": "glm-5.2",
                "max_tokens": 100,
                "messages": [{"role": "user", "content": "hi"}]
            }),
            NOW,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            String::from_utf8(body.to_vec()).unwrap(),
            "gateway has no live transport for this provider"
        );
    }
}
