//! The real wire to OpenCode Zen, on Cortex's own key.
//!
//! One supplier per file. The gateway in `provider_gateway.rs` decides
//! whether a call may happen and what it is allowed to cost; this file only
//! makes the call and reports honestly what came back. Mirrors
//! `supplier_openai.rs`, since Zen's `/chat/completions` family speaks the
//! same OpenAI-compatible chat-completions shape.
//!
//! This slice covers only Zen's `/chat/completions` models: DeepSeek, GLM,
//! Kimi, MiniMax. Zen also serves Claude/GPT/Gemini/Grok aliases behind
//! `/v1/messages` and `/v1/responses`, in different wire shapes; those are
//! out of scope here and are refused the same as any other unlisted model.
//!
//! Every call goes upstream with `stream: false`, whatever the caller asked
//! for, the same reason `supplier_openai.rs` does: the full answer carries
//! the one usage figure the gateway settles against, so spend is known
//! exactly before a single byte reaches the caller.

use std::future::Future;
use std::time::Duration;

use serde_json::Value;

use crate::provider_gateway::{
    GatewayRequest, ObservedUsage, ProviderTransport, TransportFailure, TransportFailureKind,
    TransportResponse,
};

const ZEN_BASE_URL: &str = "https://opencode.ai/zen/v1";

/// Long enough for a large non-streamed answer. A timeout leaves the spend
/// reserved rather than guessed at, so erring long costs nothing but waiting.
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(600);

/// The only Zen `/chat/completions` models this supplier will call, and the
/// only ones priced in `pricing.rs`'s seeded list under the `"zen"`
/// provider. Sourced from https://opencode.ai/docs/zen's pricing table and
/// https://opencode.ai/zen/v1/models, read 2026-09-21.
///
/// Deliberately excludes every free model and every model Zen's own docs
/// mark as data-collecting: Big Pickle, `deepseek-v4-flash-free`, the MiMo
/// pair, Ling, the Nemotron pair, and Muse Spark 1.3 Contributor. A model
/// not on this list is refused before a single byte reaches Zen, whatever
/// the price list on the database says — this list is the ceiling, not the
/// database.
const ALLOWED_MODELS: &[&str] = &[
    "deepseek-v4.1-flash",
    "deepseek-v4-pro",
    "deepseek-v4-flash",
    "deepseek-v4-flash-vision-exp",
    "glm-5.3-flash",
    "glm-5.3",
    "glm-5.2",
    "glm-5.1",
    "glm-5",
    "minimax-m3",
    "minimax-m2.7",
    "minimax-m2.5",
    "kimi-k3",
    "kimi-k2.7-code",
    "kimi-k2.6",
    "kimi-k2.5",
];

#[derive(Clone)]
pub(crate) struct ZenTransport {
    client: reqwest::Client,
    base_url: String,
}

impl ZenTransport {
    pub(crate) fn new() -> Self {
        Self::with_base_url(ZEN_BASE_URL)
    }

    pub(crate) fn with_base_url(base_url: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(UPSTREAM_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            client,
            base_url: base_url.into(),
        }
    }
}

impl ProviderTransport for ZenTransport {
    fn forward(
        &self,
        supplier_key: &str,
        request: &GatewayRequest,
    ) -> impl Future<Output = Result<TransportResponse, TransportFailure>> + Send {
        let client = self.client.clone();
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let key = supplier_key.to_string();
        let allowed = ALLOWED_MODELS.contains(&request.model.as_str());
        let model = request.model.clone();
        let mut body = request.body.clone();
        if let Some(fields) = body.as_object_mut() {
            fields.insert("stream".into(), Value::Bool(false));
        }

        async move {
            if !allowed {
                // Never a byte on the wire for a model that is not on the
                // allowlist, free or otherwise: this releases the
                // reservation exactly as a connection that never opened
                // would.
                return Err(TransportFailure {
                    kind: TransportFailureKind::NotSent,
                    upstream_request_id: None,
                    message: format!("zen model {model:?} is not on the allowlist"),
                });
            }

            let response = client
                .post(&url)
                .header("Authorization", format!("Bearer {key}"))
                .json(&body)
                .send()
                .await
                .map_err(|error| TransportFailure {
                    kind: send_failure_kind(&error),
                    upstream_request_id: None,
                    message: format!("zen request failed: {}", without_url(&error)),
                })?;

            let status = response.status();
            let upstream_request_id = response
                .headers()
                .get("x-request-id")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let text = response.text().await.map_err(|error| TransportFailure {
                kind: if error.is_timeout() {
                    TransportFailureKind::Timeout
                } else {
                    TransportFailureKind::Unknown
                },
                upstream_request_id: upstream_request_id.clone(),
                message: format!("zen response was cut off: {}", without_url(&error)),
            })?;

            if !status.is_success() {
                return Err(TransportFailure {
                    kind: status_failure_kind(status.as_u16()),
                    upstream_request_id,
                    message: format!("zen returned {status}: {}", error_message(&text)),
                });
            }

            let body: Value = serde_json::from_str(&text).map_err(|_| TransportFailure {
                kind: TransportFailureKind::Unknown,
                upstream_request_id: upstream_request_id.clone(),
                message: "zen returned success with a body that is not JSON".into(),
            })?;
            let usage = observed_usage(&body);
            Ok(TransportResponse {
                body,
                upstream_request_id,
                usage,
            })
        }
    }
}

/// A connection that never opened carried no request. Anything later might
/// have, so it stays reserved until someone reconciles it.
fn send_failure_kind(error: &reqwest::Error) -> TransportFailureKind {
    if error.is_connect() || error.is_builder() {
        TransportFailureKind::NotSent
    } else if error.is_timeout() {
        TransportFailureKind::Timeout
    } else {
        TransportFailureKind::Unknown
    }
}

/// Zen's error codes are not documented. Treated the same way
/// `supplier_openai.rs` treats OpenAI's: a 4xx is a refusal it does not
/// bill, a 5xx is Zen's own fault and says nothing certain about what ran,
/// so it stays unknown rather than free.
fn status_failure_kind(status: u16) -> TransportFailureKind {
    if (400..500).contains(&status) {
        TransportFailureKind::Rejected
    } else {
        TransportFailureKind::Unknown
    }
}

/// reqwest errors print the URL, which is harmless here, but never the key:
/// the key travels in a header, not the URL. Stripped anyway so the message
/// stays about what went wrong.
fn without_url(error: &reqwest::Error) -> String {
    let error = error.to_string();
    match error.split_once(" for url ") {
        Some((head, _)) => head.to_string(),
        None => error,
    }
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

/// Zen's usage, in the gateway's terms. Its docs do not describe the
/// `/chat/completions` response shape, so this assumes the standard OpenAI
/// chat-completions `usage` object — `prompt_tokens`, `completion_tokens`,
/// and `prompt_tokens_details.cached_tokens` when present — and reports
/// `None` rather than guessing when usage is missing or malformed. A `None`
/// leaves the gateway's reservation unresolved at the full reserved amount;
/// it never invents a smaller, cheaper number.
fn observed_usage(body: &Value) -> Option<ObservedUsage> {
    let usage = body.get("usage")?;
    let input = usage.get("prompt_tokens")?.as_i64()?;
    let output = usage.get("completion_tokens")?.as_i64()?;
    let cached = usage
        .get("prompt_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);

    Some(ObservedUsage {
        input_tokens: input,
        cached_input_tokens: cached,
        output_tokens: output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_gateway::SignedCapability;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;
    use axum::Json;
    use std::sync::{Arc, Mutex};

    type Seen = Arc<Mutex<Vec<(HeaderMap, Value)>>>;

    /// A stand-in Zen on a local port: answers with `status` and `reply`,
    /// and keeps what it was sent.
    async fn fake_zen(status: StatusCode, reply: Value) -> (String, Seen) {
        let seen: Seen = Arc::default();
        let log = seen.clone();
        let app = axum::Router::new().route(
            "/chat/completions",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let log = log.clone();
                let reply = reply.clone();
                async move {
                    log.lock().unwrap().push((headers, body));
                    (status, [("x-request-id", "req_fake_1")], Json(reply))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}"), seen)
    }

    fn request(model: &str, stream: bool) -> GatewayRequest {
        GatewayRequest {
            request_key: "key-1".into(),
            tenant_id: "tenant".into(),
            run_id: "run".into(),
            attempt_id: "attempt".into(),
            provider: "zen".into(),
            model: model.into(),
            max_output_tokens: 100,
            body: serde_json::json!({
                "model": model,
                "max_tokens": 100,
                "stream": stream,
                "messages": [{"role": "user", "content": "hi"}]
            }),
            capability: SignedCapability::from_exposed("unused-by-transport"),
        }
    }

    #[tokio::test]
    async fn sends_the_key_as_a_bearer_header_to_chat_completions() {
        let (url, seen) = fake_zen(
            StatusCode::OK,
            serde_json::json!({
                "id": "chatcmpl-1",
                "choices": [{"message": {"role": "assistant", "content": "hello"}}],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 4,
                    "prompt_tokens_details": {"cached_tokens": 2}
                }
            }),
        )
        .await;
        let response = ZenTransport::with_base_url(url)
            .forward("zen-test-supplier", &request("deepseek-v4-flash", true))
            .await
            .unwrap();

        let seen = seen.lock().unwrap();
        let (headers, body) = &seen[0];
        assert_eq!(headers["authorization"], "Bearer zen-test-supplier");
        assert_eq!(body["stream"], false);
        assert_eq!(response.upstream_request_id.as_deref(), Some("req_fake_1"));
        assert_eq!(
            response.usage,
            Some(ObservedUsage {
                input_tokens: 10,
                cached_input_tokens: 2,
                output_tokens: 4,
            })
        );
    }

    #[tokio::test]
    async fn missing_usage_reports_none() {
        let (url, _) = fake_zen(
            StatusCode::OK,
            serde_json::json!({"id": "chatcmpl-1", "choices": []}),
        )
        .await;
        let response = ZenTransport::with_base_url(url)
            .forward("zen-test-supplier", &request("deepseek-v4-flash", false))
            .await
            .unwrap();
        assert_eq!(response.usage, None);
    }

    #[tokio::test]
    async fn a_refusal_is_rejected_so_the_reservation_is_released() {
        let (url, _) = fake_zen(
            StatusCode::TOO_MANY_REQUESTS,
            serde_json::json!({"error": {"type": "rate_limit_error", "message": "slow down"}}),
        )
        .await;
        let failure = ZenTransport::with_base_url(url)
            .forward("zen-test-supplier", &request("deepseek-v4-flash", false))
            .await
            .unwrap_err();
        assert_eq!(failure.kind, TransportFailureKind::Rejected);
        assert!(failure.message.contains("slow down"));
        assert_eq!(failure.upstream_request_id.as_deref(), Some("req_fake_1"));
    }

    #[tokio::test]
    async fn a_server_error_stays_unknown_so_the_reservation_is_kept() {
        let (url, _) = fake_zen(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": {"type": "api_error", "message": "boom"}}),
        )
        .await;
        let failure = ZenTransport::with_base_url(url)
            .forward("zen-test-supplier", &request("deepseek-v4-flash", false))
            .await
            .unwrap_err();
        assert_eq!(failure.kind, TransportFailureKind::Unknown);
    }

    #[tokio::test]
    async fn a_connection_that_never_opened_sent_nothing() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let failure = ZenTransport::with_base_url(format!("http://{address}"))
            .forward("zen-test-supplier", &request("deepseek-v4-flash", false))
            .await
            .unwrap_err();
        assert_eq!(failure.kind, TransportFailureKind::NotSent);
        assert!(!failure.message.contains("zen-test-supplier"));
    }

    #[tokio::test]
    async fn a_non_allowlisted_model_is_refused_before_a_byte_is_sent() {
        // No fake server is even started: if the transport sent anything,
        // this would hang and time out rather than return quickly.
        let failure = ZenTransport::with_base_url("http://127.0.0.1:1")
            .forward("zen-test-supplier", &request("big-pickle", false))
            .await
            .unwrap_err();
        assert_eq!(failure.kind, TransportFailureKind::NotSent);
        assert!(failure.message.contains("big-pickle"));
    }

    #[tokio::test]
    async fn a_free_model_is_refused_before_a_byte_is_sent() {
        let failure = ZenTransport::with_base_url("http://127.0.0.1:1")
            .forward(
                "zen-test-supplier",
                &request("deepseek-v4-flash-free", false),
            )
            .await
            .unwrap_err();
        assert_eq!(failure.kind, TransportFailureKind::NotSent);
    }

    #[test]
    fn price_math_for_deepseek_v4_flash() {
        let list = crate::pricing::seed_provisional(
            1,
            "test",
            1_700_000_000,
            crate::pricing::seed_models(),
        );
        let rate = list.model("zen", "deepseek-v4-flash").unwrap();
        // $0.14/1M input, $0.028/1M cached read, $0.28/1M output.
        // 1,000 input tokens (200 of them cached) + 500 output tokens:
        // 800 uncached * 140/1000 + 200 cached * 140/1000 * 2000/10000
        //   + 500 * 280/1000
        // = 112 + 5.6 (-> 5 by integer division) + 140 = 257 micros.
        assert_eq!(rate.cost_micros(1_000, 200, 500), 257);
    }
}
