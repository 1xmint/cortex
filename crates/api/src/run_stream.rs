use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures_core::Stream;

use crate::clerk::ClerkUser;
use crate::routes::ErrorResponse;
use crate::run_payload::{build_run_graph_payload, build_run_step_payloads};
use crate::state::AppState;

pub async fn stream_run(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(run_id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, Json<ErrorResponse>)> {
    let db = state.db.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "database not available".into(),
            }),
        )
    })?;

    db.get_run_goal(&run_id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "run not found".into(),
            }),
        )
    })?;

    // Verify the requesting user owns this run
    if !db.verify_run_owner(&run_id, &user.user_id) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "access denied: run belongs to another user".into(),
            }),
        ));
    }

    let run_id_clone = run_id.clone();
    let state_clone = state.clone();

    let stream = async_stream::stream! {
        let mut last_snapshot = String::new();
        let mut tick = 0u64;

        while let Some(db) = &state_clone.db {

            let steps = build_run_step_payloads(db, &run_id_clone);
            let graph = build_run_graph_payload(db, &run_id_clone, &steps);
            let goal = db.get_run_goal(&run_id_clone).unwrap_or_default();
            let heal_count = db.get_run_heal_count(&run_id_clone);

            let snapshot = serde_json::json!({
                "run_id": run_id_clone,
                "goal": goal,
                "heal_attempts": heal_count,
                "steps": steps,
                "graph": graph,
                "tick": tick,
            });

            let snapshot_str = serde_json::to_string(&snapshot).unwrap_or_default();

            // Only send if state changed (or first tick)
            if snapshot_str != last_snapshot {
                last_snapshot = snapshot_str.clone();
                yield Ok(Event::default().event("run_update").data(snapshot_str));
            }

            // Check if run is terminal
            let all_terminal = steps.iter().all(|step| {
                let status = step
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                matches!(
                    status,
                    "succeeded" | "failed" | "recovered" | "cancelled" | "skipped"
                )
            });

            if all_terminal && !steps.is_empty() {
                let any_failed = steps.iter().any(|step| {
                    matches!(
                        step.get("status").and_then(serde_json::Value::as_str),
                        Some("failed" | "skipped")
                    )
                });
                let any_cancelled = steps.iter().any(|step| {
                    matches!(
                        step.get("status").and_then(serde_json::Value::as_str),
                        Some("cancelled")
                    )
                });
                let final_status = if any_failed {
                    "failed"
                } else if any_cancelled {
                    "cancelled"
                } else {
                    "succeeded"
                };
                yield Ok(Event::default().event("run_complete").data(
                    serde_json::json!({
                        "run_id": run_id_clone,
                        "status": final_status,
                    }).to_string()
                ));
                break;
            }

            tick += 1;
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
