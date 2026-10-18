//! The API engine: Cortex runs the model's tool loop itself over the
//! Messages API. File tools stay inside the worktree, and every command
//! goes through the sandbox, so nothing the model writes touches the host.
//!
//! The transport is swappable: `HttpTransport` talks to the real API and
//! bills the key in `ANTHROPIC_API_KEY`; `ReplayTransport` plays recorded
//! replies so tests never spend money.

use super::{CancelToken, Engine, SessionInput, SessionOutcome};
use crate::sandbox::{self, RunSpec};
use crate::{Error, Result};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

/// A session that has not finished after this many replies is stopped.
pub const MAX_TURNS: usize = 50;
pub const MODEL: &str = "claude-sonnet-5-5";
const MAX_TOKENS: u32 = 8192;
const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";
/// Wait used when a rate-limit reply does not say how long.
const RATE_LIMIT_WAIT: Duration = Duration::from_secs(60);
const DEFAULT_RUN_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_RUN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Bytes of a file `read_file` returns.
const READ_CAP: usize = 100_000;

const SYSTEM: &str = "You are working on a project in a folder. Use the tools to make \
the change the brief asks for. Paths are relative to the project folder. Commands run \
in an isolated sandbox with no network. When you are finished, reply with a short \
summary of what you changed.";

/// Sends one Messages API request and returns the reply body. An API error
/// comes back as `Ok` with `"type": "error"`; `Err` means no reply at all.
pub trait Transport {
    fn send(&mut self, body: &Value) -> Result<Value>;
    /// Removes secrets from text before it reaches a transcript or note.
    fn redact(&self, text: &str) -> String {
        text.to_string()
    }
}

/// The real API. The key is sent in a header and nowhere else.
pub struct HttpTransport {
    url: String,
    key: String,
    agent: ureq::Agent,
}

impl std::fmt::Debug for HttpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpTransport")
            .field("url", &self.url)
            .field("key", &"[redacted]")
            .finish()
    }
}

impl HttpTransport {
    pub fn from_env() -> Result<Self> {
        let key = std::env::var("ANTHROPIC_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| Error::Invalid("ANTHROPIC_API_KEY is not set".into()))?;
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(10 * 60)))
            .build()
            .into();
        Ok(Self {
            url: API_URL.into(),
            key,
            agent,
        })
    }

    /// Points the transport somewhere else, such as a local test server.
    pub fn at(mut self, url: &str) -> Self {
        self.url = url.into();
        self
    }
}

impl Transport for HttpTransport {
    fn send(&mut self, body: &Value) -> Result<Value> {
        let mut resp = self
            .agent
            .post(&self.url)
            .header("x-api-key", &self.key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .send(body.to_string())
            .map_err(|e| Error::Invalid(format!("api request: {e}")))?;
        let status = resp.status().as_u16();
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok());
        let text = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| Error::Invalid(format!("api reply: {e}")))?;
        let mut reply: Value = serde_json::from_str(&text).unwrap_or_else(|_| {
            json!({"type": "error", "error": {
                "type": if status == 429 { "rate_limit_error" } else { "http_error" },
                "message": format!("HTTP {status}: {text}")}})
        });
        if let Some(secs) = retry_after {
            reply["retry_after_s"] = json!(secs);
        }
        Ok(reply)
    }

    fn redact(&self, text: &str) -> String {
        text.replace(&self.key, "[redacted]")
    }
}

/// Plays recorded replies in order and keeps every request it was sent.
/// Requests are not compared against a recording: the tests check them.
#[derive(Debug, Default)]
pub struct ReplayTransport {
    responses: VecDeque<Value>,
    pub requests: Vec<Value>,
}

impl ReplayTransport {
    pub fn new(responses: Vec<Value>) -> Self {
        Self {
            responses: responses.into(),
            requests: Vec::new(),
        }
    }

    /// Reads `{"responses": [...]}`.
    pub fn from_json(text: &str) -> Result<Self> {
        let v: Value =
            serde_json::from_str(text).map_err(|e| Error::Invalid(format!("replay: {e}")))?;
        let responses = v["responses"]
            .as_array()
            .ok_or_else(|| Error::Invalid("replay: no responses list".into()))?;
        Ok(Self::new(responses.clone()))
    }
}

impl Transport for ReplayTransport {
    fn send(&mut self, body: &Value) -> Result<Value> {
        self.requests.push(body.clone());
        self.responses
            .pop_front()
            .ok_or_else(|| Error::Invalid("replay has no more responses".into()))
    }
}

pub struct ApiEngine<T: Transport> {
    pub transport: T,
}

impl<T: Transport> ApiEngine<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }
}

impl<T: Transport> Engine for ApiEngine<T> {
    fn name(&self) -> &str {
        "api"
    }

    fn version(&self) -> Result<String> {
        Ok(MODEL.into())
    }

    fn run(
        &mut self,
        input: &SessionInput,
        on_line: &mut dyn FnMut(&str),
        cancel: &CancelToken,
    ) -> Result<SessionOutcome> {
        let deadline = Instant::now() + input.timeout;
        let mut messages = vec![json!({"role": "user", "content": input.brief})];
        for _ in 0..MAX_TURNS {
            if cancel.is_cancelled() {
                return Ok(SessionOutcome::Failed {
                    reason: "cancelled".into(),
                });
            }
            if Instant::now() >= deadline {
                return Ok(SessionOutcome::TimedOut);
            }
            let body = json!({
                "model": MODEL,
                "max_tokens": MAX_TOKENS,
                "system": SYSTEM,
                "tools": tools(),
                "messages": messages,
            });
            let reply = match self.transport.send(&body) {
                Ok(reply) => reply,
                Err(e) => {
                    return Ok(SessionOutcome::Failed {
                        reason: self.transport.redact(&e.to_string()),
                    });
                }
            };
            on_line(&self.transport.redact(&reply.to_string()));
            if reply["type"] == "error" {
                return Ok(error_outcome(&reply, |t| self.transport.redact(t)));
            }
            let content = reply["content"].as_array().cloned().unwrap_or_default();
            messages.push(json!({"role": "assistant", "content": content}));
            if reply["stop_reason"] != "tool_use" {
                let note: Vec<&str> = content
                    .iter()
                    .filter(|c| c["type"] == "text")
                    .filter_map(|c| c["text"].as_str())
                    .collect();
                return Ok(SessionOutcome::Ended {
                    note: note.join("\n"),
                });
            }
            let results: Vec<Value> = content
                .iter()
                .filter(|c| c["type"] == "tool_use")
                .map(|call| {
                    let left = deadline.saturating_duration_since(Instant::now());
                    let (text, is_error) =
                        match call_tool(&input.worktree, &call["name"], &call["input"], left) {
                            Ok(text) => (text, false),
                            Err(text) => (text, true),
                        };
                    json!({"type": "tool_result", "tool_use_id": call["id"],
                           "content": text, "is_error": is_error})
                })
                .collect();
            messages.push(json!({"role": "user", "content": results}));
        }
        Ok(SessionOutcome::Failed {
            reason: format!("stopped after {MAX_TURNS} turns without finishing"),
        })
    }
}

fn error_outcome(reply: &Value, redact: impl Fn(&str) -> String) -> SessionOutcome {
    let kind = reply["error"]["type"].as_str().unwrap_or("unknown");
    if kind == "rate_limit_error" {
        let retry_after = reply["retry_after_s"]
            .as_u64()
            .map(Duration::from_secs)
            .unwrap_or(RATE_LIMIT_WAIT);
        return SessionOutcome::RateLimited { retry_after };
    }
    let message = reply["error"]["message"].as_str().unwrap_or("");
    SessionOutcome::Failed {
        reason: redact(&format!("api error {kind}: {message}")),
    }
}

fn tools() -> Value {
    let path = json!({"type": "string", "description": "Path relative to the project folder"});
    json!([
        {"name": "read_file", "description": "Read a text file.",
         "input_schema": {"type": "object", "properties": {"path": path}, "required": ["path"]}},
        {"name": "write_file", "description": "Create or replace a file, making folders as needed.",
         "input_schema": {"type": "object", "properties": {
             "path": path, "content": {"type": "string"}}, "required": ["path", "content"]}},
        {"name": "list_files", "description": "List one folder; folders end in /.",
         "input_schema": {"type": "object", "properties": {"path": path}, "required": ["path"]}},
        {"name": "run", "description": "Run a shell command in the project folder, inside an isolated sandbox with no network. Returns the exit code and output.",
         "input_schema": {"type": "object", "properties": {
             "command": {"type": "string"},
             "timeout_s": {"type": "integer", "description": "Seconds before the command is stopped (default 120, at most 900)"}},
          "required": ["command"]}}
    ])
}

/// Runs one tool. `Err` carries the message the model sees as an error.
fn call_tool(
    worktree: &Path,
    name: &Value,
    args: &Value,
    left: Duration,
) -> std::result::Result<String, String> {
    let text = |key: &str| {
        args[key]
            .as_str()
            .ok_or_else(|| format!("missing text argument `{key}`"))
    };
    match name.as_str().unwrap_or("") {
        "read_file" => {
            let path = inside(worktree, text("path")?, false)?;
            let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
            let mut s = String::from_utf8_lossy(&bytes[..bytes.len().min(READ_CAP)]).into_owned();
            if bytes.len() > READ_CAP {
                s.push_str("\n(file cut short)");
            }
            Ok(s)
        }
        "write_file" => {
            let path = inside(worktree, text("path")?, true)?;
            std::fs::write(&path, text("content")?).map_err(|e| e.to_string())?;
            Ok("written".into())
        }
        "list_files" => {
            let path = inside(worktree, text("path")?, false)?;
            let mut names: Vec<String> = std::fs::read_dir(&path)
                .map_err(|e| e.to_string())?
                .filter_map(|e| e.ok())
                .map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    if dir { name + "/" } else { name }
                })
                .filter(|n| n != ".git/")
                .collect();
            names.sort();
            Ok(names.join("\n"))
        }
        "run" => {
            let asked = args["timeout_s"]
                .as_u64()
                .map(Duration::from_secs)
                .unwrap_or(DEFAULT_RUN_TIMEOUT);
            let timeout = asked
                .clamp(Duration::from_secs(1), MAX_RUN_TIMEOUT)
                .min(left.max(Duration::from_secs(1)));
            let out = sandbox::run(&RunSpec {
                cmd: text("command")?.to_string(),
                workdir: worktree.to_path_buf(),
                checks_dir: None,
                timeout,
            })
            .map_err(|e| e.to_string())?;
            Ok(crate::mcp::describe(&out))
        }
        other => Err(format!("unknown tool `{other}`")),
    }
}

/// Resolves a model-given path inside the worktree, refusing anything that
/// could reach outside it: absolute paths, `..`, and symlinks that point
/// out. For a write, missing folders are made first.
fn inside(worktree: &Path, given: &str, write: bool) -> std::result::Result<PathBuf, String> {
    const OUTSIDE: &str = "path must stay inside the project folder";
    let rel = Path::new(given);
    if rel
        .components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(OUTSIDE.into());
    }
    let root = worktree.canonicalize().map_err(|e| e.to_string())?;
    let full = root.join(rel);
    let check = |p: &Path| -> std::result::Result<PathBuf, String> {
        let real = p.canonicalize().map_err(|e| e.to_string())?;
        if real.starts_with(&root) {
            Ok(real)
        } else {
            Err(OUTSIDE.into())
        }
    };
    if !write {
        return check(&full);
    }
    let file = full.file_name().ok_or_else(|| OUTSIDE.to_string())?;
    let parent = full.parent().ok_or_else(|| OUTSIDE.to_string())?;
    // The deepest folder that already exists must be inside before any
    // folder is made under it.
    let mut existing = parent;
    while !existing.exists() {
        existing = existing.parent().ok_or_else(|| OUTSIDE.to_string())?;
    }
    check(existing)?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let target = check(parent)?.join(file);
    if target
        .symlink_metadata()
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(OUTSIDE.into());
    }
    Ok(target)
}
