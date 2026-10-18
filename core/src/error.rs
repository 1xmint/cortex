/// Every failure the core reports. `Blocked` means a step cannot run safely
/// (for example, the sandbox is missing) and must not fall back to running
/// without protection.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("blocked: {0}")]
    Blocked(String),
    #[error("store: {0}")]
    Store(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("invalid: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;
