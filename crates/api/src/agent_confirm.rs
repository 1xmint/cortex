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

/// Why [`confirm_and_execute`]/[`confirm_and_execute_spoken`] refused, or the
/// tool itself failed once run. A superset of [`ConfirmActionError`] plus the
/// one failure mode that can only happen after the status flip: the tool's
/// own execution.
pub enum ConfirmAndExecuteError {
    /// No such row, another user's row, or (for the tap route) a wrong
    /// nonce — all indistinguishable from "never existed" per this module's
    /// doc comment.
    NotFound,
    Expired,
    AlreadyResolved,
    ArgsTampered,
    /// `execute_confirmed` itself refused or failed.
    ToolFailed(String),
}

impl From<ConfirmActionError> for ConfirmAndExecuteError {
    fn from(err: ConfirmActionError) -> Self {
        match err {
            ConfirmActionError::NotFound | ConfirmActionError::WrongNonce => Self::NotFound,
            ConfirmActionError::Expired => Self::Expired,
            ConfirmActionError::AlreadyResolved => Self::AlreadyResolved,
            ConfirmActionError::ArgsTampered => Self::ArgsTampered,
        }
    }
}

impl ConfirmAndExecuteError {
    fn into_response(self) -> (StatusCode, Json<ErrorResponse>) {
        match self {
            Self::NotFound => not_found(),
            Self::Expired => (
                StatusCode::CONFLICT,
                Json(ErrorResponse {
                    error: "this action has expired; ask again".into(),
                }),
            ),
            Self::AlreadyResolved => (
                StatusCode::CONFLICT,
                Json(ErrorResponse {
                    error: "this action was already confirmed or cancelled".into(),
                }),
            ),
            Self::ArgsTampered => (
                StatusCode::CONFLICT,
                Json(ErrorResponse {
                    error:
                        "this action's arguments changed since it was proposed; refusing to run it"
                            .into(),
                }),
            ),
            Self::ToolFailed(message) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ErrorResponse { error: message }),
            ),
        }
    }
}

/// The shared core of both `POST /api/agent/actions/{id}/confirm` (tap,
/// nonce supplied by the caller) and the spoken path (nonce looked up
/// server-side — see [`confirm_and_execute_spoken`]): run every check
/// `Database::confirm_pending_action` enforces (ownership, nonce match, not
/// expired, not voided, single-use atomic claim), then run the tool with
/// exactly the row's stored, hash-verified arguments, and best-effort save
/// the result to the conversation. Never trusts anything about the row
/// except what this DB round trip just read back.
async fn confirm_and_execute(
    state: &Arc<AppState>,
    db: &Database,
    user_id: &str,
    action_id: &str,
    nonce: &str,
) -> Result<serde_json::Value, ConfirmAndExecuteError> {
    let now = chrono::Utc::now().timestamp();
    let action = db.confirm_pending_action(action_id, user_id, nonce, now)?;

    let input: serde_json::Value = serde_json::from_str(&action.args_json).map_err(|e| {
        ConfirmAndExecuteError::ToolFailed(format!(
            "stored action arguments are not valid JSON: {e}"
        ))
    })?;

    let result =
        crate::agent_tools::execute_confirmed(state, db, user_id, &action.tool_name, &input)
            .await
            .map_err(|e| ConfirmAndExecuteError::ToolFailed(e.message()))?;

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
        .get_conversation(&action.conversation_id, user_id)
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

    Ok(result)
}

/// The spoken-confirm entry point: same checks and execution as
/// [`confirm_and_execute`], but the caller (a voice session) never has the
/// row's nonce — the client-side "yes" carries only the `action_id` a
/// `VoiceEvent::ConfirmRequired` already scoped to this session's owner, so
/// the nonce is looked up here, server-side, from the DB row itself.
/// Ownership is still enforced by `confirm_pending_action`'s own `user_id`
/// scoping, and the row's live `status`/`expires_at` are re-checked there
/// too — the in-memory pending slot in `voice_session.rs` is never trusted
/// for this, since it is never cleared on tap, cancel, expiry, or void.
///
/// Not called from any route yet — the transcript-driven wiring lands in
/// part 2b of the spoken-confirm plan. Exercised directly by the tests
/// below in the meantime.
#[allow(dead_code)] // Wired to the transcript matcher in part 2b.
async fn confirm_and_execute_spoken(
    state: &Arc<AppState>,
    db: &Database,
    user_id: &str,
    action_id: &str,
) -> Result<serde_json::Value, ConfirmAndExecuteError> {
    let nonce = db
        .get_pending_action(action_id, user_id)
        .ok_or(ConfirmAndExecuteError::NotFound)?
        .nonce;
    confirm_and_execute(state, db, user_id, action_id, &nonce).await
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

    let result = confirm_and_execute(&state, db, &user.user_id, &id, &req.nonce)
        .await
        .map_err(ConfirmAndExecuteError::into_response)?;

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

    /// `confirm_and_execute_spoken` exercised directly — it is not wired to
    /// any route yet (see its own doc comment), so these are the only tests
    /// that can fail if it regresses before part 2b lands.
    mod spoken {
        use super::*;

        /// `execute_confirmed("open_pr", ...)` does a real `git push`/`gh pr
        /// create`, so a genuinely successful run is not something a unit
        /// test can trigger. A run id that does not exist fails inside
        /// `create_pr_core`'s own validation, before any process spawns —
        /// which is exactly the boundary this test needs: proof that every
        /// confirm-gate check (ownership, nonce lookup, not expired, not
        /// voided, atomic single-use claim) passed, leaving only the tool's
        /// own failure. The status flip is unconditional once the gate
        /// passes, so re-reading it here is what actually proves the row
        /// went from pending to confirmed.
        #[tokio::test]
        async fn succeeds_through_the_gate_on_a_pending_unexpired_owned_row() {
            let (_dir, state) = test_state().await;
            let db = state.db.as_ref().expect("db configured");
            let now = chrono::Utc::now().timestamp();
            let action = db.insert_pending_action(
                "user-1",
                "conv-1",
                "open_pr",
                &serde_json::json!({"run_id": "no-such-run"}),
                "s",
                now,
            );

            let err = confirm_and_execute_spoken(&state, db, "user-1", &action.id)
                .await
                .expect_err("no such run: the tool itself fails, not the confirm gate");
            assert!(
                matches!(err, ConfirmAndExecuteError::ToolFailed(_)),
                "the gate must have passed for the tool to run at all"
            );

            let reread = db.get_pending_action(&action.id, "user-1").unwrap();
            assert_eq!(reread.status, "confirmed");
        }

        #[tokio::test]
        async fn refuses_an_expired_row() {
            let (_dir, state) = test_state().await;
            let db = state.db.as_ref().expect("db configured");
            let real_now = chrono::Utc::now().timestamp();
            // Inserted far enough in the "past" that its expires_at is
            // already behind the real wall clock `confirm_and_execute`
            // reads internally.
            let insert_now = real_now - crate::db::PENDING_ACTION_TTL_SECS - 10;
            let action = db.insert_pending_action(
                "user-1",
                "conv-1",
                "open_pr",
                &serde_json::json!({"run_id": "r1"}),
                "s",
                insert_now,
            );

            let err = confirm_and_execute_spoken(&state, db, "user-1", &action.id)
                .await
                .expect_err("an expired row must not confirm");
            assert!(matches!(err, ConfirmAndExecuteError::Expired));
        }

        #[tokio::test]
        async fn refuses_a_voided_row() {
            let (_dir, state) = test_state().await;
            let db = state.db.as_ref().expect("db configured");
            let now = chrono::Utc::now().timestamp();
            let first = db.insert_pending_action(
                "user-1",
                "conv-1",
                "open_pr",
                &serde_json::json!({"run_id": "r1"}),
                "first",
                now,
            );
            // A second proposal on the same conversation voids the first.
            let _second = db.insert_pending_action(
                "user-1",
                "conv-1",
                "open_pr",
                &serde_json::json!({"run_id": "r2"}),
                "second",
                now,
            );

            let err = confirm_and_execute_spoken(&state, db, "user-1", &first.id)
                .await
                .expect_err("a voided row must not confirm");
            assert!(matches!(err, ConfirmAndExecuteError::AlreadyResolved));
        }

        #[tokio::test]
        async fn refuses_another_users_row() {
            let (_dir, state) = test_state().await;
            let db = state.db.as_ref().expect("db configured");
            let now = chrono::Utc::now().timestamp();
            let action = db.insert_pending_action(
                "user-1",
                "conv-1",
                "open_pr",
                &serde_json::json!({"run_id": "r1"}),
                "s",
                now,
            );

            let err = confirm_and_execute_spoken(&state, db, "user-2", &action.id)
                .await
                .expect_err("another user's row must not confirm");
            assert!(matches!(err, ConfirmAndExecuteError::NotFound));
        }

        #[tokio::test]
        async fn refuses_a_second_call() {
            let (_dir, state) = test_state().await;
            let db = state.db.as_ref().expect("db configured");
            let now = chrono::Utc::now().timestamp();
            let action = db.insert_pending_action(
                "user-1",
                "conv-1",
                "open_pr",
                &serde_json::json!({"run_id": "no-such-run"}),
                "s",
                now,
            );

            confirm_and_execute_spoken(&state, db, "user-1", &action.id)
                .await
                .expect_err("the tool itself still fails (no such run)");

            let err = confirm_and_execute_spoken(&state, db, "user-1", &action.id)
                .await
                .expect_err("a second call on the same row must not confirm again");
            assert!(matches!(err, ConfirmAndExecuteError::AlreadyResolved));
        }
    }
}
