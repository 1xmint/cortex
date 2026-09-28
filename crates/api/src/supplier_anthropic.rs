//! The real wire to Anthropic, on Cortex's own key.
//!
//! One supplier per file. The gateway in `provider_gateway.rs` decides whether
//! a call may happen and what it is allowed to cost; this file only makes the
//! call and reports honestly what came back. Codex, OpenCode Zen and the rest
//! each get a file shaped like this one when they are added.
//!
//! Every call goes upstream with `stream: false`, whatever the caller asked
//! for. The full answer carries the one usage figure the gateway settles
//! against, so spend is known exactly before a single byte reaches the caller.
//! A caller that wanted a stream gets the finished message replayed as one;
//! see `message_as_sse` in `provider_gateway_http.rs`. The cost of that choice
//! is latency: the caller sees nothing until the model is done.

use std::future::Future;
use std::time::Duration;

use serde_json::Value;

use crate::pricing;
use crate::provider_gateway::{
    GatewayRequest, ObservedUsage, ProviderTransport, TransportFailure, TransportFailureKind,
    TransportResponse,
};

const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Long enough for a large non-streamed answer. A timeout leaves the spend
/// reserved rather than guessed at, so erring long costs nothing but waiting.
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Clone)]
pub(crate) struct AnthropicTransport {
    client: reqwest::Client,
    base_url: String,
}

impl AnthropicTransport {
    pub(crate) fn new() -> Self {
        Self::with_base_url(ANTHROPIC_BASE_URL)
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

impl ProviderTransport for AnthropicTransport {
    fn forward(
        &self,
        supplier_key: &str,
        request: &GatewayRequest,
    ) -> impl Future<Output = Result<TransportResponse, TransportFailure>> + Send {
        let client = self.client.clone();
        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let key = supplier_key.to_string();
        let mut body = request.body.clone();
        if let Some(fields) = body.as_object_mut() {
            fields.insert("stream".into(), Value::Bool(false));
        }

        async move {
            let response = client
                .post(&url)
                .header("x-api-key", &key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .json(&body)
                .send()
                .await
                .map_err(|error| TransportFailure {
                    kind: send_failure_kind(&error),
                    upstream_request_id: None,
                    message: format!("anthropic request failed: {}", without_url(&error)),
                })?;

            let status = response.status();
            let upstream_request_id = response
                .headers()
                .get("request-id")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let text = response.text().await.map_err(|error| TransportFailure {
                kind: if error.is_timeout() {
                    TransportFailureKind::Timeout
                } else {
                    TransportFailureKind::Unknown
                },
                upstream_request_id: upstream_request_id.clone(),
                message: format!("anthropic response was cut off: {}", without_url(&error)),
            })?;

            if !status.is_success() {
                return Err(TransportFailure {
                    kind: status_failure_kind(status.as_u16()),
                    upstream_request_id,
                    message: format!("anthropic returned {status}: {}", error_message(&text)),
                });
            }

            let body: Value = serde_json::from_str(&text).map_err(|_| TransportFailure {
                kind: TransportFailureKind::Unknown,
                upstream_request_id: upstream_request_id.clone(),
                message: "anthropic returned success with a body that is not JSON".into(),
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

/// Anthropic does not bill a request it refuses. Its 4xx answers and 529
/// (overloaded) are refusals. A 500 is its own fault and says nothing
/// certain about what ran, so it is treated as unknown rather than free.
fn status_failure_kind(status: u16) -> TransportFailureKind {
    if (400..500).contains(&status) || status == 529 {
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

/// Anthropic's usage, in the gateway's terms.
///
/// The parse itself lives in `pricing::parse_usage` — the one place a wire
/// `usage` object becomes token counts — so this only carries its fields
/// across into `ObservedUsage` unchanged: `input_tokens` stays Anthropic's
/// own exclusive `input_tokens` (no cache read or write folded into it),
/// `cached_input_tokens` is the cache-read count, and the two cache-write
/// counts are carried in their own fields rather than pre-multiplied into
/// an input-token-equivalent. The gateway's settlement
/// (`provider_gateway.rs`'s `forward`) applies the 1.25x/2x cache-write
/// multipliers itself, through `pricing::cost_micro_usd`, so the exact
/// integer cost is computed in exactly one place.
fn observed_usage(body: &Value) -> Option<ObservedUsage> {
    let usage = body.get("usage")?;
    let tokens = pricing::parse_usage(usage);
    Some(ObservedUsage {
        input_tokens: tokens.input_tokens,
        cached_input_tokens: tokens.cache_read_tokens,
        output_tokens: tokens.output_tokens,
        cache_write_5m_tokens: tokens.cache_write_5m_tokens,
        cache_write_1h_tokens: tokens.cache_write_1h_tokens,
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

    /// A stand-in Anthropic on a local port: answers with `status` and
    /// `reply`, and keeps what it was sent.
    async fn fake_anthropic(status: StatusCode, reply: Value) -> (String, Seen) {
        let seen: Seen = Arc::default();
        let log = seen.clone();
        let app = axum::Router::new().route(
            "/v1/messages",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let log = log.clone();
                let reply = reply.clone();
                async move {
                    log.lock().unwrap().push((headers, body));
                    (status, [("request-id", "req_fake_1")], Json(reply))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}"), seen)
    }

    fn request(stream: bool) -> GatewayRequest {
        GatewayRequest {
            request_key: "key-1".into(),
            tenant_id: "tenant".into(),
            run_id: "run".into(),
            attempt_id: "attempt".into(),
            provider: "claude".into(),
            model: "claude-sonnet-4-6".into(),
            max_output_tokens: 100,
            body: serde_json::json!({
                "model": "claude-sonnet-4-6",
                "max_tokens": 100,
                "stream": stream,
                "messages": [{"role": "user", "content": "hi"}]
            }),
            capability: SignedCapability::from_exposed("unused-by-transport"),
        }
    }

    #[tokio::test]
    async fn sends_the_key_as_a_header_and_never_asks_upstream_to_stream() {
        let (url, seen) = fake_anthropic(
            StatusCode::OK,
            serde_json::json!({
                "type": "message",
                "content": [{"type": "text", "text": "hello"}],
                "usage": {"input_tokens": 10, "output_tokens": 4}
            }),
        )
        .await;
        let response = AnthropicTransport::with_base_url(url)
            .forward("sk-test-supplier", &request(true))
            .await
            .unwrap();

        let seen = seen.lock().unwrap();
        let (headers, body) = &seen[0];
        assert_eq!(headers["x-api-key"], "sk-test-supplier");
        assert_eq!(headers["anthropic-version"], ANTHROPIC_VERSION);
        assert_eq!(body["stream"], false);
        assert_eq!(response.upstream_request_id.as_deref(), Some("req_fake_1"));
        assert_eq!(
            response.usage,
            Some(ObservedUsage {
                input_tokens: 10,
                cached_input_tokens: 0,
                output_tokens: 4,
                ..Default::default()
            })
        );
    }

    #[tokio::test]
    async fn a_refusal_is_rejected_so_the_reservation_is_released() {
        let (url, _) = fake_anthropic(
            StatusCode::TOO_MANY_REQUESTS,
            serde_json::json!({"type": "error", "error": {"type": "rate_limit_error", "message": "slow down"}}),
        )
        .await;
        let failure = AnthropicTransport::with_base_url(url)
            .forward("sk-test-supplier", &request(false))
            .await
            .unwrap_err();
        assert_eq!(failure.kind, TransportFailureKind::Rejected);
        assert!(failure.message.contains("slow down"));
        assert_eq!(failure.upstream_request_id.as_deref(), Some("req_fake_1"));
    }

    #[tokio::test]
    async fn a_server_error_stays_unknown_so_the_reservation_is_kept() {
        let (url, _) = fake_anthropic(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"type": "error", "error": {"type": "api_error", "message": "boom"}}),
        )
        .await;
        let failure = AnthropicTransport::with_base_url(url)
            .forward("sk-test-supplier", &request(false))
            .await
            .unwrap_err();
        assert_eq!(failure.kind, TransportFailureKind::Unknown);
    }

    #[tokio::test]
    async fn a_connection_that_never_opened_sent_nothing() {
        // Bind a port, then drop the listener so nothing is there.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let failure = AnthropicTransport::with_base_url(format!("http://{address}"))
            .forward("sk-test-supplier", &request(false))
            .await
            .unwrap_err();
        assert_eq!(failure.kind, TransportFailureKind::NotSent);
        assert!(!failure.message.contains("sk-test-supplier"));
    }

    #[test]
    fn cache_writes_are_carried_through_by_kind_and_never_multiplied_here() {
        // The multiplier lives in `pricing::cost_micro_usd`, settled by the
        // gateway; this function only carries Anthropic's own counts
        // through unchanged, split by kind.
        let split = observed_usage(&serde_json::json!({"usage": {
            "input_tokens": 10,
            "output_tokens": 5,
            "cache_read_input_tokens": 100,
            "cache_creation_input_tokens": 12,
            "cache_creation": {"ephemeral_5m_input_tokens": 8, "ephemeral_1h_input_tokens": 4}
        }}))
        .unwrap();
        assert_eq!(
            split,
            ObservedUsage {
                input_tokens: 10,
                cached_input_tokens: 100,
                output_tokens: 5,
                cache_write_5m_tokens: 8,
                cache_write_1h_tokens: 4,
            }
        );

        // No split reported: the whole write is billed as a 5-minute write
        // (the cheaper of the two multipliers) rather than guessing the
        // dearer one — M-D-0023, mirrored from `pricing::parse_usage`.
        let unsplit = observed_usage(&serde_json::json!({"usage": {
            "input_tokens": 10,
            "output_tokens": 5,
            "cache_creation_input_tokens": 12
        }}))
        .unwrap();
        assert_eq!(
            unsplit,
            ObservedUsage {
                input_tokens: 10,
                cached_input_tokens: 0,
                output_tokens: 5,
                cache_write_5m_tokens: 12,
                cache_write_1h_tokens: 0,
            }
        );
    }

    #[test]
    fn a_message_without_usage_reports_none() {
        assert_eq!(
            observed_usage(&serde_json::json!({"type": "message"})),
            None
        );
    }
}
