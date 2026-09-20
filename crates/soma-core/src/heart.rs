use soma_crypto::composite::{CompositeKeypair, CompositeSignature};

use crate::death::DeathCertificate;
use crate::pulse_tree::{BirthRecord, FactoryStamp, PulseTree};
use crate::types::{DelegationId, HeartId};

pub enum HeartStatus {
    Alive,
    Suspended { reason: String, since: u64 },
    Dead { certificate: DeathCertificate },
}

pub struct Heart {
    pub id: HeartId,
    pub keypair: CompositeKeypair,
    pub factory_stamp: FactoryStamp,
    pub birth_timestamp: u64,
    pub pulse_tree: PulseTree,
    pub parent_delegation: Option<DelegationId>,
    pub status: HeartStatus,
    pub soma_balance: u64,
}

impl Heart {
    pub fn new(
        factory_stamp: FactoryStamp,
        initial_soma: u64,
        timestamp: u64,
    ) -> crate::Result<Self> {
        let keypair = CompositeKeypair::generate()?;
        let id = HeartId(keypair.heart_id());

        let birth = BirthRecord {
            heart_id: id,
            factory_stamp: factory_stamp.clone(),
            initial_soma,
        };

        let pulse_tree = PulseTree::new(id, birth, &keypair, timestamp)?;

        Ok(Self {
            id,
            keypair,
            factory_stamp,
            birth_timestamp: timestamp,
            pulse_tree,
            parent_delegation: None,
            status: HeartStatus::Alive,
            soma_balance: initial_soma,
        })
    }

    pub fn is_alive(&self) -> bool {
        matches!(self.status, HeartStatus::Alive)
    }

    pub fn sign(&self, message: &[u8]) -> crate::Result<CompositeSignature> {
        match &self.status {
            HeartStatus::Alive => Ok(self.keypair.sign(message)?),
            HeartStatus::Suspended { .. } => Err(crate::Error::HeartSuspended),
            HeartStatus::Dead { .. } => Err(crate::Error::HeartNotAlive),
        }
    }

    pub fn suspend(&mut self, reason: String, timestamp: u64) -> crate::Result<()> {
        match &self.status {
            HeartStatus::Alive => {
                self.status = HeartStatus::Suspended {
                    reason,
                    since: timestamp,
                };
                Ok(())
            }
            HeartStatus::Suspended { .. } => Err(crate::Error::HeartAlreadySuspended),
            HeartStatus::Dead { .. } => Err(crate::Error::HeartNotAlive),
        }
    }

    pub fn resume(&mut self, _timestamp: u64) -> crate::Result<()> {
        match &self.status {
            HeartStatus::Suspended { .. } => {
                self.status = HeartStatus::Alive;
                Ok(())
            }
            HeartStatus::Alive => Err(crate::Error::HeartAlreadyAlive),
            HeartStatus::Dead { .. } => Err(crate::Error::HeartNotAlive),
        }
    }
}
