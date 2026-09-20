use thiserror::Error;

use crate::provider::ProviderId;

#[derive(Debug, Error)]
pub enum CortexError {
    #[error("no provider available for tier {tier}")]
    NoProviderAvailable { tier: String },

    #[error("provider {0} not authenticated")]
    ProviderNotAuthenticated(ProviderId),

    #[error("provider {0} at capacity")]
    ProviderAtCapacity(ProviderId),

    #[error("task {0} not found")]
    TaskNotFound(uuid::Uuid),

    #[error("approval required for {0} risk operation")]
    ApprovalRequired(String),

    #[error("intent could not be classified from input")]
    IntentUnclassifiable,

    #[error("ledger write failed: {0}")]
    LedgerWrite(String),

    #[error("worker execution failed: {0}")]
    WorkerExecution(String),
}
