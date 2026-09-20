use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::clerk::ClerkUser;
use crate::routes::ErrorResponse;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct AssignCredentialRequest {
    pub credential_id: String,
    pub target_type: String, // "project", "group", "global"
    pub target_id: Option<String>,
    pub permissions: Option<AssignmentPermissions>,
}

#[derive(Deserialize, Serialize)]
pub struct AssignmentPermissions {
    pub max_tokens_per_day: Option<i64>,
    pub expires_at: Option<i64>,
    pub allowed_models: Option<Vec<String>>,
}

#[derive(Serialize)]
pub struct AssignmentResponse {
    pub success: bool,
    pub assignment_id: Option<String>,
    pub message: String,
}

fn db_error(msg: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: msg.to_string(),
        }),
    )
}

fn bad_request(msg: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: msg.to_string(),
        }),
    )
}

fn not_found(msg: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorResponse {
            error: msg.to_string(),
        }),
    )
}

fn forbidden(msg: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::FORBIDDEN,
        Json(ErrorResponse {
            error: msg.to_string(),
        }),
    )
}

/// POST /api/credentials/assign
///
/// Assigns a credential to a target (project, group, or globally for the user).
/// The credential must already belong to the authenticated user.
pub async fn assign_credential(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Json(req): Json<AssignCredentialRequest>,
) -> Result<Json<AssignmentResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Validate target_type
    let valid_targets = ["project", "group", "global"];
    if !valid_targets.contains(&req.target_type.as_str()) {
        return Err(bad_request(&format!(
            "invalid target_type '{}'; must be one of: project, group, global",
            req.target_type
        )));
    }

    // Non-global targets require a target_id
    if req.target_type != "global" && req.target_id.is_none() {
        return Err(bad_request(
            "target_id is required when target_type is not 'global'",
        ));
    }

    let db = state
        .db
        .as_ref()
        .ok_or_else(|| db_error("database unavailable"))?;

    // Verify the credential belongs to this user
    let (cred, _encrypted) = db
        .get_credential(&req.credential_id)
        .ok_or_else(|| not_found("credential not found"))?;

    if cred.user_id != user.user_id {
        return Err(forbidden("credential does not belong to this user"));
    }

    // Serialize permissions to JSON if provided
    let permissions_json: Option<String> = req
        .permissions
        .as_ref()
        .map(|p| serde_json::to_string(p).unwrap_or_default());

    let assignment_id = Uuid::new_v4().to_string();
    let target_id_ref = req.target_id.as_deref();

    db.assign_credential(
        &assignment_id,
        &req.credential_id,
        &user.user_id,
        &req.target_type,
        target_id_ref,
        permissions_json.as_deref(),
    );

    db.audit_log(
        &user.user_id,
        "user",
        "credential.assigned",
        Some("credential_assignment"),
        Some(&assignment_id),
        Some(&format!(
            "{{\"credential_id\":\"{}\",\"target_type\":\"{}\"}}",
            req.credential_id, req.target_type
        )),
        None,
    );

    Ok(Json(AssignmentResponse {
        success: true,
        assignment_id: Some(assignment_id),
        message: "credential assigned successfully".to_string(),
    }))
}

/// GET /api/credentials/assignments
///
/// Lists all credential assignments for the authenticated user.
pub async fn list_assignments(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> Result<Json<Vec<crate::db::CredentialAssignment>>, (StatusCode, Json<ErrorResponse>)> {
    let db = state
        .db
        .as_ref()
        .ok_or_else(|| db_error("database unavailable"))?;

    let assignments = db.get_credential_assignments(&user.user_id);

    Ok(Json(assignments))
}

/// DELETE /api/credentials/assignments/:id
///
/// Removes a credential assignment. Only the owning user can remove their own assignments.
pub async fn remove_assignment(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(assignment_id): Path<String>,
) -> Result<Json<AssignmentResponse>, (StatusCode, Json<ErrorResponse>)> {
    let db = state
        .db
        .as_ref()
        .ok_or_else(|| db_error("database unavailable"))?;

    let removed = db.remove_credential_assignment(&user.user_id, &assignment_id);

    if removed {
        db.audit_log(
            &user.user_id,
            "user",
            "credential.unassigned",
            Some("credential_assignment"),
            Some(&assignment_id),
            None,
            None,
        );
        Ok(Json(AssignmentResponse {
            success: true,
            assignment_id: Some(assignment_id),
            message: "assignment removed".to_string(),
        }))
    } else {
        Err(not_found(
            "assignment not found or does not belong to this user",
        ))
    }
}
