//! Encrypted storage for a customer's own provider API key ("bring your own
//! key"). Today this is OpenCode Zen only; see `provider_keys.rs` for the
//! routes and `db/provider_keys.rs` for the row shape.
//!
//! # Split-key scheme (no server master key)
//!
//! There is **no server-held key-encryption key** (KEK) here. An earlier
//! version of this module kept one in `CORTEX_BYOK_KEK_V<n>` env vars; that
//! is gone. Instead, the 32-byte AES-256-GCM key for a given (user, device)
//! is generated **in the browser** (`crypto.getRandomValues`, see
//! `cortex/src/lib/zenDeviceKey.ts`) and sent to this server only inside the
//! `unlock` field of a save request, or the `X-Cortex-Key-Unlock` header of a
//! chat request. This server never persists that 32-byte secret anywhere: it
//! is used in memory, once, to encrypt or decrypt, and then dropped.
//!
//! ## Threat model this buys
//!
//! - **Database or server-at-rest theft** (a stolen backup, a leaked SQLite
//!   file): the attacker gets `nonce` and `ciphertext` columns only. Without
//!   the browser-held secret, those decrypt to nothing.
//! - **A page-script compromise (XSS) that can only read `localStorage`**:
//!   the attacker gets the browser's half (`device_id` + secret) but not the
//!   server's stored ciphertext, so they still cannot read a Zen key for a
//!   *different* device that never had that secret, and stealing the local
//!   value only exposes what that one device could already use for chat.
//! - **What this does not buy**: a live compromise of this server process
//!   while a chat request is in flight can still see the plaintext key for
//!   that one request, in memory, for the duration of that request -- the
//!   same as any server that ever calls out to a third-party API on a
//!   customer's behalf. There is no way to avoid this and still let Cortex's
//!   server make the outbound Zen call.
//!
//! This is a **dedicated** secret, not derived from `CLERK_SECRET_KEY` or any
//! other auth secret, and it never touches this module either --
//! `grep -rn "CLERK_SECRET_KEY" crates/api/src/byok.rs` must stay empty.
//!
//! # Cipher
//!
//! AES-256-GCM via the `aes-gcm` crate, already a default (non-`soma`)
//! dependency of this crate. `crates/soma-crypto` is feature-fenced behind
//! `soma` (ADR-0003) and BYOK must work with `soma` off, so it is not used
//! here.
//!
//! The nonce is 12 random bytes, freshly generated on every write -- reusing
//! a nonce under the same key breaks AES-GCM's confidentiality and integrity
//! guarantees, so each row's `nonce` column is independent even when the
//! same device re-saves a key.
//!
//! The additional authenticated data (AAD) binds the ciphertext to the exact
//! row it belongs to: `"cortex-byok:v2:" || user_id || ":" || provider ||
//! ":" || device_id`. A ciphertext moved to another user's row, another
//! provider's column, or another device's row fails to decrypt
//! (`aes_gcm::Error`, which carries no detail) rather than silently
//! decrypting into garbage that then gets sent to a provider on someone
//! else's behalf.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key};
use rand::Rng;
use zeroize::{Zeroize, Zeroizing};

/// Nonce length for AES-256-GCM, in bytes.
pub const NONCE_LEN: usize = 12;

/// Length of the browser-generated unlock secret, in bytes.
pub const UNLOCK_LEN: usize = 32;

/// A decrypted provider API key, held in memory only for the duration of one
/// request.
///
/// Deliberately opaque: no `Display`, no `Serialize`, and `Debug` is
/// hand-written to redact the value so a stray `{:?}` in a log line or panic
/// message cannot leak it. No `Clone` either -- cloning is how a value ends
/// up captured in more places than the one call site that needs it;
/// construct a fresh one from the decrypted bytes if more than one copy is
/// truly needed.
pub struct ZenApiKey(Zeroizing<String>);

impl ZenApiKey {
    pub fn new(secret: String) -> Self {
        Self(Zeroizing::new(secret))
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

/// One customer-provider-device key's on-disk representation.
#[derive(Debug)]
pub struct EncryptedKey {
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

/// Why a split-key operation failed. Deliberately does not carry the
/// underlying `aes_gcm::Error` (it has no useful detail) or any key
/// material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ByokError {
    /// Decryption failed: wrong unlock secret, wrong AAD (wrong
    /// user/provider/device), or a corrupted row.
    DecryptFailed,
    /// The `unlock` value did not decode to exactly [`UNLOCK_LEN`] bytes.
    MalformedUnlock,
}

impl std::fmt::Display for ByokError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ByokError::DecryptFailed => write!(f, "failed to decrypt stored key"),
            ByokError::MalformedUnlock => write!(f, "malformed unlock secret"),
        }
    }
}

impl std::error::Error for ByokError {}

fn aad(user_id: &str, provider: &str, device_id: &str) -> Vec<u8> {
    format!("cortex-byok:v2:{user_id}:{provider}:{device_id}").into_bytes()
}

/// Decodes a base64url-no-padding `unlock` value (from a save request body
/// or the `X-Cortex-Key-Unlock` header) into the 32-byte AES-256-GCM key.
/// Fails unless the decoded length is exactly [`UNLOCK_LEN`] -- this is the
/// entire validity check; the browser is trusted to generate it randomly.
pub fn decode_unlock(unlock: &str) -> Result<Zeroizing<[u8; UNLOCK_LEN]>, ByokError> {
    // `Zeroizing` wraps the decoded `Vec` immediately so every exit path
    // (wrong length included) wipes it on drop instead of leaving the
    // secret sitting in a freed heap allocation.
    let bytes: Zeroizing<Vec<u8>> = Zeroizing::new(
        URL_SAFE_NO_PAD
            .decode(unlock)
            .map_err(|_| ByokError::MalformedUnlock)?,
    );
    if bytes.len() != UNLOCK_LEN {
        return Err(ByokError::MalformedUnlock);
    }
    let mut array = [0u8; UNLOCK_LEN];
    array.copy_from_slice(&bytes);
    Ok(Zeroizing::new(array))
}

/// A shape check for a `device_id`: bounded length, and only characters a
/// UUID (or a similar client-generated id) would ever contain. This is not
/// cryptographic -- it exists so a malformed or oversized value from a
/// client is rejected with 400 before it reaches the database or the AAD,
/// not to constrain what a legitimate browser-generated `crypto.randomUUID()`
/// can produce.
pub fn valid_device_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Encrypt `plaintext` for `(user_id, provider, device_id)` under the
/// caller-supplied 32-byte unlock secret. The secret is never read from any
/// server-side config -- it always comes from the request that called this.
pub fn encrypt(
    user_id: &str,
    provider: &str,
    device_id: &str,
    unlock: &[u8; UNLOCK_LEN],
    plaintext: &str,
) -> Result<EncryptedKey, ByokError> {
    let key: &Key<Aes256Gcm> = unlock.into();
    let cipher = Aes256Gcm::new(key);

    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce_bytes);

    let ciphertext = cipher
        .encrypt(
            &nonce_bytes.into(),
            Payload {
                msg: plaintext.as_bytes(),
                aad: &aad(user_id, provider, device_id),
            },
        )
        .map_err(|_| ByokError::DecryptFailed)?;

    Ok(EncryptedKey {
        nonce: nonce_bytes.to_vec(),
        ciphertext,
    })
}

/// Decrypt a stored row for `(user_id, provider, device_id)` using the
/// caller-supplied unlock secret. Fails if the ciphertext/AAD/secret do not
/// match -- including a ciphertext that belongs to a different user,
/// provider, or device, or a secret that is simply wrong.
pub fn decrypt(
    user_id: &str,
    provider: &str,
    device_id: &str,
    unlock: &[u8; UNLOCK_LEN],
    encrypted: &EncryptedKey,
) -> Result<ZenApiKey, ByokError> {
    let key: &Key<Aes256Gcm> = unlock.into();
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
                aad: &aad(user_id, provider, device_id),
            },
        )
        .map_err(|_| ByokError::DecryptFailed)?;

    String::from_utf8(plaintext)
        .map(ZenApiKey::new)
        .map_err(|e| {
            // `FromUtf8Error` still owns the decrypted bytes on failure -- wipe
            // them before dropping instead of leaving the key sitting in freed
            // memory.
            let mut bytes = e.into_bytes();
            bytes.zeroize();
            ByokError::DecryptFailed
        })
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

/// Logs one warning naming any legacy `CORTEX_BYOK_KEK_*` env var that is
/// still set at startup -- names only, never values, since these were base64
/// key material under the old scheme. Purely informational: this server no
/// longer reads them for anything.
pub fn warn_if_legacy_kek_env_present() {
    let mut found: Vec<String> = Vec::new();
    if std::env::var("CORTEX_BYOK_KEK_CURRENT").is_ok() {
        found.push("CORTEX_BYOK_KEK_CURRENT".to_string());
    }
    let mut v = 1i64;
    loop {
        let name = format!("CORTEX_BYOK_KEK_V{v}");
        if std::env::var(&name).is_ok() {
            found.push(name);
            v += 1;
        } else {
            break;
        }
    }
    if !found.is_empty() {
        tracing::warn!(
            vars = ?found,
            "legacy CORTEX_BYOK_KEK_* env vars are set but no longer used (split-key BYOK has no server master key)"
        );
    }
}

/// Re-exported for callers/tests that still want to base64-encode arbitrary
/// bytes for fixtures (e.g. a fake 32-byte unlock secret) without pulling in
/// `base64` directly.
#[cfg(test)]
pub(crate) fn test_unlock_b64(bytes: [u8; UNLOCK_LEN]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unlock(byte: u8) -> [u8; UNLOCK_LEN] {
        [byte; UNLOCK_LEN]
    }

    #[test]
    fn round_trips() {
        let plaintext = "zen-test-SECRETSECRET1234";
        let key = unlock(7);
        let enc = encrypt("user-a", "zen", "device-1", &key, plaintext).expect("encrypt");
        assert!(!enc
            .ciphertext
            .windows(plaintext.len())
            .any(|w| w == plaintext.as_bytes()));
        let dec = decrypt("user-a", "zen", "device-1", &key, &enc).expect("decrypt");
        assert_eq!(dec.expose_secret(), plaintext);
    }

    #[test]
    fn encrypting_twice_uses_a_fresh_nonce_and_ciphertext() {
        let plaintext = "zen-test-SECRETSECRET1234";
        let key = unlock(7);
        let first = encrypt("user-a", "zen", "device-1", &key, plaintext).expect("encrypt 1");
        let second = encrypt("user-a", "zen", "device-1", &key, plaintext).expect("encrypt 2");
        assert_ne!(first.nonce, second.nonce, "nonces must not repeat");
        assert_ne!(
            first.ciphertext, second.ciphertext,
            "ciphertexts must differ across independent encryptions"
        );
    }

    #[test]
    fn wrong_user_cannot_decrypt() {
        let key = unlock(7);
        let enc = encrypt("user-a", "zen", "device-1", &key, "secret-key-value").expect("encrypt");
        let err = decrypt("user-b", "zen", "device-1", &key, &enc).unwrap_err();
        assert_eq!(err, ByokError::DecryptFailed);
    }

    #[test]
    fn wrong_provider_cannot_decrypt() {
        let key = unlock(7);
        let enc = encrypt("user-a", "zen", "device-1", &key, "secret-key-value").expect("encrypt");
        let err = decrypt("user-a", "other", "device-1", &key, &enc).unwrap_err();
        assert_eq!(err, ByokError::DecryptFailed);
    }

    #[test]
    fn wrong_device_cannot_decrypt() {
        let key = unlock(7);
        let enc = encrypt("user-a", "zen", "device-1", &key, "secret-key-value").expect("encrypt");
        let err = decrypt("user-a", "zen", "device-2", &key, &enc).unwrap_err();
        assert_eq!(err, ByokError::DecryptFailed);
    }

    #[test]
    fn wrong_secret_cannot_decrypt() {
        let key = unlock(7);
        let other = unlock(9);
        let enc = encrypt("user-a", "zen", "device-1", &key, "secret-key-value").expect("encrypt");
        let err = decrypt("user-a", "zen", "device-1", &other, &enc).unwrap_err();
        assert_eq!(err, ByokError::DecryptFailed);
    }

    #[test]
    fn unlock_must_decode_to_32_bytes() {
        assert!(decode_unlock(&test_unlock_b64(unlock(1))).is_ok());
        assert_eq!(
            decode_unlock("not-base64!!!").unwrap_err(),
            ByokError::MalformedUnlock
        );
        // 16 bytes, valid base64url, wrong length.
        let short = URL_SAFE_NO_PAD.encode([1u8; 16]);
        assert_eq!(
            decode_unlock(&short).unwrap_err(),
            ByokError::MalformedUnlock
        );
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

    #[test]
    fn device_id_shape_check() {
        assert!(valid_device_id("3fa85f64-5717-4562-b3fc-2c963f66afa6"));
        assert!(!valid_device_id(""));
        assert!(!valid_device_id(&"a".repeat(65)));
        assert!(!valid_device_id("has a space"));
        assert!(!valid_device_id("has/slash"));
    }
}
