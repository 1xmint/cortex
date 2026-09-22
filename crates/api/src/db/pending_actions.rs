//! The confirm-before-run queue for `Risk::Confirm` agent tools
//! (`agent_tools::Risk`, `crates/api/src/agent_tools.rs`).
//!
//! Split out for the same reason as `ledger.rs` and `verification_queue.rs`:
//! this is what stands between a model asking for a risky tool and that tool
//! actually running, and it does not belong buried between unrelated impls.
//!
//! See migration v69's comment in `mod.rs` for the table shape and the
//! reasoning behind `args_hash`, `nonce`, and single-live-row-per-conversation.

use super::*;
#[cfg(test)]
use serde_json::json;
use serde_json::Value;

/// A row of `agent_pending_actions`, read back out. `args_json` is the
/// canonical (sorted-key) form of the tool arguments as proposed — the
/// confirm route runs the tool with exactly this, never with anything the
/// caller sends.
#[derive(Debug, Clone, Serialize)]
pub struct PendingAction {
    pub id: String,
    pub user_id: String,
    pub conversation_id: String,
    pub tool_name: String,
    pub args_json: String,
    pub summary: String,
    pub nonce: String,
    pub status: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub resolved_at: Option<i64>,
}

/// A pending action lives for 5 minutes — long enough for a human to read the
/// summary and tap Confirm, short enough that a stale card cannot come back
/// to life hours later.
pub const PENDING_ACTION_TTL_SECS: i64 = 5 * 60;

/// Why `confirm_pending_action` refused. Every variant is a typed refusal —
/// nothing here ever runs the tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmActionError {
    /// No row with this id for this user. Also returned for another user's
    /// row, deliberately indistinguishable from "does not exist" — see
    /// `crate::agent_confirm` route doc comments.
    NotFound,
    /// The row exists but is no longer `pending` (already confirmed,
    /// cancelled, or expired).
    AlreadyResolved,
    /// The row is still `pending` but its `expires_at` has passed.
    Expired,
    /// The row is live but the caller's nonce does not match.
    WrongNonce,
    /// The status flip committed-worthy conditions all held, but the stored
    /// `args_json` no longer hashes to `args_hash` — something wrote to the
    /// row between proposal and confirmation. Nothing runs; the whole
    /// transaction is rolled back, including the status flip.
    ArgsTampered,
}

fn canonical_json(value: &Value) -> String {
    // `serde_json`'s `Map` is a `BTreeMap` in this workspace (no crate here
    // enables the `preserve_order` feature — see the migration v69 doc
    // comment), so `to_string` already emits object keys in sorted order.
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

fn args_hash_of(args_json: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(args_json.as_bytes());
    hex::encode(hasher.finalize())
}

fn row_to_pending_action(row: &rusqlite::Row) -> rusqlite::Result<PendingAction> {
    Ok(PendingAction {
        id: row.get(0)?,
        user_id: row.get(1)?,
        conversation_id: row.get(2)?,
        tool_name: row.get(3)?,
        args_json: row.get(4)?,
        summary: row.get(5)?,
        nonce: row.get(6)?,
        status: row.get(7)?,
        created_at: row.get(8)?,
        expires_at: row.get(9)?,
        resolved_at: row.get(10)?,
    })
}

const SELECT_COLUMNS: &str = "id, user_id, conversation_id, tool_name, args_json, summary, \
     nonce, status, created_at, expires_at, resolved_at";

impl Database {
    /// Void (`status = 'cancelled'`) every still-`pending` row for this
    /// (user, conversation) pair. Called before a new proposal is inserted so
    /// at most one pending row is ever live per conversation — a stale card
    /// in an old browser tab can never confirm an action the model isn't
    /// currently asking about.
    pub fn void_pending_actions_for_conversation(&self, user_id: &str, conversation_id: &str) {
        let conn = self.conn();
        conn.execute(
            "UPDATE agent_pending_actions SET status = 'cancelled'
             WHERE user_id = ?1 AND conversation_id = ?2 AND status = 'pending'",
            params![user_id, conversation_id],
        )
        .ok();
    }

    /// Void any prior pending row for this (user, conversation), then insert
    /// a new one. `summary` is the server-written, plain-language line the
    /// user sees — never model text (see `agent_tools` module docs).
    pub fn insert_pending_action(
        &self,
        user_id: &str,
        conversation_id: &str,
        tool_name: &str,
        args: &Value,
        summary: &str,
        now: i64,
    ) -> PendingAction {
        let args_json = canonical_json(args);
        let args_hash = args_hash_of(&args_json);
        let id = Uuid::new_v4().to_string();
        let nonce = Uuid::new_v4().to_string();
        let expires_at = now + PENDING_ACTION_TTL_SECS;

        let mut conn = self.conn();
        let tx = conn
            .transaction()
            .expect("begin pending-action transaction");
        tx.execute(
            "UPDATE agent_pending_actions SET status = 'cancelled'
             WHERE user_id = ?1 AND conversation_id = ?2 AND status = 'pending'",
            params![user_id, conversation_id],
        )
        .expect("void prior pending actions");
        tx.execute(
            "INSERT INTO agent_pending_actions
                (id, user_id, conversation_id, tool_name, args_json, args_hash, summary,
                 nonce, status, created_at, expires_at, resolved_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', ?9, ?10, NULL)",
            params![
                id,
                user_id,
                conversation_id,
                tool_name,
                args_json,
                args_hash,
                summary,
                nonce,
                now,
                expires_at,
            ],
        )
        .expect("insert pending action");
        tx.commit().expect("commit pending-action insert");

        PendingAction {
            id,
            user_id: user_id.to_string(),
            conversation_id: conversation_id.to_string(),
            tool_name: tool_name.to_string(),
            args_json,
            summary: summary.to_string(),
            nonce,
            status: "pending".to_string(),
            created_at: now,
            expires_at,
            resolved_at: None,
        }
    }

    /// Read one pending-action row, scoped to `user_id`. `None` for a
    /// missing row or one belonging to another user — the two are
    /// deliberately indistinguishable to callers outside this module.
    pub fn get_pending_action(&self, id: &str, user_id: &str) -> Option<PendingAction> {
        let conn = self.conn();
        conn.query_row(
            &format!(
                "SELECT {SELECT_COLUMNS} FROM agent_pending_actions WHERE id = ?1 AND user_id = ?2"
            ),
            params![id, user_id],
            row_to_pending_action,
        )
        .ok()
    }

    /// Cancel a still-`pending` row. The caller must present the same nonce
    /// the row was proposed with — the frontend sends it on cancel exactly
    /// as it does on confirm, so a card whose nonce the browser never saw
    /// (or a guess) cannot cancel someone else's live proposal any more than
    /// it could confirm one. Returns `true` only when exactly one row
    /// changed — a missing row, another user's row, a wrong nonce, or a row
    /// that is no longer `pending` all return `false`, never an error, since
    /// none of those is a failure the caller needs to distinguish here.
    pub fn cancel_pending_action(&self, id: &str, user_id: &str, nonce: &str, now: i64) -> bool {
        let conn = self.conn();
        let changed = conn
            .execute(
                "UPDATE agent_pending_actions SET status = 'cancelled', resolved_at = ?1
                 WHERE id = ?2 AND user_id = ?3 AND status = 'pending' AND nonce = ?4",
                params![now, id, user_id, nonce],
            )
            .unwrap_or(0);
        changed == 1
    }

    /// Atomically confirm a pending action: one `UPDATE` gated on id, owner,
    /// `pending` status, matching nonce, and a not-yet-expired `expires_at`,
    /// then a re-read of `args_json` checked against the stored `args_hash`.
    /// Either check failing rolls back the whole transaction — the status
    /// flip included — and returns a typed refusal. The tool itself is never
    /// run from here; the caller runs it only after this returns `Ok`, using
    /// the returned row's `args_json` verbatim.
    pub fn confirm_pending_action(
        &self,
        id: &str,
        user_id: &str,
        nonce: &str,
        now: i64,
    ) -> Result<PendingAction, ConfirmActionError> {
        let mut conn = self.conn();
        let tx = conn
            .transaction()
            .expect("begin pending-action confirm transaction");

        let changed = tx
            .execute(
                "UPDATE agent_pending_actions
                 SET status = 'confirmed', resolved_at = ?1
                 WHERE id = ?2 AND user_id = ?3 AND status = 'pending'
                   AND nonce = ?4 AND expires_at > ?5",
                params![now, id, user_id, nonce, now],
            )
            .expect("attempt pending-action confirm update");

        if changed != 1 {
            // The single UPDATE above folds several distinct reasons into
            // one WHERE clause; this read-only lookup (same transaction, so
            // it sees a consistent snapshot) classifies which one applied,
            // without ever granting the confirm itself. Scoped to
            // `user_id` throughout, so another user's row still reads as
            // `NotFound`.
            let existing = tx
                .query_row(
                    "SELECT status, nonce, expires_at FROM agent_pending_actions
                     WHERE id = ?1 AND user_id = ?2",
                    params![id, user_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .ok();
            let _ = tx.rollback();
            return Err(match existing {
                None => ConfirmActionError::NotFound,
                Some((status, _, _)) if status != "pending" => ConfirmActionError::AlreadyResolved,
                Some((_, _, expires_at)) if expires_at <= now => ConfirmActionError::Expired,
                Some((_, stored_nonce, _)) if stored_nonce != nonce => {
                    ConfirmActionError::WrongNonce
                }
                _ => ConfirmActionError::NotFound,
            });
        }

        let (args_json, args_hash): (String, String) = tx
            .query_row(
                "SELECT args_json, args_hash FROM agent_pending_actions WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("re-read args_json after successful confirm update");

        if args_hash_of(&args_json) != args_hash {
            let _ = tx.rollback();
            return Err(ConfirmActionError::ArgsTampered);
        }

        let row = tx
            .query_row(
                &format!("SELECT {SELECT_COLUMNS} FROM agent_pending_actions WHERE id = ?1"),
                params![id],
                row_to_pending_action,
            )
            .expect("re-read confirmed row");

        tx.commit().expect("commit pending-action confirm");
        Ok(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("pending-actions.sqlite"));
        (dir, db)
    }

    #[test]
    fn insert_then_confirm_round_trip() {
        let (_dir, db) = test_db();
        let now = 1_000_000;
        let action = db.insert_pending_action(
            "user-1",
            "conv-1",
            "open_pr",
            &json!({"run_id": "r1"}),
            "Open a pull request for run r1",
            now,
        );
        assert_eq!(action.status, "pending");

        let confirmed = db
            .confirm_pending_action(&action.id, "user-1", &action.nonce, now + 1)
            .expect("confirm succeeds");
        assert_eq!(confirmed.status, "confirmed");
        assert_eq!(confirmed.args_json, action.args_json);
    }

    #[test]
    fn new_proposal_voids_old_one() {
        let (_dir, db) = test_db();
        let now = 1_000_000;
        let first = db.insert_pending_action(
            "user-1",
            "conv-1",
            "open_pr",
            &json!({"run_id": "r1"}),
            "first",
            now,
        );
        let _second = db.insert_pending_action(
            "user-1",
            "conv-1",
            "open_pr",
            &json!({"run_id": "r2"}),
            "second",
            now + 1,
        );

        let reread = db.get_pending_action(&first.id, "user-1").unwrap();
        assert_eq!(reread.status, "cancelled");

        let err = db
            .confirm_pending_action(&first.id, "user-1", &first.nonce, now + 2)
            .unwrap_err();
        assert_eq!(err, super::ConfirmActionError::AlreadyResolved);
    }

    #[test]
    fn confirm_is_single_use() {
        let (_dir, db) = test_db();
        let now = 1_000_000;
        let action = db.insert_pending_action(
            "user-1",
            "conv-1",
            "open_pr",
            &json!({"run_id": "r1"}),
            "s",
            now,
        );
        db.confirm_pending_action(&action.id, "user-1", &action.nonce, now + 1)
            .expect("first confirm succeeds");
        let err = db
            .confirm_pending_action(&action.id, "user-1", &action.nonce, now + 2)
            .unwrap_err();
        assert_eq!(err, ConfirmActionError::AlreadyResolved);
    }

    #[test]
    fn confirm_rejects_changed_args() {
        let (_dir, db) = test_db();
        let now = 1_000_000;
        let action = db.insert_pending_action(
            "user-1",
            "conv-1",
            "open_pr",
            &json!({"run_id": "r1"}),
            "s",
            now,
        );
        db.conn()
            .execute(
                "UPDATE agent_pending_actions SET args_json = '{\"run_id\":\"tampered\"}' WHERE id = ?1",
                params![action.id],
            )
            .unwrap();

        let err = db
            .confirm_pending_action(&action.id, "user-1", &action.nonce, now + 1)
            .unwrap_err();
        assert_eq!(err, ConfirmActionError::ArgsTampered);

        // Rolled back: the status flip did not stick either.
        let reread = db.get_pending_action(&action.id, "user-1").unwrap();
        assert_eq!(reread.status, "pending");
    }

    #[test]
    fn expired_action_cannot_confirm() {
        let (_dir, db) = test_db();
        let now = 1_000_000;
        let action = db.insert_pending_action(
            "user-1",
            "conv-1",
            "open_pr",
            &json!({"run_id": "r1"}),
            "s",
            now,
        );
        let after_expiry = now + PENDING_ACTION_TTL_SECS + 1;
        let err = db
            .confirm_pending_action(&action.id, "user-1", &action.nonce, after_expiry)
            .unwrap_err();
        assert_eq!(err, ConfirmActionError::Expired);
    }

    #[test]
    fn wrong_nonce_cannot_confirm() {
        let (_dir, db) = test_db();
        let now = 1_000_000;
        let action = db.insert_pending_action(
            "user-1",
            "conv-1",
            "open_pr",
            &json!({"run_id": "r1"}),
            "s",
            now,
        );
        let err = db
            .confirm_pending_action(&action.id, "user-1", "not-the-nonce", now + 1)
            .unwrap_err();
        assert_eq!(err, ConfirmActionError::WrongNonce);
    }

    #[test]
    fn other_users_action_is_not_found() {
        let (_dir, db) = test_db();
        let now = 1_000_000;
        let action = db.insert_pending_action(
            "user-1",
            "conv-1",
            "open_pr",
            &json!({"run_id": "r1"}),
            "s",
            now,
        );
        assert!(db.get_pending_action(&action.id, "user-2").is_none());
        let err = db
            .confirm_pending_action(&action.id, "user-2", &action.nonce, now + 1)
            .unwrap_err();
        assert_eq!(err, ConfirmActionError::NotFound);
    }

    #[test]
    fn cancel_prevents_confirm() {
        let (_dir, db) = test_db();
        let now = 1_000_000;
        let action = db.insert_pending_action(
            "user-1",
            "conv-1",
            "open_pr",
            &json!({"run_id": "r1"}),
            "s",
            now,
        );
        assert!(db.cancel_pending_action(&action.id, "user-1", &action.nonce, now + 1));
        let err = db
            .confirm_pending_action(&action.id, "user-1", &action.nonce, now + 2)
            .unwrap_err();
        assert_eq!(err, ConfirmActionError::AlreadyResolved);
    }

    #[test]
    fn wrong_nonce_cannot_cancel() {
        let (_dir, db) = test_db();
        let now = 1_000_000;
        let action = db.insert_pending_action(
            "user-1",
            "conv-1",
            "open_pr",
            &json!({"run_id": "r1"}),
            "s",
            now,
        );
        assert!(!db.cancel_pending_action(&action.id, "user-1", "not-the-nonce", now + 1));
        // Still pending: the wrong-nonce cancel attempt did nothing.
        let reread = db.get_pending_action(&action.id, "user-1").unwrap();
        assert_eq!(reread.status, "pending");
    }

    #[test]
    fn canonical_json_sorts_keys() {
        let value = json!({"b": 1, "a": 2});
        assert_eq!(canonical_json(&value), "{\"a\":2,\"b\":1}");
    }
}
