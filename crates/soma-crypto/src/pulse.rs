use crate::hash::blake3;

pub fn leaf_digest(
    index: u64,
    timestamp: u64,
    prev_root: &[u8; 32],
    soma_balance: u64,
    payload: &[u8],
) -> [u8; 32] {
    let payload_hash = blake3(payload);

    let mut buf = Vec::with_capacity(8 + 8 + 32 + 8 + 32);
    buf.extend_from_slice(&index.to_be_bytes());
    buf.extend_from_slice(&timestamp.to_be_bytes());
    buf.extend_from_slice(prev_root);
    buf.extend_from_slice(&soma_balance.to_be_bytes());
    buf.extend_from_slice(&payload_hash);

    blake3(&buf)
}

pub fn running_root(prev_root: Option<&[u8; 32]>, leaf_digest: &[u8; 32]) -> [u8; 32] {
    match prev_root {
        None => *leaf_digest,
        Some(prev) => {
            let mut buf = [0u8; 64];
            buf[..32].copy_from_slice(prev);
            buf[32..].copy_from_slice(leaf_digest);
            blake3(&buf)
        }
    }
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
    fn genesis_leaf_root_equals_digest() {
        let prev_root = [0u8; 32];
        let digest = leaf_digest(0, 1000, &prev_root, 100, b"birth payload");
        let root = running_root(None, &digest);
        assert_eq!(root, digest);
    }

    #[test]
    fn second_leaf_chains() {
        let zero_root = [0u8; 32];
        let d0 = leaf_digest(0, 1000, &zero_root, 100, b"birth");
        let root0 = running_root(None, &d0);

        let d1 = leaf_digest(1, 2000, &root0, 90, b"session");
        let root1 = running_root(Some(&root0), &d1);

        assert_ne!(root0, root1);
        assert_ne!(root1, d1);
    }

    #[test]
    fn deterministic() {
        let prev = [0u8; 32];
        let a = leaf_digest(0, 1000, &prev, 100, b"test");
        let b = leaf_digest(0, 1000, &prev, 100, b"test");
        assert_eq!(a, b);
    }

    #[test]
    fn any_field_change_changes_digest() {
        let prev = [0u8; 32];
        let base = leaf_digest(0, 1000, &prev, 100, b"test");

        assert_ne!(base, leaf_digest(1, 1000, &prev, 100, b"test"));
        assert_ne!(base, leaf_digest(0, 1001, &prev, 100, b"test"));
        assert_ne!(base, leaf_digest(0, 1000, &prev, 101, b"test"));
        assert_ne!(base, leaf_digest(0, 1000, &prev, 100, b"tset"));

        let mut prev2 = prev;
        prev2[0] = 1;
        assert_ne!(base, leaf_digest(0, 1000, &prev2, 100, b"test"));
    }

    #[test]
    fn soma_balance_in_digest() {
        let prev = [0u8; 32];
        let d_100 = leaf_digest(0, 1000, &prev, 100, b"same payload");
        let d_99 = leaf_digest(0, 1000, &prev, 99, b"same payload");
        assert_ne!(d_100, d_99);
    }

    #[test]
    fn chain_verification_simulation() {
        let zero = [0u8; 32];
        let d0 = leaf_digest(0, 1000, &zero, 1000, b"birth");
        let r0 = running_root(None, &d0);

        let d1 = leaf_digest(1, 2000, &r0, 950, b"session-1");
        let r1 = running_root(Some(&r0), &d1);

        let d2 = leaf_digest(2, 3000, &r1, 900, b"session-2");
        let r2 = running_root(Some(&r1), &d2);

        let re_d0 = leaf_digest(0, 1000, &zero, 1000, b"birth");
        let re_r0 = running_root(None, &re_d0);
        assert_eq!(r0, re_r0);

        let re_d1 = leaf_digest(1, 2000, &re_r0, 950, b"session-1");
        let re_r1 = running_root(Some(&re_r0), &re_d1);
        assert_eq!(r1, re_r1);

        let re_d2 = leaf_digest(2, 3000, &re_r1, 900, b"session-2");
        let re_r2 = running_root(Some(&re_r1), &re_d2);
        assert_eq!(r2, re_r2);
    }

    #[test]
    fn committed_vectors_still_match_the_code() {
        let zero = [0u8; 32];
        let d0 = leaf_digest(0, 1716000000000, &zero, 1000, b"\x01birth-payload");
        let r0 = running_root(None, &d0);

        let d1 = leaf_digest(1, 1716000060000, &r0, 950, b"\x02session-envelope");
        let r1 = running_root(Some(&r0), &d1);

        let vectors = serde_json::json!({
            "pulse_tree": {
                "leaf_digests": [
                    {
                        "index": 0,
                        "timestamp": 1716000000000u64,
                        "prev_root_hex": hex::encode(zero),
                        "soma_balance": 1000,
                        "payload_hex": hex::encode(b"\x01birth-payload"),
                        "digest_hex": hex::encode(d0),
                    },
                    {
                        "index": 1,
                        "timestamp": 1716000060000u64,
                        "prev_root_hex": hex::encode(r0),
                        "soma_balance": 950,
                        "payload_hex": hex::encode(b"\x02session-envelope"),
                        "digest_hex": hex::encode(d1),
                    },
                ],
                "running_roots": [
                    { "after_index": 0, "root_hex": hex::encode(r0) },
                    { "after_index": 1, "root_hex": hex::encode(r1) },
                ],
            },
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
        let path = vector_path("pulse.json");
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
