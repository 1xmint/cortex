use fips203::ml_kem_768;
use fips203::traits::{Decaps, Encaps, KeyGen, SerDes};
use x25519_dalek::{PublicKey as X25519Public, StaticSecret as X25519Secret};

use rand_core::RngCore;
use zeroize::Zeroize;

use crate::hash;
use crate::Error;

/// Generate an X25519 secret without handing an RNG to x25519-dalek.
///
/// `StaticSecret::random_from_rng` binds this crate to whichever `rand_core`
/// x25519-dalek depends on — v2 wanted `rand_core 0.6` by value, v3 wants
/// `rand_core 0.10` by reference — so a routine dependency bump becomes a
/// breaking change in key agreement. An X25519 secret is 32 uniformly random
/// bytes, so producing them ourselves and calling `from` is identical
/// cryptographically and leaves the RNG choice here, where we can see it.
///
/// Same reasoning as #474 applied to Ed25519 signing keys.
fn random_x25519_secret() -> X25519Secret {
    let mut bytes = [0u8; 32];
    rand_core::OsRng.fill_bytes(&mut bytes);
    let secret = X25519Secret::from(bytes);
    // `from` copies, so this stack buffer still holds the key. Clamping means
    // it is not byte-identical to the stored secret, but it is trivially
    // convertible to it — wipe it.
    bytes.zeroize();
    secret
}

pub const MLKEM768_CT_LEN: usize = ml_kem_768::CT_LEN;

pub struct HybridKeypair {
    pub x25519_secret: X25519Secret,
    pub x25519_public: X25519Public,
    pub mlkem_dk: ml_kem_768::DecapsKey,
    pub mlkem_ek: ml_kem_768::EncapsKey,
}

pub struct HybridEncapsResult {
    pub shared_secret: [u8; 32],
    pub x25519_ephemeral: [u8; 32],
    pub mlkem_ciphertext: [u8; MLKEM768_CT_LEN],
}

impl HybridKeypair {
    pub fn generate() -> crate::Result<Self> {
        let x25519_secret = random_x25519_secret();
        let x25519_public = X25519Public::from(&x25519_secret);

        let (mlkem_ek, mlkem_dk) = ml_kem_768::KG::try_keygen().map_err(|_| Error::KeygenFailed)?;

        Ok(Self {
            x25519_secret,
            x25519_public,
            mlkem_dk,
            mlkem_ek,
        })
    }

    pub fn encaps_key_bytes(&self) -> Vec<u8> {
        let mlkem_bytes = self.mlkem_ek.clone().into_bytes();
        let mut out = Vec::with_capacity(32 + mlkem_bytes.len());
        out.extend_from_slice(self.x25519_public.as_bytes());
        out.extend_from_slice(&mlkem_bytes);
        out
    }

    pub fn decaps(
        &self,
        peer_x25519_ephemeral: &[u8; 32],
        mlkem_ciphertext: &[u8; MLKEM768_CT_LEN],
    ) -> crate::Result<[u8; 32]> {
        let peer_pk = X25519Public::from(*peer_x25519_ephemeral);
        let x25519_ss = self.x25519_secret.diffie_hellman(&peer_pk);

        let mlkem_ct = ml_kem_768::CipherText::try_from_bytes(*mlkem_ciphertext)
            .map_err(|_| Error::DecapsFailed)?;
        let mlkem_ss = self
            .mlkem_dk
            .try_decaps(&mlkem_ct)
            .map_err(|_| Error::DecapsFailed)?;
        let mlkem_ss_bytes = mlkem_ss.into_bytes();

        Ok(combine_shared_secrets(
            x25519_ss.as_bytes(),
            &mlkem_ss_bytes,
        ))
    }
}

pub fn encaps(
    peer_x25519_public: &[u8; 32],
    peer_mlkem_ek: &ml_kem_768::EncapsKey,
) -> crate::Result<HybridEncapsResult> {
    let ephemeral_secret = random_x25519_secret();
    let ephemeral_public = X25519Public::from(&ephemeral_secret);

    let peer_pk = X25519Public::from(*peer_x25519_public);
    let x25519_ss = ephemeral_secret.diffie_hellman(&peer_pk);

    let (mlkem_ss, mlkem_ct) = peer_mlkem_ek
        .try_encaps()
        .map_err(|_| Error::EncapsFailed)?;
    let mlkem_ss_bytes = mlkem_ss.into_bytes();
    let mlkem_ct_bytes = mlkem_ct.into_bytes();

    let shared_secret = combine_shared_secrets(x25519_ss.as_bytes(), &mlkem_ss_bytes);

    Ok(HybridEncapsResult {
        shared_secret,
        x25519_ephemeral: *ephemeral_public.as_bytes(),
        mlkem_ciphertext: mlkem_ct_bytes,
    })
}

fn combine_shared_secrets(x25519_ss: &[u8; 32], mlkem_ss: &[u8]) -> [u8; 32] {
    let mut input = Vec::with_capacity(32 + mlkem_ss.len());
    input.extend_from_slice(x25519_ss);
    input.extend_from_slice(mlkem_ss);

    let derived = hash::blake3_kdf(&input, "soma.room.hybrid-kem.v1", 32);
    let mut out = [0u8; 32];
    out.copy_from_slice(&derived);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hybrid_key_agreement_roundtrip() {
        let alice = HybridKeypair::generate().unwrap();

        let encaps_result = encaps(alice.x25519_public.as_bytes(), &alice.mlkem_ek).unwrap();

        let alice_ss = alice
            .decaps(
                &encaps_result.x25519_ephemeral,
                &encaps_result.mlkem_ciphertext,
            )
            .unwrap();

        assert_eq!(alice_ss, encaps_result.shared_secret);
    }

    #[test]
    fn different_sessions_different_secrets() {
        let alice = HybridKeypair::generate().unwrap();

        let r1 = encaps(alice.x25519_public.as_bytes(), &alice.mlkem_ek).unwrap();
        let r2 = encaps(alice.x25519_public.as_bytes(), &alice.mlkem_ek).unwrap();

        assert_ne!(r1.shared_secret, r2.shared_secret);
    }

    #[test]
    fn wrong_key_fails_decaps() {
        let alice = HybridKeypair::generate().unwrap();
        let bob = HybridKeypair::generate().unwrap();

        let encaps_result = encaps(alice.x25519_public.as_bytes(), &alice.mlkem_ek).unwrap();

        let bob_ss = bob
            .decaps(
                &encaps_result.x25519_ephemeral,
                &encaps_result.mlkem_ciphertext,
            )
            .unwrap();

        assert_ne!(bob_ss, encaps_result.shared_secret);
    }
}
