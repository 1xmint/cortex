use core::fmt;

#[derive(Debug)]
pub enum Error {
    Crypto(soma_crypto::Error),
    HeartNotAlive,
    HeartSuspended,
    HeartAlreadySuspended,
    HeartAlreadyAlive,
    PulseTreeSealed,
    PulseTreeEmpty,
    InvalidPulseChain { leaf_index: u64, reason: String },
    InvalidSignature { leaf_index: u64 },
    DelegationExpired,
    DelegationNotAccepted,
    DelegationAlreadyAccepted,
    ScopeWidening { capability: String },
    ConstraintWidening { field: String },
    RoomAlreadySealed,
    RoomNotOpen,
    ParticipantAlreadyExists,
    EnvelopeNotFullySigned,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Crypto(e) => write!(f, "crypto error: {e}"),
            Self::HeartNotAlive => write!(f, "heart is not alive"),
            Self::HeartSuspended => write!(f, "heart is suspended"),
            Self::HeartAlreadySuspended => write!(f, "heart is already suspended"),
            Self::HeartAlreadyAlive => write!(f, "heart is already alive"),
            Self::PulseTreeSealed => write!(f, "pulse tree is sealed (death recorded)"),
            Self::PulseTreeEmpty => write!(f, "pulse tree has no leaves"),
            Self::InvalidPulseChain { leaf_index, reason } => {
                write!(f, "invalid pulse chain at leaf {leaf_index}: {reason}")
            }
            Self::InvalidSignature { leaf_index } => {
                write!(f, "invalid signature at leaf {leaf_index}")
            }
            Self::DelegationExpired => write!(f, "delegation has expired"),
            Self::DelegationNotAccepted => write!(f, "delegation has not been accepted"),
            Self::DelegationAlreadyAccepted => write!(f, "delegation has already been accepted"),
            Self::ScopeWidening { capability } => {
                write!(f, "scope widening: capability {capability} not in parent")
            }
            Self::ConstraintWidening { field } => {
                write!(f, "constraint widening: {field}")
            }
            Self::RoomAlreadySealed => write!(f, "room is already sealed"),
            Self::RoomNotOpen => write!(f, "room is not open"),
            Self::ParticipantAlreadyExists => write!(f, "participant already in room"),
            Self::EnvelopeNotFullySigned => write!(f, "envelope is not fully signed"),
        }
    }
}

impl std::error::Error for Error {}

impl From<soma_crypto::Error> for Error {
    fn from(e: soma_crypto::Error) -> Self {
        Self::Crypto(e)
    }
}

pub type Result<T> = core::result::Result<T, Error>;
