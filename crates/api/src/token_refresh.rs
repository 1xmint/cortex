use crate::state::AppState;
use std::sync::Arc;

pub fn spawn_token_refresh_job(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(300));
        loop {
            interval.tick().await;

            let db = match &state.db {
                Some(db) => db,
                None => continue,
            };

            // Check for credentials expiring in the next 10 minutes
            let now = chrono::Utc::now().timestamp();
            let horizon = now + 600; // 10 minutes from now
            let expiring = db.get_expiring_credentials(horizon);

            if expiring.is_empty() {
                continue;
            }

            tracing::info!(count = expiring.len(), "found expiring credentials");

            for (cred, _encrypted_data) in &expiring {
                if cred.token_expires_at.is_some_and(|t| t < now) {
                    // Already expired — mark it
                    db.update_credential_status(&cred.id, "expired");
                    tracing::warn!(
                        credential_id = %cred.id,
                        user_id = %cred.user_id,
                        provider = %cred.provider,
                        "credential expired — marked as expired"
                    );
                } else {
                    // Expiring soon — log warning (Phase 2 will add actual refresh)
                    tracing::info!(
                        credential_id = %cred.id,
                        user_id = %cred.user_id,
                        provider = %cred.provider,
                        expires_at = ?cred.token_expires_at,
                        "credential expiring soon — refresh not yet implemented"
                    );
                }
            }
        }
    });
}
