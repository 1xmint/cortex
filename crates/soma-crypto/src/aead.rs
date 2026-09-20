use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::XChaCha20Poly1305;

use crate::Error;

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;

pub fn seal(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    plaintext: &[u8],
    aad: &[u8],
) -> crate::Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let payload = Payload {
        msg: plaintext,
        aad,
    };
    cipher
        .encrypt(nonce.into(), payload)
        .map_err(|_| Error::AeadSealFailed)
}

pub fn open(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    ciphertext: &[u8],
    aad: &[u8],
) -> crate::Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let payload = Payload {
        msg: ciphertext,
        aad,
    };
    cipher
        .decrypt(nonce.into(), payload)
        .map_err(|_| Error::AeadOpenFailed)
}

use rand_core::RngCore;

pub fn generate_nonce() -> [u8; NONCE_LEN] {
    let mut nonce = [0u8; NONCE_LEN];
    rand_core::OsRng.fill_bytes(&mut nonce);
    nonce
}

pub fn generate_key() -> [u8; KEY_LEN] {
    let mut key = [0u8; KEY_LEN];
    rand_core::OsRng.fill_bytes(&mut key);
    key
}

#[cfg(test)]
mod tests {
    /// Path to a committed vector, resolved from the manifest rather than the
    /// working directory.
    fn vector_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("test-vectors")
            .join(name)
    }

    use super::*;

    #[test]
    fn seal_open_roundtrip() {
        let key = generate_key();
        let nonce = generate_nonce();
        let plaintext = b"soma sealed session interior";
        let aad = b"session-id:abc123";

        let ct = seal(&key, &nonce, plaintext, aad).unwrap();
        let pt = open(&key, &nonce, &ct, aad).unwrap();
        assert_eq!(&pt, plaintext);
    }

    #[test]
    fn wrong_key_fails() {
        let key1 = generate_key();
        let key2 = generate_key();
        let nonce = generate_nonce();

        let ct = seal(&key1, &nonce, b"secret", b"").unwrap();
        assert!(open(&key2, &nonce, &ct, b"").is_err());
    }

    #[test]
    fn wrong_aad_fails() {
        let key = generate_key();
        let nonce = generate_nonce();

        let ct = seal(&key, &nonce, b"secret", b"correct-aad").unwrap();
        assert!(open(&key, &nonce, &ct, b"wrong-aad").is_err());
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let key = generate_key();
        let nonce = generate_nonce();

        let mut ct = seal(&key, &nonce, b"secret", b"").unwrap();
        ct[0] ^= 1;
        assert!(open(&key, &nonce, &ct, b"").is_err());
    }

    #[test]
    fn empty_plaintext() {
        let key = generate_key();
        let nonce = generate_nonce();

        let ct = seal(&key, &nonce, b"", b"aad").unwrap();
        let pt = open(&key, &nonce, &ct, b"aad").unwrap();
        assert!(pt.is_empty());
    }

    #[test]
    fn committed_vectors_still_match_the_code() {
        let key = [0x42u8; KEY_LEN];
        let nonce = [0x07u8; NONCE_LEN];
        let plaintext = b"soma aead test vector";
        let aad = b"soma.room.v1";

        let ct = seal(&key, &nonce, plaintext, aad).unwrap();

        let vectors = serde_json::json!({
            "xchacha20_poly1305": [{
                "key_hex": hex::encode(key),
                "nonce_hex": hex::encode(nonce),
                "plaintext_hex": hex::encode(plaintext),
                "aad_hex": hex::encode(aad),
                "ciphertext_hex": hex::encode(&ct),
            }],
        });
        let json = serde_json::to_string_pretty(&vectors).unwrap();
        // Assert against the committed vector; do not rewrite it.
        //
        // These inputs are fixed, so this JSON is byte-identical on every run —
        // which is exactly why the old `std::fs::write` looked harmless. It was
        // not: a vector the test overwrites can never disagree with the code,
        // so it proves nothing, and the identical pattern in `composite.rs`
        // (random keypair) left a clean checkout dirty on every `cargo test`.
        // Same fix everywhere, so the directory means one thing. This is F4.
        //
        // To change a vector deliberately, edit the committed file and let this
        // assertion confirm the code agrees with it.
        let path = vector_path("aead.json");
        let committed = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("committed vector {} is unreadable: {e}", path.display()));

        // Compared as parsed JSON, not as text: whitespace and line endings are
        // not part of the vector, and this checkout normalizes newlines.
        let committed: serde_json::Value =
            serde_json::from_str(&committed).expect("committed vector is valid JSON");
        let generated: serde_json::Value =
            serde_json::from_str(&json).expect("generated vector is valid JSON");

        assert_eq!(
            generated, committed,
            "generated vectors no longer match {} — if this change is intended, update the committed file in its own commit",
            path.display()
        );
    }
}
