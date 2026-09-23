use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, KeepAliveStream, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures_core::Stream;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use cortex_engine::classifier::classify_intent;

use crate::clerk::ClerkUser;
use crate::routes::ErrorResponse;
use crate::state::{AppState, StepEvent};

/// Both the Claude-tier path (below) and the Zen BYOK path
/// (`chat_zen::chat`) build their SSE stream from `step_event_to_sse`, but
/// each `async fn` gets its own anonymous `impl Stream` type -- boxing here
/// is what lets `chat()` return either one from a single function signature.
pub(crate) type BoxedSseStream =
    KeepAliveStream<Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>>;

pub(crate) fn step_event_to_sse(event: StepEvent) -> Result<Event, Infallible> {
    let data = serde_json::to_string(&event).unwrap_or_default();
    Ok(Event::default().data(data))
}

/// Serializes a `StepEvent` exactly the way `step_event_to_sse` does for the
/// `data:` line it sends the browser -- the same `serde_json::to_string`
/// call, factored out so a wire-contract test can call it directly instead
/// of reaching into the opaque `axum::response::sse::Event` it wraps.
#[cfg(test)]
pub(crate) fn step_event_wire_json(event: &StepEvent) -> String {
    serde_json::to_string(event).unwrap_or_default()
}

#[derive(Deserialize)]
pub struct ChatRequest {
    pub message: String,
    #[serde(default)]
    pub file_paths: Vec<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub routing_preferences: Option<RoutingPreferences>,
    /// `"zen:<model>"` to route this turn to an OpenCode Zen model on the
    /// customer's own key (see `chat_zen.rs`). Absent means exactly today's
    /// Claude-tier behaviour -- this field changes nothing when it is `None`.
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct RoutingPreferences {
    #[serde(default = "default_balanced")]
    pub speed: String,
    #[serde(default = "default_balanced")]
    pub intelligence: String,
    #[serde(default = "default_guided")]
    pub autonomy: String,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub budget_limit: Option<f64>,
    /// Model tier: "fast" (haiku/mini), "balanced" (sonnet/gpt-4.1), "powerful" (opus/gpt-5.5)
    #[serde(default)]
    pub model_tier: Option<String>,
}

fn default_balanced() -> String {
    "balanced".into()
}
fn default_guided() -> String {
    "guided".into()
}

const MAX_MESSAGE_LEN: usize = 32_768;
const MAX_FILE_PATHS: usize = 50;

fn system_prompt_for_intent(intent: Option<cortex_core::routing::Intent>) -> &'static str {
    use cortex_core::routing::Intent;
    match intent {
        Some(Intent::Fix) => "You are Cortex, an AI coding assistant. The user needs help debugging or fixing an issue. Analyze the problem, identify the root cause, and provide a clear fix with code.",
        Some(Intent::Add) => "You are Cortex, an AI coding assistant. The user wants to build something new. Help them design and implement the feature with clean, working code.",
        Some(Intent::Explore) => "You are Cortex, an AI coding assistant. The user wants to understand their codebase. Explain clearly how things work, reference specific files and patterns.",
        Some(Intent::Think) => "You are Cortex, an AI coding assistant helping with architecture decisions. Analyze tradeoffs, consider alternatives, and recommend a clear approach with rationale.",
        Some(Intent::Review) => "You are Cortex, an AI coding assistant. Review the code or changes the user describes. Focus on correctness, security, performance, and maintainability.",
        Some(Intent::Test) => "You are Cortex, an AI coding assistant. Help the user write or fix tests. Focus on meaningful coverage, edge cases, and clear test structure.",
        Some(Intent::Refactor) => "You are Cortex, an AI coding assistant. Help the user refactor code for clarity, performance, or maintainability while preserving behavior.",
        Some(Intent::Ship) => "You are Cortex, an AI coding assistant. Help the user prepare code for deployment — final checks, build verification, release notes, and shipping confidence.",
        None => "You are Cortex, an AI coding assistant made by HeyVera. Help the user with whatever they need — coding, debugging, planning, or answering questions. Be direct and practical.",
    }
}

/// Determine the best available provider path for a user.
/// Priority: Workspace (isolated) > Cortex-paid gateway > None
enum ProviderPath {
    /// Workspace: route to user's isolated Replit workspace
    Workspace { workspace_id: String },
    /// Cortex-paid: Cortex calls Anthropic on its own key through the
    /// private provider gateway and charges the user's credits at observed
    /// cost. See `chat_paid.rs`.
    Cortex { model: String },
    /// No provider available
    None,
}

async fn resolve_provider(
    _state: &AppState,
    user_id: &str,
    model_tier: Option<&str>,
) -> ProviderPath {
    // 0. Workspace request (workspace:{workspace_id})
    if let Some(workspace_id) = user_id.strip_prefix("workspace:") {
        return ProviderPath::Workspace {
            workspace_id: workspace_id.to_string(),
        };
    }

    // 1. Cortex-paid: the only remaining chat path. Off unless the gateway
    // is actually usable (see `provider_gateway_http::gateway_usable`),
    // which keeps this a no-op everywhere the gateway isn't configured.
    if crate::provider_gateway_http::gateway_usable().is_some() {
        return ProviderPath::Cortex {
            model: crate::chat_paid::model_for_tier(model_tier).to_string(),
        };
    }

    ProviderPath::None
}

/// The header carrying a BYOK unlock secret, `<device_id>.<base64url secret>`.
/// Never logged: see `crate::lib`'s router construction comment.
pub(crate) const KEY_UNLOCK_HEADER: &str = "x-cortex-key-unlock";
/// The header carrying only a device id, for `GET /api/chat/models` to
/// report that device's key status. Never carries a secret.
pub(crate) const KEY_DEVICE_HEADER: &str = "x-cortex-key-device";

pub async fn chat(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    headers: HeaderMap,
    Json(req): Json<ChatRequest>,
) -> Result<Sse<BoxedSseStream>, Response> {
    if req.message.len() > MAX_MESSAGE_LEN {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(ErrorResponse {
                error: format!("message exceeds {MAX_MESSAGE_LEN} bytes"),
            }),
        )
            .into_response());
    }
    if req.file_paths.len() > MAX_FILE_PATHS {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("too many file paths (max {MAX_FILE_PATHS})"),
            }),
        )
            .into_response());
    }
    let _file_paths = crate::validate::sanitize_file_paths(&req.file_paths)
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e })).into_response())?;

    if let Some(blocked) = crate::billing::check_chat_access(&state, &user.user_id) {
        return Err((StatusCode::PAYMENT_REQUIRED, Json(ErrorResponse {
            error: format!("Subscription required to access chat. Status: {blocked:?}. Go to Settings → Billing to subscribe."),
        })).into_response());
    }

    let intent = classify_intent(&req.message);

    // Zen BYOK: a completely separate path from everything below (D3 in the
    // BYOK plan) -- it never falls through to `ProviderPath::Cortex`, so
    // Cortex never spends its own money when a customer has no Zen key.
    if let Some(zen_model) = req
        .model
        .as_deref()
        .and_then(|m| m.strip_prefix("zen:"))
        .map(str::to_string)
    {
        let system_prompt = system_prompt_for_intent(intent).to_string();
        let unlock_header = headers
            .get(KEY_UNLOCK_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        return crate::chat_zen::chat(
            State(state),
            user,
            req,
            zen_model,
            system_prompt,
            unlock_header,
        )
        .await;
    }

    // Billing hole guard: `/api/chat/models` is the only source of truth for
    // what a client may echo back in `model`. A Zen id always carries the
    // "zen:" prefix (handled above) and a Claude entry's `model` is always
    // one of `claude_tier_model_values()` (see `chat_models` below). Any
    // other non-empty value is unknown to this server -- accepting it here
    // would fall through to the Claude-tier path below and bill Cortex
    // credits for a model nobody offered at that price, or none at all.
    if let Some(model) = req.model.as_deref() {
        if !claude_tier_model_values().contains(&model) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "unknown_model".into(),
                }),
            )
                .into_response());
        }
    }

    let (tx, rx) = mpsc::channel::<StepEvent>(64);

    let model_tier = req
        .routing_preferences
        .as_ref()
        .and_then(|p| p.model_tier.as_deref());
    let provider_path = resolve_provider(&state, &user.user_id, model_tier).await;

    match provider_path {
        ProviderPath::Workspace { workspace_id } => {
            let system_prompt = system_prompt_for_intent(intent).to_string();
            let user_message = req.message.clone();
            let state_clone = state.clone();
            let _user_id = user.user_id.clone();
            let conv_id = req.conversation_id.clone();

            state.vera_tracker.record_conversation(&user.user_id);
            if let Some(i) = intent {
                state.vera_tracker.record_routed(&user.user_id, i);
            }

            if let (Some(db), Some(cid)) = (&state.db, &conv_id) {
                db.add_message(cid, "user", &user_message, None, None);
            }

            tokio::spawn(async move {
                let _ = tx
                    .send(StepEvent::Started {
                        step_id: "workspace-chat".into(),
                        provider: "replit".into(),
                        model: workspace_id.clone(),
                    })
                    .await;

                // Route to workspace instead of CLI
                match route_to_workspace(&state_clone, &workspace_id, &system_prompt, &user_message)
                    .await
                {
                    Ok(response) => {
                        let _ = tx
                            .send(StepEvent::Output {
                                step_id: "workspace-chat".into(),
                                line: response,
                            })
                            .await;

                        let _ = tx
                            .send(StepEvent::Completed {
                                step_id: "workspace-chat".into(),
                                exit_code: 0,
                            })
                            .await;
                    }
                    Err(error) => {
                        let _ = tx
                            .send(StepEvent::Failed {
                                step_id: "workspace-chat".into(),
                                error,
                            })
                            .await;
                    }
                }

                if let (Some(db), Some(cid)) = (&state_clone.db, &conv_id) {
                    // For now, just acknowledge the workspace routing
                    let response = format!("Routed to workspace: {}", workspace_id);
                    db.add_message(cid, "assistant", &response, Some("replit"), None);
                }
            });
        }

        ProviderPath::Cortex { model } => {
            let system_prompt = system_prompt_for_intent(intent).to_string();
            let user_message = req.message.clone();
            let state_clone = state.clone();
            let user_id = user.user_id.clone();
            let conv_id = req.conversation_id.clone();

            state.vera_tracker.record_conversation(&user.user_id);
            if let Some(i) = intent {
                state.vera_tracker.record_routed(&user.user_id, i);
            }

            if let (Some(db), Some(cid)) = (&state.db, &conv_id) {
                db.add_message(cid, "user", &user_message, None, None);
            }

            tokio::spawn(crate::chat_paid::run(
                state_clone,
                user_id,
                conv_id,
                model,
                system_prompt,
                user_message,
                tx,
            ));
        }

        ProviderPath::None => {
            let state_conv = state.clone();
            let user_id = user.user_id.clone();
            tokio::spawn(async move {
                let _ = tx
                    .send(StepEvent::Started {
                        step_id: "chat".into(),
                        provider: "cortex".into(),
                        model: "system".into(),
                    })
                    .await;

                let _ = tx
                    .send(StepEvent::Output {
                        step_id: "chat".into(),
                        line: "Cortex chat is unavailable right now. Please try again shortly."
                            .into(),
                    })
                    .await;

                let _ = tx
                    .send(StepEvent::Completed {
                        step_id: "chat".into(),
                        exit_code: 0,
                    })
                    .await;

                state_conv.vera_tracker.record_conversation(&user_id);
            });
        }
    }

    let stream = ReceiverStream::new(rx).map(step_event_to_sse);
    let boxed: Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> = Box::pin(stream);

    Ok(Sse::new(boxed).keep_alive(KeepAlive::default()))
}

/// `GET /api/chat/models`: the Claude tiers (always available, billed to
/// Cortex credits) plus the OpenCode Zen models (D4 in the BYOK plan),
/// billed to the customer's own key. The server is the authority on whether
/// a Zen model is actually usable -- `chat_zen::chat` re-checks the same key
/// state and refuses even if a stale client sends a Zen model this response
/// marked unavailable.
/// The exact `model` values `chat_models` returns for its Claude entries --
/// the only values `chat()` accepts in `ChatRequest.model` besides a
/// `"zen:"`-prefixed one. Kept as one array so the two can never drift.
fn claude_tier_model_values() -> [&'static str; 3] {
    ["fast", "balanced", "powerful"].map(|tier| crate::chat_paid::model_for_tier(Some(tier)))
}

pub async fn chat_models(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    headers: HeaderMap,
) -> Json<ModelsResponse> {
    let mut models = Vec::new();
    for tier in ["fast", "balanced", "powerful"] {
        models.push(ModelEntry {
            provider: "claude".into(),
            model: crate::chat_paid::model_for_tier(Some(tier)).into(),
            label: tier.into(),
            billing: "credits".into(),
            available: true,
            unavailable_reason: None,
        });
    }

    // BYOK is always available (no server-held master key to be missing);
    // a device's key status is only known once that device names itself.
    let device_id = headers
        .get(KEY_DEVICE_HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|id| crate::byok::valid_device_id(id));
    let key_status: Option<String> = match device_id {
        Some(device_id) => state
            .db
            .as_ref()
            .and_then(|db| db.get_provider_key_status(&user.user_id, "zen", device_id)),
        None => None,
    };

    for model in crate::supplier_zen::allowed_models() {
        let (available, reason) = match key_status.as_deref() {
            Some("active") => (true, None),
            Some("rejected") => (false, Some("key_rejected")),
            _ => (false, Some("needs_key")),
        };
        models.push(ModelEntry {
            provider: "zen".into(),
            model: format!("zen:{model}"),
            label: (*model).to_string(),
            billing: "your_zen_key".into(),
            available,
            unavailable_reason: reason.map(str::to_string),
        });
    }

    Json(ModelsResponse { models })
}

#[derive(serde::Serialize)]
pub struct ModelEntry {
    pub provider: String,
    pub model: String,
    pub label: String,
    pub billing: String,
    pub available: bool,
    pub unavailable_reason: Option<String>,
}

#[derive(serde::Serialize)]
pub struct ModelsResponse {
    pub models: Vec<ModelEntry>,
}

/// GET /api/chat/suggestions
pub async fn chat_suggestions(
    State(state): State<Arc<AppState>>,
    _user: ClerkUser,
) -> Json<SuggestionsResponse> {
    let mut suggestions = Vec::new();

    let has_workers = !state.workers.read().await.is_empty();
    let providers = state.providers.read().await;
    let has_providers = providers.iter().any(|p| p.authenticated);
    drop(providers);

    if !has_workers {
        suggestions.push(Suggestion {
            text: "Connect a worker to start executing tasks".into(),
            category: "setup".into(),
            shortcut: None,
        });
    } else if has_providers {
        suggestions.extend([
            Suggestion {
                text: "Fix the failing tests".into(),
                category: "execute".into(),
                shortcut: Some("fix".into()),
            },
            Suggestion {
                text: "Explore the project structure".into(),
                category: "search".into(),
                shortcut: Some("explore".into()),
            },
            Suggestion {
                text: "Review recent changes".into(),
                category: "think".into(),
                shortcut: Some("review".into()),
            },
            Suggestion {
                text: "Help me build something new".into(),
                category: "execute".into(),
                shortcut: Some("add".into()),
            },
        ]);
    }

    if let Some(db) = &state.db {
        let active_runs = db.list_active_runs();
        if !active_runs.is_empty() {
            suggestions.push(Suggestion {
                text: format!("Check status of {} active run(s)", active_runs.len()),
                category: "status".into(),
                shortcut: Some("status".into()),
            });
        }
    }

    Json(SuggestionsResponse { suggestions })
}

#[derive(serde::Serialize)]
pub struct SuggestionsResponse {
    pub suggestions: Vec<Suggestion>,
}

#[derive(serde::Serialize)]
pub struct Suggestion {
    pub text: String,
    pub category: String,
    pub shortcut: Option<String>,
}

/// POST /api/chat/options
pub async fn chat_options(
    State(_state): State<Arc<AppState>>,
    _user: ClerkUser,
    Json(req): Json<ChatOptionsRequest>,
) -> Json<ChatOptionsResponse> {
    let options = extract_options_from_response(&req.assistant_message);
    Json(ChatOptionsResponse { options })
}

#[derive(Deserialize)]
pub struct ChatOptionsRequest {
    pub assistant_message: String,
}

#[derive(serde::Serialize)]
pub struct ChatOptionsResponse {
    pub options: Vec<ChatOption>,
}

#[derive(serde::Serialize)]
pub struct ChatOption {
    pub label: String,
    pub value: String,
    pub category: String,
}

fn extract_options_from_response(message: &str) -> Vec<ChatOption> {
    let mut options = Vec::new();
    for line in message.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed
            .strip_prefix(|c: char| c.is_ascii_digit())
            .and_then(|s| s.strip_prefix(". ").or_else(|| s.strip_prefix(") ")))
        {
            let label = rest.trim().to_string();
            if !label.is_empty() && label.len() < 200 {
                options.push(ChatOption {
                    label: label.clone(),
                    value: label,
                    category: "option".into(),
                });
            }
        }
    }
    options
}

// --- Workspace Routing ---

async fn route_to_workspace(
    _state: &Arc<AppState>,
    workspace_id: &str,
    system_prompt: &str,
    user_message: &str,
) -> Result<String, String> {
    // TODO: Implement actual workspace communication
    // This would connect to the Replit workspace and execute commands there

    // For now, return a workspace-aware response
    Ok(format!(
        "🔧 **Workspace Mode** (Replit: `{}`)\n\n\
         I received your message: \"{}\"\n\n\
         *This workspace is isolated from the shared VPS and has access to your \
         authenticated Claude/OpenAI subscriptions. Full workspace execution \
         is being implemented.*\n\n\
         **System Context**: {}\n\n\
         Next steps:\n\
         - Connect to workspace runtime\n\
         - Execute commands in workspace environment\n\
         - Stream results back to frontend",
        workspace_id,
        user_message,
        system_prompt.split('\n').next().unwrap_or(system_prompt)
    ))
}

/// Checks the server's real `StepEvent::ConfirmRequired` JSON against
/// `cortex/src/test/fixtures/wire-events.json`, the fixture the browser's
/// `wireContract.test.tsx` feeds through the real ingest code. The two
/// crates can't share a Rust type across the language boundary, so this
/// fixture is the only thing keeping them from drifting silently -- if this
/// test fails, the server's wire JSON changed; update the `chat_confirm_required`
/// entry in the fixture to match (and check the browser types/tests still
/// accept it) rather than changing this test's expected shape.
#[cfg(test)]
mod wire_contract_tests {
    use super::*;

    const FIXTURE_JSON: &str = include_str!("../../../cortex/src/test/fixtures/wire-events.json");

    #[test]
    fn chat_confirm_required_matches_the_checked_in_fixture() {
        let event = StepEvent::ConfirmRequired {
            action_id: "fixture-action".to_string(),
            nonce: "fixture-nonce".to_string(),
            summary: "Fixture summary text".to_string(),
            expires_at: 1_790_000_000,
        };

        let actual: serde_json::Value =
            serde_json::from_str(&step_event_wire_json(&event)).unwrap();

        let fixtures: serde_json::Value = serde_json::from_str(FIXTURE_JSON).unwrap();
        let expected = fixtures.get("chat_confirm_required").unwrap_or_else(|| {
            panic!(
                "cortex/src/test/fixtures/wire-events.json is missing \"chat_confirm_required\" -- \
                 add an entry with the server's ConfirmRequired JSON for the fixed inputs this test uses"
            )
        });

        assert_eq!(
            &actual, expected,
            "StepEvent::ConfirmRequired's wire JSON no longer matches \
             cortex/src/test/fixtures/wire-events.json's \"chat_confirm_required\" entry -- \
             update that fixture entry to the new JSON (and update the browser side that \
             consumes it, cortex/src/lib/wireContract.test.tsx) rather than changing this test"
        );
    }
}
