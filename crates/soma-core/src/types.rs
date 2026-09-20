use core::fmt;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct HeartId(pub [u8; 32]);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct RoomId(pub [u8; 32]);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct DelegationId(pub [u8; 32]);

impl fmt::Display for HeartId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Display for RoomId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Display for DelegationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl From<[u8; 32]> for HeartId {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl From<[u8; 32]> for RoomId {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl From<[u8; 32]> for DelegationId {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl From<HeartId> for [u8; 32] {
    fn from(id: HeartId) -> Self {
        id.0
    }
}

impl From<RoomId> for [u8; 32] {
    fn from(id: RoomId) -> Self {
        id.0
    }
}

impl From<DelegationId> for [u8; 32] {
    fn from(id: DelegationId) -> Self {
        id.0
    }
}
