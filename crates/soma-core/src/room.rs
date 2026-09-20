use soma_crypto::hash::blake3;

use crate::envelope::SealedSessionEnvelope;
use crate::types::{HeartId, RoomId};

pub enum RoomStatus {
    Open,
    Sealed(SealedSessionEnvelope),
}

pub struct Room {
    pub id: RoomId,
    pub host: HeartId,
    pub participants: Vec<HeartId>,
    pub status: RoomStatus,
    pub created_at: u64,
    pub sealed_at: Option<u64>,
}

impl Room {
    pub fn create(host: HeartId, timestamp: u64) -> Self {
        let mut id_input = Vec::new();
        id_input.extend_from_slice(&host.0);
        id_input.extend_from_slice(&timestamp.to_be_bytes());
        let id = RoomId(blake3(&id_input));

        Self {
            id,
            host,
            participants: vec![host],
            status: RoomStatus::Open,
            created_at: timestamp,
            sealed_at: None,
        }
    }

    pub fn add_participant(&mut self, heart_id: HeartId) -> crate::Result<()> {
        if !self.is_open() {
            return Err(crate::Error::RoomNotOpen);
        }
        if self.participants.contains(&heart_id) {
            return Err(crate::Error::ParticipantAlreadyExists);
        }
        self.participants.push(heart_id);
        Ok(())
    }

    pub fn is_open(&self) -> bool {
        matches!(self.status, RoomStatus::Open)
    }
}
