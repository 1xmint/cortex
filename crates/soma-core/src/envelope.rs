use serde::{Deserialize, Serialize};
use soma_crypto::composite::{CompositeKeypair, CompositePublicKey, CompositeSignature};
use soma_crypto::hash::blake3;

use crate::delegation::Capability;
use crate::types::{DelegationId, HeartId, RoomId};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SomaFlow {
    pub from: HeartId,
    pub to: HeartId,
    pub amount: u64,
    pub capability: Capability,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SessionOutcome {
    Success,
    Failure { error_class: String },
    Partial { completed: f32 },
}

pub struct SealedSessionEnvelope {
    pub room_id: RoomId,
    pub participants: Vec<HeartId>,
    pub capabilities_exercised: Vec<Capability>,
    pub soma_spent: u64,
    pub soma_flows: Vec<SomaFlow>,
    pub opened_at: u64,
    pub sealed_at: u64,
    pub duration_ms: u64,
    pub outcome: SessionOutcome,
    pub delegation_refs: Vec<DelegationId>,
    pub content_hash: [u8; 32],
    pub participant_signatures: Vec<(HeartId, CompositeSignature)>,
}

impl SealedSessionEnvelope {
    pub fn signable_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"soma-envelope-v1:");
        buf.extend_from_slice(&self.room_id.0);
        for p in &self.participants {
            buf.extend_from_slice(&p.0);
        }
        let caps_bytes =
            serde_json::to_vec(&self.capabilities_exercised).expect("caps serialization");
        buf.extend_from_slice(&caps_bytes);
        buf.extend_from_slice(&self.soma_spent.to_be_bytes());
        let flows_bytes = serde_json::to_vec(&self.soma_flows).expect("flows serialization");
        buf.extend_from_slice(&flows_bytes);
        buf.extend_from_slice(&self.opened_at.to_be_bytes());
        buf.extend_from_slice(&self.sealed_at.to_be_bytes());
        buf.extend_from_slice(&self.duration_ms.to_be_bytes());
        let outcome_bytes = serde_json::to_vec(&self.outcome).expect("outcome serialization");
        buf.extend_from_slice(&outcome_bytes);
        for d in &self.delegation_refs {
            buf.extend_from_slice(&d.0);
        }
        buf.extend_from_slice(&self.content_hash);
        buf
    }

    pub fn sign(&mut self, heart_id: HeartId, keypair: &CompositeKeypair) -> crate::Result<()> {
        let signable = self.signable_bytes();
        let sig = keypair.sign(&signable)?;
        self.participant_signatures.push((heart_id, sig));
        Ok(())
    }

    pub fn verify_signatures(
        &self,
        public_keys: &[(HeartId, CompositePublicKey)],
    ) -> crate::Result<()> {
        let signable = self.signable_bytes();
        for (heart_id, sig) in &self.participant_signatures {
            let pk = public_keys
                .iter()
                .find(|(id, _)| id == heart_id)
                .map(|(_, pk)| pk);

            if let Some(pk) = pk {
                if !pk.verify(&signable, sig) {
                    return Err(crate::Error::InvalidSignature {
                        leaf_index: 0, // reusing error type; not a leaf index here
                    });
                }
            }
        }
        Ok(())
    }

    pub fn is_fully_signed(&self) -> bool {
        self.participants
            .iter()
            .all(|p| self.participant_signatures.iter().any(|(id, _)| id == p))
    }

    pub fn envelope_hash(&self) -> [u8; 32] {
        let signable = self.signable_bytes();
        blake3(&signable)
    }
}
