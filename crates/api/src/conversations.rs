use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::clerk::ClerkUser;
use crate::routes::ErrorResponse;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct CreateConversationRequest {
    pub title: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateTitleRequest {
    pub title: String,
}

#[derive(Deserialize)]
pub struct AddMessageRequest {
    pub role: String,
    pub content: String,
    pub provider: Option<String>,
    pub model: Option<String>,
}

#[derive(Deserialize)]
pub struct PaginationParams {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

fn db_ref(state: &AppState) -> Result<&crate::db::Database, (StatusCode, Json<ErrorResponse>)> {
    state.db.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "database not available".into(),
            }),
        )
    })
}

pub async fn list_conversations(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    axum::extract::Query(params): axum::extract::Query<PaginationParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    // If using local auth (no real user), return empty response to prevent frontend crashes
    if user.user_id == "local" && state.clerk_secret_key.is_none() {
        return Ok(Json(serde_json::json!({ "conversations": [], "total": 0 })));
    }
    let db = db_ref(&state)?;
    let limit = params.limit.unwrap_or(50).clamp(1, 100);
    let offset = params.offset.unwrap_or(0).max(0);
    let (conversations, total) = db.list_conversations(&user.user_id, limit, offset);
    let value = serde_json::json!({
        "conversations": conversations,
        "total": total,
    });
    Ok(Json(value))
}

pub async fn create_conversation(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Json(req): Json<CreateConversationRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<ErrorResponse>)> {
    let db = db_ref(&state)?;
    let conversation = db.create_conversation(&user.user_id, req.title.as_deref());
    let value = serde_json::to_value(conversation).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "serialization failed".into(),
            }),
        )
    })?;
    Ok((StatusCode::CREATED, Json(value)))
}

pub async fn get_conversation(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let db = db_ref(&state)?;
    match db.get_conversation(&id, &user.user_id) {
        Some(conv) => serde_json::to_value(conv).map(Json).map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "serialization failed".into(),
                }),
            )
        }),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "conversation not found".into(),
            }),
        )),
    }
}

pub async fn delete_conversation(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(id): Path<String>,
) -> StatusCode {
    match &state.db {
        Some(db) if db.delete_conversation(&id, &user.user_id) => StatusCode::NO_CONTENT,
        Some(_) => StatusCode::NOT_FOUND,
        None => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub async fn update_conversation(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(id): Path<String>,
    Json(req): Json<UpdateTitleRequest>,
) -> StatusCode {
    match &state.db {
        Some(db) if db.update_conversation_title(&id, &user.user_id, &req.title) => StatusCode::OK,
        Some(_) => StatusCode::NOT_FOUND,
        None => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub async fn add_message(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(id): Path<String>,
    Json(req): Json<AddMessageRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<ErrorResponse>)> {
    let db = db_ref(&state)?;

    // Verify conversation belongs to this user
    if db.get_conversation(&id, &user.user_id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "conversation not found".into(),
            }),
        ));
    }

    let message = db.add_message(
        &id,
        &req.role,
        &req.content,
        req.provider.as_deref(),
        req.model.as_deref(),
    );

    let value = serde_json::to_value(message).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "serialization failed".into(),
            }),
        )
    })?;
    Ok((StatusCode::CREATED, Json(value)))
}
