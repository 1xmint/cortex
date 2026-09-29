use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use cortex_core::routing::RoutingDecision;
use cortex_core::task::TaskContract;
use cortex_core::usage::{estimate_cost_by_provider, CostProjection, StepCostEstimate};
use cortex_engine::decomposer::decompose_goal;

use crate::billing::PremiumUser;
use crate::clerk::ClerkUser;
use crate::github;
#[cfg(feature = "soma")]
use crate::lock::LockRecovering;
use crate::run_payload::{build_run_graph_payload, build_run_step_payloads};
use crate::scheduler;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct RouteRequest {
    pub input: String,
    #[serde(default)]
    pub file_paths: Vec<String>,
    #[serde(default)]
    pub routing_preferences: Option<crate::chat::RoutingPreferences>,
}

#[derive(Serialize)]
pub struct RouteResponse {
    pub task: TaskContract,
    pub decision: RoutingDecision,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

/// The shape every JSON handler in this crate returns.
///
/// Here rather than in a feature module because both products' handlers use it,
/// and a shared helper living inside one of them is how a split gets undone.
pub type ApiResult<T> = Result<T, (axum::http::StatusCode, Json<ErrorResponse>)>;

/// The database handle, or a 500 that says why.
pub fn db_ref(state: &crate::state::AppState) -> ApiResult<&crate::db::Database> {
    state.db.as_ref().ok_or_else(|| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "database not available".into(),
            }),
        )
    })
}

/// GET /api/health — detailed subsystem health.
///
/// Reports the status of each subsystem (database, docker, soma, scheduler,
/// workers). The top-level `status` reflects only *critical* subsystems so the
/// check stays stable across environments where optional subsystems (Docker
/// containers, Soma) are intentionally absent:
///   - `ok`        all critical subsystems healthy
///   - `unhealthy` a critical subsystem (database) is down
///
/// A separate `degraded` boolean flags when a non-critical subsystem is down
/// (useful for dashboards) without flipping the liveness contract. Returns
/// HTTP 503 when `unhealthy` so load balancers can drain the node, otherwise
/// HTTP 200. Subsystem gauges are also published to Prometheus.
pub async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let started = std::time::Instant::now();

    // ── Database (critical) ───────────────────────────────────────────────
    let db_ok = state.db.as_ref().map(|d| d.health_check()).unwrap_or(false);
    crate::metrics::set_subsystem_up("database", db_ok);

    // ── Docker containers (non-critical: API still serves without it) ──────
    let docker_ok = state.container_manager.is_some();
    crate::metrics::set_subsystem_up("docker", docker_ok);

    // ── Soma identity (non-critical) ──────────────────────────────────────
    // Compiled out by default. A build without the feature does not report a
    // Soma subsystem at all, rather than reporting one that is permanently
    // down — a health check that always shows a red light teaches operators to
    // ignore it.
    #[cfg(feature = "soma")]
    let (soma_did, heartbeat_count, soma_ok) = {
        let did = state.soma_heart.as_ref().map(|h| h.did().to_string());
        let beats = state
            .soma_heart
            .as_ref()
            .map(|h| h.heartbeat_chain.lock_recovering().len())
            .unwrap_or(0);
        let ok = state.soma_heart.is_some();
        crate::metrics::set_subsystem_up("soma", ok);
        (did, beats, ok)
    };

    // ── Scheduler + workers (informational) ───────────────────────────────
    let scheduler_ok = state.scheduler_tx.read().await.is_some();
    let worker_count = state.workers.read().await.len();
    crate::metrics::set_subsystem_up("scheduler", scheduler_ok);

    // ── Container population (best-effort; published to Prometheus) ────────
    let (containers_total, containers_running) = match &state.db {
        Some(db) => {
            let containers = db.list_all_containers();
            let running = containers.iter().filter(|c| c.status == "running").count();
            let stopped = containers.len() - running;
            crate::metrics::set_container_population(running as i64, stopped as i64);
            (containers.len(), running)
        }
        None => (0, 0),
    };

    // Aggregate. Database is the only hard dependency for serving the API; the
    // top-level status therefore tracks critical subsystems only. Non-critical
    // subsystems being down is surfaced via `degraded` without flipping `ok`.
    let (status, http_status) = if !db_ok {
        ("unhealthy", StatusCode::SERVICE_UNAVAILABLE)
    } else {
        ("ok", StatusCode::OK)
    };
    #[cfg(feature = "soma")]
    let degraded = !docker_ok || !soma_ok || !scheduler_ok;
    #[cfg(not(feature = "soma"))]
    let degraded = !docker_ok || !scheduler_ok;

    #[cfg_attr(not(feature = "soma"), allow(unused_mut))]
    let mut body = serde_json::json!({
        "status": status,
        "degraded": degraded,
        "service": "cortex",
        "version": env!("CARGO_PKG_VERSION"),
        "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "checks": {
            "database": { "ok": db_ok, "critical": true },
            "docker": { "ok": docker_ok, "critical": false },
            "scheduler": { "ok": scheduler_ok, "critical": false },
        },
        "workers": worker_count,
        "containers": {
            "total": containers_total,
            "running": containers_running,
        },
        "check_duration_ms": started.elapsed().as_millis() as u64,
    });

    // Added rather than nulled, so the absence of the keys is the signal that
    // this build has no Soma in it.
    #[cfg(feature = "soma")]
    {
        body["checks"]["soma"] = serde_json::json!({ "ok": soma_ok, "critical": false });
        body["soma"] = serde_json::json!({
            "did": soma_did,
            "protocol": "soma-delegation/0.1",
            "heartbeats": heartbeat_count,
        });
    }

    (http_status, Json(body))
}

pub async fn deploy_info() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "service": "cortex",
        "version": env!("CARGO_PKG_VERSION"),
        "commit": option_env!("GITHUB_SHA"),
    }))
}

pub async fn get_providers(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> Json<Vec<cortex_core::provider::ProviderStatus>> {
    // If using local auth (no real user), return empty to prevent frontend crashes
    if user.user_id == "local" && state.clerk_secret_key.is_none() {
        return Json(Vec::new());
    }
    let providers = state.providers.read().await;
    Json(providers.clone())
}

// --- Runs API ---

#[derive(Deserialize)]
pub struct CreateRunRequest {
    pub goal: String,
    #[serde(default)]
    pub file_paths: Vec<String>,
    /// Stable repository/workspace scope for conflict prevention.
    ///
    /// Prefer `github:{owner}/{repo}` when known. Omit for backward-compatible
    /// single-workspace behavior.
    #[serde(default)]
    pub repo_key: Option<String>,
    #[serde(default = "default_profile")]
    pub profile: String,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub authority_scope_id: Option<String>,
    #[serde(default)]
    pub authority_handoff_id: Option<String>,
    #[serde(default)]
    pub authority_reason: Option<String>,
}

fn default_profile() -> String {
    "auto".to_string()
}

#[derive(Serialize)]
pub struct CreateRunResponse {
    pub run_id: String,
    pub steps: usize,
    pub authority_scope_id: Option<String>,
}

fn trim_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn validate_optional_id(
    label: &str,
    value: Option<&str>,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if let Some(value) = value {
        if value.len() > 256
            || !value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | ':' | '.' | '/'))
        {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("invalid {label}"),
                }),
            ));
        }
    }
    Ok(())
}

fn validate_run_authority(
    db: &crate::db::Database,
    user_id: &str,
    authority_scope_id: Option<String>,
    authority_handoff_id: Option<String>,
    authority_reason: Option<String>,
    repo_key: Option<&str>,
) -> Result<Option<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    validate_optional_id("authority_scope_id", authority_scope_id.as_deref())?;
    validate_optional_id("authority_handoff_id", authority_handoff_id.as_deref())?;
    let authority_scope_id = trim_optional(authority_scope_id);
    let authority_handoff_id = trim_optional(authority_handoff_id);
    let authority_reason = trim_optional(authority_reason);

    if let Some(repo_key) = repo_key {
        if authority_scope_id.is_none() {
            if let Some(scope) =
                db.find_non_personal_authority_resource_scope(user_id, "github_repo", repo_key)
            {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: format!(
                            "authority_scope_id is required for {} work on {repo_key}",
                            scope.kind
                        ),
                    }),
                ));
            }
        }
    }

    let Some(scope_id) = authority_scope_id else {
        db.ensure_personal_authority_scope(user_id);
        return Ok(Some(serde_json::json!({
            "scope_id": format!("personal:{user_id}"),
            "scope_kind": "personal",
            "role": "owner",
            "handoff_id": null,
            "reason": authority_reason,
        })));
    };

    let scope = db
        .get_authority_scope_for_user(user_id, &scope_id)
        .ok_or_else(|| {
            (
                StatusCode::FORBIDDEN,
                Json(ErrorResponse {
                    error: "authority_scope_id is not available to this user".into(),
                }),
            )
        })?;

    if scope.kind != "personal" {
        let repo_key = repo_key.ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "repo_key is required for non-personal authority runs".into(),
                }),
            )
        })?;
        if !db.authority_resource_allows(user_id, &scope.id, "github_repo", repo_key, "write") {
            return Err((
                StatusCode::FORBIDDEN,
                Json(ErrorResponse {
                    error: "authority scope does not grant write access to repo_key".into(),
                }),
            ));
        }
        let requires_handoff = scope
            .policy
            .get("requires_org_handoff")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        if requires_handoff && authority_handoff_id.is_none() {
            return Err((
                StatusCode::FORBIDDEN,
                Json(ErrorResponse {
                    error: "authority_handoff_id is required for this authority scope".into(),
                }),
            ));
        }
    }

    Ok(Some(serde_json::json!({
        "scope_id": scope.id,
        "scope_kind": scope.kind,
        "role": scope.role,
        "handoff_id": authority_handoff_id,
        "reason": authority_reason,
    })))
}

fn validate_pr_authority(
    db: &crate::db::Database,
    user_id: &str,
    run_id: &str,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let context = db
        .get_run_pr_authority_context(run_id, user_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "run not found".into(),
                }),
            )
        })?;
    let repo_key = context.repo_key.as_deref();
    if !db.run_has_pr_write_lease(run_id, repo_key) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "PR creation requires a run-owned write lease".into(),
            }),
        ));
    }

    let scope_id = context
        .authority_scope_id
        .as_deref()
        .or_else(|| {
            context
                .authority_context
                .get("scope_id")
                .and_then(|value| value.as_str())
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("personal:{user_id}"));

    let scope = db
        .get_authority_scope_for_user(user_id, &scope_id)
        .ok_or_else(|| {
            (
                StatusCode::FORBIDDEN,
                Json(ErrorResponse {
                    error: "run authority scope is not available to this user".into(),
                }),
            )
        })?;

    if scope.kind == "personal" {
        return Ok(());
    }

    let Some(repo_key) = repo_key else {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "non-personal PR creation requires a repo_key".into(),
            }),
        ));
    };
    if !db.authority_resource_allows(user_id, &scope.id, "github_repo", repo_key, "write") {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "run authority scope does not grant write access to repo_key".into(),
            }),
        ));
    }

    let handoff_id = context
        .authority_context
        .get("handoff_id")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if handoff_id.is_none() {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "non-personal PR creation requires an authority_handoff_id".into(),
            }),
        ));
    }

    Ok(())
}

pub async fn create_run(
    State(state): State<Arc<AppState>>,
    user: PremiumUser,
    Json(req): Json<CreateRunRequest>,
) -> Result<Json<CreateRunResponse>, (StatusCode, Json<ErrorResponse>)> {
    if req.goal.len() > 32_768 {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(ErrorResponse {
                error: "goal exceeds 32KB".into(),
            }),
        ));
    }
    if req.file_paths.len() > 50 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "too many file paths (max 50)".into(),
            }),
        ));
    }
    if let Some(repo_key) = req.repo_key.as_deref() {
        if repo_key.len() > 256
            || !repo_key
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | ':' | '.' | '/'))
        {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "invalid repo_key".into(),
                }),
            ));
        }
    }
    for (label, value) in [
        ("task_id", req.task_id.as_deref()),
        ("group_id", req.group_id.as_deref()),
        ("conversation_id", req.conversation_id.as_deref()),
    ] {
        if let Some(value) = value {
            if value.len() > 256
                || !value
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | ':' | '.'))
            {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: format!("invalid {label}"),
                    }),
                ));
            }
        }
    }
    let file_paths = crate::validate::sanitize_file_paths(&req.file_paths)
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e })))?;

    if req.task_id.is_some() && req.group_id.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "group_id is required when task_id is provided".into(),
            }),
        ));
    }

    let db = state.db.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "database not available".into(),
            }),
        )
    })?;

    // Item F of the M-D-0024 settlement redesign: production dispatch
    // requires a usable provider gateway. Refusing here, before a run row
    // even exists, is cheaper for the customer than accepting the run and
    // letting every one of its steps discover the same thing one at a time
    // in the scheduler's own `dispatch_money_gate` check (which still runs,
    // for runs created before an outage started or outside this endpoint).
    // No run means nothing downstream can be dispatched, leased or charged
    // for a call that was never going to be possible.
    if crate::is_production_env() && !crate::provider_gateway_http::is_gateway_on() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: cortex_core::billing_binding::GATEWAY_DOWN_MESSAGE.into(),
            }),
        ));
    }

    if req.task_id.is_some() || req.conversation_id.is_some() {
        if let (Some(task_id), Some(group_id)) = (req.task_id.as_deref(), req.group_id.as_deref()) {
            if !db.cortex_task_exists(&user.user_id, group_id, task_id) {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: "task_id is not known for this group".into(),
                    }),
                ));
            }
        }

        if let Some(conversation_id) = req.conversation_id.as_deref() {
            if !db.conversation_exists(&user.user_id, conversation_id) {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: "conversation_id is not owned by this user".into(),
                    }),
                ));
            }
        }
    }

    let authority_context = validate_run_authority(
        db,
        &user.user_id,
        req.authority_scope_id.clone(),
        req.authority_handoff_id.clone(),
        req.authority_reason.clone(),
        req.repo_key.as_deref(),
    )?;
    let response_authority_scope_id = authority_context
        .as_ref()
        .and_then(|value| value.get("scope_id"))
        .and_then(|value| value.as_str())
        .map(String::from);

    let scheduler_tx = state.scheduler_tx.read().await;
    let tx = scheduler_tx.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "scheduler not ready".into(),
            }),
        )
    })?;

    let user_id = &user.user_id;

    let run_id = scheduler::create_run_from_goal(
        &state,
        tx,
        user_id,
        &req.goal,
        &file_paths,
        req.repo_key.as_deref(),
        &req.profile,
        req.task_id.as_deref(),
        req.group_id.as_deref(),
        req.conversation_id.as_deref(),
        authority_context,
    )
    .await
    .map_err(|e| {
        let status = if e.starts_with("resource conflict:") {
            StatusCode::CONFLICT
        } else {
            StatusCode::BAD_REQUEST
        };
        (status, Json(ErrorResponse { error: e }))
    })?;

    // Count steps
    let steps = state
        .db
        .as_ref()
        .map(|db| db.get_all_step_statuses(&run_id).len())
        .unwrap_or(0);

    Ok(Json(CreateRunResponse {
        run_id,
        steps,
        authority_scope_id: response_authority_scope_id,
    }))
}

// --- User-scoped run listing ---

#[derive(Deserialize)]
pub struct ListRunsQuery {
    #[serde(default = "default_run_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

#[derive(Deserialize)]
pub struct ListRunEventsQuery {
    #[serde(default = "default_run_events_limit")]
    pub limit: usize,
}

fn default_run_limit() -> usize {
    50
}

fn default_run_events_limit() -> usize {
    200
}

pub async fn list_runs(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    axum::extract::Query(query): axum::extract::Query<ListRunsQuery>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, Json<ErrorResponse>)> {
    let db = state.db.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "database not available".into(),
            }),
        )
    })?;

    let runs = db.list_user_runs(&user.user_id, query.limit, query.offset);
    Ok(Json(runs))
}

pub async fn get_run(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let db = state.db.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "database not available".into(),
            }),
        )
    })?;

    let goal = db.get_run_goal(&id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "run not found".into(),
            }),
        )
    })?;

    // Verify the requesting user owns this run
    if !db.verify_run_owner(&id, &user.user_id) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "access denied: run belongs to another user".into(),
            }),
        ));
    }

    let steps = build_run_step_payloads(db, &id);
    let graph = build_run_graph_payload(db, &id, &steps);
    let (task_id, group_id, conversation_id) =
        db.get_run_binding(&id).unwrap_or((None, None, None));

    Ok(Json(serde_json::json!({
        "id": id,
        "goal": goal,
        "task_id": task_id,
        "group_id": group_id,
        "conversation_id": conversation_id,
        "steps": steps,
        "graph": graph,
    })))
}

pub async fn get_run_events(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<ListRunEventsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let db = state.db.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "database not available".into(),
            }),
        )
    })?;

    if db.get_run_goal(&id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "run not found".into(),
            }),
        ));
    }

    if !db.verify_run_owner(&id, &user.user_id) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "access denied: run belongs to another user".into(),
            }),
        ));
    }

    let events = db.list_run_operations_events(&id, query.limit);
    Ok(Json(serde_json::json!({
        "run_id": id,
        "events": events,
    })))
}

pub async fn get_verifier_report(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    axum::extract::Path((run_id, step_id, report_id)): axum::extract::Path<(
        String,
        String,
        String,
    )>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let db = state.db.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "database not available".into(),
            }),
        )
    })?;

    let report = db
        .get_verifier_report_for_run_step(&user.user_id, &run_id, &step_id, &report_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "verifier report not found".into(),
                }),
            )
        })?;

    Ok(Json(report))
}

/// The receipt for a step: the verdict, and the checks that produced it.
///
/// Distinct from `get_verifier_report` above, which serves the legacy
/// worker-reported evidence. This one serves verdicts Cortex executed itself,
/// which is the thing a charge is actually bound to.
pub async fn get_receipt(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    axum::extract::Path((run_id, step_id)): axum::extract::Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let db = state.db.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "database not available".into(),
            }),
        )
    })?;

    // Ownership first, and a non-owner gets the same 404 as a missing receipt
    // rather than a 403 — otherwise the status code itself reveals which run
    // ids exist.
    let not_found = || {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "receipt not found".into(),
            }),
        )
    };
    match db.get_run_user_id(&run_id) {
        Some(owner) if owner == user.user_id => {}
        _ => return Err(not_found()),
    }

    let receipt = db.get_receipt(&run_id, &step_id).ok_or_else(not_found)?;

    serde_json::to_value(&receipt).map(Json).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("could not serialize receipt: {e}"),
            }),
        )
    })
}

pub async fn get_ledger(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    // When a DB is available, return user-scoped decisions from the DB.
    // The file-based ledger doesn't carry user_id, so DB decisions are
    // the proper source for multi-user isolation.
    if let Some(db) = &state.db {
        let decisions = db.list_decisions(50, Some(&user.user_id));
        return Ok(Json(serde_json::json!(decisions)));
    }

    // Fallback to file-based ledger (single-user / local dev mode)
    let entries = state.ledger.recent(50).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;
    Ok(Json(serde_json::json!(entries)))
}

// --- PR creation ---

#[derive(Deserialize)]
pub struct CreatePrRequest {
    /// PR title. Defaults to the run's goal.
    pub title: Option<String>,
    /// Base branch to merge into. Defaults to "main".
    #[serde(default = "default_base_branch")]
    pub base: String,
}

fn default_base_branch() -> String {
    "main".to_string()
}

#[derive(Serialize)]
pub struct CreatePrResponse {
    pub pr_url: String,
    pub branch: String,
}

/// Build a rich PR body with step summaries, files changed, cost, and duration.
fn build_pr_body(
    run_id: &str,
    goal: &str,
    branch: &str,
    db: &crate::db::Database,
    authority_scope_id: Option<&str>,
    is_failed: bool,
    failed_checks: &[String],
) -> String {
    let steps = db.get_all_step_statuses(run_id);
    let mut body =
        format!("## Cortex Run `{run_id}`\n\n**Goal:** {goal}\n\n**Branch:** `{branch}`\n");

    if is_failed {
        let checks = if failed_checks.is_empty() {
            "check names unavailable; see the verification receipt".to_string()
        } else {
            failed_checks.join(", ")
        };
        body.push_str(&format!(
            "\n> **Delivered as a draft with failed checks.** This attempt failed verification; the following checks did not pass: {checks}. The work is still delivered — review before merging.\n",
        ));
    }

    // Step summary table
    if !steps.is_empty() {
        body.push_str("\n### Steps\n\n");
        body.push_str("| # | Kind | Objective | Status |\n");
        body.push_str("|---|------|-----------|--------|\n");

        let mut all_files: Vec<String> = Vec::new();

        for (i, (step_id, status)) in steps.iter().enumerate() {
            let (kind, _work_kind, _tier, _risk, objective) = db
                .get_step_details(step_id)
                .unwrap_or_else(|| ("unknown".into(), "".into(), "".into(), "".into(), "".into()));

            let status_icon = match status.as_str() {
                "completed" => "done",
                "failed" => "FAILED",
                "running" => "running",
                _ => status.as_str(),
            };

            body.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                i + 1,
                kind,
                truncate_str(&objective, 60),
                status_icon,
            ));

            // Collect files changed per step
            if let Some(files_json) = db.get_step_files_changed(step_id) {
                if let Ok(files) = serde_json::from_str::<Vec<String>>(&files_json) {
                    for f in files {
                        if !all_files.contains(&f) {
                            all_files.push(f);
                        }
                    }
                }
            }
        }

        // Files changed
        if !all_files.is_empty() {
            body.push_str(&format!("\n### Files Changed ({})\n\n", all_files.len()));
            // Show up to 30 files, then summarize
            let show = all_files.len().min(30);
            for f in &all_files[..show] {
                body.push_str(&format!("- `{f}`\n"));
            }
            if all_files.len() > 30 {
                body.push_str(&format!("\n...and {} more files\n", all_files.len() - 30,));
            }
        }
    }

    // Run timing — query created_at and finished_at from the runs table
    if let Some(run_info) = db.list_user_runs_by_id(run_id) {
        if let (Some(started), Some(finished)) = (
            run_info.get("started_at").and_then(|v| v.as_i64()),
            run_info.get("finished_at").and_then(|v| v.as_i64()),
        ) {
            let duration_secs = (finished - started) / 1000;
            let mins = duration_secs / 60;
            let secs = duration_secs % 60;
            body.push_str(&format!("\n**Duration:** {mins}m {secs}s\n"));
        }
    }

    // --- Provenance section ---
    body.push_str("\n### Provenance\n\n");
    body.push_str(&format!("- **Run ID:** `{run_id}`\n"));
    body.push_str(&format!("- **Cortex Link:** `cortex://runs/{run_id}`\n"));

    // Authority scope (if present on the run)
    if let Some(scope_id) = authority_scope_id {
        body.push_str(&format!("- **Authority Scope:** `{scope_id}`\n"));
    }

    // Step summary counts
    if !steps.is_empty() {
        let total = steps.len();
        let passed = steps.iter().filter(|(_, s)| s == "completed").count();
        let failed = steps.iter().filter(|(_, s)| s == "failed").count();
        body.push_str(&format!(
            "- **Steps:** {total} total, {passed} passed, {failed} failed\n"
        ));
    }

    // Verified by Cortex badge — check if any step has verified evidence
    let has_evidence = steps.iter().any(|(step_id, _)| {
        db.get_latest_verifier_report(step_id)
            .map(|r| r.is_verified_success())
            .unwrap_or(false)
    });
    if has_evidence {
        body.push_str(
            "\n> **Verified by Cortex** — this PR includes steps with verified evidence.\n",
        );
    }

    body.push_str("\n---\n*Automated PR created by [Cortex](https://github.com/cortex)*\n");
    body
}

/// The `[failed checks]` title prefix a draft deliverable for a sealed
/// `Failed` verdict gets. No code in this repo creates GitHub labels
/// (checked: nothing here calls the labels API), so the title prefix is the
/// visible "failed checks" marker instead of a label.
fn failed_pr_title(title: String, is_failed: bool) -> String {
    if is_failed {
        format!("[failed checks] {title}")
    } else {
        title
    }
}

/// Derive the PR title and draft flag together from whether the run has a
/// failed step: `create_pr_core` uses this single decision point rather than
/// letting the title prefix and the draft flag drift apart.
fn pr_title_and_draft(base_title: String, is_failed: bool) -> (String, bool) {
    (failed_pr_title(base_title, is_failed), is_failed)
}

/// Build the `gh pr create` argv for the CLI fallback path. Pulled out so the
/// draft flag's wiring can be asserted directly, without shelling out to `gh`.
fn gh_pr_create_args(title: &str, body: &str, base: &str, head: &str, draft: bool) -> Vec<String> {
    let mut args = vec![
        "pr".to_string(),
        "create".to_string(),
        "--title".to_string(),
        title.to_string(),
        "--body".to_string(),
        body.to_string(),
        "--base".to_string(),
        base.to_string(),
        "--head".to_string(),
        head.to_string(),
    ];
    if draft {
        args.push("--draft".to_string());
    }
    args
}

/// Truncate a string, appending "..." if it exceeds `max_len`.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

/// The checks `create_pr_core` needs before it may push or open anything:
/// the run exists, the requesting user owns it, PR authority is granted, and
/// the run actually produced a branch. Split out so the `open_pr` agent tool
/// (`agent_tools.rs`, via `chat_paid.rs`'s confirm loop) can run exactly this
/// validation *before* a `Risk::Confirm` proposal is ever written to
/// `agent_pending_actions` — refusing up front, with no pending row created,
/// rather than only discovering the run can't be PR'd after the user has
/// already tapped Confirm.
///
/// A run with a sealed `Failed` verdict passes this check — it is still
/// delivered, just as a draft PR (see `create_pr_core`). Only a step still
/// mid-verification is refused here, since there is nothing sealed yet to
/// deliver either way.
///
/// Returns `(goal, branch)` on success — both are needed to build the PR
/// title/body.
pub(crate) fn validate_run_for_pr(
    db: &crate::db::Database,
    user_id: &str,
    run_id: &str,
) -> Result<(String, String), (StatusCode, Json<ErrorResponse>)> {
    // Verify the run exists
    let goal = db.get_run_goal(run_id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "run not found".into(),
            }),
        )
    })?;

    // Verify the requesting user owns this run
    if !db.verify_run_owner(run_id, user_id) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "access denied: run belongs to another user".into(),
            }),
        ));
    }

    validate_pr_authority(db, user_id, run_id)?;

    // A run whose latest sealed verdict for any step is `Failed` is still
    // delivered — the customer paid for the calls the attempt used and gets
    // the work either way. `create_pr_core` opens it as a draft PR titled
    // with a `[failed checks]` prefix instead of refusing it outright; see
    // `run_has_failed_step` there. Only a step still mid-verification (below)
    // is withheld, because it could still land on Failed or Verified and
    // there is nothing to deliver yet either way.
    if db.run_has_pending_verification(run_id) {
        return Err((
            StatusCode::CONFLICT,
            Json(ErrorResponse {
                error: "verification_pending: this run's verification has not finished yet; try again once it completes.".into(),
            }),
        ));
    }

    // Get the branch
    let branch = db.get_run_branch(run_id).ok_or_else(|| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ErrorResponse {
                error: "run has no branch — no changes were made".into(),
            }),
        )
    })?;

    Ok((goal, branch))
}

/// The shared core behind `POST /api/runs/{id}/pr` and the `open_pr` agent
/// tool's confirm handler (`crate::agent_confirm`): validate, push the run's
/// branch, and create the GitHub PR. Tries the GitHub API first (if
/// `GITHUB_TOKEN` is set), falls back to `gh` CLI.
pub(crate) async fn create_pr_core(
    state: &AppState,
    db: &crate::db::Database,
    user_id: &str,
    run_id: &str,
    title: Option<String>,
    base: &str,
) -> Result<CreatePrResponse, (StatusCode, Json<ErrorResponse>)> {
    let (goal, branch) = validate_run_for_pr(db, user_id, run_id)?;

    // A sealed `Failed` verdict on any step still gets delivered, just as a
    // draft PR clearly marked as such — the customer paid for the calls the
    // attempt used and gets the work either way. `validate_run_for_pr` above
    // already refused a run with a step still mid-verification, so a `false`
    // here means every step that has a sealed verdict at all came back
    // `Verified` (or had no verification requested).
    let is_failed = db.run_has_failed_step(run_id);
    let failed_checks = db.run_failed_check_names(run_id);

    // Push the branch to origin
    let push_output = std::process::Command::new("git")
        .args(["push", "-u", "origin", &branch])
        .current_dir(&state.workspace_dir)
        .output()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("failed to run git push: {e}"),
                }),
            )
        })?;

    if !push_output.status.success() {
        let stderr = String::from_utf8_lossy(&push_output.stderr);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("git push failed: {stderr}"),
            }),
        ));
    }

    // Build PR metadata
    let (title, draft) = pr_title_and_draft(
        title.unwrap_or_else(|| format!("cortex: {goal}")),
        is_failed,
    );
    let authority_scope_id = db
        .get_run_pr_authority_context(run_id, user_id)
        .and_then(|ctx| ctx.authority_scope_id);
    let body = build_pr_body(
        run_id,
        &goal,
        &branch,
        db,
        authority_scope_id.as_deref(),
        is_failed,
        &failed_checks,
    );

    // Try GitHub API first, fall back to gh CLI
    if let Some(gh_client) = &state.github_client {
        if let Some((owner, repo)) = github::parse_github_remote(&state.workspace_dir) {
            match gh_client
                .create_pull_request(&owner, &repo, &title, &body, &branch, base, draft)
                .await
            {
                Ok(pr) => {
                    tracing::info!("PR #{} created via GitHub API: {}", pr.number, pr.html_url);
                    return Ok(CreatePrResponse {
                        pr_url: pr.html_url,
                        branch,
                    });
                }
                Err(e) => {
                    tracing::warn!("GitHub API PR creation failed, falling back to gh CLI: {e}");
                    // Fall through to gh CLI below
                }
            }
        } else {
            tracing::warn!("could not parse owner/repo from git remote, falling back to gh CLI");
        }
    }

    // Fallback: create PR via gh CLI
    let gh_args = gh_pr_create_args(&title, &body, base, &branch, draft);
    let pr_output = std::process::Command::new("gh")
        .args(&gh_args)
        .current_dir(&state.workspace_dir)
        .output()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("failed to run gh pr create: {e}"),
                }),
            )
        })?;

    if !pr_output.status.success() {
        let stderr = String::from_utf8_lossy(&pr_output.stderr);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("gh pr create failed: {stderr}"),
            }),
        ));
    }

    let pr_url = String::from_utf8_lossy(&pr_output.stdout)
        .trim()
        .to_string();

    Ok(CreatePrResponse { pr_url, branch })
}

/// `POST /api/runs/{id}/pr` — push the run's branch and create a GitHub PR.
/// See `create_pr_core` for the actual work; this handler is just the HTTP
/// extractor shell shared with the `open_pr` agent tool.
pub async fn create_pr(
    State(state): State<Arc<AppState>>,
    user: PremiumUser,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<CreatePrRequest>,
) -> Result<Json<CreatePrResponse>, (StatusCode, Json<ErrorResponse>)> {
    let db = state.db.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "database not available".into(),
            }),
        )
    })?;
    let result = create_pr_core(&state, db, &user.user_id, &id, req.title, &req.base).await?;
    Ok(Json(result))
}

// --- Cancel run ---

#[derive(Deserialize, Default)]
pub struct CancelRunRequest {
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Serialize)]
pub struct CancelRunResponse {
    pub run_id: String,
    pub status: String,
    pub already_terminal: bool,
    pub cancelled_steps: usize,
    pub signalled_steps: usize,
}

/// Stop a run the caller owns. Uses the plain signed-in user extractor
/// (`ClerkUser`), not `PremiumUser` — a lapsed subscriber still needs to be
/// able to stop their own spend, cancel button or not.
pub async fn cancel_run(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    axum::extract::Path(id): axum::extract::Path<String>,
    body: Option<Json<CancelRunRequest>>,
) -> Result<Json<CancelRunResponse>, (StatusCode, Json<ErrorResponse>)> {
    let db = state.db.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "database not available".into(),
            }),
        )
    })?;

    let reason = body
        .and_then(|Json(req)| req.reason)
        .filter(|r| !r.trim().is_empty())
        .unwrap_or_else(|| "cancelled by user".to_string());

    let outcome = db
        .cancel_run(&id, &user.user_id, &reason)
        .map_err(|err| match err {
            crate::db::CancelError::NotFound => (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "run not found".into(),
                }),
            ),
            crate::db::CancelError::Internal(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error: err }),
            ),
        })?;

    if outcome.already_terminal {
        let status = db
            .list_user_runs_by_id(&id)
            .and_then(|run| {
                run.get("status")
                    .and_then(|s| s.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "unknown".to_string());
        return Ok(Json(CancelRunResponse {
            run_id: id,
            status,
            already_terminal: true,
            cancelled_steps: 0,
            signalled_steps: 0,
        }));
    }

    // Signal every worker that was mid-flight on this run, record what it had
    // already spent, and drop its SSE sender so nothing keeps writing to a
    // step that no longer exists as far as the caller is concerned.
    let in_flight_ids: Vec<String> = outcome
        .in_flight
        .iter()
        .map(|(step_id, _, _)| step_id.clone())
        .collect();
    let mut signalled_steps = 0usize;
    {
        let workers = state.workers.read().await;
        for (step_id, assigned_worker, lease_gen) in &outcome.in_flight {
            let mut signalled = false;
            if let Some(worker) = assigned_worker.as_deref().and_then(|w| workers.get(w)) {
                if worker
                    .tx
                    .send(cortex_core::protocol::BrainMessage::CancelStep {
                        step_id: step_id.clone(),
                        reason: reason.clone(),
                    })
                    .await
                    .is_ok()
                {
                    signalled_steps += 1;
                    signalled = true;
                }
            }
            // A signalled worker is still assigned to this step (cancel_run
            // leaves `assigned_worker` in place for exactly this reason), so
            // its late StepCompleted/StepFailed report is accepted by the ws
            // quiet path and carries the real token counts — recording usage
            // here too would double it. Only record here when the worker
            // could not be signalled (not connected, or the send failed): it
            // will never report back, so this is the only place its partial
            // usage (provider, model, duration; no token counts available
            // from the route) is ever captured.
            if !signalled {
                crate::ws::record_step_usage(
                    db,
                    step_id,
                    *lease_gen,
                    Some(&user.user_id),
                    None,
                    None,
                );
            }
        }
    }
    for step_id in &in_flight_ids {
        state.remove_step_sender(step_id).await;
    }

    state
        .emit_scheduler_event(cortex_engine::captain::SchedulerEvent::RunCancelled {
            run_id: id.clone(),
            in_flight: in_flight_ids,
        })
        .await;
    state
        .emit_mc_event(
            &user.user_id,
            crate::mission_control::MissionControlEvent::RunCompleted {
                run_id: id.clone(),
                status: "cancelled".to_string(),
                total_cost: None,
            },
        )
        .await;

    Ok(Json(CancelRunResponse {
        run_id: id,
        status: "cancelled".to_string(),
        already_terminal: false,
        cancelled_steps: outcome.cancelled_steps.len(),
        signalled_steps,
    }))
}

// --- Cost Projection ---

/// Map a step kind string to the default provider to use for estimation.
fn default_provider_for_tier(tier: &str) -> &'static str {
    match tier {
        "search" => "claude",
        "execute" => "claude",
        "think" => "claude",
        _ => "claude",
    }
}

/// Fallback token estimates when no historical data exists.
fn fallback_tokens(kind: &str) -> (i64, i64, i64) {
    // (tokens_in, tokens_out, duration_ms)
    match kind {
        "search" => (1_000, 500, 15_000),
        "execute" => (5_000, 3_000, 120_000),
        "think" | "review" => (8_000, 5_000, 180_000),
        "test" | "build" | "lint" => (3_000, 2_000, 60_000),
        "gate" => (2_000, 1_000, 30_000),
        "heal" => (5_000, 3_000, 120_000),
        _ => (3_000, 2_000, 60_000),
    }
}

/// Project the cost of a run without executing it. Shared by the
/// `POST /api/runs/estimate` route and the `run_estimate` agent tool
/// (`agent_tools.rs`) so the two never compute two different numbers for the
/// same goal.
pub(crate) fn estimate_run_projection(
    db: &crate::db::Database,
    user_id: &str,
    goal: &str,
    file_paths: &[String],
    profile: &str,
) -> Result<CostProjection, String> {
    let file_paths = crate::validate::sanitize_file_paths(file_paths)?;
    // Decompose the goal into steps (same as create_run)
    let builder = decompose_goal(user_id, goal, &file_paths, profile)?;

    let mut step_estimates = Vec::new();
    let mut total_confidence_sum = 0.0_f64;

    for step in builder.steps() {
        let kind = step.kind.as_str();
        let tier = &step.tier;
        let provider = default_provider_for_tier(tier);

        // Try historical data first, fall back to defaults
        let (tokens_in, tokens_out, duration_ms, confidence) =
            if let Some((avg_in, avg_out, avg_dur, sample_count)) =
                db.get_historical_step_costs(user_id, tier, provider)
            {
                // Confidence: min(1.0, sample_count / 10) — 10+ samples = full confidence
                let conf = (sample_count as f64 / 10.0).min(1.0);
                (avg_in, avg_out, avg_dur, conf)
            } else {
                let (fb_in, fb_out, fb_dur) = fallback_tokens(kind);
                (fb_in, fb_out, fb_dur, 0.0)
            };

        let cost = estimate_cost_by_provider(provider, tokens_in, tokens_out);
        total_confidence_sum += confidence;

        step_estimates.push(StepCostEstimate {
            kind: kind.to_string(),
            tier: tier.clone(),
            provider: provider.to_string(),
            estimated_tokens_in: tokens_in,
            estimated_tokens_out: tokens_out,
            estimated_cost: cost,
            estimated_duration_ms: duration_ms,
        });
    }

    let step_count = step_estimates.len();
    let estimated_total_cost: f64 = step_estimates.iter().map(|s| s.estimated_cost).sum();
    let total_duration_ms: i64 = step_estimates.iter().map(|s| s.estimated_duration_ms).sum();
    let estimated_duration_minutes = total_duration_ms as f64 / 60_000.0;
    let confidence = if step_count > 0 {
        total_confidence_sum / step_count as f64
    } else {
        0.0
    };

    Ok(CostProjection {
        estimated_total_cost,
        estimated_duration_minutes,
        step_estimates,
        confidence,
    })
}

/// `POST /api/runs/estimate` — project the cost of a run without executing it.
pub async fn estimate_run(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Json(req): Json<CreateRunRequest>,
) -> Result<Json<CostProjection>, (StatusCode, Json<ErrorResponse>)> {
    let db = state.db.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "database not available".into(),
            }),
        )
    })?;

    let projection =
        estimate_run_projection(db, &user.user_id, &req.goal, &req.file_paths, &req.profile)
            .map_err(|e| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e })))?;

    Ok(Json(projection))
}

// --- Deployment Capability Adapters ---

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DeploymentAdapter {
    pub id: String,
    pub user_id: String,
    pub adapter_type: String,
    pub environment: String,
    pub config_json: serde_json::Value,
    pub status: String,
    pub last_inspected_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DeploymentStatus {
    pub adapter_type: String,
    pub environment: String,
    pub status: String,
    pub commit_sha: Option<String>,
    pub deployed_at: Option<i64>,
    pub health_check_url: Option<String>,
    pub drift_detected: bool,
}

/// Inspect the deployment status for a given adapter.
///
/// For `github_actions` and `cloudflare_pages`, extracts status from the adapter
/// config. Other adapter types return a placeholder status.
fn inspect_deployment_status(adapter: &DeploymentAdapter) -> DeploymentStatus {
    let config = &adapter.config_json;

    match adapter.adapter_type.as_str() {
        "github_actions" => DeploymentStatus {
            adapter_type: adapter.adapter_type.clone(),
            environment: adapter.environment.clone(),
            status: config
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            commit_sha: config
                .get("commit_sha")
                .and_then(|v| v.as_str())
                .map(String::from),
            deployed_at: config.get("deployed_at").and_then(|v| v.as_i64()),
            health_check_url: config
                .get("health_check_url")
                .and_then(|v| v.as_str())
                .map(String::from),
            drift_detected: config
                .get("drift_detected")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        },
        "cloudflare_pages" => DeploymentStatus {
            adapter_type: adapter.adapter_type.clone(),
            environment: adapter.environment.clone(),
            status: config
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            commit_sha: config
                .get("commit_sha")
                .and_then(|v| v.as_str())
                .map(String::from),
            deployed_at: config.get("deployed_at").and_then(|v| v.as_i64()),
            health_check_url: config
                .get("health_check_url")
                .and_then(|v| v.as_str())
                .map(String::from),
            drift_detected: config
                .get("drift_detected")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        },
        _ => DeploymentStatus {
            adapter_type: adapter.adapter_type.clone(),
            environment: adapter.environment.clone(),
            status: "unsupported".to_string(),
            commit_sha: None,
            deployed_at: None,
            health_check_url: None,
            drift_detected: false,
        },
    }
}

/// `GET /api/deployment-adapters` — list all configured deployment adapters for the user.
pub async fn get_deployment_adapters(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let db = state.db.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "database not available".into(),
            }),
        )
    })?;

    let adapters = db.list_deployment_adapters(&user.user_id);
    let statuses: Vec<serde_json::Value> = adapters
        .iter()
        .map(|adapter| {
            let status = inspect_deployment_status(adapter);
            serde_json::json!({
                "adapter": {
                    "id": adapter.id,
                    "adapter_type": adapter.adapter_type,
                    "environment": adapter.environment,
                    "status": adapter.status,
                    "last_inspected_at": adapter.last_inspected_at,
                    "created_at": adapter.created_at,
                    "updated_at": adapter.updated_at,
                },
                "deployment_status": {
                    "adapter_type": status.adapter_type,
                    "environment": status.environment,
                    "status": status.status,
                    "commit_sha": status.commit_sha,
                    "deployed_at": status.deployed_at,
                    "health_check_url": status.health_check_url,
                    "drift_detected": status.drift_detected,
                },
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "adapters": statuses,
    })))
}

#[cfg(test)]
mod validate_run_for_pr_tests {
    use super::*;
    use crate::db::{Database, ResourceLeaseRequest};
    use cortex_core::verification::{
        CheckExecution, CheckOutcome, CheckSource, CheckSpec, Verdict,
    };

    /// Real `AppState` (own tempdir, own sqlite database) -- same shortcut
    /// `admin::provider_holds_tests::test_state` and
    /// `agent_confirm::tests::test_state` use.
    async fn test_state() -> (tempfile::TempDir, std::sync::Arc<AppState>) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".cortex")).unwrap();
        let state = AppState::new(
            dir.path().join(".cortex/ledger.jsonl"),
            dir.path().to_path_buf(),
            None,
        )
        .await;
        (dir, state)
    }

    /// Guards a whole test body while HEYVERA_ENV=production is set, and
    /// always clears it afterward (even on panic) -- this is the only test
    /// in this binary that depends on `is_production_env`, so the window is
    /// scoped as tightly as possible around the single call under test.
    struct ProductionEnvGuard;

    impl ProductionEnvGuard {
        fn set() -> Self {
            std::env::set_var("HEYVERA_ENV", "production");
            ProductionEnvGuard
        }
    }

    impl Drop for ProductionEnvGuard {
        fn drop(&mut self) {
            std::env::remove_var("HEYVERA_ENV");
        }
    }

    /// Item F of the M-D-0024 settlement redesign: in production, with no
    /// usable provider gateway, `create_run` must refuse before a run row
    /// even exists -- a customer must never be told a run is under way when
    /// nothing behind it can make a priced call. Calls the handler directly
    /// (no HTTP, no Clerk) since `PremiumUser` is a plain constructible
    /// struct outside of axum's extractor machinery.
    #[tokio::test]
    async fn create_run_in_production_without_a_gateway_returns_service_unavailable() {
        let (_dir, state) = test_state().await;
        let _guard = ProductionEnvGuard::set();
        assert!(
            crate::is_production_env(),
            "test setup: HEYVERA_ENV=production must be visible to is_production_env"
        );
        assert!(
            !crate::provider_gateway_http::is_gateway_on(),
            "test setup: no supplier keys are configured, so the gateway must be off"
        );

        let user = PremiumUser {
            user_id: "user-1".to_string(),
        };
        let req = CreateRunRequest {
            goal: "ship it".to_string(),
            profile: "auto".to_string(),
            file_paths: vec![],
            repo_key: None,
            task_id: None,
            group_id: None,
            conversation_id: None,
            authority_scope_id: None,
            authority_handoff_id: None,
            authority_reason: None,
        };

        let result = create_run(State(state), user, Json(req)).await;
        let Err((status, body)) = result else {
            panic!("a production run with no gateway must be refused, not accepted");
        };
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            body.0.error,
            cortex_core::billing_binding::GATEWAY_DOWN_MESSAGE
        );
    }

    fn write_lease(path: &str) -> ResourceLeaseRequest {
        ResourceLeaseRequest {
            resource_type: "path".to_string(),
            repo_key: "default".to_string(),
            resource_key: path.to_string(),
            mode: "write".to_string(),
            reason: Some("test".to_string()),
            metadata: serde_json::json!({}),
        }
    }

    /// A run with one step under it, a write lease so `validate_pr_authority`
    /// clears (the personal-scope authority check is otherwise auto-granted
    /// by `get_authority_scope_for_user`), and a recorded branch so the run
    /// looks like one that actually produced changes.
    fn make_run(db: &Database, user_id: &str, step_id: &str) -> String {
        let run_id = db
            .create_run_with_steps_and_resource_leases(
                user_id,
                "do the thing",
                "auto",
                &["src/lib.rs".to_string()],
                None,
                None,
                None,
                &[write_lease("src/lib.rs")],
                &[(
                    step_id.to_string(),
                    "execute".to_string(),
                    "modify".to_string(),
                    None,
                    "standard".to_string(),
                    "low".to_string(),
                    "o".to_string(),
                    0,
                )],
                &[],
            )
            .expect("create run");
        db.record_run_branch(&run_id, "cortex/do-the-thing");
        run_id
    }

    fn verdict_spec(id: &str) -> CheckSpec {
        CheckSpec {
            id: id.to_string(),
            source: CheckSource::Contract,
            command: vec!["cargo".into(), "test".into()],
            timeout_secs: 60,
            required: true,
        }
    }

    fn verdict_execution(spec_id: &str, outcome: CheckOutcome) -> CheckExecution {
        CheckExecution {
            spec_id: spec_id.to_string(),
            exit_code: if matches!(outcome, CheckOutcome::Passed) {
                Some(0)
            } else {
                Some(1)
            },
            outcome,
            duration_ms: 1200,
            output_digest: "sha256:deadbeef".to_string(),
            output_tail: "ok".to_string(),
            runner_image: "cortex/runner@sha256:abc".to_string(),
        }
    }

    /// Seals `attempt` for `step_id` with the given verdict, driving the real
    /// claim/record/finish path rather than hand-seeding ledger rows. Billing
    /// is deliberately not touched here: a `Failed` verdict is unbilled in
    /// production (`billing_binding::BillingEffect::None`), and the gate
    /// under test must not depend on billing state either way.
    fn seal_verification_attempt(
        db: &Database,
        run_id: &str,
        step_id: &str,
        attempt: i64,
        verdict: Verdict,
    ) -> String {
        let specs = vec![verdict_spec("check-1")];
        db.save_check_specs(run_id, step_id, &specs)
            .expect("freeze specs");
        let vid = db
            .claim_verification(run_id, step_id, attempt, "tree-hash", "img@sha256:1")
            .expect("claim verification");
        let outcome = if matches!(verdict, Verdict::Failed) {
            CheckOutcome::Failed
        } else {
            CheckOutcome::Passed
        };
        db.record_check_execution(&vid, &specs[0], &verdict_execution("check-1", outcome))
            .expect("record execution");
        db.finish_verification(&vid, verdict).expect("seal");
        vid
    }

    fn seal_verification(db: &Database, run_id: &str, step_id: &str, verdict: Verdict) -> String {
        seal_verification_attempt(db, run_id, step_id, 1, verdict)
    }

    /// Sets a step's status directly, bypassing the lifecycle CAS —
    /// tests use this to plant a step in whatever state (`verifying`,
    /// `recovered`, `cancelled`, ...) a scenario needs without wiring up a
    /// whole delivery, heal, or cancellation path.
    fn set_step_status(db: &Database, step_id: &str, status: &str) {
        db.conn()
            .execute(
                "UPDATE steps SET status = ?1 WHERE id = ?2",
                rusqlite::params![status, step_id],
            )
            .expect("set step status");
    }

    /// The owner's settled rule: a customer pays for the calls a failed
    /// attempt used and receives the work as a draft PR labelled "failed
    /// checks" — `validate_run_for_pr` must not refuse a sealed `Failed`
    /// verdict, and `create_pr_core`'s draft/title/body logic (exercised here
    /// via its building blocks, since the real function pushes to git and
    /// calls out to GitHub) must mark the deliverable as such.
    #[tokio::test]
    async fn a_failed_run_is_delivered_as_a_draft_marked_failed_checks() {
        let (_dir, state) = test_state().await;
        let db = state.db.as_ref().unwrap();
        let run_id = make_run(db, "user-1", "step-1");
        seal_verification(db, &run_id, "step-1", Verdict::Failed);

        let (goal, branch) = match validate_run_for_pr(db, "user-1", &run_id) {
            Ok(ok) => ok,
            Err((status, body)) => panic!(
                "a failed run must still be delivered: {status} {}",
                body.0.error
            ),
        };
        assert_eq!(branch, "cortex/do-the-thing");

        assert!(db.run_has_failed_step(&run_id));
        let failed_checks = db.run_failed_check_names(&run_id);
        assert_eq!(failed_checks, vec!["check-1".to_string()]);

        let title = failed_pr_title(format!("cortex: {goal}"), db.run_has_failed_step(&run_id));
        assert!(title.starts_with("[failed checks] "), "got: {title}");

        let body = build_pr_body(&run_id, &goal, &branch, db, None, true, &failed_checks);
        assert!(
            body.contains("Delivered as a draft with failed checks") && body.contains("check-1"),
            "body must name the failed checks: {body}"
        );
    }

    /// When a step failed but no check names were recorded (e.g. an
    /// execution-level failure with nothing sealed to name), the body must
    /// still show the draft banner, pointing at the verification receipt
    /// instead of an empty list.
    #[tokio::test]
    async fn a_failed_run_with_no_named_checks_points_at_the_verification_receipt() {
        let (_dir, state) = test_state().await;
        let db = state.db.as_ref().unwrap();
        let run_id = make_run(db, "user-1", "step-1");

        let body = build_pr_body(
            &run_id,
            "do the thing",
            "cortex/do-the-thing",
            db,
            None,
            true,
            &[],
        );
        assert!(
            body.contains("check names unavailable; see the verification receipt"),
            "body must fall back to the receipt pointer when no check names are known: {body}"
        );
    }

    /// `pr_title_and_draft` is the single decision point `create_pr_core`
    /// uses for both the title prefix and the draft flag -- this test fails
    /// if either half of that wiring is removed.
    #[test]
    fn pr_title_and_draft_marks_a_failed_run_as_a_draft_with_the_failed_checks_prefix() {
        let (title, draft) = pr_title_and_draft("cortex: do the thing".to_string(), true);
        assert!(title.starts_with("[failed checks] "), "got: {title}");
        assert!(draft, "a failed run must open as a draft PR");
    }

    #[test]
    fn pr_title_and_draft_leaves_a_verified_run_as_a_normal_pr() {
        let (title, draft) = pr_title_and_draft("cortex: do the thing".to_string(), false);
        assert_eq!(title, "cortex: do the thing");
        assert!(!draft, "a verified run must not open as a draft PR");
    }

    /// `create_pr_core`'s `gh` CLI fallback path -- asserts the exact argv
    /// so a dropped `--draft` push is caught here, not in production.
    #[test]
    fn gh_pr_create_args_includes_draft_flag_for_a_failed_run() {
        let args = gh_pr_create_args(
            "[failed checks] cortex: do the thing",
            "body",
            "main",
            "cortex/do-the-thing",
            true,
        );
        assert!(
            args.iter().any(|a| a == "--draft"),
            "failed run's argv must request a draft PR: {args:?}"
        );
    }

    #[test]
    fn gh_pr_create_args_omits_draft_flag_for_a_verified_run() {
        let args = gh_pr_create_args(
            "cortex: do the thing",
            "body",
            "main",
            "cortex/do-the-thing",
            false,
        );
        assert!(
            !args.iter().any(|a| a == "--draft"),
            "verified run's argv must not request a draft PR: {args:?}"
        );
    }

    #[tokio::test]
    async fn a_verified_run_clears_the_gate() {
        let (_dir, state) = test_state().await;
        let db = state.db.as_ref().unwrap();
        let run_id = make_run(db, "user-1", "step-1");
        // The step is actually mid-verification (`verifying`) right up until
        // its verdict seals, same as in production -- setting this is what
        // makes the assertion below exercise the `delivered`/`verifying`
        // pending filter instead of vacuously passing because the step was
        // never in either status.
        set_step_status(db, "step-1", "verifying");
        seal_verification(db, &run_id, "step-1", Verdict::Verified);

        let (_goal, branch) = match validate_run_for_pr(db, "user-1", &run_id) {
            Ok(ok) => ok,
            Err((status, body)) => {
                panic!(
                    "a verified run must clear the gate: {status} {}",
                    body.0.error
                )
            }
        };
        assert_eq!(branch, "cortex/do-the-thing");
    }

    #[tokio::test]
    async fn a_step_failed_then_retried_to_verified_clears_the_gate() {
        // Latest attempt wins: a step that failed once but was retried to a
        // sealed `Verified` attempt must not withhold the run.
        let (_dir, state) = test_state().await;
        let db = state.db.as_ref().unwrap();
        let run_id = make_run(db, "user-1", "step-1");
        set_step_status(db, "step-1", "verifying");
        seal_verification_attempt(db, &run_id, "step-1", 1, Verdict::Failed);
        seal_verification_attempt(db, &run_id, "step-1", 2, Verdict::Verified);

        let (_goal, branch) = match validate_run_for_pr(db, "user-1", &run_id) {
            Ok(ok) => ok,
            Err((status, body)) => panic!(
                "a retried, now-verified step must clear the gate: {status} {}",
                body.0.error
            ),
        };
        assert_eq!(branch, "cortex/do-the-thing");
    }

    #[tokio::test]
    async fn a_step_mid_verification_is_refused_as_pending_with_409() {
        let (_dir, state) = test_state().await;
        let db = state.db.as_ref().unwrap();
        let run_id = make_run(db, "user-1", "step-1");
        // Specs frozen and an attempt claimed, but never sealed, with the
        // step actually mid-verification (as it is by the time a job can be
        // claimed in production).
        db.save_check_specs(&run_id, "step-1", &[verdict_spec("check-1")])
            .expect("freeze specs");
        set_step_status(db, "step-1", "verifying");
        db.claim_verification(&run_id, "step-1", 1, "tree-hash", "img@sha256:1")
            .expect("claim verification");

        let err = validate_run_for_pr(db, "user-1", &run_id)
            .expect_err("a run mid-verification must not be handed over");
        assert_eq!(err.0, StatusCode::CONFLICT);
        assert!(
            err.1 .0.error.starts_with("verification_pending:"),
            "got: {}",
            err.1 .0.error
        );
    }

    /// F1 regression: after a heal, the original step is flipped to
    /// `recovered` with its frozen specs (from dispatch) still in place and
    /// no receipt ever sealed for it. The retry step id the heal chain
    /// created is what actually gets verified and charged. Before the fix,
    /// the recovered original's frozen-but-unsealed specs read as pending
    /// forever and the run could never get a PR.
    #[tokio::test]
    async fn a_healed_run_clears_the_gate_once_the_retry_is_verified() {
        let (_dir, state) = test_state().await;
        let db = state.db.as_ref().unwrap();
        let run_id = make_run(db, "user-1", "step-original");

        // scheduler.rs freezes specs at dispatch, before the worker runs.
        db.save_check_specs(&run_id, "step-original", &[verdict_spec("check-1")])
            .expect("freeze specs on the original at dispatch");
        set_step_status(db, "step-original", "verifying");

        // The worker reports StepFailed; `mark_step_recovered` requires the
        // step to be `failed` first (as try_heal's caller leaves it after the
        // failure report), then flips it to `recovered` and the heal chain
        // mints a new retry step id -- no verdict is ever sealed for the
        // original.
        set_step_status(db, "step-original", "failed");
        assert!(
            db.mark_step_recovered("step-original"),
            "mark_step_recovered should succeed from `failed`"
        );

        // The heal chain's retry step, sealed Verified.
        db.conn()
            .execute(
                "INSERT INTO steps (id, run_id, kind, tier, risk, objective, created_at, updated_at)
                 VALUES ('step-retry', ?1, 'execute', 'standard', 'low', 'o', 0, 0)",
                rusqlite::params![run_id],
            )
            .expect("insert retry step");
        seal_verification(db, &run_id, "step-retry", Verdict::Verified);

        let (_goal, branch) = match validate_run_for_pr(db, "user-1", &run_id) {
            Ok(ok) => ok,
            Err((status, body)) => panic!(
                "a healed run whose retry is verified must clear the gate: {status} {}",
                body.0.error
            ),
        };
        assert_eq!(branch, "cortex/do-the-thing");
    }

    /// A step cancelled after dispatch keeps its frozen specs too (dispatch
    /// froze them before the cancellation could happen), and must not block
    /// delivery either.
    #[tokio::test]
    async fn a_cancelled_step_with_frozen_specs_does_not_block_the_run() {
        let (_dir, state) = test_state().await;
        let db = state.db.as_ref().unwrap();
        let run_id = make_run(db, "user-1", "step-1");
        db.save_check_specs(&run_id, "step-1", &[verdict_spec("check-1")])
            .expect("freeze specs");
        set_step_status(db, "step-1", "verifying");
        set_step_status(db, "step-1", "cancelled");

        match validate_run_for_pr(db, "user-1", &run_id) {
            Ok(_) => {}
            Err((status, body)) => panic!(
                "a cancelled step with frozen specs must not withhold the PR: {status} {}",
                body.0.error
            ),
        }
    }
}
