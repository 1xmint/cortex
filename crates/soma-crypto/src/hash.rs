pub fn blake3(data: &[u8]) -> [u8; 32] {
    *blake3::hash(data).as_bytes()
}

pub fn blake3_keyed(key: &[u8; 32], data: &[u8]) -> [u8; 32] {
    *blake3::keyed_hash(key, data).as_bytes()
}

pub fn blake3_kdf(key: &[u8], context: &str, len: usize) -> Vec<u8> {
    let mut output = vec![0u8; len];
    let mut deriver = blake3::Hasher::new_derive_key(context);
    deriver.update(key);
    let mut reader = deriver.finalize_xof();
    reader.fill(&mut output);
    output
}

pub fn heart_id(ed25519_pk: &[u8; 32], mldsa65_pk: &[u8]) -> [u8; 32] {
    let mut input = Vec::with_capacity(32 + mldsa65_pk.len());
    input.extend_from_slice(ed25519_pk);
    input.extend_from_slice(mldsa65_pk);
    blake3(&input)
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
    fn blake3_deterministic() {
        let a = blake3(b"soma protocol");
        let b = blake3(b"soma protocol");
        assert_eq!(a, b);
    }

    #[test]
    fn blake3_different_inputs() {
        let a = blake3(b"soma");
        let b = blake3(b"vera");
        assert_ne!(a, b);
    }

    #[test]
    fn blake3_kdf_domain_separation() {
        let key = b"root-secret-material-for-testing";
        let a = blake3_kdf(key, "soma.heart.sign.v1", 32);
        let b = blake3_kdf(key, "soma.room.agree.v1", 32);
        assert_ne!(a, b);
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn heart_id_deterministic() {
        let ed_pk = [42u8; 32];
        let pq_pk = vec![7u8; 1952];
        let id1 = heart_id(&ed_pk, &pq_pk);
        let id2 = heart_id(&ed_pk, &pq_pk);
        assert_eq!(id1, id2);
    }

    #[test]
    fn heart_id_changes_with_any_key() {
        let ed_pk = [42u8; 32];
        let pq_pk = vec![7u8; 1952];
        let id1 = heart_id(&ed_pk, &pq_pk);

        let mut ed_pk2 = ed_pk;
        ed_pk2[0] ^= 1;
        let id2 = heart_id(&ed_pk2, &pq_pk);
        assert_ne!(id1, id2);

        let mut pq_pk2 = pq_pk.clone();
        pq_pk2[0] ^= 1;
        let id3 = heart_id(&ed_pk, &pq_pk2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn committed_vectors_still_match_the_code() {
        let vectors = serde_json::json!({
            "blake3": [
                {
                    "input_hex": hex::encode(b"soma protocol"),
                    "output_hex": hex::encode(blake3(b"soma protocol")),
                },
                {
                    "input_hex": hex::encode(b""),
                    "output_hex": hex::encode(blake3(b"")),
                },
            ],
            "blake3_kdf": [
                {
                    "key_hex": hex::encode(b"root-secret-material-for-testing"),
                    "context": "soma.heart.sign.v1",
                    "output_len": 32,
                    "output_hex": hex::encode(blake3_kdf(b"root-secret-material-for-testing", "soma.heart.sign.v1", 32)),
                },
            ],
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
        let path = vector_path("hash.json");
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
