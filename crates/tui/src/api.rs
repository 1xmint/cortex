//! Thin async client over the Cortex HTTP API.
//!
//! Mirrors the auth + request conventions used by the web frontend
//! (`cortex/src/lib/cortexApi.ts`): a bearer token in the `Authorization`
//! header, and an SSE body for `/api/chat` whose lines are `data: {json}`.

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::config::Config;

/// A streaming chat event mirroring the server's `StepEvent`
/// (`crates/api/src/state.rs`). Tagged by `type` in snake_case.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEvent {
    Started {
        #[serde(default)]
        provider: String,
        #[serde(default)]
        model: String,
    },
    Output {
        line: String,
    },
    Completed {
        #[serde(default)]
        #[allow(dead_code)] // present in the wire format; not surfaced in the UI
        exit_code: i32,
    },
    Failed {
        error: String,
    },
}

/// A single conversation summary from `/api/conversations`.
///
/// Used by the conversation-history sidebar (see [`ApiClient::conversations`]).
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // fields consumed via serde + history picker (incremental rollout)
pub struct Conversation {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
}

/// A run/task summary from `/api/runs`. The server returns loosely-typed JSON,
/// so we extract just the fields the status panel needs.
#[derive(Debug, Clone)]
pub struct Run {
    pub id: String,
    pub status: String,
    pub goal: String,
}

#[derive(Serialize)]
struct ChatBody<'a> {
    message: &'a str,
    file_paths: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    conversation_id: Option<&'a str>,
}

#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    config: Config,
}

impl ApiClient {
    pub fn new(config: Config) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("cortex-tui/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("building HTTP client")?;
        Ok(Self { http, config })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    fn auth_header(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.config.token {
            Some(token) => builder.header("authorization", format!("Bearer {token}")),
            // No token → rely on the server's local/dev fallback (user_id "local").
            None => builder,
        }
    }

    /// Health check against `/api/health`. Returns Ok(()) on a 2xx response.
    pub async fn health(&self) -> Result<()> {
        let req = self.http.get(self.config.url("/api/health"));
        let res = self
            .auth_header(req)
            .send()
            .await
            .context("connecting to Cortex API")?;
        if res.status().is_success() {
            Ok(())
        } else {
            Err(anyhow!("health check returned HTTP {}", res.status()))
        }
    }

    /// List recent conversations for the authenticated user.
    #[allow(dead_code)] // wired into the history picker (incremental rollout)
    pub async fn conversations(&self) -> Result<Vec<Conversation>> {
        let req = self
            .http
            .get(self.config.url("/api/conversations?limit=50"));
        let res = self.auth_header(req).send().await?;
        if !res.status().is_success() {
            return Err(anyhow!("conversations: HTTP {}", res.status()));
        }
        let value: serde_json::Value = res.json().await?;
        let list = value
            .get("conversations")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let conversations = list
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect();
        Ok(conversations)
    }

    /// List recent runs/tasks for the status panel.
    pub async fn runs(&self) -> Result<Vec<Run>> {
        let req = self.http.get(self.config.url("/api/runs?limit=20"));
        let res = self.auth_header(req).send().await?;
        if !res.status().is_success() {
            return Err(anyhow!("runs: HTTP {}", res.status()));
        }
        let arr: Vec<serde_json::Value> = res.json().await?;
        let runs = arr
            .into_iter()
            .map(|v| Run {
                id: string_field(&v, "id"),
                status: string_field(&v, "status"),
                goal: first_string_field(&v, &["goal", "title", "objective"]),
            })
            .collect();
        Ok(runs)
    }

    /// Stream a chat response. Posts to `/api/chat`, parses the SSE body, and
    /// forwards each decoded [`ChatEvent`] over `tx`. Returns when the stream
    /// closes or a terminal event is observed.
    pub async fn stream_chat(
        &self,
        message: String,
        file_paths: Vec<String>,
        conversation_id: Option<String>,
        tx: mpsc::UnboundedSender<ChatEvent>,
    ) -> Result<()> {
        let body = ChatBody {
            message: &message,
            file_paths: &file_paths,
            conversation_id: conversation_id.as_deref(),
        };

        let req = self
            .http
            .post(self.config.url("/api/chat"))
            .header("content-type", "application/json")
            .json(&body);

        let res = self
            .auth_header(req)
            .send()
            .await
            .context("sending chat request")?;

        if !res.status().is_success() {
            let status = res.status();
            let detail = res.text().await.unwrap_or_default();
            let msg = extract_error(&detail).unwrap_or(detail);
            return Err(anyhow!("chat failed (HTTP {status}): {msg}"));
        }

        let mut stream = res.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("reading chat stream")?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // SSE frames are newline-delimited; keep the trailing partial line.
            while let Some(idx) = buffer.find('\n') {
                let line: String = buffer.drain(..=idx).collect();
                let line = line.trim_end();
                let Some(json) = line.strip_prefix("data:") else {
                    continue;
                };
                let json = json.trim();
                if json.is_empty() {
                    continue;
                }
                if let Ok(event) = serde_json::from_str::<ChatEvent>(json) {
                    let terminal = matches!(
                        event,
                        ChatEvent::Completed { .. } | ChatEvent::Failed { .. }
                    );
                    // Receiver gone → stop streaming.
                    if tx.send(event).is_err() {
                        return Ok(());
                    }
                    if terminal {
                        return Ok(());
                    }
                }
            }
        }

        Ok(())
    }
}

fn string_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

fn first_string_field(v: &serde_json::Value, keys: &[&str]) -> String {
    for key in keys {
        if let Some(s) = v.get(*key).and_then(|x| x.as_str()) {
            if !s.is_empty() {
                return s.to_string();
            }
        }
    }
    String::new()
}

/// Pull an `error` field out of a JSON error body, if present.
fn extract_error(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> ChatEvent {
        serde_json::from_str(json).expect("valid ChatEvent")
    }

    #[test]
    fn deserializes_server_step_events() {
        // These payloads mirror the server's StepEvent serialization exactly.
        match parse(r#"{"type":"started","step_id":"chat","provider":"cortex","model":"x"}"#) {
            ChatEvent::Started { provider, model } => {
                assert_eq!(provider, "cortex");
                assert_eq!(model, "x");
            }
            other => panic!("expected Started, got {other:?}"),
        }

        match parse(r#"{"type":"output","step_id":"chat","line":"hello"}"#) {
            ChatEvent::Output { line } => assert_eq!(line, "hello"),
            other => panic!("expected Output, got {other:?}"),
        }

        match parse(r#"{"type":"completed","step_id":"chat","exit_code":0}"#) {
            ChatEvent::Completed { exit_code } => assert_eq!(exit_code, 0),
            other => panic!("expected Completed, got {other:?}"),
        }

        match parse(r#"{"type":"failed","step_id":"chat","error":"boom"}"#) {
            ChatEvent::Failed { error } => assert_eq!(error, "boom"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn extracts_error_field() {
        assert_eq!(
            extract_error(r#"{"error":"nope"}"#),
            Some("nope".to_string())
        );
        assert_eq!(extract_error("not json"), None);
    }
}
