use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use cortex_core::usage::UsageLimits;
use serde::Deserialize;

use crate::billing;
use crate::clerk::ClerkUser;
use crate::routes::ErrorResponse;
use crate::state::AppState;

/// GET /api/usage — authenticated user's usage summary (24h + 30d).
pub async fn get_usage(
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

    let now_ms = Utc::now().timestamp_millis();
    let last_24h = now_ms - 86_400_000;
    let last_30d = now_ms - (30 * 86_400_000);

    let summary_24h = db.get_user_usage_summary(&user.user_id, last_24h);
    let summary_30d = db.get_user_usage_summary(&user.user_id, last_30d);

    // Also check billing gate status for display
    let limits = UsageLimits::default();
    let gate = billing::check_usage_gate(db, &user.user_id, &limits, false);

    Ok(Json(serde_json::json!({
        "user_id": user.user_id,
        "last_24h": summary_24h,
        "last_30d": summary_30d,
        "gate": gate,
        "limits": limits,
    })))
}

/// GET /api/usage/daily — daily breakdown for charts (last 30 days by default).
pub async fn get_daily_usage(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Query(query): Query<DailyQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let db = state.db.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "database not available".into(),
            }),
        )
    })?;

    let days = query.days.unwrap_or(30).min(90); // cap at 90 days
    let daily = db.get_user_daily_usage(&user.user_id, days);

    Ok(Json(serde_json::json!({
        "user_id": user.user_id,
        "days": days,
        "daily": daily,
    })))
}

#[derive(Deserialize)]
pub struct DailyQuery {
    pub days: Option<u32>,
}

/// GET /api/admin/usage — system-wide usage summary (admin).
pub async fn admin_usage(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    crate::admin::authorize_admin(&state, &user).await?;
    let db = state.db.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "database not available".into(),
            }),
        )
    })?;

    let now_ms = Utc::now().timestamp_millis();
    let last_24h = now_ms - 86_400_000;
    let last_30d = now_ms - (30 * 86_400_000);

    let summary_24h = db.get_system_usage_summary(last_24h);
    let summary_30d = db.get_system_usage_summary(last_30d);

    Ok(Json(serde_json::json!({
        "last_24h": summary_24h,
        "last_30d": summary_30d,
    })))
}

/// GET /api/admin/usage/users — per-user usage breakdown (admin).
pub async fn admin_usage_users(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Query(query): Query<AdminUsersQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    crate::admin::authorize_admin(&state, &user).await?;
    let db = state.db.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "database not available".into(),
            }),
        )
    })?;

    let hours = query.hours.unwrap_or(24).min(720); // cap at 30 days
    let since_ms = Utc::now().timestamp_millis() - (hours as i64 * 3_600_000);
    let per_user = db.get_per_user_usage(since_ms);

    let users: Vec<serde_json::Value> = per_user
        .into_iter()
        .map(|(user_id, summary)| {
            serde_json::json!({
                "user_id": user_id,
                "summary": summary,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "hours": hours,
        "users": users,
    })))
}

#[derive(Deserialize)]
pub struct AdminUsersQuery {
    pub hours: Option<u32>,
}
