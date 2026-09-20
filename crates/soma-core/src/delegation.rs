use serde::{Deserialize, Serialize};
use soma_crypto::composite::{CompositeKeypair, CompositeSignature};
use soma_crypto::hash::blake3;

use crate::heart::Heart;
use crate::types::{DelegationId, HeartId};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Capability {
    CodeExecution,
    FileAccess,
    NetworkAccess,
    DelegateAuthority,
    SpendSoma,
    Custom(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DelegationScope {
    pub capabilities: Vec<Capability>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DelegationConstraints {
    pub max_depth: u8,
    pub spend_cap: u64,
    pub spend_remaining: u64,
    pub ttl_ms: u64,
    pub expires_at: u64,
    pub intent: String,
    pub cascade_revoke: bool,
}

pub struct DelegationToken {
    pub id: DelegationId,
    pub parent_heart: HeartId,
    pub child_heart: HeartId,
    pub scope: DelegationScope,
    pub constraints: DelegationConstraints,
    pub parent_signature: CompositeSignature,
    pub child_signature: Option<CompositeSignature>,
    pub timestamp: u64,
}

impl DelegationToken {
    pub fn create(
        parent: &Heart,
        child_id: HeartId,
        scope: DelegationScope,
        constraints: DelegationConstraints,
        timestamp: u64,
    ) -> crate::Result<Self> {
        let signable = Self::signable_bytes(&parent.id, &child_id, &scope, &constraints, timestamp);

        let parent_signature = parent.sign(&signable)?;

        let id_bytes = blake3(&signable);
        let id = DelegationId(id_bytes);

        Ok(Self {
            id,
            parent_heart: parent.id,
            child_heart: child_id,
            scope,
            constraints,
            parent_signature,
            child_signature: None,
            timestamp,
        })
    }

    pub fn accept(&mut self, child_keypair: &CompositeKeypair) -> crate::Result<()> {
        if self.child_signature.is_some() {
            return Err(crate::Error::DelegationAlreadyAccepted);
        }

        let signable = Self::signable_bytes(
            &self.parent_heart,
            &self.child_heart,
            &self.scope,
            &self.constraints,
            self.timestamp,
        );

        let sig = child_keypair.sign(&signable)?;
        self.child_signature = Some(sig);
        Ok(())
    }

    pub fn is_valid_at(&self, timestamp: u64) -> bool {
        self.child_signature.is_some() && timestamp <= self.constraints.expires_at
    }

    pub fn verify_narrowing(&self, parent_token: &DelegationToken) -> crate::Result<()> {
        for cap in &self.scope.capabilities {
            if !parent_token.scope.capabilities.contains(cap) {
                return Err(crate::Error::ScopeWidening {
                    capability: format!("{cap:?}"),
                });
            }
        }

        if self.constraints.max_depth >= parent_token.constraints.max_depth {
            return Err(crate::Error::ConstraintWidening {
                field: format!(
                    "max_depth: child {} >= parent {}",
                    self.constraints.max_depth, parent_token.constraints.max_depth
                ),
            });
        }

        if self.constraints.spend_cap > parent_token.constraints.spend_remaining {
            return Err(crate::Error::ConstraintWidening {
                field: format!(
                    "spend_cap: child {} > parent remaining {}",
                    self.constraints.spend_cap, parent_token.constraints.spend_remaining
                ),
            });
        }

        if self.constraints.ttl_ms > parent_token.constraints.ttl_ms {
            return Err(crate::Error::ConstraintWidening {
                field: format!(
                    "ttl_ms: child {} > parent {}",
                    self.constraints.ttl_ms, parent_token.constraints.ttl_ms
                ),
            });
        }

        Ok(())
    }

    fn signable_bytes(
        parent_heart: &HeartId,
        child_heart: &HeartId,
        scope: &DelegationScope,
        constraints: &DelegationConstraints,
        timestamp: u64,
    ) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"soma-delegation-v1:");
        buf.extend_from_slice(&parent_heart.0);
        buf.extend_from_slice(&child_heart.0);
        let scope_bytes = serde_json::to_vec(scope).expect("scope serialization");
        buf.extend_from_slice(&scope_bytes);
        let constraint_bytes = serde_json::to_vec(constraints).expect("constraints serialization");
        buf.extend_from_slice(&constraint_bytes);
        buf.extend_from_slice(&timestamp.to_be_bytes());
        buf
    }
}
