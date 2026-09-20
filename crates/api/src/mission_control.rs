use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::clerk;
#[cfg(feature = "soma")]
use crate::lock::LockRecovering;
use crate::state::AppState;

/// Events pushed to Mission Control frontend clients.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MissionControlEvent {
    RunCreated {
        run_id: String,
        goal: String,
        step_count: usize,
    },
    StepDispatched {
        run_id: String,
        step_id: String,
        provider: String,
        model: String,
        tier: String,
    },
    StepStarted {
        run_id: String,
        step_id: String,
        provider: String,
        model: String,
    },
    StepOutput {
        run_id: String,
        step_id: String,
        line: String,
    },
    /// A worker handed back a commit and our verifier has started grading it.
    ///
    /// Deliberately not `StepCompleted`: nothing has been checked, so no pane
    /// may render this as done. `exit_code` and `files_changed` are the
    /// worker's own report and are labelled as diagnostics where they surface.
    StepDelivered {
        run_id: String,
        step_id: String,
        exit_code: i32,
        files_changed: Vec<String>,
        cost_estimate: Option<f64>,
    },
    StepFailed {
        run_id: String,
        step_id: String,
        error: String,
        failure_kind: String,
    },
    RunCompleted {
        run_id: String,
        status: String,
        total_cost: Option<f64>,
    },
    WorkerConnected {
        worker_id: String,
        providers: Vec<String>,
    },
    WorkerDisconnected {
        worker_id: String,
    },
    RoutingDecision {
        step_id: String,
        provider: String,
        model: String,
        rationale: String,
        pressure: f64,
    },
    BanditUpdate {
        provider: String,
        task_family: String,
        risk: String,
        trials: u32,
        mean_reward: f64,
        success: bool,
    },
}

/// Unique ID for each subscriber so we can remove the right one on disconnect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriberId(Uuid);

pub struct McSubscriber {
    pub id: SubscriberId,
    pub tx: mpsc::Sender<MissionControlEvent>,
}

/// Per-step throttle state for StepOutput events.
struct OutputThrottle {
    last_sent: Instant,
    pending: Option<MissionControlEvent>,
}

#[derive(Debug, Deserialize)]
pub struct McQueryParams {
    pub token: Option<String>,
}

pub async fn mc_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<McQueryParams>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_mc_connection(socket, state, params))
}

async fn handle_mc_connection(mut socket: WebSocket, state: Arc<AppState>, params: McQueryParams) {
    // Authenticate via JWT token from query param
    let user_id = match authenticate_mc(&state, params.token.as_deref().unwrap_or("")).await {
        Ok(uid) => uid,
        Err(e) => {
            tracing::warn!("mission control auth failed: {e}");
            let err = serde_json::json!({
                "type": "error",
                "code": "auth_failed",
                "message": e,
            });
            let _ = socket
                .send(Message::Text(serde_json::to_string(&err).unwrap().into()))
                .await;
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    };

    tracing::info!("mission control client connected: user={user_id}");

    // Send welcome with protocol version
    let welcome = serde_json::json!({
        "type": "welcome",
        "user_id": user_id,
        "protocol": "mc/v1",
        "events": [
            "run_created", "step_dispatched", "step_started", "step_output",
            "step_completed", "step_failed", "run_completed",
            "worker_connected", "worker_disconnected",
            "routing_decision", "bandit_update"
        ],
    });
    if socket
        .send(Message::Text(
            serde_json::to_string(&welcome).unwrap().into(),
        ))
        .await
        .is_err()
    {
        return;
    }

    // Subscribe to MC events for this user
    let (tx, mut rx) = mpsc::channel::<MissionControlEvent>(128);
    let sub_id = SubscriberId(Uuid::new_v4());
    state.subscribe_mc(&user_id, sub_id, tx).await;

    // Per-step output throttling: max 1 StepOutput per 500ms per step
    let mut throttles: HashMap<String, OutputThrottle> = HashMap::new();
    let throttle_interval = Duration::from_millis(500);

    // Create a flush ticker for throttled output
    let mut flush_interval = tokio::time::interval(Duration::from_millis(250));
    flush_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                // Apply throttling for StepOutput events
                if let MissionControlEvent::StepOutput { ref step_id, .. } = event {
                    let step_key = step_id.clone();
                    let throttle = throttles.entry(step_key).or_insert_with(|| OutputThrottle {
                        last_sent: Instant::now() - throttle_interval,
                        pending: None,
                    });

                    if throttle.last_sent.elapsed() >= throttle_interval {
                        // Enough time has passed, send immediately
                        throttle.last_sent = Instant::now();
                        throttle.pending = None;
                        let json = serde_json::to_string(&event).unwrap();
                        if socket.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    } else {
                        // Too soon, buffer the latest (drop intermediate lines)
                        throttle.pending = Some(event);
                    }
                } else {
                    // Non-output events: send immediately
                    // If this is a StepCompleted/StepFailed, flush any pending output first
                    let flush_step = match &event {
                        MissionControlEvent::StepDelivered { step_id, .. }
                        | MissionControlEvent::StepFailed { step_id, .. } => Some(step_id.clone()),
                        _ => None,
                    };

                    if let Some(step_key) = flush_step {
                        if let Some(throttle) = throttles.remove(&step_key) {
                            if let Some(pending) = throttle.pending {
                                let json = serde_json::to_string(&pending).unwrap();
                                if socket.send(Message::Text(json.into())).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }

                    let json = serde_json::to_string(&event).unwrap();
                    if socket.send(Message::Text(json.into())).await.is_err() {
                        break;
                    }
                }
            }

            _ = flush_interval.tick() => {
                // Flush any pending throttled output whose interval has passed
                let now = Instant::now();
                let mut to_remove = Vec::new();

                for (step_key, throttle) in throttles.iter_mut() {
                    if throttle.pending.is_some() && now.duration_since(throttle.last_sent) >= throttle_interval {
                        if let Some(pending) = throttle.pending.take() {
                            throttle.last_sent = now;
                            let json = serde_json::to_string(&pending).unwrap();
                            if socket.send(Message::Text(json.into())).await.is_err() {
                                to_remove.clear(); // will break outer loop
                                break;
                            }
                        }
                    }
                    // Clean up throttle entries for steps that haven't had output in a while
                    if throttle.pending.is_none() && now.duration_since(throttle.last_sent) > Duration::from_secs(60) {
                        to_remove.push(step_key.clone());
                    }
                }

                for key in to_remove {
                    throttles.remove(&key);
                }
            }

            result = socket.recv() => {
                match result {
                    // MC clients are read-only observers; we accept pings/pongs but
                    // ignore any text messages. Close on disconnect.
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => continue,
                }
            }
        }
    }

    state.unsubscribe_mc(&user_id, sub_id).await;
    tracing::info!("mission control client disconnected: user={user_id}");
}

// --- Snapshot endpoint: GET /api/mc/snapshot ---
// Provides the current system state so the frontend map can render immediately
// on connect, before any delta events arrive.

#[derive(Debug, Serialize)]
pub struct McSnapshot {
    pub heart: Option<McHeartState>,
    pub workers: Vec<McWorkerState>,
    pub active_runs: Vec<McRunState>,
    pub providers: Vec<McProviderState>,
    pub spend: Option<McSpendState>,
}

#[derive(Debug, Serialize)]
pub struct McHeartState {
    pub did: String,
    pub heartbeat_count: usize,
    pub head_hash: String,
    pub has_lineage: bool,
    pub root_did: Option<String>,
    pub capabilities: Vec<String>,
    pub revoked_count: usize,
}

#[derive(Debug, Serialize)]
pub struct McWorkerState {
    pub worker_id: String,
    pub user_id: String,
    pub providers: Vec<String>,
    pub disabled_providers: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct McRunState {
    pub run_id: String,
    pub goal: String,
    pub status: String,
    pub step_count: usize,
    pub steps_completed: usize,
    pub steps_failed: usize,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct McProviderState {
    pub id: String,
    pub label: String,
    pub authenticated: bool,
    pub pressure: f64,
    pub tiers: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct McSpendState {
    pub total_spend: f64,
    pub delegation_count: usize,
    pub active_delegation_count: usize,
}

pub async fn mc_snapshot(
    State(state): State<Arc<AppState>>,
    _user: crate::clerk::ClerkUser,
) -> axum::Json<McSnapshot> {
    // Heart state. `None` in a default build — there is no heart to report.
    #[cfg(feature = "soma")]
    let heart = state.soma_heart.as_ref().map(|h| {
        let chain = h.heartbeat_chain.lock_recovering();
        let revoked = h.revoked_delegations.lock_recovering();
        let capabilities = h
            .lineage
            .as_ref()
            .map(soma::lineage::effective_capabilities)
            .unwrap_or_else(|| vec!["*".into()]);
        McHeartState {
            did: h.did().to_string(),
            heartbeat_count: chain.len(),
            head_hash: chain.head_hash().to_string(),
            has_lineage: h.lineage.is_some(),
            root_did: h.root_did.clone(),
            capabilities,
            revoked_count: revoked.len(),
        }
    });
    #[cfg(not(feature = "soma"))]
    let heart: Option<McHeartState> = None;

    // Worker states
    let workers_guard = state.workers.read().await;
    let workers: Vec<McWorkerState> = workers_guard
        .iter()
        .map(|(id, w)| McWorkerState {
            worker_id: id.clone(),
            user_id: w.user_id.clone(),
            providers: w
                .available_providers
                .iter()
                .map(|p| p.to_string())
                .collect(),
            disabled_providers: w.disabled_providers.iter().map(|p| p.to_string()).collect(),
        })
        .collect();
    drop(workers_guard);

    // Active runs from DB
    let active_runs = if let Some(db) = &state.db {
        db.list_active_runs()
            .into_iter()
            .map(|r| McRunState {
                run_id: r.id.clone(),
                goal: r.goal.clone(),
                status: r.status.clone(),
                step_count: r.step_count,
                steps_completed: r.steps_completed,
                steps_failed: r.steps_failed,
                created_at: r.created_at.clone(),
            })
            .collect()
    } else {
        Vec::new()
    };

    // Provider states
    let providers_guard = state.providers.read().await;
    let providers: Vec<McProviderState> = providers_guard
        .iter()
        .map(|p| McProviderState {
            id: p.provider.to_string(),
            label: p.provider.to_string(),
            authenticated: p.authenticated,
            pressure: p.pressure,
            tiers: p.available_tiers.iter().map(|t| format!("{t:?}")).collect(),
        })
        .collect();
    drop(providers_guard);

    // Spend state. Soma's spend log is not the Cortex ledger; it is Soma's own
    // record and it is absent in a default build.
    #[cfg(feature = "soma")]
    let spend = state.soma_heart.as_ref().map(|h| {
        let logs = h.spend_logs.lock_recovering();
        let mut total = 0.0;
        let mut active = 0usize;
        for log in logs.values() {
            total += log.cumulative();
            if log.last_activity_ms() > 0 {
                active += 1;
            }
        }
        McSpendState {
            total_spend: total,
            delegation_count: logs.len(),
            active_delegation_count: active,
        }
    });
    #[cfg(not(feature = "soma"))]
    let spend: Option<McSpendState> = None;

    axum::Json(McSnapshot {
        heart,
        workers,
        active_runs,
        providers,
        spend,
    })
}

async fn authenticate_mc(state: &AppState, token: &str) -> Result<String, String> {
    let clerk_secret = match &state.clerk_secret_key {
        Some(key) => key,
        None => {
            // No Clerk configured — local dev mode
            return Ok("local".to_string());
        }
    };

    if token.is_empty() {
        return Err("missing token query parameter — connect with ?token=<jwt>".into());
    }

    let keys = clerk::get_or_refresh_jwks_pub(
        &state.jwks_cache,
        &state.jwks_stampede,
        clerk_secret,
        false,
    )
    .await?;

    match clerk::verify_token_pub(token, &keys) {
        Ok(user_id) => Ok(user_id),
        Err(_) => {
            // Retry with fresh JWKS (key rotation)
            let keys = clerk::get_or_refresh_jwks_pub(
                &state.jwks_cache,
                &state.jwks_stampede,
                clerk_secret,
                true,
            )
            .await?;
            clerk::verify_token_pub(token, &keys)
        }
    }
}
