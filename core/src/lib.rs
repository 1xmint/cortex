//! Cortex core: missions, the event log, the sandbox, checks and engines.

pub mod bench;
pub mod checks;
pub mod engine;
pub mod error;
pub mod event;
pub mod git;
pub mod mcp;
pub mod mission;
pub mod sandbox;
pub mod store;

pub use error::{Error, Result};

/// The commit this binary was built from (12 characters), or `unknown`.
pub const BUILD: &str = env!("CORTEX_BUILD");
