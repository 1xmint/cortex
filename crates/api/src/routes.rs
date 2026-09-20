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

#[derive(Serialize)]
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
/// BYOS, Soma) are intentionally absent:
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

    // ── Docker / BYOS (non-critical: API still serves without it) ──────────
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
) -> String {
    let steps = db.get_all_step_statuses(run_id);
    let mut body =
        format!("## Cortex Run `{run_id}`\n\n**Goal:** {goal}\n\n**Branch:** `{branch}`\n");

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

/// Truncate a string, appending "..." if it exceeds `max_len`.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

/// `POST /api/runs/{id}/pr` — push the run's branch and create a GitHub PR.
///
/// Tries the GitHub API first (if `GITHUB_TOKEN` is set), falls back to `gh` CLI.
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

    // Verify the run exists
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

    validate_pr_authority(db, &user.user_id, &id)?;

    // Get the branch
    let branch = db.get_run_branch(&id).ok_or_else(|| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ErrorResponse {
                error: "run has no branch — no changes were made".into(),
            }),
        )
    })?;

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
    let title = req.title.unwrap_or_else(|| format!("cortex: {goal}"));
    let authority_scope_id = db
        .get_run_pr_authority_context(&id, &user.user_id)
        .and_then(|ctx| ctx.authority_scope_id);
    let body = build_pr_body(&id, &goal, &branch, db, authority_scope_id.as_deref());

    // Try GitHub API first, fall back to gh CLI
    if let Some(gh_client) = &state.github_client {
        if let Some((owner, repo)) = github::parse_github_remote(&state.workspace_dir) {
            match gh_client
                .create_pull_request(&owner, &repo, &title, &body, &branch, &req.base)
                .await
            {
                Ok(pr) => {
                    tracing::info!("PR #{} created via GitHub API: {}", pr.number, pr.html_url);
                    return Ok(Json(CreatePrResponse {
                        pr_url: pr.html_url,
                        branch,
                    }));
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
    let pr_output = std::process::Command::new("gh")
        .args([
            "pr", "create", "--title", &title, "--body", &body, "--base", &req.base, "--head",
            &branch,
        ])
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

    Ok(Json(CreatePrResponse { pr_url, branch }))
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

    let file_paths = crate::validate::sanitize_file_paths(&req.file_paths)
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e })))?;
    // Decompose the goal into steps (same as create_run)
    let builder = decompose_goal(&user.user_id, &req.goal, &file_paths, &req.profile)
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e })))?;

    let mut step_estimates = Vec::new();
    let mut total_confidence_sum = 0.0_f64;

    for step in builder.steps() {
        let kind = step.kind.as_str();
        let tier = &step.tier;
        let provider = default_provider_for_tier(tier);

        // Try historical data first, fall back to defaults
        let (tokens_in, tokens_out, duration_ms, confidence) =
            if let Some((avg_in, avg_out, avg_dur, sample_count)) =
                db.get_historical_step_costs(&user.user_id, tier, provider)
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

    Ok(Json(CostProjection {
        estimated_total_cost,
        estimated_duration_minutes,
        step_estimates,
        confidence,
    }))
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
