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

    // The tool has already run by this point — its result must reach the
    // caller either way. `messages.conversation_id` is a `NOT NULL` foreign
    // key (`Database::add_message` panics on a violation), and the
    // conversation the row was proposed against can in principle be gone by
    // now (deleted, or — before the chat_paid.rs fix that refuses a
    // proposal with no owned conversation — never valid at all), so this is
    // best-effort: save the message when the conversation still exists for
    // this user, but never let a missing conversation turn a successful
    // confirm into a 500 or a panic.
    if db
        .get_conversation(&action.conversation_id, &user.user_id)
        .is_some()
    {
        db.add_message(
            &action.conversation_id,
            "assistant",
            &result.to_string(),
            None,
            None,
        );
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a real `AppState` (own tempdir, own sqlite database) so these
    /// tests call `confirm_action`/`cancel_action` exactly as the router
    /// would, without a router or Clerk auth in the way — `PremiumUser` is
    /// constructed directly, the same shortcut `crate::routes` tests use.
    async fn test_state() -> (tempfile::TempDir, Arc<AppState>) {
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

    /// A pending row scoped to `user-1` reads as a plain 404 for `user-2` on
    /// both routes — deliberately indistinguishable from a row that never
    /// existed at all (see the module doc comment).
    #[tokio::test]
    async fn other_users_action_is_404() {
        let (_dir, state) = test_state().await;
        let db = state.db.as_ref().expect("db configured");
        let now = chrono::Utc::now().timestamp();
        let action = db.insert_pending_action(
            "user-1",
            "conv-1",
            "open_pr",
            &serde_json::json!({"run_id": "r1"}),
            "Open a pull request for run r1",
            now,
        );

        let other = PremiumUser {
            user_id: "user-2".to_string(),
        };

        let confirm_err = confirm_action(
            State(state.clone()),
            other.clone(),
            Path(action.id.clone()),
            Json(ActionNonceRequest {
                nonce: action.nonce.clone(),
            }),
        )
        .await
        .expect_err("another user's row must not confirm");
        assert_eq!(confirm_err.0, StatusCode::NOT_FOUND);

        let cancel_err = cancel_action(
            State(state.clone()),
            other,
            Path(action.id.clone()),
            Json(ActionNonceRequest {
                nonce: action.nonce.clone(),
            }),
        )
        .await
        .expect_err("another user's row must not cancel");
        assert_eq!(cancel_err.0, StatusCode::NOT_FOUND);

        // The row itself is untouched — still pending, still readable by its
        // actual owner. `user-2`'s 404s never leaked a status flip.
        let reread = db
            .get_pending_action(&action.id, "user-1")
            .expect("the owner can still see their own row");
        assert_eq!(reread.status, "pending");
        drop(state);
    }
}
