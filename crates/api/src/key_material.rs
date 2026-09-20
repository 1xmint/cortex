//! Product-neutral API-key primitives: generate, prefix, hash.
//!
//! These three operations are shared by every bearer credential in this
//! workspace, and they are the *only* part that is shared. Everything above
//! them — which table a key resolves against, what principal it yields, what
//! scopes it carries — is product-specific and stays with its product.
//!
//! They were previously private to `agent_auth`, the HeyVera Socials linked-
//! agent module, whose lookups run against `social_linked_agents`. Cortex's
//! worker credential needs the same three operations and none of that
//! resolution, so the primitives moved here rather than Cortex reaching into
//! another product's auth module. `agent_auth` now calls into this module, and
//! its `hvak_` keys are byte-for-byte what they were before.
//!
//! # On hashing
//!
//! [`hash_api_key`] is a plain SHA-256 digest, and that is the correct choice
//! for these credentials — see the note on [`hash_api_key`] itself.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::Rng;
use sha2::{Digest, Sha256};

/// How many characters of a secret are kept for display.
///
/// The stored prefix exists so a human can tell two keys apart in a list. It is
/// deliberately short: the scheme tag (`hvak_`, `cwk_`) plus a handful of
/// base64 characters is enough to disambiguate and far too little to brute
/// force the remaining ~180 bits.
const DISPLAY_PREFIX_CHARS: usize = 12;

/// Entropy of the random portion of a key, in bytes.
///
/// 32 bytes from the OS CSPRNG. This number is the entire security argument for
/// the plain digest in [`hash_api_key`]; do not lower it.
const SECRET_BYTES: usize = 32;

/// A freshly issued key, in the three forms a caller needs.
///
/// `secret` is the only copy of the plaintext that will ever exist — nothing
/// here writes it anywhere. Show it to its owner once and drop it.
#[derive(Debug, Clone)]
pub struct IssuedKey {
    /// Full plaintext secret, including the scheme prefix. Never persisted.
    pub secret: String,
    /// Truncated, non-secret form, safe to store and to list.
    pub display_prefix: String,
    /// SHA-256 hex of `secret`. This is what gets persisted and looked up.
    pub hash: String,
}

/// Issue a new API key: `scheme_prefix` + 32 CSPRNG bytes (base64url, unpadded).
///
/// `scheme_prefix` is the product tag and is expected to end in `_`
/// (`hvak_`, `cwk_`). It is part of the hashed material, so a key issued under
/// one scheme cannot be replayed as another even if the random half collided.
pub fn generate_api_key(scheme_prefix: &str) -> IssuedKey {
    let mut bytes = [0u8; SECRET_BYTES];
    rand::rng().fill_bytes(&mut bytes);
    let secret = format!("{scheme_prefix}{}", URL_SAFE_NO_PAD.encode(bytes));
    let display_prefix = display_prefix(&secret);
    let hash = hash_api_key(&secret);
    IssuedKey {
        secret,
        display_prefix,
        hash,
    }
}

/// Display prefix: first 12 characters + ellipsis.
pub fn display_prefix(secret: &str) -> String {
    let take = secret.len().min(DISPLAY_PREFIX_CHARS);
    format!("{}…", &secret[..take])
}

/// SHA-256 hex of the full secret, scheme prefix included.
///
/// # Why a bare digest and not a KDF
///
/// A password is low-entropy and chosen by a human, so a stolen password
/// database is guessable and must be made expensive to guess — that is what
/// bcrypt/argon2 buy. These are not passwords. Each is 32 bytes straight from
/// the OS CSPRNG: 256 bits of uniform entropy that no human picked, could pick,
/// or could reuse from another site. There is no dictionary, no guessing
/// advantage, and nothing for a work factor to slow down — an attacker holding
/// the hash column must invert SHA-256 or exhaust 2^256.
///
/// A KDF here would buy nothing and cost per-request latency on every worker
/// frame and every agent write. The digest is the right primitive, and it is
/// only right *because* [`SECRET_BYTES`] is 32. If a caller ever hashes a
/// user-chosen string with this function, that argument collapses.
pub fn hash_api_key(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_key_carries_its_scheme_and_hashes_to_itself() {
        let key = generate_api_key("cwk_");
        assert!(key.secret.starts_with("cwk_"));
        // 32 bytes base64url-unpadded is 43 chars, plus the scheme tag.
        assert_eq!(key.secret.len(), "cwk_".len() + 43);
        assert_eq!(key.display_prefix.chars().count(), DISPLAY_PREFIX_CHARS + 1);
        assert!(key.display_prefix.ends_with('…'));
        assert_eq!(key.hash.len(), 64);
        assert_eq!(hash_api_key(&key.secret), key.hash);
        assert_ne!(key.hash, key.secret);
    }

    #[test]
    fn two_keys_are_not_the_same_key() {
        let a = generate_api_key("cwk_");
        let b = generate_api_key("cwk_");
        assert_ne!(a.secret, b.secret);
        assert_ne!(a.hash, b.hash);
    }

    /// The scheme tag is inside the hashed material, so the same random half
    /// under two schemes is two different credentials.
    #[test]
    fn the_scheme_prefix_is_part_of_the_hash() {
        let key = generate_api_key("cwk_");
        let random_half = key.secret.strip_prefix("cwk_").unwrap();
        assert_ne!(hash_api_key(&format!("hvak_{random_half}")), key.hash);
    }

    /// The display prefix must stay non-secret: a short head of the key only.
    #[test]
    fn display_prefix_reveals_only_the_head() {
        let key = generate_api_key("hvak_");
        let shown = key.display_prefix.trim_end_matches('…');
        assert_eq!(shown.len(), DISPLAY_PREFIX_CHARS);
        assert!(key.secret.starts_with(shown));
        assert!(key.secret.len() > key.display_prefix.len() * 2);
    }
}
