//! Encrypted storage for a customer's own provider API key ("bring your own
//! key"). Today this is OpenCode Zen only; see `provider_keys.rs` for the
//! routes and `db/provider_keys.rs` for the row shape.
//!
//! # Master key (KEK)
//!
//! The key-encryption key comes from a versioned environment variable,
//! `CORTEX_BYOK_KEK_V<n>`, 32 random bytes, base64-encoded (`openssl rand
//! -base64 32`). `CORTEX_BYOK_KEK_CURRENT` names which version new writes
//! use; reads use whichever version a row's `key_version` column says.
//!
//! This is a **dedicated** secret, not derived from `CLERK_SECRET_KEY` or any
//! other auth secret. A deleted helper in this codebase once derived a
//! storage key from the Clerk secret; rotating Clerk would have silently made
//! every stored customer key unreadable, and it tied an auth secret to a
//! storage secret for no reason. Do not bring that back —
//! `grep -rn "CLERK_SECRET_KEY" crates/api/src/byok.rs` must stay empty.
//!
//! **No KEK means the feature is off.** There is no dev fallback key in any
//! build, test or otherwise — tests set `CORTEX_BYOK_KEK_V1` explicitly. The
//! old helper's `"dev-only-insecure-key-replace-me!"` must not come back.
//!
//! # Cipher
//!
//! AES-256-GCM via the `aes-gcm` crate, already a default (non-`soma`)
//! dependency of this crate. `crates/soma-crypto` is feature-fenced behind
//! `soma` (ADR-0003) and BYOK must work with `soma` off, so it is not used
//! here.
//!
//! The nonce is 12 random bytes, freshly generated on every write — reusing a
//! nonce under the same key breaks AES-GCM's confidentiality and integrity
//! guarantees, so each row's `nonce` column is independent even when a key is
//! replaced.
//!
//! The additional authenticated data (AAD) binds the ciphertext to the row it
//! belongs to: `"cortex-byok:v1:" || user_id || ":" || provider`. A ciphertext
//! moved to another user's row, or to another provider's column, fails to
//! decrypt (`aes_gcm::Error`, which carries no detail) rather than silently
//! decrypting into garbage that then gets sent to a provider on someone
//! else's behalf.

use std::collections::HashMap;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key};
use base64::{engine::general_purpose::STANDARD, Engine};
use rand::Rng;

/// Nonce length for AES-256-GCM, in bytes.
pub const NONCE_LEN: usize = 12;

/// A decrypted provider API key, held in memory only for the duration of one
/// request.
///
/// Deliberately opaque: no `Display`, no `Serialize`, and `Debug` is
/// hand-written to redact the value so a stray `{:?}` in a log line or panic
/// message cannot leak it. No `Clone` either — cloning is how a value ends up
/// captured in more places than the one call site that needs it; construct
/// a fresh one from the decrypted bytes if more than one copy is truly
/// needed.
pub struct ZenApiKey(String);

impl ZenApiKey {
    pub fn new(secret: String) -> Self {
        Self(secret)
    }

    /// The plaintext key. Named loudly so a caller cannot reach for it by
    /// accident the way `.0` or an `AsRef` conversion would let them.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    /// Last 4 characters, for the write-only display contract: routes return
    /// this and nothing else that identifies the key.
    pub fn last4(&self) -> String {
        last4_of(&self.0)
    }
}

impl std::fmt::Debug for ZenApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ZenApiKey").field(&"<redacted>").finish()
    }
}

pub fn last4_of(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() <= 4 {
        chars.into_iter().collect()
    } else {
        chars[chars.len() - 4..].iter().collect()
    }
}

/// One customer-provider key's on-disk representation.
#[derive(Debug)]
pub struct EncryptedKey {
    pub key_version: i64,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

/// Why a KEK operation failed. Deliberately does not carry the underlying
/// `aes_gcm::Error` (it has no useful detail) or any key material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ByokError {
    /// No `CORTEX_BYOK_KEK_V<current>` is configured. The feature is off.
    NotConfigured,
    /// A specific `key_version` was requested but that KEK version's env var
    /// is unset. This is the "operator retired a KEK a row still needs"
    /// case — recoverable by the customer re-entering their key, not a bug.
    VersionUnavailable(i64),
    /// Decryption failed: wrong key, wrong AAD (wrong user/provider), or a
    /// corrupted row.
    DecryptFailed,
    /// The configured KEK is not valid 32-byte base64.
    MalformedKek,
}

impl std::fmt::Display for ByokError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ByokError::NotConfigured => write!(f, "provider keys are not enabled"),
            ByokError::VersionUnavailable(v) => {
                write!(f, "key encryption key version {v} is not available")
            }
            ByokError::DecryptFailed => write!(f, "failed to decrypt stored key"),
            ByokError::MalformedKek => write!(f, "master key is malformed"),
        }
    }
}

impl std::error::Error for ByokError {}

/// Reads `CORTEX_BYOK_KEK_V<n>` / `CORTEX_BYOK_KEK_CURRENT` from the process
/// environment on every call. Env vars, not a cached snapshot, so an operator
/// rotating in a fresh version via the deploy's secret store takes effect on
/// the next request without a restart-timed race — the tradeoff is one env
/// lookup per encrypt/decrypt, which is negligible next to the AES-GCM call
/// itself.
pub struct KekRing;

impl KekRing {
    /// The version new writes should use, or `None` if BYOK is off (no
    /// `CORTEX_BYOK_KEK_CURRENT`, or that version's key is unset).
    pub fn current_version() -> Option<i64> {
        let current: i64 = std::env::var("CORTEX_BYOK_KEK_CURRENT")
            .ok()?
            .trim()
            .parse()
            .ok()?;
        Self::load(current).ok()?;
        Some(current)
    }

    /// Whether BYOK is configured at all. Routes use this to return 503
    /// "provider keys are not enabled" instead of touching the database.
    pub fn enabled() -> bool {
        Self::current_version().is_some()
    }

    fn load(version: i64) -> Result<[u8; 32], ByokError> {
        let var = format!("CORTEX_BYOK_KEK_V{version}");
        let raw = std::env::var(&var).map_err(|_| ByokError::VersionUnavailable(version))?;
        let bytes = STANDARD
            .decode(raw.trim())
            .map_err(|_| ByokError::MalformedKek)?;
        let arr: [u8; 32] = bytes.try_into().map_err(|_| ByokError::MalformedKek)?;
        Ok(arr)
    }

    /// Every KEK version currently configured, for `rewrap_provider_keys`
    /// and admin/introspection. Scans `CORTEX_BYOK_KEK_V1` upward and stops
    /// at the first gap, which matches how versions are meant to be assigned
    /// (a contiguous, ever-increasing sequence; retiring one only removes
    /// its own env var once nothing references it, per the module doc).
    pub fn configured_versions() -> Vec<i64> {
        let mut versions = Vec::new();
        let mut v = 1i64;
        loop {
            if std::env::var(format!("CORTEX_BYOK_KEK_V{v}")).is_ok() {
                versions.push(v);
                v += 1;
            } else {
                break;
            }
        }
        versions
    }
}

fn aad(user_id: &str, provider: &str) -> Vec<u8> {
    format!("cortex-byok:v1:{user_id}:{provider}").into_bytes()
}

/// Encrypt `plaintext` for `(user_id, provider)` under the current KEK
/// version. Fails with [`ByokError::NotConfigured`] if no current KEK is set
/// — callers must check this before ever prompting for or receiving a key.
pub fn encrypt(user_id: &str, provider: &str, plaintext: &str) -> Result<EncryptedKey, ByokError> {
    let version = KekRing::current_version().ok_or(ByokError::NotConfigured)?;
    let key_bytes = KekRing::load(version)?;
    let key: &Key<Aes256Gcm> = &key_bytes.into();
    let cipher = Aes256Gcm::new(key);

    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce_bytes);

    let ciphertext = cipher
        .encrypt(
            &nonce_bytes.into(),
            Payload {
                msg: plaintext.as_bytes(),
                aad: &aad(user_id, provider),
            },
        )
        .map_err(|_| ByokError::DecryptFailed)?;

    Ok(EncryptedKey {
        key_version: version,
        nonce: nonce_bytes.to_vec(),
        ciphertext,
    })
}

/// Decrypt a stored row for `(user_id, provider)`. Fails if the row's
/// `key_version` KEK is unavailable, or if the ciphertext/AAD do not match —
/// including a ciphertext that belongs to a different user or provider.
pub fn decrypt(
    user_id: &str,
    provider: &str,
    encrypted: &EncryptedKey,
) -> Result<ZenApiKey, ByokError> {
    let key_bytes = KekRing::load(encrypted.key_version)?;
    let key: &Key<Aes256Gcm> = &key_bytes.into();
    let cipher = Aes256Gcm::new(key);

    if encrypted.nonce.len() != NONCE_LEN {
        return Err(ByokError::DecryptFailed);
    }
    let nonce_bytes: [u8; NONCE_LEN] = encrypted
        .nonce
        .as_slice()
        .try_into()
        .map_err(|_| ByokError::DecryptFailed)?;

    let plaintext = cipher
        .decrypt(
            &nonce_bytes.into(),
            Payload {
                msg: &encrypted.ciphertext,
                aad: &aad(user_id, provider),
            },
        )
        .map_err(|_| ByokError::DecryptFailed)?;

    String::from_utf8(plaintext)
        .map(ZenApiKey::new)
        .map_err(|_| ByokError::DecryptFailed)
}

/// Re-encrypt a row under the current KEK version if it is not already
/// there. Idempotent: called again on an already-current row, `should_rewrap`
/// is false and nothing happens.
pub fn should_rewrap(row_version: i64, current_version: i64) -> bool {
    row_version != current_version
}

/// Format validation for a submitted key, independent of any live check
/// against the provider. 8 to 512 printable ASCII characters, no whitespace.
pub fn validate_key_format(candidate: &str) -> Result<(), &'static str> {
    if candidate.len() < 8 || candidate.len() > 512 {
        return Err("key must be between 8 and 512 characters");
    }
    if !candidate.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
        return Err("key must be printable ASCII");
    }
    if candidate.chars().any(|c| c.is_whitespace()) {
        return Err("key must not contain whitespace");
    }
    Ok(())
}

/// Rewraps every stored row whose `key_version` is not the current one,
/// re-encrypting it under the current KEK and leaving everything else about
/// the row unchanged. Returns how many rows were rewrapped, keyed by
/// provider, for a startup log line.
///
/// Bounded and idempotent: a row already at the current version is skipped,
/// so running this at every startup costs nothing once a fleet has finished
/// rotating. It does nothing (`current_version` is `None`) when BYOK is off.
///
/// A row that fails to decrypt or re-encrypt is left as-is (for the customer
/// to re-enter their key) rather than failing the whole batch; the number of
/// such rows is logged here — never the row's user, provider, or key
/// material — so a stuck rotation is visible instead of silently skipped.
pub fn rewrap_provider_keys(db: &crate::db::Database) -> Result<HashMap<String, usize>, ByokError> {
    let Some(current) = KekRing::current_version() else {
        return Ok(HashMap::new());
    };
    let rows = db.list_provider_key_rows_needing_rewrap(current);
    let mut rewrapped: HashMap<String, usize> = HashMap::new();
    let mut skipped: usize = 0;
    for row in rows {
        let old_version = row.key_version;
        let old_nonce = row.nonce.clone();
        let encrypted = EncryptedKey {
            key_version: row.key_version,
            nonce: row.nonce.clone(),
            ciphertext: row.ciphertext.clone(),
        };
        let plaintext = match decrypt(&row.user_id, &row.provider, &encrypted) {
            Ok(p) => p,
            Err(_) => {
                // Unreadable under any KEK we have; leave it for the
                // customer to re-enter.
                skipped += 1;
                continue;
            }
        };
        match encrypt(&row.user_id, &row.provider, plaintext.expose_secret()) {
            Ok(fresh) => {
                db.rewrap_provider_key_row(
                    &row.user_id,
                    &row.provider,
                    old_version,
                    &old_nonce,
                    &fresh,
                );
                *rewrapped.entry(row.provider.clone()).or_insert(0) += 1;
            }
            Err(_) => skipped += 1,
        }
    }
    if skipped > 0 {
        tracing::warn!(
            skipped,
            "BYOK rewrap: rows skipped (decrypt/encrypt failed)"
        );
    }
    Ok(rewrapped)
}

// Environment variables are process-global, and cargo runs unit tests across
// this crate's whole test binary (including `db::provider_keys::tests`) on
// multiple threads by default — two tests racing to set different
// `CORTEX_BYOK_KEK_*` vars would be flaky. One lock, shared by every test
// module that touches these vars, so two locks in the same binary can't fail
// to exclude each other.
#[cfg(test)]
pub(crate) static BYOK_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    fn with_test_kek<T>(f: impl FnOnce() -> T) -> T {
        let _guard = BYOK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CORTEX_BYOK_KEK_V1", STANDARD.encode([7u8; 32]));
        std::env::set_var("CORTEX_BYOK_KEK_CURRENT", "1");
        let result = f();
        std::env::remove_var("CORTEX_BYOK_KEK_V1");
        std::env::remove_var("CORTEX_BYOK_KEK_CURRENT");
        std::env::remove_var("CORTEX_BYOK_KEK_V2");
        result
    }

    #[test]
    fn round_trips() {
        with_test_kek(|| {
            let plaintext = "zen-test-SECRETSECRET1234";
            let enc = encrypt("user-a", "zen", plaintext).expect("encrypt");
            assert!(!enc
                .ciphertext
                .windows(plaintext.len())
                .any(|w| w == plaintext.as_bytes()));
            let dec = decrypt("user-a", "zen", &enc).expect("decrypt");
            assert_eq!(dec.expose_secret(), plaintext);
        });
    }

    #[test]
    fn encrypting_twice_uses_a_fresh_nonce_and_ciphertext() {
        with_test_kek(|| {
            let plaintext = "zen-test-SECRETSECRET1234";
            let first = encrypt("user-a", "zen", plaintext).expect("encrypt 1");
            let second = encrypt("user-a", "zen", plaintext).expect("encrypt 2");
            assert_ne!(first.nonce, second.nonce, "nonces must not repeat");
            assert_ne!(
                first.ciphertext, second.ciphertext,
                "ciphertexts must differ across independent encryptions"
            );
        });
    }

    #[test]
    fn wrong_user_cannot_decrypt() {
        with_test_kek(|| {
            let enc = encrypt("user-a", "zen", "secret-key-value").expect("encrypt");
            let err = decrypt("user-b", "zen", &enc).unwrap_err();
            assert_eq!(err, ByokError::DecryptFailed);
        });
    }

    #[test]
    fn wrong_provider_cannot_decrypt() {
        with_test_kek(|| {
            let enc = encrypt("user-a", "zen", "secret-key-value").expect("encrypt");
            let err = decrypt("user-a", "other", &enc).unwrap_err();
            assert_eq!(err, ByokError::DecryptFailed);
        });
    }

    #[test]
    fn no_kek_means_not_configured() {
        let _guard = BYOK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("CORTEX_BYOK_KEK_V1");
        std::env::remove_var("CORTEX_BYOK_KEK_CURRENT");
        assert!(!KekRing::enabled());
        let err = encrypt("user-a", "zen", "secret-key-value").unwrap_err();
        assert_eq!(err, ByokError::NotConfigured);
    }

    #[test]
    fn rotation_rewraps_under_new_version() {
        with_test_kek(|| {
            let old = encrypt("user-a", "zen", "secret-key-value").expect("encrypt v1");
            assert_eq!(old.key_version, 1);

            std::env::set_var("CORTEX_BYOK_KEK_V2", STANDARD.encode([9u8; 32]));
            std::env::set_var("CORTEX_BYOK_KEK_CURRENT", "2");

            assert!(should_rewrap(old.key_version, 2));
            let plaintext = decrypt("user-a", "zen", &old).expect("v1 key still readable");
            let fresh = encrypt("user-a", "zen", plaintext.expose_secret()).expect("encrypt v2");
            assert_eq!(fresh.key_version, 2);
            assert!(!should_rewrap(fresh.key_version, 2));
            let dec = decrypt("user-a", "zen", &fresh).expect("decrypt v2");
            assert_eq!(dec.expose_secret(), plaintext.expose_secret());
        });
    }

    #[test]
    fn last4_of_short_key() {
        assert_eq!(last4_of("ab"), "ab");
        assert_eq!(last4_of("abcdefgh"), "efgh");
    }

    #[test]
    fn validate_key_format_rejects_short_and_whitespace() {
        assert!(validate_key_format("short").is_err());
        assert!(validate_key_format("has a space here").is_err());
        assert!(validate_key_format(&"a".repeat(513)).is_err());
        assert!(validate_key_format("zen-test-SECRETSECRET1234").is_ok());
    }

    #[test]
    fn debug_never_shows_the_key() {
        let key = ZenApiKey::new("zen-test-SECRETSECRET1234".to_string());
        let shown = format!("{key:?}");
        assert!(!shown.contains("SECRETSECRET"));
    }
}
