//! The real wire to OpenAI, on Cortex's own key.
//!
//! One supplier per file. The gateway in `provider_gateway.rs` decides whether
//! a call may happen and what it is allowed to cost; this file only makes the
//! call and reports honestly what came back. Mirrors `supplier_anthropic.rs`.
//!
//! Every call goes upstream with `stream: false`, whatever the caller asked
//! for. The full answer carries the one usage figure the gateway settles
//! against, so spend is known exactly before a single byte reaches the
//! caller. A caller that wanted a stream gets the finished message replayed
//! as one; see `message_as_sse` in `provider_gateway_http.rs`.

use std::future::Future;
use std::time::Duration;

use serde_json::Value;

use crate::provider_gateway::{
    GatewayRequest, ObservedUsage, ProviderTransport, TransportFailure, TransportFailureKind,
    TransportResponse,
};

const OPENAI_BASE_URL: &str = "https://api.openai.com";

/// Long enough for a large non-streamed answer. A timeout leaves the spend
/// reserved rather than guessed at, so erring long costs nothing but waiting.
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Clone)]
pub(crate) struct OpenAiTransport {
    client: reqwest::Client,
    base_url: String,
}

impl OpenAiTransport {
    pub(crate) fn new() -> Self {
        Self::with_base_url(OPENAI_BASE_URL)
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

impl ProviderTransport for OpenAiTransport {
    fn forward(
        &self,
        supplier_key: &str,
        request: &GatewayRequest,
    ) -> impl Future<Output = Result<TransportResponse, TransportFailure>> + Send {
        let client = self.client.clone();
        let url = format!("{}/v1/responses", self.base_url.trim_end_matches('/'));
        let key = supplier_key.to_string();
        let mut body = request.body.clone();
        if let Some(fields) = body.as_object_mut() {
            fields.insert("stream".into(), Value::Bool(false));
        }

        async move {
            let response = client
                .post(&url)
                .header("Authorization", format!("Bearer {key}"))
                .json(&body)
                .send()
                .await
                .map_err(|error| TransportFailure {
                    kind: send_failure_kind(&error),
                    upstream_request_id: None,
                    message: format!("openai request failed: {}", without_url(&error)),
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
                message: format!("openai response was cut off: {}", without_url(&error)),
            })?;

            if !status.is_success() {
                return Err(TransportFailure {
                    kind: status_failure_kind(status.as_u16()),
                    upstream_request_id,
                    message: format!("openai returned {status}: {}", error_message(&text)),
                });
            }

            let body: Value = serde_json::from_str(&text).map_err(|_| TransportFailure {
                kind: TransportFailureKind::Unknown,
                upstream_request_id: upstream_request_id.clone(),
                message: "openai returned success with a body that is not JSON".into(),
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

/// OpenAI does not bill a request it refuses. Its 4xx answers (including 429)
/// are refusals. A 5xx is its own fault and says nothing certain about what
/// ran, so it is treated as unknown rather than free.
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

/// OpenAI's usage, in the gateway's terms. The Responses API reports total
/// input tokens plus, separately, how many of those were served from cache;
/// unlike Anthropic there is no separate cache-write charge to account for.
/// When `cached_tokens` is absent, none of the input is counted as cached
/// rather than guessed at.
fn observed_usage(body: &Value) -> Option<ObservedUsage> {
    let usage = body.get("usage")?;
    let input = usage.get("input_tokens")?.as_i64()?;
    let output = usage.get("output_tokens")?.as_i64()?;
    let cached = usage
        .get("input_tokens_details")
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

    /// A stand-in OpenAI on a local port: answers with `status` and `reply`,
    /// and keeps what it was sent.
    async fn fake_openai(status: StatusCode, reply: Value) -> (String, Seen) {
        let seen: Seen = Arc::default();
        let log = seen.clone();
        let app = axum::Router::new().route(
            "/v1/responses",
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

    fn request(stream: bool) -> GatewayRequest {
        GatewayRequest {
            request_key: "key-1".into(),
            tenant_id: "tenant".into(),
            run_id: "run".into(),
            attempt_id: "attempt".into(),
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            max_output_tokens: 100,
            body: serde_json::json!({
                "model": "gpt-5.5",
                "max_tokens": 100,
                "stream": stream,
                "messages": [{"role": "user", "content": "hi"}]
            }),
            capability: SignedCapability::from_exposed("unused-by-transport"),
        }
    }

    #[tokio::test]
    async fn sends_the_key_as_a_bearer_header_and_never_asks_upstream_to_stream() {
        let (url, seen) = fake_openai(
            StatusCode::OK,
            serde_json::json!({
                "type": "response",
                "output": [{"type": "message", "content": [{"type": "output_text", "text": "hello"}]}],
                "usage": {"input_tokens": 10, "output_tokens": 4, "input_tokens_details": {"cached_tokens": 2}}
            }),
        )
        .await;
        let response = OpenAiTransport::with_base_url(url)
            .forward("sk-test-supplier", &request(true))
            .await
            .unwrap();

        let seen = seen.lock().unwrap();
        let (headers, body) = &seen[0];
        assert_eq!(headers["authorization"], "Bearer sk-test-supplier");
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
    async fn a_refusal_is_rejected_so_the_reservation_is_released() {
        let (url, _) = fake_openai(
            StatusCode::TOO_MANY_REQUESTS,
            serde_json::json!({"error": {"type": "rate_limit_error", "message": "slow down"}}),
        )
        .await;
        let failure = OpenAiTransport::with_base_url(url)
            .forward("sk-test-supplier", &request(false))
            .await
            .unwrap_err();
        assert_eq!(failure.kind, TransportFailureKind::Rejected);
        assert!(failure.message.contains("slow down"));
        assert_eq!(failure.upstream_request_id.as_deref(), Some("req_fake_1"));
    }

    #[tokio::test]
    async fn a_server_error_stays_unknown_so_the_reservation_is_kept() {
        let (url, _) = fake_openai(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": {"type": "api_error", "message": "boom"}}),
        )
        .await;
        let failure = OpenAiTransport::with_base_url(url)
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
        let failure = OpenAiTransport::with_base_url(format!("http://{address}"))
            .forward("sk-test-supplier", &request(false))
            .await
            .unwrap_err();
        assert_eq!(failure.kind, TransportFailureKind::NotSent);
        assert!(!failure.message.contains("sk-test-supplier"));
    }

    #[test]
    fn a_message_without_usage_reports_none() {
        assert_eq!(
            observed_usage(&serde_json::json!({"type": "response"})),
            None
        );
    }
}
