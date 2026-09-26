use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::clerk::ClerkUser;
use crate::db::{CodeRedemption, PromoCode};
use crate::routes::ErrorResponse;
use crate::run_payload::{build_run_graph_payload, build_run_step_payloads};
use crate::state::AppState;

fn admin_set() -> HashSet<String> {
    let raw = std::env::var("CORTEX_ADMIN_EMAILS")
        .or_else(|_| std::env::var("CORTEX_ADMIN_USERS"))
        .unwrap_or_default();
    raw.split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Pure decision for the local-dev admin bypass: only when the key is
/// absent from BOTH the state and the environment, and the runtime is not
/// production. main.rs turns an empty `CLERK_SECRET_KEY=""` into a `None`
/// state, and that must stay fail-closed here, so the env check is kept.
/// The state check lets tests that build a keyed `AppState` exercise the
/// real path without touching the env. The production check ensures a
/// misconfigured production deploy (keyless, but no CORTEX_AUTH_DISABLED
/// override) still denies admin access rather than granting it.
fn local_dev_admin_bypass(state_keyless: bool, env_key_absent: bool, production: bool) -> bool {
    state_keyless && env_key_absent && !production
}

pub async fn authorize_admin(
    state: &AppState,
    user: &ClerkUser,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let admins = admin_set();
    if admins.is_empty() {
        if local_dev_admin_bypass(
            state.clerk_secret_key.is_none(),
            std::env::var("CLERK_SECRET_KEY").is_err(),
            crate::clerk::is_production_runtime(),
        ) {
            return Ok(());
        }
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "admin access required".into(),
            }),
        ));
    }

    if admins.contains(&user.user_id.to_lowercase()) {
        return Ok(());
    }

    if let Some(clerk_secret) = &state.clerk_secret_key {
        if let Ok(email) = lookup_clerk_email(clerk_secret, &user.user_id).await {
            if admins.contains(&email.to_lowercase()) {
                return Ok(());
            }
        }
    }

    Err((
        StatusCode::FORBIDDEN,
        Json(ErrorResponse {
            error: "admin access required".into(),
        }),
    ))
}

/// Alias for authorize_admin - checks if user has admin privileges and returns Result
pub async fn resolve_admin(
    state: &AppState,
    user: &ClerkUser,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    authorize_admin(state, user).await
}

pub async fn require_admin_middleware(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    request: Request,
    next: Next,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    authorize_admin(&state, &user).await?;
    Ok(next.run(request).await)
}

async fn lookup_clerk_email(clerk_secret: &str, user_id: &str) -> Result<String, String> {
    let url = format!("https://api.clerk.com/v1/users/{user_id}");
    let client = reqwest::Client::new();
    let res = client
        .get(&url)
        .bearer_auth(clerk_secret)
        .send()
        .await
        .map_err(|e| format!("clerk user lookup failed: {e}"))?;

    if !res.status().is_success() {
        return Err(format!("clerk returned {}", res.status()));
    }

    let body: serde_json::Value = res.json().await.map_err(|e| format!("parse error: {e}"))?;

    body.get("email_addresses")
        .and_then(|arr| arr.as_array())
        .and_then(|arr| arr.first())
        .and_then(|obj| obj.get("email_address"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "no email found".into())
}

// --- Worker status ---

pub async fn get_workers(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    authorize_admin(&state, &user).await?;

    let in_memory: Vec<serde_json::Value> = {
        let workers = state.workers.read().await;
        workers
            .values()
            .map(|w| {
                serde_json::json!({
                    "id": w.worker_id,
                    "user_id": w.user_id,
                    "providers": w.available_providers.iter()
                        .map(|p| p.to_string()).collect::<Vec<_>>(),
                    "connected": true,
                })
            })
            .collect()
    };

    let db_workers = state
        .db
        .as_ref()
        .map(|db| db.get_worker_list())
        .unwrap_or_default();

    Ok(Json(serde_json::json!({
        "connected": in_memory,
        "all": db_workers,
        "count": in_memory.len(),
    })))
}

// --- System stats ---

pub async fn system_stats(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    // If using local auth (no real user), return empty stats to prevent frontend crashes
    if user.user_id == "local" && state.clerk_secret_key.is_none() {
        return Ok(Json(serde_json::json!({
            "total_runs": 0,
            "total_steps": 0,
            "workers_connected_live": 0
        })));
    }

    authorize_admin(&state, &user).await?;
    let stats = state
        .db
        .as_ref()
        .map(|db| db.system_stats())
        .unwrap_or_else(|| serde_json::json!({"error": "database not available"}));

    let worker_count = state.workers.read().await.len();

    let mut stats = stats;
    if let Some(obj) = stats.as_object_mut() {
        obj.insert(
            "workers_connected_live".to_string(),
            serde_json::json!(worker_count),
        );
    }

    Ok(Json(stats))
}

// --- Decision transparency ---

#[derive(Deserialize)]
pub struct DecisionQuery {
    #[serde(default = "default_limit")]
    pub limit: usize,
    pub user_id: Option<String>,
}

fn default_limit() -> usize {
    50
}

pub async fn list_decisions(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Query(query): Query<DecisionQuery>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, Json<ErrorResponse>)> {
    authorize_admin(&state, &user).await?;
    let decisions = state
        .db
        .as_ref()
        .map(|db| db.list_decisions(query.limit, query.user_id.as_deref()))
        .unwrap_or_default();

    Ok(Json(decisions))
}

// --- Run listing (all users) ---

#[derive(Deserialize)]
pub struct RunListQuery {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

pub async fn list_all_runs(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Query(query): Query<RunListQuery>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, Json<ErrorResponse>)> {
    authorize_admin(&state, &user).await?;
    let runs = state
        .db
        .as_ref()
        .map(|db| {
            db.list_all_runs(query.limit, query.offset)
                .into_iter()
                .map(|(id, user_id, goal, status, created_at)| {
                    serde_json::json!({
                        "id": id,
                        "user_id": user_id,
                        "goal": goal,
                        "status": status,
                        "created_at": created_at,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Json(runs))
}

// --- Run detail with full step DAG ---

pub async fn get_run_detail(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    authorize_admin(&state, &user).await?;
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

    let profile = db.get_run_profile(&id).unwrap_or_else(|| "auto".into());
    let heal_count = db.get_run_heal_count(&id);
    let step_details = build_run_step_payloads(db, &id);
    let graph = build_run_graph_payload(db, &id, &step_details);

    Ok(Json(serde_json::json!({
        "id": id,
        "goal": goal,
        "profile": profile,
        "heal_attempts": heal_count,
        "steps": step_details,
        "graph": graph,
    })))
}

// --- Provider pressure dashboard ---

pub async fn pressure_dashboard(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> Json<serde_json::Value> {
    let db = match &state.db {
        Some(db) => db,
        None => return Json(serde_json::json!({"error": "database not available"})),
    };

    let window_ms = cortex_core::evaluator::WINDOW_SECS * 1000;
    let raw = db.pressure_for_user(&user.user_id, window_ms);
    let reliability = db.provider_reliability(&user.user_id, 24);

    let pressure: Vec<serde_json::Value> = raw
        .into_iter()
        .map(|(provider, tier, tokens)| {
            let budget =
                cortex_core::evaluator::token_budget(parse_provider(&provider), parse_tier(&tier));
            let ratio = if budget > 0 {
                tokens as f64 / budget as f64
            } else {
                0.0
            };
            let state = cortex_core::evaluator::PressureState::from_ratio(ratio);

            serde_json::json!({
                "provider": provider,
                "tier": tier,
                "tokens_used": tokens,
                "budget": budget,
                "ratio": ratio,
                "state": format!("{:?}", state),
            })
        })
        .collect();

    let reliability_data: Vec<serde_json::Value> = reliability
        .into_iter()
        .map(|(provider, total, successes)| {
            let rate = if total > 0 {
                successes as f64 / total as f64
            } else {
                1.0
            };
            serde_json::json!({
                "provider": provider,
                "total": total,
                "successes": successes,
                "rate": rate,
            })
        })
        .collect();

    Json(serde_json::json!({
        "user_id": user.user_id,
        "window_hours": cortex_core::evaluator::WINDOW_SECS / 3600,
        "pressure": pressure,
        "reliability_24h": reliability_data,
    }))
}

fn parse_provider(s: &str) -> cortex_core::provider::ProviderId {
    match s {
        "claude" => cortex_core::provider::ProviderId::Claude,
        "openai" => cortex_core::provider::ProviderId::Openai,
        "gemini" => cortex_core::provider::ProviderId::Gemini,
        "zen" => cortex_core::provider::ProviderId::Zen,
        _ => cortex_core::provider::ProviderId::Claude,
    }
}

fn parse_tier(s: &str) -> cortex_core::provider::Tier {
    match s {
        "search" => cortex_core::provider::Tier::Search,
        "think" => cortex_core::provider::Tier::Think,
        _ => cortex_core::provider::Tier::Execute,
    }
}

// --- Promo Code Management ---

#[derive(Deserialize)]
pub struct CreatePromoCodeRequest {
    pub code: String,
    pub discount_type: String,
    pub discount_value: f64,
    #[serde(default = "default_max_uses")]
    pub max_uses: i32,
    pub expires_at: Option<String>,
    pub description: Option<String>,
    pub discount_options: Option<Vec<DiscountOption>>,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct DiscountOption {
    pub label: String,
    pub discount_type: String,
    pub discount_value: f64,
}

fn default_max_uses() -> i32 {
    25
}

#[derive(Deserialize)]
pub struct UpdatePromoCodeRequest {
    pub active: Option<bool>,
    pub max_uses: Option<i32>,
    pub expires_at: Option<Option<String>>,
    pub description: Option<Option<String>>,
}

#[derive(Serialize)]
pub struct PromoCodeListResponse {
    pub codes: Vec<PromoCode>,
    pub total: usize,
}

#[derive(Serialize)]
pub struct RedemptionListResponse {
    pub redemptions: Vec<CodeRedemption>,
    pub total: usize,
}

pub async fn create_promo_code(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Json(req): Json<CreatePromoCodeRequest>,
) -> Result<Json<PromoCode>, (StatusCode, Json<ErrorResponse>)> {
    authorize_admin(&state, &user).await?;
    let db = state.db.as_ref().ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: "database unavailable".into(),
        }),
    ))?;

    if req.code.len() < 3 || req.code.len() > 32 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "code must be 3-32 characters".into(),
            }),
        ));
    }
    if !req
        .code
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "code may only contain letters, numbers, hyphens, and underscores".into(),
            }),
        ));
    }
    match req.discount_type.as_str() {
        "trial_extension" | "percent_off" | "free_trial" => {}
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "discount_type must be trial_extension, percent_off, or free_trial"
                        .into(),
                }),
            ));
        }
    }

    let options_json = req
        .discount_options
        .as_ref()
        .map(|opts| serde_json::to_string(opts).unwrap_or_default());
    let promo = db
        .create_promo_code(
            &req.code,
            &req.discount_type,
            req.discount_value,
            req.max_uses,
            req.expires_at.as_deref(),
            &user.user_id,
            req.description.as_deref(),
            options_json.as_deref(),
        )
        .map_err(|e| (StatusCode::CONFLICT, Json(ErrorResponse { error: e })))?;

    tracing::info!("admin {} created promo code {}", user.user_id, promo.code);
    Ok(Json(promo))
}

pub async fn list_promo_codes(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> Result<Json<PromoCodeListResponse>, (StatusCode, Json<ErrorResponse>)> {
    authorize_admin(&state, &user).await?;
    let db = state.db.as_ref().ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: "database unavailable".into(),
        }),
    ))?;
    let codes = db.list_promo_codes();
    let total = codes.len();
    Ok(Json(PromoCodeListResponse { codes, total }))
}

pub async fn update_promo_code(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(id): Path<String>,
    Json(req): Json<UpdatePromoCodeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    authorize_admin(&state, &user).await?;
    let db = state.db.as_ref().ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: "database unavailable".into(),
        }),
    ))?;
    let updated = db.update_promo_code(
        &id,
        req.active,
        req.max_uses,
        req.expires_at.as_ref().map(|e| e.as_deref()),
        req.description.as_ref().map(|d| d.as_deref()),
    );
    if updated {
        tracing::info!("admin {} updated promo code {}", user.user_id, id);
        Ok(Json(serde_json::json!({"updated": true})))
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "promo code not found".into(),
            }),
        ))
    }
}

pub async fn delete_promo_code(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    authorize_admin(&state, &user).await?;
    let db = state.db.as_ref().ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: "database unavailable".into(),
        }),
    ))?;
    if db.delete_promo_code(&id) {
        tracing::info!("admin {} deleted promo code {}", user.user_id, id);
        Ok(Json(serde_json::json!({"deleted": true})))
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "promo code not found".into(),
            }),
        ))
    }
}

// ─── Audit Log ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AuditLogQuery {
    #[serde(default = "default_audit_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
    /// Legacy page-based navigation (overrides offset when present).
    pub page: Option<i64>,
}

fn default_audit_limit() -> i64 {
    50
}

pub async fn get_audit_log(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Query(query): Query<AuditLogQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    resolve_admin(&state, &user).await?;
    let db = state.db.as_ref().ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: "database unavailable".into(),
        }),
    ))?;
    let limit = query.limit.clamp(1, 200);
    let offset = if let Some(page) = query.page {
        page.saturating_sub(1).max(0) * limit
    } else {
        query.offset.max(0)
    };
    let entries = db.get_audit_log(limit, offset);
    let count = entries.len();
    Ok(Json(serde_json::json!({
        "entries": entries,
        "limit": limit,
        "offset": offset,
        "count": count,
    })))
}

#[derive(Deserialize)]
pub struct RedemptionQuery {
    pub code: Option<String>,
}

// ─── Container Monitoring ─────────────────────────────────────────────────────

pub async fn list_containers(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    resolve_admin(&state, &user).await?;
    let db = state.db.as_ref().ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: "database unavailable".into(),
        }),
    ))?;
    let containers = db.list_all_containers();
    let count = containers.len();
    Ok(Json(serde_json::json!({
        "containers": containers,
        "count": count,
    })))
}

pub async fn container_stats(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    resolve_admin(&state, &user).await?;
    let db = state.db.as_ref().ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: "database unavailable".into(),
        }),
    ))?;
    let containers = db.list_all_containers();
    let total = containers.len();
    let running = containers.iter().filter(|c| c.status == "running").count();
    let stopped = containers.iter().filter(|c| c.status != "running").count();
    // Keep Prometheus population gauge fresh on admin views.
    crate::metrics::set_container_population(running as i64, stopped as i64);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let avg_age_secs = if total > 0 {
        containers.iter().map(|c| now - c.created_at).sum::<i64>() / total as i64
    } else {
        0
    };
    Ok(Json(serde_json::json!({
        "total": total,
        "running": running,
        "stopped": stopped,
        "avg_age_secs": avg_age_secs,
    })))
}

pub async fn list_redemptions(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Query(query): Query<RedemptionQuery>,
) -> Result<Json<RedemptionListResponse>, (StatusCode, Json<ErrorResponse>)> {
    authorize_admin(&state, &user).await?;
    let db = state.db.as_ref().ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: "database unavailable".into(),
        }),
    ))?;
    let redemptions = db.list_redemptions(query.code.as_deref());
    let total = redemptions.len();
    Ok(Json(RedemptionListResponse { redemptions, total }))
}

// ─── Provider Gateway Holds ─────────────────────────────────────────────────
//
// Operator visibility into `provider_request_reservations` rows stuck
// `reserved` or `unresolved` (see `crates/api/src/db/provider_gateway.rs`).
// This is read/manual-resolve only — nothing here reconciles against a
// supplier's own usage report; that remains a human decision until a report
// pipeline exists.

#[derive(Deserialize)]
pub struct ProviderHoldsQuery {
    #[serde(default = "default_provider_holds_limit")]
    pub limit: i64,
}

fn default_provider_holds_limit() -> i64 {
    50
}

pub async fn get_provider_holds(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Query(query): Query<ProviderHoldsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    authorize_admin(&state, &user).await?;
    let db = state.db.as_ref().ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: "database unavailable".into(),
        }),
    ))?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let summary = db.provider_holds_summary(now_ms, query.limit);
    Ok(Json(serde_json::json!({
        "reserved_count": summary.reserved_count,
        "reserved_total_micro_usd": summary.reserved_total_micro_usd,
        "unresolved_count": summary.unresolved_count,
        "unresolved_total_micro_usd": summary.unresolved_total_micro_usd,
        "mismatch_count": summary.mismatch_count,
        "oldest_age_ms": summary.oldest_age_ms,
        "funded_total_micro_usd": summary.funded_total_micro_usd,
        "over_threshold": summary.over_threshold,
        "rows": summary.rows,
    })))
}

#[derive(Deserialize)]
pub struct SettleHoldRequest {
    pub amount_micro_usd: i64,
    pub reason: String,
}

#[derive(Deserialize)]
pub struct ReleaseHoldRequest {
    pub reason: String,
}

/// A hold's amount must be checked against its current reserved amount
/// before settling; this reads the row outside any transaction purely to
/// validate the request shape early (empty reason, out-of-range amount)
/// with a 400 rather than a 409. The actual "is this row still resolvable"
/// check happens again, atomically with the write, in
/// `admin_settle_provider_hold`/`admin_release_provider_hold` — this lookup
/// is not relied on for correctness, only for a friendlier error on garbage
/// input.
fn find_hold_for_validation(
    db: &crate::db::Database,
    request_key: &str,
) -> Result<crate::db::ProviderReservation, (StatusCode, Json<ErrorResponse>)> {
    db.get_provider_reservation(request_key).ok_or((
        StatusCode::NOT_FOUND,
        Json(ErrorResponse {
            error: "provider hold not found".into(),
        }),
    ))
}

fn admin_hold_error_response(
    error: crate::db::AdminHoldError,
) -> (StatusCode, Json<ErrorResponse>) {
    match error {
        crate::db::AdminHoldError::NotFound => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "provider hold not found".into(),
            }),
        ),
        crate::db::AdminHoldError::Conflict(message) => {
            (StatusCode::CONFLICT, Json(ErrorResponse { error: message }))
        }
    }
}

pub async fn settle_provider_hold(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(request_key): Path<String>,
    Json(req): Json<SettleHoldRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    authorize_admin(&state, &user).await?;
    let db = state.db.as_ref().ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: "database unavailable".into(),
        }),
    ))?;
    if req.reason.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "reason is required".into(),
            }),
        ));
    }
    let reservation = find_hold_for_validation(db, &request_key)?;
    if req.amount_micro_usd < 0 || req.amount_micro_usd > reservation.reserved_micro_usd {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!(
                    "amount_micro_usd must be between 0 and the reserved amount ({})",
                    reservation.reserved_micro_usd
                ),
            }),
        ));
    }
    let now_ms = chrono::Utc::now().timestamp_millis();
    let (prior, updated) = db
        .admin_settle_provider_hold(&request_key, req.amount_micro_usd, None, now_ms)
        .map_err(admin_hold_error_response)?;
    db.audit_log(
        &user.user_id,
        "admin",
        "provider_hold_settle",
        Some("provider_request_reservation"),
        Some(&request_key),
        Some(
            &serde_json::json!({
                "amount_micro_usd": req.amount_micro_usd,
                "reason": req.reason,
                "reserved_micro_usd": prior.reserved_micro_usd,
                "prior_status": prior.status,
            })
            .to_string(),
        ),
        None,
    );
    tracing::warn!(
        admin = %user.user_id,
        request_key = %request_key,
        amount_micro_usd = req.amount_micro_usd,
        reason = %req.reason,
        "admin manually settled a provider gateway hold"
    );
    Ok(Json(serde_json::json!({
        "request_key": updated.request_key,
        "status": updated.status,
        "observed_micro_usd": updated.observed_micro_usd,
    })))
}

pub async fn release_provider_hold(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(request_key): Path<String>,
    Json(req): Json<ReleaseHoldRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    authorize_admin(&state, &user).await?;
    let db = state.db.as_ref().ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: "database unavailable".into(),
        }),
    ))?;
    if req.reason.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "reason is required".into(),
            }),
        ));
    }
    let now_ms = chrono::Utc::now().timestamp_millis();
    let (prior, updated) = db
        .admin_release_provider_hold(&request_key, &req.reason, now_ms)
        .map_err(admin_hold_error_response)?;
    db.audit_log(
        &user.user_id,
        "admin",
        "provider_hold_release",
        Some("provider_request_reservation"),
        Some(&request_key),
        Some(
            &serde_json::json!({
                "reason": req.reason,
                "reserved_micro_usd": prior.reserved_micro_usd,
                "prior_status": prior.status,
            })
            .to_string(),
        ),
        None,
    );
    tracing::warn!(
        admin = %user.user_id,
        request_key = %request_key,
        reason = %req.reason,
        "admin manually released a provider gateway hold"
    );
    Ok(Json(serde_json::json!({
        "request_key": updated.request_key,
        "status": updated.status,
    })))
}

#[cfg(test)]
mod provider_holds_tests {
    use super::*;
    use crate::db::SpendAuthorization;
    use crate::provider_gateway::GatewayCapability;

    const NOW: i64 = 1_800_000_000_000;

    /// Real `AppState` (own tempdir, own sqlite database), same shortcut
    /// `agent_confirm::tests::test_state` uses. No `clerk_secret_key`, so
    /// `authorize_admin`'s local-dev bypass applies and any `ClerkUser` is
    /// treated as an admin -- fine for exercising the handlers' own
    /// validation logic directly, without going through the router.
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

    fn admin_user() -> ClerkUser {
        ClerkUser {
            user_id: "admin-1".into(),
        }
    }

    fn reserve(db: &crate::db::Database, request_key: &str) {
        let price_list_id = db.active_price_list().unwrap().id;
        db.set_supplier_capacity("claude", 1_000_000, NOW).ok();
        let authorization = SpendAuthorization {
            id: format!("auth-{request_key}"),
            user_id: "tenant-1".into(),
            run_id: "run-1".into(),
            attempt_id: "attempt-1".into(),
            provider: "claude".into(),
            model: "claude-sonnet-5".into(),
            price_list_id,
            max_micro_usd: 1_000_000,
            expires_at_ms: NOW + 60_000,
        };
        db.create_spend_authorization(&authorization, NOW).unwrap();
        let claims = GatewayCapability::new(
            authorization.id,
            authorization.user_id,
            authorization.run_id,
            authorization.attempt_id,
            authorization.provider,
            authorization.model,
            authorization.expires_at_ms,
        );
        db.reserve_provider_request(&claims, request_key, "digest", 1_000, NOW)
            .unwrap();
    }

    fn settle_req(amount_micro_usd: i64, reason: &str) -> SettleHoldRequest {
        SettleHoldRequest {
            amount_micro_usd,
            reason: reason.to_string(),
        }
    }

    fn release_req(reason: &str) -> ReleaseHoldRequest {
        ReleaseHoldRequest {
            reason: reason.to_string(),
        }
    }

    #[tokio::test]
    async fn settle_rejects_empty_or_whitespace_reason_with_400() {
        let (_dir, state) = test_state().await;
        let db = state.db.as_ref().unwrap();
        reserve(db, "chat:a");

        for reason in ["", "   "] {
            let result = settle_provider_hold(
                State(state.clone()),
                admin_user(),
                Path("chat:a".into()),
                Json(settle_req(500, reason)),
            )
            .await;
            let err = result.unwrap_err();
            assert_eq!(err.0, StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn settle_rejects_out_of_range_amount_with_400() {
        let (_dir, state) = test_state().await;
        let db = state.db.as_ref().unwrap();
        reserve(db, "chat:a");

        let too_low = settle_provider_hold(
            State(state.clone()),
            admin_user(),
            Path("chat:a".into()),
            Json(settle_req(-1, "operator review")),
        )
        .await;
        assert_eq!(too_low.unwrap_err().0, StatusCode::BAD_REQUEST);

        let too_high = settle_provider_hold(
            State(state.clone()),
            admin_user(),
            Path("chat:a".into()),
            Json(settle_req(1_001, "operator review")),
        )
        .await;
        assert_eq!(too_high.unwrap_err().0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn settle_on_an_already_settled_row_is_409_unchanged_and_unaudited() {
        let (_dir, state) = test_state().await;
        let db = state.db.as_ref().unwrap();
        reserve(db, "chat:a");
        db.settle_provider_request("chat:a", 500, None, NOW + 1)
            .unwrap();

        let result = settle_provider_hold(
            State(state.clone()),
            admin_user(),
            Path("chat:a".into()),
            Json(settle_req(500, "operator review")),
        )
        .await;
        assert_eq!(result.unwrap_err().0, StatusCode::CONFLICT);

        let row = db.get_provider_reservation("chat:a").unwrap();
        assert_eq!(row.status, "settled");
        assert_eq!(row.observed_micro_usd, Some(500));

        let entries = db.get_audit_log(50, 0);
        assert!(
            entries.iter().all(|e| e.action != "provider_hold_settle"),
            "a conflicting settle attempt must not write an audit row"
        );
    }

    #[tokio::test]
    async fn release_writes_exactly_one_audit_row_with_actor_reason_and_amount() {
        let (_dir, state) = test_state().await;
        let db = state.db.as_ref().unwrap();
        reserve(db, "chat:a");

        let result = release_provider_hold(
            State(state.clone()),
            admin_user(),
            Path("chat:a".into()),
            Json(release_req("stuck after crash")),
        )
        .await;
        assert!(result.is_ok());

        let row = db.get_provider_reservation("chat:a").unwrap();
        assert_eq!(row.status, "released");

        let entries: Vec<_> = db
            .get_audit_log(50, 0)
            .into_iter()
            .filter(|e| e.action == "provider_hold_release")
            .collect();
        assert_eq!(entries.len(), 1, "exactly one audit row for the release");
        let entry = &entries[0];
        assert_eq!(entry.user_id, "admin-1");
        let details: serde_json::Value =
            serde_json::from_str(entry.metadata.as_deref().unwrap()).unwrap();
        assert_eq!(details["reason"], "stuck after crash");
        assert_eq!(details["reserved_micro_usd"], 1_000);
        assert_eq!(details["prior_status"], "reserved");
    }

    /// The local-dev admin bypass (empty admin list, no key in state or env)
    /// must not fire in production. This is a pure unit test of the
    /// decision function so it cannot race other tests in this lib binary
    /// over the process-wide `CORTEX_ENV` variable.
    #[test]
    fn production_denies_the_keyless_local_dev_admin_bypass() {
        assert!(!local_dev_admin_bypass(true, true, true));
    }

    /// The pure `local_dev_admin_bypass` test above proves the decision
    /// function is correct, but proves nothing about whether
    /// `authorize_admin` actually calls `is_production_runtime()` at its call
    /// site rather than, say, a hard-coded `false`. This drives the real
    /// `authorize_admin` with `CORTEX_ENV=production` set so a regression in
    /// the wiring -- not just in the pure helper -- fails a test. It mutates
    /// the process-wide `CORTEX_ENV` variable and restores it before
    /// returning, matching the save/restore pattern used for
    /// `CORTEX_SINGLE_NODE` in `verification_dispatcher::tests`.
    #[tokio::test]
    async fn authorize_admin_denies_the_keyless_bypass_in_production() {
        let saved_env = std::env::var("CORTEX_ENV").ok();
        let saved_clerk_key = std::env::var("CLERK_SECRET_KEY").ok();
        std::env::remove_var("CORTEX_ADMIN_EMAILS");
        std::env::remove_var("CORTEX_ADMIN_USERS");
        std::env::remove_var("CLERK_SECRET_KEY");
        std::env::set_var("CORTEX_ENV", "production");

        let (_dir, state) = test_state().await;
        let result = authorize_admin(&state, &admin_user()).await;

        match saved_env {
            Some(value) => std::env::set_var("CORTEX_ENV", value),
            None => std::env::remove_var("CORTEX_ENV"),
        }
        match saved_clerk_key {
            Some(value) => std::env::set_var("CLERK_SECRET_KEY", value),
            None => std::env::remove_var("CLERK_SECRET_KEY"),
        }

        let err = result.expect_err(
            "production must never grant the keyless local-dev admin bypass, \
             even when the admin list, state key, and env key are all absent",
        );
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[test]
    fn local_dev_admin_bypass_fires_only_when_keyless_everywhere_and_not_production() {
        assert!(local_dev_admin_bypass(true, true, false));
        assert!(!local_dev_admin_bypass(false, true, false));
        assert!(!local_dev_admin_bypass(true, false, false));
        assert!(!local_dev_admin_bypass(false, false, false));
        assert!(!local_dev_admin_bypass(true, true, true));
        assert!(!local_dev_admin_bypass(false, true, true));
        assert!(!local_dev_admin_bypass(true, false, true));
        assert!(!local_dev_admin_bypass(false, false, true));
    }
}
