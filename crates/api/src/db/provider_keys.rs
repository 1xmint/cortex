//! Row-level storage for `user_provider_keys` (customer BYOK provider keys).
//!
//! Every query here is scoped by `user_id` in the `WHERE` clause, not just by
//! the AAD binding in `byok.rs` — belt and suspenders. A caller that forgets
//! to check the AAD result still cannot read or touch another user's row,
//! because the row is never returned or updated in the first place.

use super::*;
use crate::byok::EncryptedKey;

/// What `GET /api/provider-keys` returns. Never carries `nonce` or
/// `ciphertext` — those fields do not exist on this type at all, so a future
/// `#[derive(Serialize)]` on the wrong struct cannot accidentally leak them.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderKeySummary {
    pub provider: String,
    pub last4: String,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_used_at: Option<i64>,
}

/// A row as needed to decrypt and use the key server-side. Also never
/// `Serialize` — this type never crosses the HTTP boundary.
pub struct ProviderKeyRow {
    pub user_id: String,
    pub provider: String,
    pub key_version: i64,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
    pub status: String,
    /// The row's `updated_at` as read here. Callers that later act on this
    /// key (e.g. `mark_provider_key_rejected`) pass it back so the update
    /// only applies if the row has not changed since -- a user replacing
    /// the key while an old request is still in flight must not have the
    /// new key marked rejected because of the old one's failure.
    pub updated_at: i64,
}

impl Database {
    pub fn list_provider_keys(&self, user_id: &str) -> Vec<ProviderKeySummary> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT provider, last4, status, created_at, updated_at, last_used_at
                 FROM user_provider_keys WHERE user_id = ?1 ORDER BY provider",
            )
            .expect("prepare list_provider_keys");
        let rows = stmt
            .query_map(params![user_id], |row| {
                Ok(ProviderKeySummary {
                    provider: row.get(0)?,
                    last4: row.get(1)?,
                    status: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    last_used_at: row.get(5)?,
                })
            })
            .expect("query list_provider_keys");
        rows.filter_map(Result::ok).collect()
    }

    /// Insert or replace the key for `(user_id, provider)`. Always resets
    /// `status` to `'active'`, per D2/D3: replacing a rejected key should
    /// make the provider selectable again on the next chat request.
    pub fn upsert_provider_key(
        &self,
        user_id: &str,
        provider: &str,
        encrypted: &EncryptedKey,
        last4: &str,
        now_ms: i64,
    ) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO user_provider_keys
                (user_id, provider, key_version, nonce, ciphertext, last4, status,
                 created_at, updated_at, last_used_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', ?7, ?7, NULL)
             ON CONFLICT(user_id, provider) DO UPDATE SET
                key_version = excluded.key_version,
                nonce = excluded.nonce,
                ciphertext = excluded.ciphertext,
                last4 = excluded.last4,
                status = 'active',
                updated_at = excluded.updated_at",
            params![
                user_id,
                provider,
                encrypted.key_version,
                encrypted.nonce,
                encrypted.ciphertext,
                last4,
                now_ms,
            ],
        )
        .expect("upsert user_provider_keys");
    }

    /// Hard delete. Returns whether a row existed.
    pub fn delete_provider_key(&self, user_id: &str, provider: &str) -> bool {
        let conn = self.conn();
        let affected = conn
            .execute(
                "DELETE FROM user_provider_keys WHERE user_id = ?1 AND provider = ?2",
                params![user_id, provider],
            )
            .expect("delete user_provider_keys");
        affected > 0
    }

    /// The row needed to decrypt and use this user's key, if one exists.
    pub fn get_provider_key_row(&self, user_id: &str, provider: &str) -> Option<ProviderKeyRow> {
        let conn = self.conn();
        conn.query_row(
            "SELECT user_id, provider, key_version, nonce, ciphertext, status, updated_at
             FROM user_provider_keys WHERE user_id = ?1 AND provider = ?2",
            params![user_id, provider],
            |row| {
                Ok(ProviderKeyRow {
                    user_id: row.get(0)?,
                    provider: row.get(1)?,
                    key_version: row.get(2)?,
                    nonce: row.get(3)?,
                    ciphertext: row.get(4)?,
                    status: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            },
        )
        .ok()
    }

    /// Marks the key rejected, but only if the row is still the same one the
    /// caller decrypted -- `expected_updated_at` is that row's `updated_at`
    /// as read at decrypt time. If the customer has since replaced the key
    /// (which bumps `updated_at`), this is a no-op: a 401 for the old key
    /// must never mark the new one rejected.
    pub fn mark_provider_key_rejected(
        &self,
        user_id: &str,
        provider: &str,
        expected_updated_at: i64,
        now_ms: i64,
    ) {
        let conn = self.conn();
        conn.execute(
            "UPDATE user_provider_keys SET status = 'rejected', updated_at = ?4
             WHERE user_id = ?1 AND provider = ?2 AND updated_at = ?3",
            params![user_id, provider, expected_updated_at, now_ms],
        )
        .expect("mark provider key rejected");
    }

    pub fn touch_provider_key_last_used(&self, user_id: &str, provider: &str, now_ms: i64) {
        let conn = self.conn();
        conn.execute(
            "UPDATE user_provider_keys SET last_used_at = ?3
             WHERE user_id = ?1 AND provider = ?2",
            params![user_id, provider, now_ms],
        )
        .expect("touch provider key last_used_at");
    }

    /// Every row whose `key_version` is not `current_version`, for
    /// `byok::rewrap_provider_keys`. Bounded by the table's size, which is
    /// one row per (user, provider) — no pagination needed at BYOK's scale.
    pub fn list_provider_key_rows_needing_rewrap(
        &self,
        current_version: i64,
    ) -> Vec<ProviderKeyRow> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT user_id, provider, key_version, nonce, ciphertext, status, updated_at
                 FROM user_provider_keys WHERE key_version != ?1",
            )
            .expect("prepare list_provider_key_rows_needing_rewrap");
        let rows = stmt
            .query_map(params![current_version], |row| {
                Ok(ProviderKeyRow {
                    user_id: row.get(0)?,
                    provider: row.get(1)?,
                    key_version: row.get(2)?,
                    nonce: row.get(3)?,
                    ciphertext: row.get(4)?,
                    status: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            })
            .expect("query list_provider_key_rows_needing_rewrap");
        rows.filter_map(Result::ok).collect()
    }

    /// Rewrite a row's ciphertext/nonce/version in place. Leaves `last4`,
    /// `status`, `created_at` and `last_used_at` untouched — this is a key
    /// rotation, not a key change, so nothing about the row's history or
    /// display state should move.
    ///
    /// Guarded by `old_version`/`old_nonce` matching the row this rewrap was
    /// computed from: without it, a customer replacing their key between the
    /// read and this write would have their new key overwritten with a fresh
    /// encryption of the stale plaintext.
    pub fn rewrap_provider_key_row(
        &self,
        user_id: &str,
        provider: &str,
        old_version: i64,
        old_nonce: &[u8],
        fresh: &EncryptedKey,
    ) {
        let conn = self.conn();
        conn.execute(
            "UPDATE user_provider_keys
             SET key_version = ?3, nonce = ?4, ciphertext = ?5
             WHERE user_id = ?1 AND provider = ?2 AND key_version = ?6 AND nonce = ?7",
            params![
                user_id,
                provider,
                fresh.key_version,
                fresh.nonce,
                fresh.ciphertext,
                old_version,
                old_nonce,
            ],
        )
        .expect("rewrap user_provider_keys row");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::byok::{self, BYOK_ENV_LOCK};
    use base64::{engine::general_purpose::STANDARD, Engine};

    fn test_db() -> Database {
        let dir = tempfile::tempdir().expect("tempdir");
        Database::open(&dir.path().join("test.db"))
    }

    fn with_test_kek<T>(f: impl FnOnce() -> T) -> T {
        // Shared with `byok::tests` — see `BYOK_ENV_LOCK`'s doc comment: two
        // separate locks in the same test binary would not exclude each
        // other and these both mutate the same process-global env vars.
        let _guard = BYOK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CORTEX_BYOK_KEK_V1", STANDARD.encode([3u8; 32]));
        std::env::set_var("CORTEX_BYOK_KEK_CURRENT", "1");
        let result = f();
        std::env::remove_var("CORTEX_BYOK_KEK_V1");
        std::env::remove_var("CORTEX_BYOK_KEK_CURRENT");
        std::env::remove_var("CORTEX_BYOK_KEK_V2");
        result
    }

    #[test]
    fn upsert_then_list_then_delete_round_trips() {
        with_test_kek(|| {
            let db = test_db();
            let enc = byok::encrypt("user-a", "zen", "zen-test-SECRETSECRET1234").unwrap();
            db.upsert_provider_key("user-a", "zen", &enc, "1234", 1000);

            let listed = db.list_provider_keys("user-a");
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].last4, "1234");
            assert_eq!(listed[0].status, "active");

            assert!(db.delete_provider_key("user-a", "zen"));
            assert!(db.list_provider_keys("user-a").is_empty());
            assert!(!db.delete_provider_key("user-a", "zen"));
        });
    }

    #[test]
    fn user_b_cannot_read_or_delete_user_a_key() {
        with_test_kek(|| {
            let db = test_db();
            let enc = byok::encrypt("user-a", "zen", "zen-test-SECRETSECRET1234").unwrap();
            db.upsert_provider_key("user-a", "zen", &enc, "1234", 1000);

            assert!(db.list_provider_keys("user-b").is_empty());
            assert!(db.get_provider_key_row("user-b", "zen").is_none());
            assert!(!db.delete_provider_key("user-b", "zen"));
            // user-a's row is untouched by user-b's attempts.
            assert_eq!(db.list_provider_keys("user-a").len(), 1);
        });
    }

    #[test]
    fn rewrap_moves_rows_to_current_version_and_is_idempotent() {
        with_test_kek(|| {
            let db = test_db();
            let enc = byok::encrypt("user-a", "zen", "zen-test-SECRETSECRET1234").unwrap();
            assert_eq!(enc.key_version, 1);
            db.upsert_provider_key("user-a", "zen", &enc, "1234", 1000);

            std::env::set_var("CORTEX_BYOK_KEK_V2", STANDARD.encode([5u8; 32]));
            std::env::set_var("CORTEX_BYOK_KEK_CURRENT", "2");

            let rewrapped = byok::rewrap_provider_keys(&db).unwrap();
            assert_eq!(rewrapped.get("zen").copied(), Some(1));

            let row = db.get_provider_key_row("user-a", "zen").unwrap();
            assert_eq!(row.key_version, 2);
            let plaintext = byok::decrypt(
                "user-a",
                "zen",
                &byok::EncryptedKey {
                    key_version: row.key_version,
                    nonce: row.nonce.clone(),
                    ciphertext: row.ciphertext.clone(),
                },
            )
            .unwrap();
            assert_eq!(plaintext.expose_secret(), "zen-test-SECRETSECRET1234");

            // Second run is a no-op: nothing left below the current version.
            let rewrapped_again = byok::rewrap_provider_keys(&db).unwrap();
            assert!(rewrapped_again.is_empty());
        });
    }

    #[test]
    fn mark_rejected_with_stale_updated_at_leaves_row_active() {
        with_test_kek(|| {
            let db = test_db();
            let enc = byok::encrypt("user-a", "zen", "zen-test-SECRETSECRET1234").unwrap();
            db.upsert_provider_key("user-a", "zen", &enc, "1234", 1000);

            let stale_row = db.get_provider_key_row("user-a", "zen").unwrap();
            assert_eq!(stale_row.updated_at, 1000);

            // The customer replaces the key -- bumps `updated_at` -- while a
            // 401 for the old key is still in flight.
            let replacement = byok::encrypt("user-a", "zen", "zen-test-SECRETSECRET5678").unwrap();
            db.upsert_provider_key("user-a", "zen", &replacement, "5678", 2000);

            // The in-flight failure marks rejected using the stale
            // `updated_at` it read before the replacement landed.
            db.mark_provider_key_rejected("user-a", "zen", stale_row.updated_at, 3000);

            let row = db.get_provider_key_row("user-a", "zen").unwrap();
            assert_eq!(row.status, "active");
            assert_eq!(row.updated_at, 2000);
        });
    }
}
