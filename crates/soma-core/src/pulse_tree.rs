use serde::{Deserialize, Serialize};
use soma_crypto::composite::{CompositeKeypair, CompositePublicKey, CompositeSignature};
use soma_crypto::hash::blake3;
use soma_crypto::pulse::{leaf_digest, running_root};

use crate::death::{DeathIssuer, DeathReason};
use crate::types::{DelegationId, HeartId, RoomId};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FactoryStamp {
    pub code_hash: [u8; 32],
    pub version: String,
    pub timestamp: u64,
    pub builder_heart: Option<HeartId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BirthRecord {
    pub heart_id: HeartId,
    pub factory_stamp: FactoryStamp,
    pub initial_soma: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SealedSessionRef {
    pub room_id: RoomId,
    pub envelope_hash: [u8; 32],
    pub soma_delta: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DelegationRecord {
    pub delegation_id: DelegationId,
    pub child_heart: HeartId,
    pub intent: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RevocationRecord {
    pub delegation_id: DelegationId,
    pub reason: RevocationReason,
    pub timestamp: u64,
    pub revoker_heart: HeartId,
    pub children_revoked: Vec<DelegationId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RevocationReason {
    ParentRevoked,
    CascadeRevoke,
    Expired,
    SpendCapExhausted,
    HeartDeath,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpendReceiptRecord {
    pub amount: u64,
    pub recipient: HeartId,
    pub capability: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeathRecord {
    pub heart_id: HeartId,
    pub final_root: [u8; 32],
    pub final_leaf_count: u64,
    pub reason: DeathReason,
    pub issued_at: u64,
    pub issuer: DeathIssuer,
    pub successor: Option<HeartId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PulsePayload {
    Birth(BirthRecord),
    SealedSession(SealedSessionRef),
    Delegation(DelegationRecord),
    DelegationRevocation(RevocationRecord),
    SpendReceipt(SpendReceiptRecord),
    Death(DeathRecord),
}

impl PulsePayload {
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("PulsePayload serialization should not fail")
    }

    pub fn is_death(&self) -> bool {
        matches!(self, PulsePayload::Death(_))
    }
}

pub struct PulseLeaf {
    pub index: u64,
    pub timestamp: u64,
    pub prev_root: [u8; 32],
    pub soma_balance: u64,
    pub payload: PulsePayload,
    pub payload_hash: [u8; 32],
    pub signature: CompositeSignature,
}

pub struct PulseTree {
    pub heart_id: HeartId,
    pub root: [u8; 32],
    pub leaves: Vec<PulseLeaf>,
    pub leaf_count: u64,
}

impl PulseTree {
    pub fn new(
        heart_id: HeartId,
        birth: BirthRecord,
        keypair: &CompositeKeypair,
        timestamp: u64,
    ) -> crate::Result<Self> {
        let soma_balance = birth.initial_soma;
        let payload = PulsePayload::Birth(birth);
        let payload_bytes = payload.to_bytes();
        let payload_hash = blake3(&payload_bytes);
        let prev_root = [0u8; 32];

        let digest = leaf_digest(0, timestamp, &prev_root, soma_balance, &payload_bytes);
        let signature = keypair.sign(&digest)?;
        let root = running_root(None, &digest);

        let leaf = PulseLeaf {
            index: 0,
            timestamp,
            prev_root,
            soma_balance,
            payload,
            payload_hash,
            signature,
        };

        Ok(Self {
            heart_id,
            root,
            leaves: vec![leaf],
            leaf_count: 1,
        })
    }

    pub fn append(
        &mut self,
        payload: PulsePayload,
        soma_balance: u64,
        keypair: &CompositeKeypair,
        timestamp: u64,
    ) -> crate::Result<&PulseLeaf> {
        if self.is_sealed() {
            return Err(crate::Error::PulseTreeSealed);
        }

        let index = self.leaf_count;
        let prev_root = self.root;
        let payload_bytes = payload.to_bytes();
        let payload_hash = blake3(&payload_bytes);

        let digest = leaf_digest(index, timestamp, &prev_root, soma_balance, &payload_bytes);
        let signature = keypair.sign(&digest)?;
        let new_root = running_root(Some(&prev_root), &digest);

        let leaf = PulseLeaf {
            index,
            timestamp,
            prev_root,
            soma_balance,
            payload,
            payload_hash,
            signature,
        };

        self.root = new_root;
        self.leaves.push(leaf);
        self.leaf_count += 1;

        Ok(self.leaves.last().unwrap())
    }

    pub fn verify(&self, public_key: &CompositePublicKey) -> crate::Result<()> {
        if self.leaves.is_empty() {
            return Err(crate::Error::PulseTreeEmpty);
        }

        let mut expected_root = [0u8; 32];

        for (i, leaf) in self.leaves.iter().enumerate() {
            if leaf.index != i as u64 {
                return Err(crate::Error::InvalidPulseChain {
                    leaf_index: leaf.index,
                    reason: format!("expected index {i}, got {}", leaf.index),
                });
            }

            let expected_prev = if i == 0 { [0u8; 32] } else { expected_root };
            if leaf.prev_root != expected_prev {
                return Err(crate::Error::InvalidPulseChain {
                    leaf_index: leaf.index,
                    reason: "prev_root mismatch".to_string(),
                });
            }

            let payload_bytes = leaf.payload.to_bytes();
            let digest = leaf_digest(
                leaf.index,
                leaf.timestamp,
                &leaf.prev_root,
                leaf.soma_balance,
                &payload_bytes,
            );

            if !public_key.verify(&digest, &leaf.signature) {
                return Err(crate::Error::InvalidSignature {
                    leaf_index: leaf.index,
                });
            }

            if i == 0 {
                expected_root = running_root(None, &digest);
            } else {
                expected_root = running_root(Some(&leaf.prev_root), &digest);
            }
        }

        if expected_root != self.root {
            return Err(crate::Error::InvalidPulseChain {
                leaf_index: self.leaf_count - 1,
                reason: "final root mismatch".to_string(),
            });
        }

        Ok(())
    }

    pub fn current_root(&self) -> &[u8; 32] {
        &self.root
    }

    pub fn is_sealed(&self) -> bool {
        self.leaves
            .last()
            .is_some_and(|leaf| leaf.payload.is_death())
    }
}
