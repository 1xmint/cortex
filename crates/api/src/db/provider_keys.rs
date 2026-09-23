//! Row-level storage for `user_provider_device_keys` (customer BYOK provider
//! keys, one row per user + provider + device -- see `crates/api/src/byok.rs`
//! for the split-key scheme).
//!
//! Every query here is scoped by `user_id` in the `WHERE` clause, not just by
//! the AAD binding in `byok.rs` -- belt and suspenders. A caller that forgets
//! to check the AAD result still cannot read or touch another user's row,
//! because the row is never returned or updated in the first place.

use super::*;
use crate::byok::EncryptedKey;
use rusqlite::OptionalExtension;

/// Maximum device rows per (user, provider), enforced atomically inside
/// `upsert_provider_key_capped`'s transaction.
pub const MAX_DEVICES_PER_PROVIDER: i64 = 10;

/// Why `upsert_provider_key_capped` refused to write.
#[derive(Debug)]
pub enum ProviderKeyCapError {
    /// This is a genuinely new device and `(user_id, provider)` already has
    /// `MAX_DEVICES_PER_PROVIDER` rows.
    CapReached,
    /// A `rusqlite` failure unrelated to the cap (begin/read/write/commit).
    Db(String),
}

/// What `GET /api/provider-keys` returns. Never carries `nonce` or
/// `ciphertext` -- those fields do not exist on this type at all, so a future
/// `#[derive(Serialize)]` on the wrong struct cannot accidentally leak them.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderKeySummary {
    pub provider: String,
    pub device_id: String,
    pub last4: String,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_used_at: Option<i64>,
}

/// A row as needed to decrypt and use the key server-side. Also never
/// `Serialize` -- this type never crosses the HTTP boundary.
pub struct ProviderKeyRow {
    pub user_id: String,
    pub provider: String,
    pub device_id: String,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
    pub status: String,
    /// The row's `updated_at` as read here. Callers that later act on this
    /// key (e.g. `mark_provider_key_rejected`) pass it back so the update
    /// only applies if the row has not changed since -- a user replacing
    /// the key on this device while an old request is still in flight must
    /// not have the new key marked rejected because of the old one's
    /// failure.
    pub updated_at: i64,
}

impl Database {
    pub fn list_provider_keys(&self, user_id: &str) -> Vec<ProviderKeySummary> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT provider, device_id, last4, status, created_at, updated_at, last_used_at
                 FROM user_provider_device_keys WHERE user_id = ?1
                 ORDER BY provider, created_at",
            )
            .expect("prepare list_provider_keys");
        let rows = stmt
            .query_map(params![user_id], |row| {
                Ok(ProviderKeySummary {
                    provider: row.get(0)?,
                    device_id: row.get(1)?,
                    last4: row.get(2)?,
                    status: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                    last_used_at: row.get(6)?,
                })
            })
            .expect("query list_provider_keys");
        rows.filter_map(Result::ok).collect()
    }

    /// How many device rows already exist for `(user_id, provider)`, for the
    /// route's device-cap check (`byok::MAX_DEVICES_PER_PROVIDER`).
    #[cfg(test)]
    pub fn count_provider_key_devices(&self, user_id: &str, provider: &str) -> i64 {
        let conn = self.conn();
        conn.query_row(
            "SELECT COUNT(*) FROM user_provider_device_keys WHERE user_id = ?1 AND provider = ?2",
            params![user_id, provider],
            |row| row.get(0),
        )
        .expect("count_provider_key_devices")
    }

    /// Insert or replace the key for `(user_id, provider, device_id)`,
    /// enforcing `MAX_DEVICES_PER_PROVIDER` for genuinely new devices.
    ///
    /// The existence check, the device count, and the write all happen
    /// inside one `BEGIN IMMEDIATE` transaction on this connection's single
    /// lock, so two concurrent saves for new devices on the same
    /// `(user_id, provider)` cannot both read "9 devices, room for one
    /// more" and both insert -- `BEGIN IMMEDIATE` takes the writer lock up
    /// front, so the second save blocks until the first commits and then
    /// sees the up-to-date count. Always resets `status` to `'active'`:
    /// replacing a rejected key on the same device should make it
    /// selectable again on the next chat request.
    pub fn upsert_provider_key_capped(
        &self,
        user_id: &str,
        provider: &str,
        device_id: &str,
        encrypted: &EncryptedKey,
        last4: &str,
        now_ms: i64,
    ) -> Result<(), ProviderKeyCapError> {
        let conn = self.conn();

        conn.execute("BEGIN IMMEDIATE", [])
            .map_err(|e| ProviderKeyCapError::Db(format!("failed to begin transaction: {e}")))?;

        let existing: bool = conn
            .query_row(
                "SELECT 1 FROM user_provider_device_keys
                 WHERE user_id = ?1 AND provider = ?2 AND device_id = ?3",
                params![user_id, provider, device_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|e| {
                conn.execute("ROLLBACK", []).ok();
                ProviderKeyCapError::Db(format!("failed to check existing device row: {e}"))
            })?
            .is_some();

        if !existing {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM user_provider_device_keys
                     WHERE user_id = ?1 AND provider = ?2",
                    params![user_id, provider],
                    |row| row.get(0),
                )
                .map_err(|e| {
                    conn.execute("ROLLBACK", []).ok();
                    ProviderKeyCapError::Db(format!("failed to count device rows: {e}"))
                })?;
            if count >= MAX_DEVICES_PER_PROVIDER {
                conn.execute("ROLLBACK", []).ok();
                return Err(ProviderKeyCapError::CapReached);
            }
        }

        let write = conn.execute(
            "INSERT INTO user_provider_device_keys
                (user_id, provider, device_id, nonce, ciphertext, last4, status,
                 created_at, updated_at, last_used_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', ?7, ?7, NULL)
             ON CONFLICT(user_id, provider, device_id) DO UPDATE SET
                nonce = excluded.nonce,
                ciphertext = excluded.ciphertext,
                last4 = excluded.last4,
                status = 'active',
                updated_at = excluded.updated_at",
            params![
                user_id,
                provider,
                device_id,
                encrypted.nonce,
                encrypted.ciphertext,
                last4,
                now_ms,
            ],
        );
        if let Err(e) = write {
            conn.execute("ROLLBACK", []).ok();
            return Err(ProviderKeyCapError::Db(format!(
                "failed to upsert user_provider_device_keys: {e}"
            )));
        }

        conn.execute("COMMIT", [])
            .map_err(|e| ProviderKeyCapError::Db(format!("failed to commit transaction: {e}")))?;
        Ok(())
    }

    /// Test/fixture convenience over `upsert_provider_key_capped` that
    /// panics on any error, including the cap -- production code always
    /// goes through `upsert_provider_key_capped` so the route can map a
    /// reached cap to its 409 response.
    #[cfg(test)]
    pub fn upsert_provider_key(
        &self,
        user_id: &str,
        provider: &str,
        device_id: &str,
        encrypted: &EncryptedKey,
        last4: &str,
        now_ms: i64,
    ) {
        self.upsert_provider_key_capped(user_id, provider, device_id, encrypted, last4, now_ms)
            .expect("upsert_provider_key_capped");
    }

    /// Hard delete one device's row. Returns whether a row existed.
    pub fn delete_provider_key_device(
        &self,
        user_id: &str,
        provider: &str,
        device_id: &str,
    ) -> bool {
        let conn = self.conn();
        let affected = conn
            .execute(
                "DELETE FROM user_provider_device_keys
                 WHERE user_id = ?1 AND provider = ?2 AND device_id = ?3",
                params![user_id, provider, device_id],
            )
            .expect("delete user_provider_device_keys (one device)");
        affected > 0
    }

    /// Hard delete every device's row for `(user_id, provider)`. Returns
    /// whether at least one row existed.
    pub fn delete_provider_keys_all_devices(&self, user_id: &str, provider: &str) -> bool {
        let conn = self.conn();
        let affected = conn
            .execute(
                "DELETE FROM user_provider_device_keys WHERE user_id = ?1 AND provider = ?2",
                params![user_id, provider],
            )
            .expect("delete user_provider_device_keys (all devices)");
        affected > 0
    }

    /// The row needed to decrypt and use this user's key on this device, if
    /// one exists.
    pub fn get_provider_key_row(
        &self,
        user_id: &str,
        provider: &str,
        device_id: &str,
    ) -> Option<ProviderKeyRow> {
        let conn = self.conn();
        conn.query_row(
            "SELECT user_id, provider, device_id, nonce, ciphertext, status, updated_at
             FROM user_provider_device_keys
             WHERE user_id = ?1 AND provider = ?2 AND device_id = ?3",
            params![user_id, provider, device_id],
            |row| {
                Ok(ProviderKeyRow {
                    user_id: row.get(0)?,
                    provider: row.get(1)?,
                    device_id: row.get(2)?,
                    nonce: row.get(3)?,
                    ciphertext: row.get(4)?,
                    status: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            },
        )
        .ok()
    }

    /// Just the row's `status`, for `GET /api/chat/models` -- that endpoint
    /// never needs the ciphertext, only whether this device's key is usable.
    pub fn get_provider_key_status(
        &self,
        user_id: &str,
        provider: &str,
        device_id: &str,
    ) -> Option<String> {
        let conn = self.conn();
        conn.query_row(
            "SELECT status FROM user_provider_device_keys
             WHERE user_id = ?1 AND provider = ?2 AND device_id = ?3",
            params![user_id, provider, device_id],
            |row| row.get(0),
        )
        .ok()
    }

    /// Marks the key rejected, but only if the row is still the same one the
    /// caller decrypted -- `expected_updated_at` is that row's `updated_at`
    /// as read at decrypt time. If the customer has since replaced the key
    /// on this device (which bumps `updated_at`), this is a no-op: a 401 for
    /// the old key must never mark the new one rejected.
    pub fn mark_provider_key_rejected(
        &self,
        user_id: &str,
        provider: &str,
        device_id: &str,
        expected_updated_at: i64,
        now_ms: i64,
    ) {
        let conn = self.conn();
        conn.execute(
            "UPDATE user_provider_device_keys SET status = 'rejected', updated_at = ?5
             WHERE user_id = ?1 AND provider = ?2 AND device_id = ?3 AND updated_at = ?4",
            params![user_id, provider, device_id, expected_updated_at, now_ms],
        )
        .expect("mark provider key rejected");
    }

    pub fn touch_provider_key_last_used(
        &self,
        user_id: &str,
        provider: &str,
        device_id: &str,
        now_ms: i64,
    ) {
        let conn = self.conn();
        conn.execute(
            "UPDATE user_provider_device_keys SET last_used_at = ?4
             WHERE user_id = ?1 AND provider = ?2 AND device_id = ?3",
            params![user_id, provider, device_id, now_ms],
        )
        .expect("touch provider key last_used_at");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::byok;

    fn test_db() -> Database {
        let dir = tempfile::tempdir().expect("tempdir");
        Database::open(&dir.path().join("test.db"))
    }

    fn unlock(byte: u8) -> [u8; byok::UNLOCK_LEN] {
        [byte; byok::UNLOCK_LEN]
    }

    #[test]
    fn upsert_then_list_then_delete_round_trips() {
        let db = test_db();
        let key = unlock(3);
        let enc = byok::encrypt(
            "user-a",
            "zen",
            "device-1",
            &key,
            "zen-test-SECRETSECRET1234",
        )
        .unwrap();
        db.upsert_provider_key("user-a", "zen", "device-1", &enc, "1234", 1000);

        let listed = db.list_provider_keys("user-a");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].last4, "1234");
        assert_eq!(listed[0].status, "active");
        assert_eq!(listed[0].device_id, "device-1");

        assert!(db.delete_provider_key_device("user-a", "zen", "device-1"));
        assert!(db.list_provider_keys("user-a").is_empty());
        assert!(!db.delete_provider_key_device("user-a", "zen", "device-1"));
    }

    #[test]
    fn multiple_devices_coexist_and_delete_all_removes_every_row() {
        let db = test_db();
        let key = unlock(3);
        for device in ["device-1", "device-2", "device-3"] {
            let enc =
                byok::encrypt("user-a", "zen", device, &key, "zen-test-SECRETSECRET1234").unwrap();
            db.upsert_provider_key("user-a", "zen", device, &enc, "1234", 1000);
        }
        assert_eq!(db.list_provider_keys("user-a").len(), 3);
        assert_eq!(db.count_provider_key_devices("user-a", "zen"), 3);

        assert!(db.delete_provider_keys_all_devices("user-a", "zen"));
        assert!(db.list_provider_keys("user-a").is_empty());
        assert!(!db.delete_provider_keys_all_devices("user-a", "zen"));
    }

    #[test]
    fn user_b_cannot_read_or_delete_user_a_key() {
        let db = test_db();
        let key = unlock(3);
        let enc = byok::encrypt(
            "user-a",
            "zen",
            "device-1",
            &key,
            "zen-test-SECRETSECRET1234",
        )
        .unwrap();
        db.upsert_provider_key("user-a", "zen", "device-1", &enc, "1234", 1000);

        assert!(db.list_provider_keys("user-b").is_empty());
        assert!(db
            .get_provider_key_row("user-b", "zen", "device-1")
            .is_none());
        assert!(!db.delete_provider_key_device("user-b", "zen", "device-1"));
        // user-a's row is untouched by user-b's attempts.
        assert_eq!(db.list_provider_keys("user-a").len(), 1);
    }

    #[test]
    fn capped_upsert_refuses_an_11th_new_device_but_allows_replacing_an_existing_one() {
        let db = test_db();
        let key = unlock(3);

        for i in 0..MAX_DEVICES_PER_PROVIDER {
            let device = format!("device-{i}");
            let enc =
                byok::encrypt("user-a", "zen", &device, &key, "zen-test-SECRETSECRET1234").unwrap();
            db.upsert_provider_key_capped("user-a", "zen", &device, &enc, "1234", 1000)
                .expect("first 10 devices should be allowed");
        }
        assert_eq!(
            db.list_provider_keys("user-a").len(),
            MAX_DEVICES_PER_PROVIDER as usize
        );

        // An 11th, genuinely new device is refused.
        let enc = byok::encrypt(
            "user-a",
            "zen",
            "device-11th",
            &key,
            "zen-test-SECRETSECRET1234",
        )
        .unwrap();
        let err = db
            .upsert_provider_key_capped("user-a", "zen", "device-11th", &enc, "1234", 2000)
            .expect_err("11th new device should be refused");
        assert!(matches!(err, ProviderKeyCapError::CapReached));
        assert_eq!(
            db.list_provider_keys("user-a").len(),
            MAX_DEVICES_PER_PROVIDER as usize
        );

        // Replacing an existing device's row is still allowed at the cap.
        let replacement = byok::encrypt(
            "user-a",
            "zen",
            "device-0",
            &key,
            "zen-test-SECRETSECRET5678",
        )
        .unwrap();
        db.upsert_provider_key_capped("user-a", "zen", "device-0", &replacement, "5678", 3000)
            .expect("replacing an existing device should be allowed at the cap");
        assert_eq!(
            db.list_provider_keys("user-a").len(),
            MAX_DEVICES_PER_PROVIDER as usize
        );
        let row = db
            .get_provider_key_row("user-a", "zen", "device-0")
            .unwrap();
        assert_eq!(row.updated_at, 3000);
    }

    #[test]
    fn mark_rejected_with_stale_updated_at_leaves_row_active() {
        let db = test_db();
        let key = unlock(3);
        let enc = byok::encrypt(
            "user-a",
            "zen",
            "device-1",
            &key,
            "zen-test-SECRETSECRET1234",
        )
        .unwrap();
        db.upsert_provider_key("user-a", "zen", "device-1", &enc, "1234", 1000);

        let stale_row = db
            .get_provider_key_row("user-a", "zen", "device-1")
            .unwrap();
        assert_eq!(stale_row.updated_at, 1000);

        // The customer replaces the key on this device -- bumps `updated_at`
        // -- while a 401 for the old key is still in flight.
        let replacement = byok::encrypt(
            "user-a",
            "zen",
            "device-1",
            &key,
            "zen-test-SECRETSECRET5678",
        )
        .unwrap();
        db.upsert_provider_key("user-a", "zen", "device-1", &replacement, "5678", 2000);

        // The in-flight failure marks rejected using the stale
        // `updated_at` it read before the replacement landed.
        db.mark_provider_key_rejected("user-a", "zen", "device-1", stale_row.updated_at, 3000);

        let row = db
            .get_provider_key_row("user-a", "zen", "device-1")
            .unwrap();
        assert_eq!(row.status, "active");
        assert_eq!(row.updated_at, 2000);
    }
}
