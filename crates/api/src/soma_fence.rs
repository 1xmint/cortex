//! The fence between Cortex and Soma.
//!
//! Soma is a work-in-progress project. It is not part of Cortex, and until it
//! is, nothing on the Cortex execution path may depend on it. This module is
//! the single place where the two meet: everything Soma contributes to the
//! HTTP surface enters through here, and with the `soma` feature off — the
//! default — what enters is nothing.
//!
//! The rule this enforces, stated once so it can be tested: **with the feature
//! off, no Cortex receipt, routing decision, or ledger row may carry a value
//! that originated in Soma.** See `docs/adr/ADR-0003-soma-feature-fence.md`.

use std::sync::Arc;

use axum::response::Response;
use axum::Router;

use crate::state::AppState;

/// The `/api/soma/*` routes, or an empty router when the feature is off.
///
/// Merged into the public router rather than chained into it so that the
/// feature gate lives in one place instead of on six `.route()` calls.
#[cfg(feature = "soma")]
pub fn routes() -> Router<Arc<AppState>> {
    use axum::routing::{get, post};

    Router::new()
        // Public — lets clients discover Cortex's DID.
        .route("/api/soma/identity", get(soma_identity))
        // Delegation bridge (Clerk user → Soma session).
        .route("/api/soma/session", post(crate::soma_bridge::create_session))
        .route("/api/soma/revoke", post(crate::soma_bridge::revoke_delegation))
        .route("/api/soma/me", get(crate::soma_bridge::get_user_identity))
        .route("/api/soma/spend", get(crate::soma_bridge::get_spend))
        .route(
            "/api/soma/spend/{delegation_id}",
            get(crate::soma_bridge::get_spend_detail),
        )
}

#[cfg(feature = "soma")]
async fn soma_identity(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> impl axum::response::IntoResponse {
    use crate::lock::LockRecovering;

    match &state.soma_heart {
        Some(heart) => {
            let chain = heart.heartbeat_chain.lock_recovering();
            let capabilities = heart
                .lineage
                .as_ref()
                .map(::soma::lineage::effective_capabilities)
                .unwrap_or_else(|| vec!["*".into()]);
            axum::Json(serde_json::json!({
                "did": heart.did(),
                "genome": heart.identity.genome,
                "protocol": "soma-delegation/0.1",
                "heartbeats": chain.len(),
                "head_hash": chain.head_hash(),
                "capabilities": capabilities,
                "root_did": heart.root_did,
                "has_lineage": heart.lineage.is_some(),
            }))
        }
        None => axum::Json(serde_json::json!({
            "error": "soma heart not initialized"
        })),
    }
}

/// No Soma routes. A request to `/api/soma/*` reaches the router's fallback and
/// is answered as an unknown path — which is the truth, not a stub that
/// pretends the subsystem exists in a degraded state.
#[cfg(not(feature = "soma"))]
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
}

/// Advertises the heart's DID and heartbeat head on every response.
///
/// The function is always present because it is a layer in the middle of a
/// router builder; its body is what the feature gates. With the feature off it
/// is a pass-through that adds no headers, and `AppState` has no `soma_heart`
/// for it to read.
pub async fn headers_middleware(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    request: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Response {
    #[cfg_attr(not(feature = "soma"), allow(unused_mut))]
    let mut response = next.run(request).await;

    #[cfg(feature = "soma")]
    {
        use crate::lock::LockRecovering;

        if let Some(heart) = &state.soma_heart {
            let headers = response.headers_mut();
            headers.insert("X-Soma-Protocol", "soma-delegation/0.1".parse().unwrap());
            headers.insert("X-Soma-Heart-DID", heart.did().parse().unwrap());
            let chain = heart.heartbeat_chain.lock_recovering();
            if let Ok(val) = chain.head_hash().parse() {
                headers.insert("X-Soma-Heartbeat-Head", val);
            }
        }
    }

    #[cfg(not(feature = "soma"))]
    let _ = state;

    response
}

/// Whether this build compiled the Soma surface in.
///
/// Exists so the assertions in `soma_isolation` can read the fence's state
/// through one name instead of scattering `cfg!(feature = "soma")` through the
/// tests they are supposed to be checking.
pub const COMPILED_IN: bool = cfg!(feature = "soma");
