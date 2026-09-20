use std::sync::Arc;

use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use soma::delegation::{verify_delegation, verify_delegation_chain, Delegation, InvocationContext};
use soma::heartbeat::{HeartbeatChain, HeartbeatEventType};
use soma::identity::HeartIdentity;
use soma::lineage::HeartLineage;
use soma::spend::SpendLog;

use crate::lock::LockRecovering;
use crate::state::AppState;

/// Cortex's own Soma heart identity — the agent's cryptographic self.
/// Created once at startup, used to sign heartbeats, birth certificates, and spend receipts.
pub struct CortexHeart {
    pub identity: HeartIdentity,
    pub heartbeat_chain: std::sync::Mutex<HeartbeatChain>,
    pub spend_logs: std::sync::Mutex<std::collections::HashMap<String, SpendLog>>,
    pub revoked_delegations: std::sync::Mutex<std::collections::HashSet<String>>,
    pub invocation_counts: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    pub lineage: Option<HeartLineage>,
    pub root_did: Option<String>,
}

impl CortexHeart {
    pub fn new() -> Result<Self, soma::SomaError> {
        let heart_path = dirs_next::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".cortex")
            .join("soma-heart.json");
        let existed = heart_path.exists();
        let identity =
            HeartIdentity::load_or_create(&heart_path, "heyvera", "cortex-router", "cortex-v0.1")?;
        if existed {
            tracing::info!(
                "soma heart loaded from {} — DID: {}",
                heart_path.display(),
                identity.did
            );
        } else {
            tracing::info!(
                "soma heart created at {} — DID: {}",
                heart_path.display(),
                identity.did
            );
        }

        // Load lineage chain if available
        let lineage_path = heart_path.parent().unwrap().join("lineage.json");
        let (lineage, root_did) = if lineage_path.exists() {
            match std::fs::read_to_string(&lineage_path) {
                Ok(json) => match serde_json::from_str::<HeartLineage>(&json) {
                    Ok(l) => {
                        let root = l.root_did.clone();
                        match soma::lineage::verify_lineage_chain(&l) {
                            Ok(true) => {
                                let caps = soma::lineage::effective_capabilities(&l);
                                tracing::info!(
                                    "soma lineage verified — root: {}, capabilities: [{}]",
                                    root,
                                    caps.join(", ")
                                );
                                (Some(l), Some(root))
                            }
                            Ok(false) => {
                                tracing::warn!(
                                    "soma lineage chain INVALID — running without lineage"
                                );
                                (None, None)
                            }
                            Err(e) => {
                                tracing::warn!("soma lineage verification error: {e}");
                                (None, None)
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("soma lineage parse error: {e}");
                        (None, None)
                    }
                },
                Err(e) => {
                    tracing::warn!("soma lineage read error: {e}");
                    (None, None)
                }
            }
        } else {
            tracing::info!(
                "no soma lineage found at {} — running as standalone heart",
                lineage_path.display()
            );
            (None, None)
        };

        let heartbeat_chain = Self::load_heartbeats();
        let spend_logs = Self::load_spend_logs();
        let revoked = Self::load_revoked();
        let invocation_counts = Self::load_invocation_counts();

        Ok(Self {
            identity,
            heartbeat_chain: std::sync::Mutex::new(heartbeat_chain),
            spend_logs: std::sync::Mutex::new(spend_logs),
            revoked_delegations: std::sync::Mutex::new(revoked),
            invocation_counts: std::sync::Mutex::new(invocation_counts),
            lineage,
            root_did,
        })
    }

    /// Record a heartbeat event. Persists to disk every 50 heartbeats.
    pub fn record_heartbeat(
        &self,
        event_type: HeartbeatEventType,
        event_data: &str,
    ) -> soma::heartbeat::Heartbeat {
        let mut chain = self.heartbeat_chain.lock_recovering();
        let hb = chain.record(event_type, event_data);
        if chain.len() % 50 == 0 {
            Self::persist_chain_inner(&chain);
        }
        hb
    }

    fn heartbeat_path() -> std::path::PathBuf {
        dirs_next::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".cortex")
            .join("heartbeats.json")
    }

    fn persist_chain_inner(chain: &HeartbeatChain) {
        let path = Self::heartbeat_path();
        match serde_json::to_string(chain) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    tracing::warn!("failed to persist heartbeat chain: {e}");
                }
            }
            Err(e) => tracing::warn!("failed to serialize heartbeat chain: {e}"),
        }
    }

    pub fn persist_heartbeats(&self) {
        let chain = self.heartbeat_chain.lock_recovering();
        Self::persist_chain_inner(&chain);
        tracing::info!("heartbeat chain persisted ({} entries)", chain.len());
    }

    fn load_heartbeats() -> HeartbeatChain {
        let path = Self::heartbeat_path();
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(json) => match serde_json::from_str::<HeartbeatChain>(&json) {
                    Ok(chain) => {
                        if chain.verify() {
                            tracing::info!(
                                "heartbeat chain loaded: {} entries, head={}",
                                chain.len(),
                                &chain.head_hash()[..12]
                            );
                            return chain;
                        }
                        tracing::warn!(
                            "persisted heartbeat chain failed verification — starting fresh"
                        );
                    }
                    Err(e) => tracing::warn!("failed to parse heartbeat chain: {e}"),
                },
                Err(e) => tracing::warn!("failed to read heartbeat chain: {e}"),
            }
        }
        HeartbeatChain::new()
    }

    /// Record a spend against a delegation. Persists after every write.
    pub fn record_spend(
        &self,
        delegation_id: &str,
        amount: f64,
        capability: &str,
    ) -> Result<soma::spend::SpendReceipt, soma::SomaError> {
        let mut logs = self.spend_logs.lock_recovering();
        let log = logs
            .entry(delegation_id.to_string())
            .or_insert_with(|| SpendLog::new(delegation_id));
        let receipt = log.record(
            amount,
            capability,
            &self.identity.did,
            &self.identity.secret_key,
            &self.identity.public_key,
        )?;
        Self::persist_spend_logs_inner(&logs);
        Ok(receipt)
    }

    /// Get cumulative spend for a delegation.
    pub fn cumulative_spend(&self, delegation_id: &str) -> f64 {
        let logs = self.spend_logs.lock_recovering();
        logs.get(delegation_id)
            .map(|l| l.cumulative())
            .unwrap_or(0.0)
    }

    fn spend_logs_path() -> std::path::PathBuf {
        dirs_next::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".cortex")
            .join("spend-logs.json")
    }

    fn persist_spend_logs_inner(logs: &std::collections::HashMap<String, SpendLog>) {
        let path = Self::spend_logs_path();
        match serde_json::to_string(logs) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    tracing::warn!("failed to persist spend logs: {e}");
                }
            }
            Err(e) => tracing::warn!("failed to serialize spend logs: {e}"),
        }
    }

    pub fn persist_spend_logs(&self) {
        let logs = self.spend_logs.lock_recovering();
        Self::persist_spend_logs_inner(&logs);
        tracing::info!("spend logs persisted ({} delegations)", logs.len());
    }

    fn load_spend_logs() -> std::collections::HashMap<String, SpendLog> {
        let path = Self::spend_logs_path();
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(json) => match serde_json::from_str(&json) {
                    Ok(logs) => {
                        let logs: std::collections::HashMap<String, SpendLog> = logs;
                        tracing::info!("spend logs loaded: {} delegations", logs.len());
                        return logs;
                    }
                    Err(e) => tracing::warn!("failed to parse spend logs: {e}"),
                },
                Err(e) => tracing::warn!("failed to read spend logs: {e}"),
            }
        }
        std::collections::HashMap::new()
    }

    pub fn revoke_delegation(&self, delegation_id: &str) {
        let mut revoked = self.revoked_delegations.lock_recovering();
        revoked.insert(delegation_id.to_string());
        Self::persist_revoked_inner(&revoked);
        self.record_heartbeat(
            HeartbeatEventType::DelegationRevoked,
            &serde_json::json!({
                "delegation_id": delegation_id,
            })
            .to_string(),
        );
        tracing::info!("delegation revoked: {delegation_id}");
    }

    pub fn is_revoked(&self, delegation_id: &str) -> bool {
        self.revoked_delegations
            .lock_recovering()
            .contains(delegation_id)
    }

    fn revoked_path() -> std::path::PathBuf {
        dirs_next::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".cortex")
            .join("revoked-delegations.json")
    }

    fn persist_revoked_inner(revoked: &std::collections::HashSet<String>) {
        let path = Self::revoked_path();
        match serde_json::to_string(revoked) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    tracing::warn!("failed to persist revocation set: {e}");
                }
            }
            Err(e) => tracing::warn!("failed to serialize revocation set: {e}"),
        }
    }

    fn load_revoked() -> std::collections::HashSet<String> {
        let path = Self::revoked_path();
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(json) => match serde_json::from_str(&json) {
                    Ok(set) => {
                        let set: std::collections::HashSet<String> = set;
                        tracing::info!("revocation set loaded: {} entries", set.len());
                        return set;
                    }
                    Err(e) => tracing::warn!("failed to parse revocation set: {e}"),
                },
                Err(e) => tracing::warn!("failed to read revocation set: {e}"),
            }
        }
        std::collections::HashSet::new()
    }

    /// Prune spend logs for delegations with no activity in the given retention window.
    pub fn prune_spend_logs(&self, retention_ms: u64) -> usize {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let cutoff = now_ms.saturating_sub(retention_ms);

        let mut logs = self.spend_logs.lock_recovering();
        let before = logs.len();
        logs.retain(|_, log| log.last_activity_ms() > cutoff);
        let pruned = before - logs.len();
        if pruned > 0 {
            Self::persist_spend_logs_inner(&logs);
            tracing::info!(
                "pruned {pruned} expired spend logs ({} remaining)",
                logs.len()
            );
        }
        pruned
    }

    /// Get the current invocation count for a delegation and increment it.
    pub fn increment_invocations(&self, delegation_id: &str) -> u64 {
        let mut counts = self.invocation_counts.lock_recovering();
        let count = counts.entry(delegation_id.to_string()).or_insert(0);
        let current = *count;
        *count += 1;
        if *count % 100 == 0 {
            Self::persist_invocation_counts_inner(&counts);
        }
        current
    }

    /// Get the current invocation count for a delegation without incrementing.
    pub fn invocation_count(&self, delegation_id: &str) -> u64 {
        self.invocation_counts
            .lock()
            .unwrap()
            .get(delegation_id)
            .copied()
            .unwrap_or(0)
    }

    fn invocation_counts_path() -> std::path::PathBuf {
        dirs_next::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".cortex")
            .join("invocation-counts.json")
    }

    fn persist_invocation_counts_inner(counts: &std::collections::HashMap<String, u64>) {
        let path = Self::invocation_counts_path();
        match serde_json::to_string(counts) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    tracing::warn!("failed to persist invocation counts: {e}");
                }
            }
            Err(e) => tracing::warn!("failed to serialize invocation counts: {e}"),
        }
    }

    pub fn persist_invocation_counts(&self) {
        let counts = self.invocation_counts.lock_recovering();
        Self::persist_invocation_counts_inner(&counts);
    }

    fn load_invocation_counts() -> std::collections::HashMap<String, u64> {
        let path = Self::invocation_counts_path();
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(json) => match serde_json::from_str(&json) {
                    Ok(counts) => {
                        let counts: std::collections::HashMap<String, u64> = counts;
                        tracing::info!("invocation counts loaded: {} delegations", counts.len());
                        return counts;
                    }
                    Err(e) => tracing::warn!("failed to parse invocation counts: {e}"),
                },
                Err(e) => tracing::warn!("failed to read invocation counts: {e}"),
            }
        }
        std::collections::HashMap::new()
    }

    pub fn did(&self) -> &str {
        &self.identity.did
    }
}

/// The authenticated identity extracted from a request.
/// Supports three auth methods:
/// 1. Soma delegation token (preferred)
/// 2. Clerk JWT (legacy, for web UI users)
/// 3. API key (for programmatic access)
#[derive(Debug, Clone, Serialize)]
pub struct AuthenticatedIdentity {
    /// The user's identifier (Clerk user_id, DID, or API key hash)
    pub user_id: String,
    /// The auth method used
    pub method: AuthMethod,
    /// Soma DID (if authenticated via Soma delegation)
    pub did: Option<String>,
    /// Active delegation (if using Soma auth)
    pub delegation_id: Option<String>,
    /// Capabilities granted to this identity
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    SomaDelegation,
    ClerkJwt,
    ApiKey,
    Anonymous,
}

/// Soma delegation headers per SOMA-DELEGATION-SPEC.
const SOMA_AUTH_PREFIX: &str = "Soma ";
const SOMA_DELEGATION_CHAIN_HEADER: &str = "X-Soma-Delegation-Chain";

/// Extract an authenticated identity from the request.
/// Priority: Soma delegation > Clerk JWT > API key > anonymous (dev mode only)
pub async fn extract_identity(
    parts: &Parts,
    state: &Arc<AppState>,
) -> Result<AuthenticatedIdentity, AuthError> {
    let auth_header = parts
        .headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok());

    // 1. Check for Soma delegation token
    if let Some(header) = auth_header {
        if let Some(token) = header.strip_prefix(SOMA_AUTH_PREFIX) {
            return verify_soma_token(token, parts, state).await;
        }
    }

    // 2. Check for Clerk JWT (Bearer token)
    if let Some(header) = auth_header {
        if let Some(token) = header.strip_prefix("Bearer ") {
            return verify_clerk_token(token, state).await;
        }
    }

    // 3. Check for API key
    if let Some(api_key) = parts.headers.get("X-Api-Key").and_then(|v| v.to_str().ok()) {
        return verify_api_key(api_key, state).await;
    }

    // 4. Anonymous access when auth is not configured
    if state.clerk_secret_key.is_none() {
        return Ok(AuthenticatedIdentity {
            user_id: "anonymous".into(),
            method: AuthMethod::Anonymous,
            did: None,
            delegation_id: None,
            capabilities: vec!["*".into()],
        });
    }

    Err(AuthError::Unauthorized(
        "no valid authentication provided".into(),
    ))
}

async fn verify_soma_token(
    token: &str,
    parts: &Parts,
    state: &Arc<AppState>,
) -> Result<AuthenticatedIdentity, AuthError> {
    let delegation: Delegation = serde_json::from_str(token)
        .map_err(|e| AuthError::InvalidToken(format!("malformed soma delegation token: {e}")))?;

    // Check revocation set before expensive crypto verification
    if let Some(heart) = &state.soma_heart {
        if heart.is_revoked(&delegation.id) {
            return Err(AuthError::Unauthorized(
                "delegation has been revoked".into(),
            ));
        }
    }

    // Check for delegation chain header
    let chain_header = parts
        .headers
        .get(SOMA_DELEGATION_CHAIN_HEADER)
        .and_then(|v| v.to_str().ok());

    // Look up cumulative spend for this delegation (budget enforcement)
    let cumulative_spend = state
        .soma_heart
        .as_ref()
        .map(|h| h.cumulative_spend(&delegation.id));

    // Look up invocation count for MaxInvocations enforcement
    let invocation_count = state
        .soma_heart
        .as_ref()
        .map(|h| h.invocation_count(&delegation.id));

    // Extract host from request for HostAllowlist enforcement
    let host = parts
        .headers
        .get("Host")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let cortex_did = state
        .soma_heart
        .as_ref()
        .map(|h| h.did().to_string())
        .unwrap_or_default();

    let ctx = InvocationContext {
        invoker_did: delegation.subject_did.clone(),
        audience_did: Some(cortex_did),
        capability: "route:*".into(),
        cumulative_credits_spent: cumulative_spend,
        invocation_count,
        host,
        ..Default::default()
    };

    // Verify the delegation issuer is Cortex's own heart (trusted root).
    // Without this check, anyone could self-issue a delegation.
    let cortex_did_ref = state.soma_heart.as_ref().map(|h| h.did().to_string());

    if let Some(chain_json) = chain_header {
        let chain: Vec<Delegation> = serde_json::from_str(chain_json)
            .map_err(|e| AuthError::InvalidToken(format!("malformed delegation chain: {e}")))?;

        // Chain root must be issued by Cortex's heart
        if let (Some(first), Some(cortex_did)) = (chain.first(), &cortex_did_ref) {
            if first.issuer_did != *cortex_did {
                return Err(AuthError::Unauthorized(format!(
                    "delegation chain root issuer {} is not Cortex heart {}",
                    first.issuer_did, cortex_did
                )));
            }
        }

        let result = verify_delegation_chain(&chain, &ctx)
            .map_err(|e| AuthError::VerificationFailed(format!("chain verification error: {e}")))?;

        if !result.is_valid() {
            return Err(AuthError::Unauthorized(format!(
                "delegation chain invalid: {result:?}"
            )));
        }
    } else {
        // Single delegation must be issued by Cortex's heart
        if let Some(ref cortex_did) = cortex_did_ref {
            if delegation.issuer_did != *cortex_did {
                return Err(AuthError::Unauthorized(format!(
                    "delegation issuer {} is not Cortex heart {}",
                    delegation.issuer_did, cortex_did
                )));
            }
        }

        let result = verify_delegation(&delegation, &ctx).map_err(|e| {
            AuthError::VerificationFailed(format!("delegation verification error: {e}"))
        })?;

        if !result.is_valid() {
            return Err(AuthError::Unauthorized(format!(
                "delegation invalid: {result:?}"
            )));
        }
    }

    // Increment invocation count and record heartbeat
    if let Some(heart) = &state.soma_heart {
        heart.increment_invocations(&delegation.id);
        heart.record_heartbeat(
            HeartbeatEventType::QueryReceived,
            &serde_json::json!({
                "delegation_id": delegation.id,
                "subject_did": delegation.subject_did,
                "capabilities": delegation.capabilities,
            })
            .to_string(),
        );
    }

    Ok(AuthenticatedIdentity {
        user_id: delegation.subject_did.clone(),
        method: AuthMethod::SomaDelegation,
        did: Some(delegation.subject_did),
        delegation_id: Some(delegation.id),
        capabilities: delegation.capabilities,
    })
}

async fn verify_clerk_token(
    token: &str,
    state: &Arc<AppState>,
) -> Result<AuthenticatedIdentity, AuthError> {
    // Delegate to existing Clerk verification
    let user_id = crate::clerk::verify_clerk_jwt(token, state)
        .await
        .map_err(|e| AuthError::Unauthorized(format!("clerk JWT invalid: {e}")))?;

    Ok(AuthenticatedIdentity {
        user_id: user_id.clone(),
        method: AuthMethod::ClerkJwt,
        did: None,
        delegation_id: None,
        capabilities: vec!["route:*".into(), "chat:*".into()],
    })
}

async fn verify_api_key(
    _api_key: &str,
    _state: &Arc<AppState>,
) -> Result<AuthenticatedIdentity, AuthError> {
    // API key verification will be implemented when we add API key management
    Err(AuthError::Unauthorized(
        "API key auth not yet implemented".into(),
    ))
}

#[derive(Debug)]
pub enum AuthError {
    Unauthorized(String),
    InvalidToken(String),
    VerificationFailed(String),
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AuthError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            AuthError::InvalidToken(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AuthError::VerificationFailed(msg) => (StatusCode::FORBIDDEN, msg.clone()),
        };
        let body = serde_json::json!({ "error": message });
        (status, axum::Json(body)).into_response()
    }
}

// `soma_headers_middleware` moved to `crate::soma_fence`. It is a layer in the
// middle of a router builder, so it has to exist in both configurations; the
// fence owns it and gates its body.
