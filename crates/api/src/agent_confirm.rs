//! `POST /api/agent/actions/{id}/confirm` and `.../cancel` — the only two
//! ways a `Risk::Confirm` agent tool call (see `agent_tools.rs`) ever
//! actually runs, or is dropped, once it has been proposed as a row in
//! `agent_pending_actions` (`db::pending_actions`).
//!
//! Both routes require auth, and a row that does not exist, or belongs to
//! another user, is deliberately indistinguishable from one that never
//! existed: 404 either way. Confirm additionally requires the caller's
//! nonce to match and the row to still be live (`pending`, not expired);
//! cancel now requires the same nonce match per the frontend contract in
//! PR #27 — a missing or wrong nonce on cancel is also a 404, not a 400,
//! so a guess can't distinguish "wrong nonce" from "no such row".

use crate::billing::PremiumUser;
use crate::db::{ConfirmActionError, Database};
use crate::routes::ErrorResponse;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct ActionNonceRequest {
    pub nonce: String,
}

#[derive(Debug, Serialize)]
pub struct ConfirmActionResponse {
    pub status: String,
    pub result: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct CancelActionResponse {
    pub status: String,
}

fn not_found() -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorResponse {
            error: "no such pending action".into(),
        }),
    )
}

fn db_from_state(state: &AppState) -> Result<&Database, (StatusCode, Json<ErrorResponse>)> {
    state.db.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "database not available".into(),
            }),
        )
    })
}

/// `POST /api/agent/actions/{id}/confirm` — run the `Risk::Confirm` tool
/// this pending row proposed, using only its stored, hash-verified
/// arguments (never anything in this request), and save the tool's result
/// to the conversation as an assistant message.
pub async fn confirm_action(
    State(state): State<Arc<AppState>>,
    user: PremiumUser,
    Path(id): Path<String>,
    Json(req): Json<ActionNonceRequest>,
) -> Result<Json<ConfirmActionResponse>, (StatusCode, Json<ErrorResponse>)> {
    let db = db_from_state(&state)?;

    // Scope-check first so a wrong nonce on someone else's row still reads
    // as plain 404, not a hint that the id exists.
    if db.get_pending_action(&id, &user.user_id).is_none() {
        return Err(not_found());
    }

    let now = chrono::Utc::now().timestamp();
    let action =
        match db.confirm_pending_action(&id, &user.user_id, &req.nonce, now) {
            Ok(action) => action,
            Err(ConfirmActionError::NotFound) => return Err(not_found()),
            Err(ConfirmActionError::WrongNonce) => return Err(not_found()),
            Err(ConfirmActionError::Expired) => {
                return Err((
                    StatusCode::CONFLICT,
                    Json(ErrorResponse {
                        error: "this action has expired; ask again".into(),
                    }),
                ))
            }
            Err(ConfirmActionError::AlreadyResolved) => {
                return Err((
                    StatusCode::CONFLICT,
                    Json(ErrorResponse {
                        error: "this action was already confirmed or cancelled".into(),
                    }),
                ))
            }
            Err(ConfirmActionError::ArgsTampered) => return Err((
                StatusCode::CONFLICT,
                Json(ErrorResponse {
                    error:
                        "this action's arguments changed since it was proposed; refusing to run it"
                            .into(),
                }),
            )),
        };

    let input: serde_json::Value = serde_json::from_str(&action.args_json).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("stored action arguments are not valid JSON: {e}"),
            }),
        )
    })?;

    let result =
        crate::agent_tools::execute_confirmed(&state, db, &user.user_id, &action.tool_name, &input)
            .await
            .map_err(|e| {
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(ErrorResponse { error: e.message() }),
                )
            })?;

    db.add_message(
        &action.conversation_id,
        "assistant",
        &result.to_string(),
        None,
        None,
    );

    Ok(Json(ConfirmActionResponse {
        status: "confirmed".into(),
        result,
    }))
}

/// `POST /api/agent/actions/{id}/cancel` — drop a pending action without
/// running it. Per the frontend contract (PR #27) this also takes
/// `{"nonce": "..."}` and requires it to match, the same as confirm; a
/// missing row, another user's row, or a wrong nonce are all a 404.
pub async fn cancel_action(
    State(state): State<Arc<AppState>>,
    user: PremiumUser,
    Path(id): Path<String>,
    Json(req): Json<ActionNonceRequest>,
) -> Result<Json<CancelActionResponse>, (StatusCode, Json<ErrorResponse>)> {
    let db = db_from_state(&state)?;

    if db.get_pending_action(&id, &user.user_id).is_none() {
        return Err(not_found());
    }

    let now = chrono::Utc::now().timestamp();
    if !db.cancel_pending_action(&id, &user.user_id, &req.nonce, now) {
        return Err(not_found());
    }

    Ok(Json(CancelActionResponse {
        status: "cancelled".into(),
    }))
}
