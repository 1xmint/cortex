use serde::{Deserialize, Serialize};
use soma_crypto::composite::CompositeKeypair;
use soma_crypto::composite::CompositeSignature;

use crate::types::HeartId;

pub struct DeathCertificate {
    pub heart_id: HeartId,
    pub final_root: [u8; 32],
    pub final_leaf_count: u64,
    pub reason: DeathReason,
    pub issued_at: u64,
    pub issuer: DeathIssuer,
    pub successor: Option<HeartId>,
    pub issuer_signature: Option<CompositeSignature>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DeathReason {
    OwnerRequested,
    DelegationRevoked,
    ParentDied,
    Starvation,
    TrustCollapse,
    Inactivity,
    FactoryObsolescence,
    ProtocolViolation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DeathIssuer {
    Self_(HeartId),
    Parent(HeartId),
    Protocol,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SuccessionLink {
    pub predecessor_heart: HeartId,
    pub predecessor_final_root: [u8; 32],
    pub successor_heart: HeartId,
    pub declared_at: u64,
    pub activated_at: u64,
}

impl DeathCertificate {
    pub fn to_signable_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.heart_id.0);
        buf.extend_from_slice(&self.final_root);
        buf.extend_from_slice(&self.final_leaf_count.to_be_bytes());
        buf.extend_from_slice(format!("{:?}", self.reason).as_bytes());
        buf.extend_from_slice(&self.issued_at.to_be_bytes());
        buf.extend_from_slice(format!("{:?}", self.issuer).as_bytes());
        if let Some(ref s) = self.successor {
            buf.extend_from_slice(&s.0);
        }
        buf
    }

    pub fn sign(&mut self, keypair: &CompositeKeypair) -> crate::Result<()> {
        let bytes = self.to_signable_bytes();
        let sig = keypair.sign(&bytes)?;
        self.issuer_signature = Some(sig);
        Ok(())
    }
}
