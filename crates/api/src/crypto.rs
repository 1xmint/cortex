use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use sha2::{Digest, Sha256};

const NONCE_LEN: usize = 12;

fn derive_key() -> [u8; 32] {
    // Derive from CLERK_SECRET_KEY (already required in production) with a domain separator.
    // Falls back to CORTEX_KEY_ENCRYPTION_SECRET if set, then to a dev-only key.
    let secret = std::env::var("CLERK_SECRET_KEY")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::env::var("CORTEX_KEY_ENCRYPTION_SECRET")
                .ok()
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| {
            if crate::is_production_env() {
                panic!("CLERK_SECRET_KEY must be set in production");
            }
            tracing::warn!("using insecure dev encryption key — set CLERK_SECRET_KEY");
            "dev-only-insecure-key-replace-me!".into()
        });
    // Domain-separated hash so the same secret produces different keys for different purposes
    let mut hasher = Sha256::new();
    hasher.update(b"cortex-api-key-encryption:");
    hasher.update(secret.as_bytes());
    let hash = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&hash);
    key
}

pub fn encrypt(plaintext: &str) -> Result<String, String> {
    let key = derive_key();
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| e.to_string())?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut combined = nonce_bytes.to_vec();
    combined.extend_from_slice(&ciphertext);
    Ok(base64::engine::general_purpose::STANDARD.encode(&combined))
}

pub fn decrypt(encoded: &str) -> Result<String, String> {
    let key = derive_key();
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| e.to_string())?;
    let combined = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| e.to_string())?;
    if combined.len() < NONCE_LEN {
        return Err("ciphertext too short".into());
    }
    let (nonce_bytes, ciphertext) = combined.split_at(NONCE_LEN);
    let nonce = Nonce::from_slice(nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| "decryption failed — wrong key or corrupted data".to_string())?;
    String::from_utf8(plaintext).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        std::env::set_var("CLERK_SECRET_KEY", "test-key-32-bytes-long-exactly!!");
        let original = "sk-ant-api03-test-key-1234567890";
        let encrypted = encrypt(original).unwrap();
        assert_ne!(encrypted, original);
        let decrypted = decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, original);
    }
}
