use core::fmt;

#[derive(Debug)]
pub enum Error {
    InvalidKeyLength { expected: usize, got: usize },
    InvalidSignature,
    SigningFailed,
    KeygenFailed,
    AeadSealFailed,
    AeadOpenFailed,
    EncapsFailed,
    DecapsFailed,
    UnknownSuite(u8),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidKeyLength { expected, got } => {
                write!(f, "invalid key length: expected {expected}, got {got}")
            }
            Self::InvalidSignature => write!(f, "invalid signature"),
            Self::SigningFailed => write!(f, "signing operation failed"),
            Self::KeygenFailed => write!(f, "key generation failed"),
            Self::AeadSealFailed => write!(f, "AEAD seal failed"),
            Self::AeadOpenFailed => write!(f, "AEAD open failed (authentication failure)"),
            Self::EncapsFailed => write!(f, "KEM encapsulation failed"),
            Self::DecapsFailed => write!(f, "KEM decapsulation failed"),
            Self::UnknownSuite(id) => write!(f, "unknown suite id: 0x{id:02x}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = core::result::Result<T, Error>;
