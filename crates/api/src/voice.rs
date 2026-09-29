//! `POST /api/voice/dictation/token` — dictation is refused for now.
//!
//! Cortex bills pass-through: a customer pays exactly what each model call
//! cost, measured after the fact, never a guessed or fixed price. Dictation
//! cannot be measured that way yet. The browser would hold an ephemeral
//! OpenAI token and stream audio straight to OpenAI, so nothing comes back
//! through Cortex to settle against, and the only way to charge for it would
//! be a guess made up front.
//!
//! Until dictation is brokered through Cortex (so its usage is observed and
//! metered before launch), the route refuses every request with "Dictation is
//! not available yet". It never touches the ledger, never mints a token and
//! never calls the supplier.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Serialize;

use crate::clerk::ClerkUser;
use crate::routes::ErrorResponse;
use crate::state::AppState;

/// What the route used to return; kept so the client's response type does not
/// change shape. It is never produced while dictation is refused.
#[derive(Debug, Serialize, PartialEq)]
pub struct DictationTokenResponse {
    pub token: String,
    pub expires_at: i64,
    pub seconds: i64,
}

const NOT_AVAILABLE: &str = "Dictation is not available yet";

fn not_available() -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ErrorResponse {
            error: NOT_AVAILABLE.into(),
        }),
    )
}

pub async fn dictation_token(
    State(_state): State<Arc<AppState>>,
    _user: ClerkUser,
    _headers: HeaderMap,
) -> Result<Json<DictationTokenResponse>, (StatusCode, Json<ErrorResponse>)> {
    Err(not_available())
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER: &str = "user-1";

    fn ledger_rows(state: &AppState) -> i64 {
        state
            .db
            .as_ref()
            .unwrap()
            .conn()
            .query_row("SELECT COUNT(*) FROM credit_transactions", [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    #[tokio::test]
    async fn dictation_is_refused_with_zero_ledger_rows_and_an_untouched_balance() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(
            dir.path().join(".cortex/ledger.jsonl"),
            dir.path().to_path_buf(),
            None,
        )
        .await;
        state
            .db
            .as_ref()
            .unwrap()
            .init_credit_balance(USER, 1_000)
            .unwrap();
        let rows_before = ledger_rows(&state);

        let (status, Json(body)) = dictation_token(
            State(state.clone()),
            ClerkUser {
                user_id: USER.into(),
            },
            HeaderMap::new(),
        )
        .await
        .expect_err("dictation must be refused");

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body.error, "Dictation is not available yet");
        assert_eq!(ledger_rows(&state), rows_before, "no ledger rows written");

        let balance = state.db.as_ref().unwrap().get_credit_balance_row(USER);
        assert_eq!(balance.unwrap().subscription_remaining, 1_000);
    }

    #[tokio::test]
    async fn a_retried_request_with_an_idempotency_key_is_refused_the_same_way() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(
            dir.path().join(".cortex/ledger.jsonl"),
            dir.path().to_path_buf(),
            None,
        )
        .await;
        state
            .db
            .as_ref()
            .unwrap()
            .init_credit_balance(USER, 1_000)
            .unwrap();
        let rows_before = ledger_rows(&state);

        for _ in 0..2 {
            let mut headers = HeaderMap::new();
            headers.insert("Idempotency-Key", "same-key".parse().unwrap());
            let (status, _) = dictation_token(
                State(state.clone()),
                ClerkUser {
                    user_id: USER.into(),
                },
                headers,
            )
            .await
            .expect_err("dictation must be refused");
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        }
        assert_eq!(ledger_rows(&state), rows_before);
    }
}
