use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::clerk::ClerkUser;
use crate::routes::ErrorResponse;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct SaveKeyRequest {
    pub api_key: String,
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

const VALID_PROVIDERS: &[&str] = &["claude", "openai"];

pub async fn save_api_key(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(provider): Path<String>,
    Json(req): Json<SaveKeyRequest>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    if !VALID_PROVIDERS.contains(&provider.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("invalid provider: {provider}"),
            }),
        ));
    }
    if req.api_key.is_empty() || req.api_key.len() > 500 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid API key length".into(),
            }),
        ));
    }
    let db = db_ref(&state)?;
    let encrypted = crate::crypto::encrypt(&req.api_key).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("encryption failed: {e}"),
            }),
        )
    })?;
    db.upsert_api_key(&user.user_id, &provider, &encrypted);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_api_keys(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> Result<Json<Vec<crate::db::UserApiKeyInfo>>, (StatusCode, Json<ErrorResponse>)> {
    let db = db_ref(&state)?;
    Ok(Json(db.list_api_keys(&user.user_id)))
}

pub async fn delete_api_key(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(provider): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let db = db_ref(&state)?;
    if db.delete_api_key(&user.user_id, &provider) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "no key found for that provider".into(),
            }),
        ))
    }
}
