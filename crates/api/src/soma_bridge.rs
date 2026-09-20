use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use soma::crypto::encode_base64;
use soma::delegation::{create_delegation, Caveat, Delegation};
use soma::identity::HeartIdentity;

use crate::clerk::ClerkUser;
use crate::lock::LockRecovering;
use crate::routes::ErrorResponse;
use crate::state::AppState;

/// A user's Soma identity — created on first session request, persisted across sessions.
#[derive(Debug, Serialize)]
pub struct UserSomaIdentity {
    pub did: String,
    pub public_key: String,
    pub created_at: u64,
}

/// Response from POST /api/soma/session
#[derive(Serialize)]
pub struct SessionResponse {
    pub delegation: Delegation,
    pub user_identity: UserSomaIdentity,
    pub cortex_did: String,
    pub root_did: Option<String>,
}

/// Response from GET /api/soma/identity (user's own identity)
#[derive(Serialize)]
pub struct UserIdentityResponse {
    pub did: String,
    pub public_key: String,
    pub has_delegation: bool,
}

fn user_soma_dir(state: &AppState, user_id: &str) -> PathBuf {
    state
        .workspace_dir
        .join(".cortex")
        .join("users")
        .join(user_id)
}

fn load_or_create_user_identity(state: &AppState, user_id: &str) -> Result<HeartIdentity, String> {
    let dir = user_soma_dir(state, user_id);
    let path = dir.join("soma-identity.json");

    HeartIdentity::load_or_create(&path, "heyvera-user", user_id, &format!("user-{user_id}"))
        .map_err(|e| format!("failed to create user soma identity: {e}"))
}

fn load_root_heart() -> Option<HeartIdentity> {
    // Try HEYVERA_ROOT_HEART_PATH env var first, then default location
    let path = std::env::var("HEYVERA_ROOT_HEART_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs_next::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".heyvera")
                .join("root-heart.json")
        });

    match HeartIdentity::load(&path) {
        Ok(h) => {
            tracing::debug!("root heart loaded from {}", path.display());
            Some(h)
        }
        Err(e) => {
            tracing::warn!(
                "root heart not available at {} — delegation bridge disabled: {e}",
                path.display()
            );
            None
        }
    }
}

/// POST /api/soma/session — Issue a session-scoped Soma delegation to the authenticated Clerk user.
pub async fn create_session(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> Result<Json<SessionResponse>, (StatusCode, Json<ErrorResponse>)> {
    // 1. Load or create the user's Soma identity
    let user_identity = load_or_create_user_identity(&state, &user.user_id).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { error: e }),
        )
    })?;

    // 2. Load the root heart (issuer of delegations)
    let root = load_root_heart().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "root heart not configured — cannot issue delegations".into(),
            }),
        )
    })?;

    // 3. Build session-scoped caveats
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let session_ttl_hours: u64 = std::env::var("SOMA_SESSION_TTL_HOURS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(24);

    let cortex_did = state
        .soma_heart
        .as_ref()
        .map(|h| h.did().to_string())
        .unwrap_or_default();

    let caveats = vec![
        Caveat::ExpiresAt {
            timestamp: now_ms + session_ttl_hours * 3600 * 1000,
        },
        Caveat::Capabilities {
            allow: vec!["route:*".into(), "chat:*".into()],
        },
        Caveat::Audience {
            did: cortex_did.clone(),
        },
    ];

    // 4. Issue delegation from root heart to user
    let delegation = create_delegation(
        &root.secret_key,
        &root.public_key,
        &root.did,
        &user_identity.did,
        vec!["route:*".into(), "chat:*".into()],
        caveats,
        None,
    )
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("failed to create delegation: {e}"),
            }),
        )
    })?;

    // 5. Record heartbeat
    if let Some(heart) = &state.soma_heart {
        heart.record_heartbeat(
            ::soma::heartbeat::HeartbeatEventType::DelegationIssued,
            &serde_json::json!({
                "delegation_id": delegation.id,
                "user_id": user.user_id,
                "subject_did": user_identity.did,
                "ttl_hours": session_ttl_hours,
            })
            .to_string(),
        );
    }

    tracing::info!(
        "soma session issued: user={} did={} delegation={} ttl={}h",
        user.user_id,
        user_identity.did,
        delegation.id,
        session_ttl_hours,
    );

    Ok(Json(SessionResponse {
        delegation,
        user_identity: UserSomaIdentity {
            did: user_identity.did,
            public_key: encode_base64(&user_identity.public_key),
            created_at: user_identity.genome.genome.created_at,
        },
        cortex_did,
        root_did: Some(root.did),
    }))
}

/// POST /api/soma/revoke — Revoke a delegation (admin/self-service).
pub async fn revoke_delegation(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Json(req): Json<RevokeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let heart = state.soma_heart.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "soma heart not configured".into(),
            }),
        )
    })?;

    // Users can only revoke their own delegations (by subject_did match)
    let user_dir = user_soma_dir(&state, &user.user_id);
    let user_identity_path = user_dir.join("soma-identity.json");
    let user_did = HeartIdentity::load(&user_identity_path)
        .ok()
        .map(|id| id.did);

    let is_own = user_did.as_deref() == Some(&req.subject_did);
    let is_admin = !is_own && crate::admin::authorize_admin(&state, &user).await.is_ok();

    if !is_own && !is_admin {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "can only revoke your own delegations".into(),
            }),
        ));
    }

    heart.revoke_delegation(&req.delegation_id);

    Ok(Json(serde_json::json!({
        "revoked": true,
        "delegation_id": req.delegation_id,
    })))
}

#[derive(Deserialize)]
pub struct RevokeRequest {
    pub delegation_id: String,
    pub subject_did: String,
}

/// GET /api/soma/me — Get the authenticated user's Soma identity.
pub async fn get_user_identity(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> Result<Json<UserIdentityResponse>, (StatusCode, Json<ErrorResponse>)> {
    let dir = user_soma_dir(&state, &user.user_id);
    let path = dir.join("soma-identity.json");

    if path.exists() {
        let identity = HeartIdentity::load(&path).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("failed to load identity: {e}"),
                }),
            )
        })?;
        Ok(Json(UserIdentityResponse {
            did: identity.did,
            public_key: encode_base64(&identity.public_key),
            has_delegation: true,
        }))
    } else {
        Ok(Json(UserIdentityResponse {
            did: String::new(),
            public_key: String::new(),
            has_delegation: false,
        }))
    }
}

// --- Spend API ---

#[derive(Serialize)]
pub struct SpendSummary {
    pub delegations: Vec<DelegationSpendSummary>,
    pub total_spend: f64,
}

#[derive(Serialize)]
pub struct DelegationSpendSummary {
    pub delegation_id: String,
    pub cumulative_spend: f64,
    pub receipt_count: usize,
    pub last_activity_ms: u64,
}

#[derive(Serialize)]
pub struct DelegationSpendDetail {
    pub delegation_id: String,
    pub receipts: Vec<ReceiptSummary>,
    pub cumulative: f64,
}

#[derive(Serialize)]
pub struct ReceiptSummary {
    pub amount: f64,
    pub cumulative: f64,
    pub capability: String,
    pub timestamp: u64,
}

/// GET /api/soma/spend — Summary of all spend logs.
pub async fn get_spend(
    State(state): State<Arc<AppState>>,
    _user: ClerkUser,
) -> Result<Json<SpendSummary>, (StatusCode, Json<ErrorResponse>)> {
    let heart = state.soma_heart.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "soma heart not configured".into(),
            }),
        )
    })?;

    let logs = heart.spend_logs.lock_recovering();
    let mut total_spend = 0.0;
    let delegations: Vec<DelegationSpendSummary> = logs
        .values()
        .map(|log| {
            let cumulative = log.cumulative();
            total_spend += cumulative;
            DelegationSpendSummary {
                delegation_id: log.delegation_id().to_string(),
                cumulative_spend: cumulative,
                receipt_count: log.len(),
                last_activity_ms: log.last_activity_ms(),
            }
        })
        .collect();

    Ok(Json(SpendSummary {
        delegations,
        total_spend,
    }))
}

/// GET /api/soma/spend/:delegation_id — Detailed receipts for one delegation.
pub async fn get_spend_detail(
    State(state): State<Arc<AppState>>,
    _user: ClerkUser,
    axum::extract::Path(delegation_id): axum::extract::Path<String>,
) -> Result<Json<DelegationSpendDetail>, (StatusCode, Json<ErrorResponse>)> {
    let heart = state.soma_heart.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "soma heart not configured".into(),
            }),
        )
    })?;

    let logs = heart.spend_logs.lock_recovering();
    let log = logs.get(&delegation_id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("no spend log for delegation {delegation_id}"),
            }),
        )
    })?;

    let receipts: Vec<ReceiptSummary> = log
        .receipts()
        .iter()
        .map(|r| ReceiptSummary {
            amount: r.amount,
            cumulative: r.cumulative,
            capability: r.capability.clone(),
            timestamp: r.timestamp,
        })
        .collect();

    Ok(Json(DelegationSpendDetail {
        delegation_id,
        receipts,
        cumulative: log.cumulative(),
    }))
}
