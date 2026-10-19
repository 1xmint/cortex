//! Full model requests and responses, kept apart from the event log.
//!
//! A `ModelCall` event stays small: it holds tokens, cost and timing, and
//! points at its bodies here by id and SHA-256. Bodies can be deleted
//! (to forget a person's data or save space) while the audit event stays.

use crate::engine::ModelCallRecord;
use crate::event::Event;
use crate::store::{MissionId, Store};
use crate::{Error, Result};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Where a stored body lives, and what it was when it was stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadRef {
    pub id: String,
    pub sha256: String,
    pub bytes: u64,
}

/// What a model call cost, as far as Cortex can know.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Spend {
    /// Billed per token, in millionths of a dollar.
    Usd {
        micros: u64,
    },
    /// Covered by a flat subscription.
    Subscription,
    Unknown,
}

/// Token counts as the provider reported them; `None` when it did not.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    #[serde(default)]
    pub input: Option<u64>,
    #[serde(default)]
    pub output: Option<u64>,
    #[serde(default)]
    pub cache_read: Option<u64>,
    #[serde(default)]
    pub cache_write: Option<u64>,
}

impl Tokens {
    /// Reads an Anthropic `usage` object (API reply or Claude Code line).
    pub fn from_usage(usage: &serde_json::Value) -> Tokens {
        Tokens {
            input: usage["input_tokens"].as_u64(),
            output: usage["output_tokens"].as_u64(),
            cache_read: usage["cache_read_input_tokens"].as_u64(),
            cache_write: usage["cache_creation_input_tokens"].as_u64(),
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Store {
    /// Stores one body for the mission and returns where it lives.
    pub fn put_payload(&self, m: &MissionId, body: &[u8]) -> Result<PayloadRef> {
        let r = PayloadRef {
            id: uuid::Uuid::new_v4().to_string(),
            sha256: sha256_hex(body),
            bytes: body.len() as u64,
        };
        let n = self.conn().execute(
            "INSERT INTO payloads (id, mission_id, project_id, sha256, body, at)
             SELECT ?1, id, project_id, ?2, ?3, ?4 FROM missions WHERE id = ?5",
            params![r.id, r.sha256, body, crate::store::now_ms(), m.0],
        )?;
        if n == 0 {
            return Err(Error::Store(format!("no mission {}", m.0)));
        }
        Ok(r)
    }

    /// The stored body, or `None` once it has been deleted.
    pub fn payload(&self, r: &PayloadRef) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT body FROM payloads WHERE id = ?1",
                params![r.id],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Deletes a body; the `ModelCall` event that pointed at it stays.
    pub fn delete_payload(&self, r: &PayloadRef) -> Result<()> {
        self.conn()
            .execute("DELETE FROM payloads WHERE id = ?1", params![r.id])?;
        Ok(())
    }

    /// Stores the call's bodies, then appends one `ModelCall` event that
    /// points at them. Returns the event's sequence number.
    pub fn record_model_call(&self, m: &MissionId, rec: ModelCallRecord) -> Result<u64> {
        let request = rec.request.map(|b| self.put_payload(m, &b)).transpose()?;
        let response = rec.response.map(|b| self.put_payload(m, &b)).transpose()?;
        self.append(
            m,
            &Event::ModelCall {
                session: rec.session,
                role: rec.role,
                provider: rec.provider,
                profile: rec.profile,
                model: rec.model,
                params: rec.params,
                tokens: rec.tokens,
                estimated_usd_micros: rec.estimated_usd_micros,
                spent: rec.spent,
                millis: rec.millis,
                request,
                response,
            },
        )
    }
}
