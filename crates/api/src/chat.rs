use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures_core::Stream;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use cortex_engine::classifier::classify_intent;

use crate::clerk::ClerkUser;
use crate::llm_client::{self, ChatMessage, Provider};
use crate::routes::ErrorResponse;
use crate::state::{AppState, StepEvent};

fn step_event_to_sse(event: StepEvent) -> Result<Event, Infallible> {
    let data = serde_json::to_string(&event).unwrap_or_default();
    Ok(Event::default().data(data))
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
    /// BYOK model tier: "fast" (haiku/mini), "balanced" (sonnet/gpt-4.1), "powerful" (opus/gpt-5.5)
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
/// Priority: Workspace (isolated) > BYOS subscription (CLI) > BYOK API key > None
enum ProviderPath {
    /// Workspace: route to user's isolated Replit workspace
    Workspace { workspace_id: String },
    /// BYOS: authenticated subscription via CLI tool on the server
    Subscription {
        provider: Provider,
        model: String,
        credential_data: String,
    },
    /// BYOK: raw API key (use cheap model by default)
    ApiKey {
        provider: Provider,
        api_key: String,
        model: String,
    },
    /// Stored key exists but decryption failed (key rotation or corruption)
    DecryptFailed { provider: String },
    /// No provider available
    None,
}

fn byok_model(provider: &Provider, tier: Option<&str>) -> String {
    match (provider, tier.unwrap_or("fast")) {
        (Provider::Claude, "powerful") => "claude-opus-4-6".into(),
        (Provider::Claude, "balanced") => "claude-sonnet-4-6".into(),
        (Provider::Claude, _) => "claude-haiku-4-5".into(),
        (Provider::Openai, "powerful") => "gpt-4.1".into(),
        (Provider::Openai, "balanced") => "gpt-4.1".into(),
        (Provider::Openai, _) => "gpt-4.1-mini".into(),
        // Zen model IDs per opencode.ai/docs/zen; uncalibrated defaults.
        (Provider::Zen, "powerful") => "kimi-k3".into(),
        (Provider::Zen, "balanced") => "glm-5.2".into(),
        (Provider::Zen, _) => "glm-5".into(),
    }
}

async fn resolve_provider(
    state: &AppState,
    user_id: &str,
    model_tier: Option<&str>,
) -> ProviderPath {
    // TODO: check credential_assignments when project context is available
    // 0. Workspace request (workspace:{workspace_id})
    if let Some(workspace_id) = user_id.strip_prefix("workspace:") {
        return ProviderPath::Workspace {
            workspace_id: workspace_id.to_string(),
        };
    }

    // 1. Check user_credentials table (unified multi-credential system)
    if let Some(db) = &state.db {
        if let Some((cred, encrypted_data)) = db.get_any_credential(user_id) {
            match crate::crypto::decrypt(&encrypted_data) {
                Ok(decrypted) => {
                    let provider = match Provider::from_str(&cred.provider) {
                        Some(p) => p,
                        None => return ProviderPath::None,
                    };

                    // Touch the credential (update last_used_at)
                    db.touch_credential(&cred.id);

                    match cred.credential_type.as_str() {
                        "api_key" => {
                            let model = byok_model(&provider, model_tier);
                            return ProviderPath::ApiKey {
                                provider,
                                api_key: decrypted,
                                model,
                            };
                        }
                        "subscription" => {
                            // Zen has no subscription auth — a "subscription"
                            // credential for it is a data error; fall back to
                            // treating the secret as an API key.
                            if matches!(provider, Provider::Zen) {
                                let model = byok_model(&provider, model_tier);
                                return ProviderPath::ApiKey {
                                    provider,
                                    api_key: decrypted,
                                    model,
                                };
                            }
                            let model = match (&provider, model_tier.unwrap_or("fast")) {
                                (Provider::Claude, "powerful") => "claude-sonnet-4-6".into(),
                                (Provider::Claude, _) => "claude-sonnet-4-6".into(),
                                (Provider::Openai, "powerful") => "gpt-4.1".into(),
                                (Provider::Openai, _) => "gpt-4.1-mini".into(),
                                (Provider::Zen, _) => unreachable!("handled above"),
                            };
                            return ProviderPath::Subscription {
                                provider,
                                model,
                                credential_data: decrypted,
                            };
                        }
                        _ => {
                            let model = byok_model(&provider, model_tier);
                            return ProviderPath::ApiKey {
                                provider,
                                api_key: decrypted,
                                model,
                            };
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(user_id, provider = %cred.provider, "failed to decrypt credential: {e}");
                    return ProviderPath::DecryptFailed {
                        provider: cred.provider,
                    };
                }
            }
        }

        // 2. Fallback: check legacy user_api_keys table
        if let Some((provider_name, encrypted_key)) = db.get_any_api_key(user_id) {
            match crate::crypto::decrypt(&encrypted_key) {
                Ok(api_key) => {
                    if let Some(provider) = Provider::from_str(&provider_name) {
                        let model = byok_model(&provider, model_tier);
                        return ProviderPath::ApiKey {
                            provider,
                            api_key,
                            model,
                        };
                    }
                }
                Err(e) => {
                    tracing::warn!(user_id, provider = %provider_name, "failed to decrypt legacy API key: {e}");
                    return ProviderPath::DecryptFailed {
                        provider: provider_name,
                    };
                }
            }
        }
    }

    ProviderPath::None
}

pub async fn chat(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Json(req): Json<ChatRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, Json<ErrorResponse>)> {
    if req.message.len() > MAX_MESSAGE_LEN {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(ErrorResponse {
                error: format!("message exceeds {MAX_MESSAGE_LEN} bytes"),
            }),
        ));
    }
    if req.file_paths.len() > MAX_FILE_PATHS {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("too many file paths (max {MAX_FILE_PATHS})"),
            }),
        ));
    }
    let _file_paths = crate::validate::sanitize_file_paths(&req.file_paths)
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e })))?;

    if let Some(blocked) = crate::billing::check_chat_access(&state, &user.user_id) {
        return Err((StatusCode::PAYMENT_REQUIRED, Json(ErrorResponse {
            error: format!("Subscription required to access chat. Status: {blocked:?}. Go to Settings → Billing to subscribe."),
        })));
    }

    let intent = classify_intent(&req.message);
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

        ProviderPath::Subscription {
            provider,
            model,
            credential_data,
        } => {
            let system_prompt = system_prompt_for_intent(intent).to_string();
            let user_message = req.message.clone();
            let state_clone = state.clone();
            let user_id = user.user_id.clone();
            let conv_id = req.conversation_id.clone();
            let provider_name = provider.name().to_string();

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
                        step_id: "chat".into(),
                        provider: provider_name.clone(),
                        model: model.clone(),
                    })
                    .await;

                let (chunk_tx, mut chunk_rx) = mpsc::channel::<String>(64);
                let tx_clone = tx.clone();

                // Resolve container or tmpfs path, then stream in a child task
                let use_container = if let (Some(cm), Some(db)) =
                    (&state_clone.container_manager, &state_clone.db)
                {
                    match cm.ensure_container(db, &user_id, provider.name()).await {
                        Ok(container_id) => {
                            db.touch_container_activity(&user_id);
                            Some(container_id)
                        }
                        Err(e) => {
                            tracing::warn!("container unavailable, falling back to tmpfs: {e}");
                            None
                        }
                    }
                } else {
                    None
                };

                let stream_handle = if let Some(container_id) = use_container {
                    let sc = state_clone.clone();
                    tokio::spawn(async move {
                        if let Some(cm) = &sc.container_manager {
                            llm_client::stream_chat_via_container(
                                &provider,
                                &model,
                                &system_prompt,
                                &user_message,
                                cm,
                                &container_id,
                                chunk_tx,
                            )
                            .await
                        } else {
                            Err("container manager disappeared".into())
                        }
                    })
                } else {
                    tokio::spawn(async move {
                        llm_client::stream_chat_cli_isolated(
                            &provider,
                            Some(&model),
                            &credential_data,
                            &user_id,
                            &system_prompt,
                            &user_message,
                            chunk_tx,
                        )
                        .await
                    })
                };

                let mut full_response = String::new();
                while let Some(chunk) = chunk_rx.recv().await {
                    full_response.push_str(&chunk);
                    let _ = tx_clone
                        .send(StepEvent::Output {
                            step_id: "chat".into(),
                            line: chunk,
                        })
                        .await;
                }

                match stream_handle.await {
                    Ok(Ok(())) => {
                        let _ = tx_clone
                            .send(StepEvent::Completed {
                                step_id: "chat".into(),
                                exit_code: 0,
                            })
                            .await;
                    }
                    Ok(Err(e)) => {
                        let _ = tx_clone
                            .send(StepEvent::Failed {
                                step_id: "chat".into(),
                                error: e,
                            })
                            .await;
                    }
                    Err(e) => {
                        let _ = tx_clone
                            .send(StepEvent::Failed {
                                step_id: "chat".into(),
                                error: format!("task panicked: {e}"),
                            })
                            .await;
                    }
                }

                if let (Some(db), Some(cid)) = (&state_clone.db, &conv_id) {
                    if !full_response.is_empty() {
                        db.add_message(
                            cid,
                            "assistant",
                            &full_response,
                            Some(&provider_name),
                            None,
                        );
                    }
                }
            });
        }

        ProviderPath::ApiKey {
            provider,
            api_key,
            model,
        } => {
            let system_prompt = system_prompt_for_intent(intent).to_string();
            let user_message = req.message.clone();
            let state_clone = state.clone();
            let conv_id = req.conversation_id.clone();
            let provider_name = provider.name().to_string();

            // Budget enforcement — check before spending user's money
            if let Some(db) = &state.db {
                let enforcer = crate::budget_enforcer::BudgetEnforcer::new(db);
                let est_input =
                    crate::cost_estimator::CostEstimator::estimate_tokens_from_text(&req.message);
                let est_output =
                    crate::cost_estimator::CostEstimator::estimate_output_tokens("chat", &model);
                let (allowed, budget_result, warning) = enforcer.check_budget_before_request(
                    &user.user_id,
                    provider.name(),
                    &model,
                    est_input,
                    est_output,
                    true,
                );

                if let Some(w) = &warning {
                    enforcer.record_warning(w);
                }

                if !allowed {
                    let budget_total = budget_result.daily_spent + budget_result.daily_remaining;
                    let reason = if budget_result.daily_remaining <= 0.0 {
                        format!(
                            "Daily budget limit reached (${:.2} / ${:.2}).",
                            budget_result.daily_spent, budget_total
                        )
                    } else if budget_result.weekly_remaining <= 0.0 {
                        format!(
                            "Weekly budget limit reached (${:.2} spent this week).",
                            budget_result.weekly_spent
                        )
                    } else {
                        format!(
                            "Monthly budget limit reached (${:.2} spent this month).",
                            budget_result.monthly_spent
                        )
                    };
                    let _ = tx
                        .send(StepEvent::Failed {
                            step_id: "chat".into(),
                            error: format!(
                                "{reason} Adjust your limits in Settings → Budget & Costs."
                            ),
                        })
                        .await;
                    let stream = ReceiverStream::new(rx).map(step_event_to_sse);
                    return Ok(Sse::new(stream).keep_alive(KeepAlive::default()));
                }
            }

            state.vera_tracker.record_conversation(&user.user_id);
            if let Some(i) = intent {
                state.vera_tracker.record_routed(&user.user_id, i);
            }

            if let (Some(db), Some(cid)) = (&state.db, &conv_id) {
                db.add_message(cid, "user", &user_message, None, None);
            }

            // Start cost session
            let session_id = state.db.as_ref().map(|db| {
                let enforcer = crate::budget_enforcer::BudgetEnforcer::new(db);
                let est_cost = {
                    let est_in = crate::cost_estimator::CostEstimator::estimate_tokens_from_text(
                        &user_message,
                    );
                    let est_out = crate::cost_estimator::CostEstimator::estimate_output_tokens(
                        "chat", &model,
                    );
                    let (cost, _) = crate::cost_estimator::CostEstimator::estimate_request_cost(
                        provider.name(),
                        &model,
                        est_in,
                        est_out,
                        None,
                    );
                    cost
                };
                enforcer.start_cost_session(
                    &user.user_id,
                    provider.name(),
                    "byok",
                    est_cost,
                    0,
                    Some(&model),
                )
            });

            tokio::spawn(async move {
                let _ = tx
                    .send(StepEvent::Started {
                        step_id: "chat".into(),
                        provider: provider_name.clone(),
                        model: model.clone(),
                    })
                    .await;

                let (chunk_tx, mut chunk_rx) = mpsc::channel::<String>(64);
                let tx_clone = tx.clone();

                let messages = vec![ChatMessage {
                    role: "user".into(),
                    content: user_message,
                }];

                let model_for_cost = model.clone();
                let stream_handle = tokio::spawn(async move {
                    llm_client::stream_chat_api(
                        &provider,
                        &api_key,
                        Some(&model),
                        &system_prompt,
                        &messages,
                        chunk_tx,
                    )
                    .await
                });

                let mut full_response = String::new();
                while let Some(chunk) = chunk_rx.recv().await {
                    full_response.push_str(&chunk);
                    let _ = tx_clone
                        .send(StepEvent::Output {
                            step_id: "chat".into(),
                            line: chunk,
                        })
                        .await;
                }

                let _success = match stream_handle.await {
                    Ok(Ok(())) => {
                        let _ = tx_clone
                            .send(StepEvent::Completed {
                                step_id: "chat".into(),
                                exit_code: 0,
                            })
                            .await;
                        true
                    }
                    Ok(Err(e)) => {
                        let _ = tx_clone
                            .send(StepEvent::Failed {
                                step_id: "chat".into(),
                                error: e,
                            })
                            .await;
                        false
                    }
                    Err(e) => {
                        let _ = tx_clone
                            .send(StepEvent::Failed {
                                step_id: "chat".into(),
                                error: format!("task panicked: {e}"),
                            })
                            .await;
                        false
                    }
                };

                // Finalize cost session
                if let (Some(db), Some(sid)) = (&state_clone.db, &session_id) {
                    let enforcer = crate::budget_enforcer::BudgetEnforcer::new(db);
                    let tokens_out =
                        crate::cost_estimator::CostEstimator::estimate_tokens_from_text(
                            &full_response,
                        );
                    let (actual_cost, _) =
                        crate::cost_estimator::CostEstimator::estimate_request_cost(
                            &provider_name,
                            &model_for_cost,
                            0,
                            tokens_out,
                            None,
                        );
                    enforcer.finalize_cost_session(sid, actual_cost, tokens_out);
                }

                if let (Some(db), Some(cid)) = (&state_clone.db, &conv_id) {
                    if !full_response.is_empty() {
                        db.add_message(
                            cid,
                            "assistant",
                            &full_response,
                            Some(&provider_name),
                            None,
                        );
                    }
                }
            });
        }

        ProviderPath::DecryptFailed { provider } => {
            tokio::spawn(async move {
                let _ = tx
                    .send(StepEvent::Started {
                        step_id: "chat".into(),
                        provider: "cortex".into(),
                        model: "system".into(),
                    })
                    .await;

                let _ = tx.send(StepEvent::Output {
                    step_id: "chat".into(),
                    line: format!(
                        "Your stored **{provider}** API key could not be decrypted. This can happen after a server update.\n\n\
                         Please re-add your API key in **Settings → API Keys** to restore access."
                    ),
                }).await;

                let _ = tx
                    .send(StepEvent::Completed {
                        step_id: "chat".into(),
                        exit_code: 0,
                    })
                    .await;
            });
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

                let _ = tx.send(StepEvent::Output {
                    step_id: "chat".into(),
                    line: "To get started, connect your AI provider:\n\n\
                           **Option 1 (Recommended):** Link your Claude or OpenAI subscription — your existing plan covers usage.\n\n\
                           **Option 2:** Add an API key in **Settings → API Keys** — pay-per-use with your own key.\n\n\
                           Once connected, I can help you build, debug, review, and ship code.".into(),
                }).await;

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

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
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
