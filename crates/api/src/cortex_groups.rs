//! Cortex group tasks, authority, and approvals.
//!
//! Split out of `integrations.rs` on 2026-09-05. That file was classified as a
//! Socials module because of its Slack and Replit OAuth, and the classification
//! was wrong: roughly two thirds of it was the Cortex task-state machine —
//! `cortex_groups`, `cortex_tasks`, `cortex_authority_scopes`,
//! `cortex_authority_delegations`, `cortex_approval_requests` — sitting behind
//! the same `mod`.
//!
//! The consequence, measured in `docs/proposals/cortex-socials-split-inventory.md`:
//! `integrations.rs` was the only file in the crate reaching Cortex's step
//! lifecycle from the Socials side, which made the two products look far more
//! entangled than they are. With this moved, the shared runtime surface between
//! them is two ledger calls from `pulse.rs` and nothing else.
//!
//! What stays in `integrations.rs` is the Socials half plus `slack_command` and
//! `import_replit_workspace`, the two functions where a Socials integration
//! creates Cortex work. Those two are the seam, and they now call across a
//! module boundary that names it.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::clerk::ClerkUser;
use crate::db::CortexTaskStateEvent;
use crate::routes::{db_ref, ApiResult, ErrorResponse};
use crate::state::AppState;

pub async fn get_group_tasks(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(group_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    // If using local auth (no real user), return empty task state to prevent frontend crashes
    if user.user_id == "local" && state.clerk_secret_key.is_none() {
        return Ok(Json(empty_task_state(&group_id)));
    }

    let db = db_ref(&state)?;
    let state = db
        .get_group_task_state(&user.user_id, &group_id)
        .and_then(|value| normalize_task_state(&group_id, &value).ok())
        .unwrap_or_else(|| empty_task_state(&group_id));
    Ok(Json(state))
}

pub async fn get_group_task_projection(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path((group_id, task_id)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    let db = db_ref(&state)?;
    let projection = db
        .get_cortex_task_projection(&user.user_id, &group_id, &task_id, 100)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "task not found".into(),
                }),
            )
        })?;
    Ok(Json(projection))
}

#[derive(Deserialize)]
pub struct AttachTaskChatRequest {
    pub conversation_id: String,
}

pub async fn attach_group_task_chat(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path((group_id, task_id)): Path<(String, String)>,
    Json(request): Json<AttachTaskChatRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let conversation_id = request.conversation_id.trim();
    if conversation_id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "conversation_id is required".into(),
            }),
        ));
    }

    let db = db_ref(&state)?;
    if !db.cortex_task_exists(&user.user_id, &group_id, &task_id) {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "task not found".into(),
            }),
        ));
    }
    if !db.conversation_exists(&user.user_id, conversation_id) {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "conversation not found".into(),
            }),
        ));
    }
    if !db.attach_cortex_task_chat(&user.user_id, &group_id, &task_id, conversation_id) {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "task not found".into(),
            }),
        ));
    }

    let projection = db
        .get_cortex_task_projection(&user.user_id, &group_id, &task_id, 100)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "task not found".into(),
                }),
            )
        })?;
    Ok(Json(projection))
}

pub async fn create_group_task(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(group_id): Path<String>,
    Json(task): Json<serde_json::Value>,
) -> ApiResult<Json<serde_json::Value>> {
    let db = db_ref(&state)?;
    let current = db
        .get_group_task_state(&user.user_id, &group_id)
        .and_then(|value| normalize_task_state(&group_id, &value).ok())
        .unwrap_or_else(|| empty_task_state(&group_id));
    let (next_state, task_id, event_payload) = apply_task_create(&group_id, &current, &task)
        .map_err(|error| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error })))?;
    require_evidence_for_done_transitions(db, &user.user_id, &group_id, &current, &next_state)
        .map_err(|error| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error })))?;
    let events = vec![task_state_task_event(
        task_id,
        "task.created".to_string(),
        event_payload,
    )];
    let saved = db
        .upsert_group_task_state_with_events(&user.user_id, &group_id, &next_state, &events, None)
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error }),
            )
        })?;
    Ok(Json(saved))
}

pub async fn apply_group_task_actions(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(group_id): Path<String>,
    Json(request): Json<serde_json::Value>,
) -> ApiResult<Json<serde_json::Value>> {
    let db = db_ref(&state)?;
    let current = db
        .get_group_task_state(&user.user_id, &group_id)
        .and_then(|value| normalize_task_state(&group_id, &value).ok())
        .unwrap_or_else(|| empty_task_state(&group_id));
    let (next_state, events) = apply_task_actions(&group_id, &current, &request)
        .map_err(|error| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error })))?;
    require_evidence_for_done_transitions(db, &user.user_id, &group_id, &current, &next_state)
        .map_err(|error| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error })))?;
    let events = events
        .into_iter()
        .map(|event| {
            if let Some(task_id) = event.task_id {
                task_state_task_event(task_id, event.event_type, event.payload)
            } else {
                task_state_manager_event(&group_id, event.event_type, event.payload)
            }
        })
        .collect::<Vec<_>>();
    let saved = db
        .upsert_group_task_state_with_events(&user.user_id, &group_id, &next_state, &events, None)
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error }),
            )
        })?;
    Ok(Json(saved))
}

pub async fn patch_group_task(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path((group_id, task_id)): Path<(String, String)>,
    Json(patch): Json<serde_json::Value>,
) -> ApiResult<Json<serde_json::Value>> {
    let db = db_ref(&state)?;
    let attached_conversation_id = patch
        .get("projectChatConversationId")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if let Some(conversation_id) = attached_conversation_id.as_deref() {
        if !db.conversation_exists(&user.user_id, conversation_id) {
            return Err((
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "conversation not found".into(),
                }),
            ));
        }
    }
    let current = db
        .get_group_task_state(&user.user_id, &group_id)
        .and_then(|value| normalize_task_state(&group_id, &value).ok())
        .unwrap_or_else(|| empty_task_state(&group_id));
    let (next_state, event_type, event_payload) =
        apply_task_patch(&group_id, &task_id, &current, &patch).map_err(|error| {
            let status = if error == "task not found" {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, Json(ErrorResponse { error }))
        })?;
    require_evidence_for_done_transitions(db, &user.user_id, &group_id, &current, &next_state)
        .map_err(|error| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error })))?;
    let events = vec![task_state_task_event(
        task_id.clone(),
        event_type,
        event_payload,
    )];
    let saved = db
        .upsert_group_task_state_with_events(
            &user.user_id,
            &group_id,
            &next_state,
            &events,
            attached_conversation_id
                .as_deref()
                .map(|conversation_id| (task_id.as_str(), conversation_id)),
        )
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error }),
            )
        })?;
    Ok(Json(saved))
}

pub async fn get_group_operations_summary(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(group_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let db = db_ref(&state)?;
    Ok(Json(db.get_group_operations_summary(
        &user.user_id,
        &group_id,
        25,
    )))
}

pub async fn get_group_operations_graph(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(group_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let db = db_ref(&state)?;
    Ok(Json(db.get_group_operations_graph(
        &user.user_id,
        &group_id,
        100,
    )))
}

pub async fn get_personal_operations_summary(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> ApiResult<Json<serde_json::Value>> {
    let db = db_ref(&state)?;
    Ok(Json(db.get_personal_operations_summary(&user.user_id, 50)))
}

pub async fn list_authority_scopes(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> ApiResult<Json<serde_json::Value>> {
    let db = db_ref(&state)?;
    let scopes = db
        .list_authority_scopes_for_user(&user.user_id)
        .into_iter()
        .map(|scope| {
            let resources = db.list_authority_resources_for_user(&user.user_id, &scope.id);
            serde_json::json!({
                "id": scope.id,
                "kind": scope.kind,
                "name": scope.name,
                "description": scope.description,
                "source": scope.source,
                "external_id": scope.external_id,
                "status": scope.status,
                "role": scope.role,
                "policy": scope.policy,
                "created_at": scope.created_at,
                "updated_at": scope.updated_at,
                "resources": resources,
            })
        })
        .collect::<Vec<_>>();

    Ok(Json(serde_json::json!({
        "scopes": scopes,
    })))
}

#[derive(Deserialize)]
pub struct CreateAuthorityScopeRequest {
    pub name: String,
    pub description: Option<String>,
    pub kind: Option<String>,
    pub authority_level: String,
    pub resources: Vec<AuthorityScopeResourceRequest>,
    pub members: Vec<AuthorityScopeMemberRequest>,
}

#[derive(Deserialize)]
pub struct AuthorityScopeResourceRequest {
    pub resource_type: String,
    pub resource_key: String,
    pub access: String,
}

#[derive(Deserialize)]
pub struct AuthorityScopeMemberRequest {
    pub user_id: String,
    pub role: String,
}

/// Deserialised from the request body. The fields are never read in Rust yet
/// -- the handler that will consume them is not written -- but the shape is the
/// wire contract, so it stays and says so.
#[allow(dead_code)]
#[derive(Deserialize)]
pub struct UpdateAuthorityScopeRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
}

#[derive(Deserialize)]
pub struct DelegateAuthorityRequest {
    pub to_user_id: String,
    pub scope_id: String,
    pub reason: Option<String>,
    pub expires_at: Option<i64>,
    pub conditions: Option<serde_json::Value>,
}

#[derive(Serialize)]
pub struct AuthorityDelegation {
    pub id: String,
    pub from_user_id: String,
    pub to_user_id: String,
    pub scope_id: String,
    pub status: String,
    pub reason: Option<String>,
    pub expires_at: Option<i64>,
    pub conditions: Option<serde_json::Value>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub async fn create_authority_scope(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Json(req): Json<CreateAuthorityScopeRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let db = db_ref(&state)?;

    // Validate request
    if req.name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "scope name cannot be empty".into(),
            }),
        ));
    }

    let scope_id = format!(
        "{}:{}",
        req.kind.as_deref().unwrap_or("team"),
        Uuid::new_v4()
    );
    let description = req
        .description
        .unwrap_or_else(|| format!("Authority scope for {}", req.name));
    let kind = req.kind.unwrap_or_else(|| "team".to_string());

    // Create scope with appropriate authority level policy
    let policy = serde_json::json!({
        "authority_level": req.authority_level,
        "created_by": user.user_id,
        "approval_policy": match req.authority_level.as_str() {
            "owner" | "admin" => "automatic",
            _ => "user"
        }
    });

    let scope = db.upsert_authority_scope(
        &user.user_id,
        &scope_id,
        &kind,
        &req.name,
        &description,
        "cortex",
        None,
        policy,
    );

    // Add creator as owner
    db.add_authority_membership(&scope_id, &user.user_id, "owner");

    // Add members
    for member in req.members {
        db.add_authority_membership(&scope_id, &member.user_id, &member.role);
    }

    // Add resources
    for resource in req.resources {
        db.add_authority_resource(
            &scope_id,
            &resource.resource_type,
            &resource.resource_key,
            &resource.access,
            serde_json::json!({}),
        );
    }

    let resources = db.list_authority_resources_for_user(&user.user_id, &scope_id);

    Ok(Json(serde_json::json!({
        "id": scope.id,
        "kind": scope.kind,
        "name": scope.name,
        "description": scope.description,
        "source": scope.source,
        "external_id": scope.external_id,
        "status": scope.status,
        "role": scope.role,
        "policy": scope.policy,
        "created_at": scope.created_at,
        "updated_at": scope.updated_at,
        "resources": resources,
    })))
}

pub async fn update_authority_scope(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(scope_id): Path<String>,
    Json(_req): Json<UpdateAuthorityScopeRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let db = db_ref(&state)?;

    // Verify user has admin access to this scope
    let scope = db
        .get_authority_scope_for_user(&user.user_id, &scope_id)
        .ok_or_else(|| {
            (
                StatusCode::FORBIDDEN,
                Json(ErrorResponse {
                    error: "access denied or scope not found".into(),
                }),
            )
        })?;

    if scope.role != "owner" && scope.role != "admin" {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "insufficient permissions to modify scope".into(),
            }),
        ));
    }

    // Update scope fields as needed
    // Note: This would require adding an update method to the Database impl
    // For now, return the current scope
    let resources = db.list_authority_resources_for_user(&user.user_id, &scope_id);

    Ok(Json(serde_json::json!({
        "id": scope.id,
        "kind": scope.kind,
        "name": scope.name,
        "description": scope.description,
        "source": scope.source,
        "external_id": scope.external_id,
        "status": scope.status,
        "role": scope.role,
        "policy": scope.policy,
        "created_at": scope.created_at,
        "updated_at": scope.updated_at,
        "resources": resources,
    })))
}

pub async fn delegate_authority(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Json(req): Json<DelegateAuthorityRequest>,
) -> ApiResult<Json<AuthorityDelegation>> {
    let db = db_ref(&state)?;

    // Verify user has authority to delegate from this scope
    let scope = db
        .get_authority_scope_for_user(&user.user_id, &req.scope_id)
        .ok_or_else(|| {
            (
                StatusCode::FORBIDDEN,
                Json(ErrorResponse {
                    error: "access denied or scope not found".into(),
                }),
            )
        })?;

    if scope.role != "owner" && scope.role != "admin" {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "insufficient permissions to delegate authority".into(),
            }),
        ));
    }

    let delegation_id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp_millis();

    // Store delegation record (would need new table/method)
    let delegation = AuthorityDelegation {
        id: delegation_id,
        from_user_id: user.user_id.clone(),
        to_user_id: req.to_user_id,
        scope_id: req.scope_id,
        status: "active".to_string(),
        reason: req.reason,
        expires_at: req.expires_at,
        conditions: req.conditions,
        created_at: now,
        updated_at: now,
    };

    tracing::info!(
        delegation_id = %delegation.id,
        delegated_by = %user.user_id,
        granted_to = %delegation.to_user_id,
        scope_id = %delegation.scope_id,
        "authority delegation created"
    );

    Ok(Json(delegation))
}

pub async fn revoke_authority_delegation(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(delegation_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let _db = db_ref(&state)?;

    tracing::info!(
        delegation_id = %delegation_id,
        revoked_by = %user.user_id,
        "authority delegation revoked"
    );

    Ok(Json(serde_json::json!({
        "revoked": true
    })))
}

pub async fn list_authority_delegations(
    State(state): State<Arc<AppState>>,
    _user: ClerkUser,
) -> ApiResult<Json<serde_json::Value>> {
    let _db = db_ref(&state)?;

    // For now, return empty list since we need to implement delegation storage
    Ok(Json(serde_json::json!({
        "delegations": []
    })))
}

#[derive(Deserialize)]
pub struct ApprovalRequestQuery {
    pub status: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateApprovalRequest {
    pub task_id: Option<String>,
    pub conversation_id: Option<String>,
    pub run_id: Option<String>,
    pub title: String,
    pub body: Option<String>,
    pub priority: Option<String>,
    pub requested_by: Option<String>,
}

#[derive(Deserialize)]
pub struct ResolveApprovalRequest {
    pub status: String,
    pub decision: Option<serde_json::Value>,
}

fn trim_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub async fn list_group_approval_requests(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(group_id): Path<String>,
    Query(query): Query<ApprovalRequestQuery>,
) -> ApiResult<Json<Vec<crate::db::CortexApprovalRequest>>> {
    let db = db_ref(&state)?;
    let status = trim_optional(query.status);
    Ok(Json(db.list_cortex_approval_requests(
        &user.user_id,
        &group_id,
        status.as_deref(),
        100,
    )))
}

pub async fn create_group_approval_request(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(group_id): Path<String>,
    Json(request): Json<CreateApprovalRequest>,
) -> ApiResult<Json<crate::db::CortexApprovalRequest>> {
    let title = request.title.trim();
    if title.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "approval title is required".into(),
            }),
        ));
    }
    let db = db_ref(&state)?;
    let task_id = trim_optional(request.task_id);
    if let Some(task_id) = task_id.as_deref() {
        if !db.cortex_task_exists(&user.user_id, &group_id, task_id) {
            return Err((
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "task not found".into(),
                }),
            ));
        }
    }
    let conversation_id = trim_optional(request.conversation_id);
    let run_id = trim_optional(request.run_id);
    let body = request.body.unwrap_or_default();
    let priority = request.priority.unwrap_or_else(|| "normal".to_string());
    let requested_by = request.requested_by.unwrap_or_else(|| "cortex".to_string());
    Ok(Json(db.create_cortex_approval_request(
        &user.user_id,
        &group_id,
        task_id.as_deref(),
        conversation_id.as_deref(),
        run_id.as_deref(),
        title,
        &body,
        priority.trim(),
        requested_by.trim(),
    )))
}

pub async fn resolve_group_approval_request(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path((group_id, request_id)): Path<(String, String)>,
    Json(request): Json<ResolveApprovalRequest>,
) -> ApiResult<Json<crate::db::CortexApprovalRequest>> {
    let decision = request.decision.unwrap_or_else(|| serde_json::json!({}));
    let db = db_ref(&state)?;
    let resolved = db
        .resolve_cortex_approval_request(
            &user.user_id,
            &group_id,
            &request_id,
            request.status.trim(),
            &decision,
        )
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "pending approval request not found".into(),
                }),
            )
        })?;
    Ok(Json(resolved))
}

pub async fn update_group_tasks(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(group_id): Path<String>,
    Json(state_json): Json<serde_json::Value>,
) -> ApiResult<Json<serde_json::Value>> {
    let db = db_ref(&state)?;
    let current = db
        .get_group_task_state(&user.user_id, &group_id)
        .and_then(|value| normalize_task_state(&group_id, &value).ok())
        .unwrap_or_else(|| empty_task_state(&group_id));
    let normalized = normalize_task_state(&group_id, &state_json)
        .map_err(|error| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error })))?;
    require_evidence_for_done_transitions(db, &user.user_id, &group_id, &current, &normalized)
        .map_err(|error| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error })))?;
    let reconciliation_events = derive_task_state_reconciliation_events(&current, &normalized);
    let mut events = reconciliation_events
        .into_iter()
        .map(|event| {
            if let Some(task_id) = event.task_id {
                task_state_task_event(task_id, event.event_type, event.payload)
            } else {
                task_state_manager_event(&group_id, event.event_type, event.payload)
            }
        })
        .collect::<Vec<_>>();
    let reconciliation_event_count = events.len();
    events.push(task_state_manager_event(
        &group_id,
        "task_state.reconciled".to_string(),
        serde_json::json!({
            "source": "bulk_update",
            "event_count": reconciliation_event_count,
            "task_count": normalized
                .get("tasks")
                .and_then(|value| value.as_array())
                .map(|tasks| tasks.len())
                .unwrap_or(0),
        }),
    ));
    let saved = db
        .upsert_group_task_state_with_events(&user.user_id, &group_id, &normalized, &events, None)
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error }),
            )
        })?;
    db.record_integration_event(
        &user.user_id,
        "cortex",
        Some(&group_id),
        "task_state_updated",
        "queued",
        &normalized,
    );
    Ok(Json(saved))
}

pub(crate) fn empty_task_state(group_id: &str) -> serde_json::Value {
    serde_json::json!({
        "tasks": [],
        "members": [],
        "activity": [{
            "id": format!("activity-{}", Uuid::new_v4()),
            "groupId": group_id,
            "kind": "note",
            "actor": "Cortex",
            "summary": "Task manager is ready for Slack and Replit coordination.",
            "createdAt": chrono::Utc::now().to_rfc3339()
        }],
        "updatedAt": chrono::Utc::now().to_rfc3339()
    })
}

fn task_status<'a>(state: &'a serde_json::Value, task_id: &str) -> Option<&'a str> {
    state
        .get("tasks")
        .and_then(|value| value.as_array())
        .and_then(|tasks| {
            tasks
                .iter()
                .find(|task| task.get("id").and_then(|value| value.as_str()) == Some(task_id))
        })
        .and_then(|task| task.get("status"))
        .and_then(|value| value.as_str())
}

fn require_evidence_for_done_transitions(
    db: &crate::db::Database,
    user_id: &str,
    group_id: &str,
    current: &serde_json::Value,
    next: &serde_json::Value,
) -> Result<(), String> {
    let Some(tasks) = next.get("tasks").and_then(|value| value.as_array()) else {
        return Err("task state requires a tasks array".to_string());
    };

    for task in tasks {
        let task_id = task
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "task id is required".to_string())?;
        let next_status = task.get("status").and_then(|value| value.as_str());
        if next_status != Some("done") || task_status(current, task_id) == Some("done") {
            continue;
        }
        if !db.cortex_task_has_evidence_backed_completion(user_id, group_id, task_id) {
            return Err(format!(
                "task {task_id} cannot be marked done until Cortex has a successful verified run"
            ));
        }
    }

    Ok(())
}

fn normalize_task_state(
    group_id: &str,
    value: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    const MAX_TASKS: usize = 250;
    const MAX_MEMBERS: usize = 100;
    const MAX_ACTIVITY: usize = 500;

    let object = value
        .as_object()
        .ok_or_else(|| "task state must be an object".to_string())?;
    let tasks = object
        .get("tasks")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "task state requires a tasks array".to_string())?;
    let members = object
        .get("members")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "task state requires a members array".to_string())?;
    let activity = object
        .get("activity")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "task state requires an activity array".to_string())?;

    if tasks.len() > MAX_TASKS {
        return Err(format!("task state exceeds {MAX_TASKS} tasks"));
    }
    if members.len() > MAX_MEMBERS {
        return Err(format!("task state exceeds {MAX_MEMBERS} members"));
    }
    if activity.len() > MAX_ACTIVITY {
        return Err(format!(
            "task state exceeds {MAX_ACTIVITY} activity entries"
        ));
    }

    let normalized_tasks: Result<Vec<_>, _> = tasks
        .iter()
        .enumerate()
        .map(|(index, task)| normalize_task(group_id, task, index))
        .collect();
    let normalized_members: Result<Vec<_>, _> = members
        .iter()
        .enumerate()
        .map(|(index, member)| normalize_member(member, index))
        .collect();
    let normalized_activity: Result<Vec<_>, _> = activity
        .iter()
        .enumerate()
        .map(|(index, entry)| normalize_activity(group_id, entry, index))
        .collect();

    Ok(serde_json::json!({
        "tasks": normalized_tasks?,
        "members": normalized_members?,
        "activity": normalized_activity?,
        "updatedAt": optional_string(object.get("updatedAt"), 64)?.unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
    }))
}

fn apply_task_patch(
    group_id: &str,
    task_id: &str,
    state: &serde_json::Value,
    patch: &serde_json::Value,
) -> Result<(serde_json::Value, String, serde_json::Value), String> {
    const ALLOWED_FIELDS: &[&str] = &[
        "title",
        "description",
        "status",
        "assigneeId",
        "repo",
        "priority",
        "projectChatConversationId",
        "projectChatLaunchedAt",
        "latestRunId",
        "latestRunStatus",
        "latestRunSyncedAt",
        "latestRunStepSummary",
    ];

    let patch_object = patch
        .as_object()
        .ok_or_else(|| "task patch must be an object".to_string())?;
    if patch_object.is_empty() {
        return Err("task patch must include at least one field".to_string());
    }
    for key in patch_object.keys() {
        if !ALLOWED_FIELDS.contains(&key.as_str()) {
            return Err(format!("task patch field {key} is not allowed"));
        }
    }

    let mut next_state = state.clone();
    let tasks = next_state
        .get_mut("tasks")
        .and_then(|value| value.as_array_mut())
        .ok_or_else(|| "task state requires a tasks array".to_string())?;
    let task = tasks
        .iter_mut()
        .find(|task| task.get("id").and_then(|value| value.as_str()) == Some(task_id))
        .ok_or_else(|| "task not found".to_string())?;
    let previous_task = task.clone();
    let task_object = task
        .as_object_mut()
        .ok_or_else(|| "task must be an object".to_string())?;
    for (key, value) in patch_object {
        task_object.insert(key.clone(), value.clone());
    }
    let updated_at = chrono::Utc::now().to_rfc3339();
    task_object.insert(
        "updatedAt".to_string(),
        serde_json::Value::String(updated_at.clone()),
    );

    let event_type = if patch_object.contains_key("status") {
        "task.status_changed"
    } else if patch_object.contains_key("assigneeId") {
        "task.assigned"
    } else if patch_object.contains_key("projectChatConversationId")
        || patch_object.contains_key("projectChatLaunchedAt")
    {
        "task.linked"
    } else {
        "task.updated"
    }
    .to_string();

    let task_title = task_object
        .get("title")
        .and_then(|value| value.as_str())
        .unwrap_or("Task");
    let activity_kind = if event_type == "task.status_changed" {
        "status"
    } else if event_type == "task.assigned" {
        "assigned"
    } else if event_type == "task.linked" {
        "linked"
    } else {
        "note"
    };
    let activity_summary = if event_type == "task.status_changed" {
        format!(
            "{task_title} moved to {}.",
            task_object
                .get("status")
                .and_then(|value| value.as_str())
                .unwrap_or("updated")
        )
    } else if event_type == "task.assigned" {
        format!("{task_title} assignment updated.")
    } else if event_type == "task.linked" {
        format!("{task_title} linked to Project Chat.")
    } else {
        format!("{task_title} updated.")
    };

    let activity = next_state
        .get_mut("activity")
        .and_then(|value| value.as_array_mut())
        .ok_or_else(|| "task state requires an activity array".to_string())?;
    activity.insert(
        0,
        serde_json::json!({
            "id": Uuid::new_v4().to_string(),
            "groupId": group_id,
            "taskId": task_id,
            "kind": activity_kind,
            "actor": "Cortex",
            "summary": activity_summary,
            "createdAt": updated_at,
        }),
    );
    activity.truncate(500);
    next_state["updatedAt"] = serde_json::Value::String(chrono::Utc::now().to_rfc3339());

    let normalized = normalize_task_state(group_id, &next_state)?;
    let next_task = normalized
        .get("tasks")
        .and_then(|value| value.as_array())
        .and_then(|tasks| {
            tasks
                .iter()
                .find(|task| task.get("id").and_then(|value| value.as_str()) == Some(task_id))
        })
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));

    Ok((
        normalized,
        event_type,
        serde_json::json!({
            "patch": patch,
            "previous": previous_task,
            "next": next_task,
        }),
    ))
}

fn apply_task_create(
    group_id: &str,
    state: &serde_json::Value,
    task: &serde_json::Value,
) -> Result<(serde_json::Value, String, serde_json::Value), String> {
    let normalized_task = normalize_task(group_id, task, 0)?;
    let task_id = normalized_task
        .get("id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "task id is required".to_string())?
        .to_string();

    let mut next_state = state.clone();
    let tasks = next_state
        .get_mut("tasks")
        .and_then(|value| value.as_array_mut())
        .ok_or_else(|| "task state requires a tasks array".to_string())?;
    if tasks
        .iter()
        .any(|task| task.get("id").and_then(|value| value.as_str()) == Some(task_id.as_str()))
    {
        return Err("task already exists".to_string());
    }
    tasks.insert(0, normalized_task.clone());

    let created_at = chrono::Utc::now().to_rfc3339();
    let task_title = normalized_task
        .get("title")
        .and_then(|value| value.as_str())
        .unwrap_or("Task");
    let activity_kind = if normalized_task
        .get("assigneeId")
        .and_then(|value| value.as_str())
        .is_some()
    {
        "assigned"
    } else {
        "created"
    };
    let activity_summary = if activity_kind == "assigned" {
        format!("{task_title} created and assigned.")
    } else {
        format!("{task_title} created.")
    };

    let activity = next_state
        .get_mut("activity")
        .and_then(|value| value.as_array_mut())
        .ok_or_else(|| "task state requires an activity array".to_string())?;
    activity.insert(
        0,
        serde_json::json!({
            "id": Uuid::new_v4().to_string(),
            "groupId": group_id,
            "taskId": task_id,
            "kind": activity_kind,
            "actor": "Cortex",
            "summary": activity_summary,
            "createdAt": created_at,
        }),
    );
    activity.truncate(500);
    next_state["updatedAt"] = serde_json::Value::String(created_at);

    let normalized = normalize_task_state(group_id, &next_state)?;
    let created_task = normalized
        .get("tasks")
        .and_then(|value| value.as_array())
        .and_then(|tasks| {
            tasks.iter().find(|task| {
                task.get("id").and_then(|value| value.as_str()) == Some(task_id.as_str())
            })
        })
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));

    Ok((
        normalized,
        task_id,
        serde_json::json!({
            "task": created_task,
        }),
    ))
}

struct TaskActionEvent {
    task_id: Option<String>,
    event_type: String,
    payload: serde_json::Value,
}

fn task_state_task_event(
    task_id: String,
    event_type: String,
    payload: serde_json::Value,
) -> CortexTaskStateEvent {
    CortexTaskStateEvent {
        entity_type: "task".to_string(),
        entity_id: task_id.clone(),
        task_id: Some(task_id),
        event_type,
        payload,
    }
}

fn task_state_manager_event(
    group_id: &str,
    event_type: String,
    payload: serde_json::Value,
) -> CortexTaskStateEvent {
    CortexTaskStateEvent {
        entity_type: "task_manager".to_string(),
        entity_id: group_id.to_string(),
        task_id: None,
        event_type,
        payload,
    }
}

fn task_id(value: &serde_json::Value) -> Option<&str> {
    value.get("id").and_then(|value| value.as_str())
}

fn derive_task_state_reconciliation_events(
    current: &serde_json::Value,
    next: &serde_json::Value,
) -> Vec<TaskActionEvent> {
    let current_tasks = current
        .get("tasks")
        .and_then(|value| value.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let next_tasks = next
        .get("tasks")
        .and_then(|value| value.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    let mut current_by_id = std::collections::BTreeMap::new();
    for task in current_tasks {
        if let Some(id) = task_id(task) {
            current_by_id.insert(id.to_string(), task);
        }
    }

    let mut next_by_id = std::collections::BTreeMap::new();
    for task in next_tasks {
        if let Some(id) = task_id(task) {
            next_by_id.insert(id.to_string(), task);
        }
    }

    let mut events = Vec::new();
    for (id, next_task) in &next_by_id {
        let Some(current_task) = current_by_id.get(id) else {
            events.push(TaskActionEvent {
                task_id: Some(id.clone()),
                event_type: "task.created".to_string(),
                payload: serde_json::json!({
                    "source": "bulk_update",
                    "task": next_task,
                }),
            });
            continue;
        };

        if *current_task == *next_task {
            continue;
        }

        let event_type = if current_task.get("status") != next_task.get("status") {
            "task.status_changed"
        } else if current_task.get("assigneeId") != next_task.get("assigneeId") {
            "task.assigned"
        } else if current_task.get("projectChatConversationId")
            != next_task.get("projectChatConversationId")
            || current_task.get("projectChatLaunchedAt") != next_task.get("projectChatLaunchedAt")
        {
            "task.linked"
        } else {
            "task.updated"
        };

        events.push(TaskActionEvent {
            task_id: Some(id.clone()),
            event_type: event_type.to_string(),
            payload: serde_json::json!({
                "source": "bulk_update",
                "previous": current_task,
                "next": next_task,
            }),
        });
    }

    for (id, current_task) in &current_by_id {
        if next_by_id.contains_key(id) {
            continue;
        }
        events.push(TaskActionEvent {
            task_id: Some(id.clone()),
            event_type: "task.removed".to_string(),
            payload: serde_json::json!({
                "source": "bulk_update",
                "previous": current_task,
            }),
        });
    }

    events
}

fn apply_task_actions(
    group_id: &str,
    state: &serde_json::Value,
    request: &serde_json::Value,
) -> Result<(serde_json::Value, Vec<TaskActionEvent>), String> {
    let object = request
        .as_object()
        .ok_or_else(|| "task actions request must be an object".to_string())?;
    let actions = object
        .get("actions")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "task actions request requires an actions array".to_string())?;
    if actions.is_empty() {
        return Err("task actions request requires at least one action".to_string());
    }
    if actions.len() > 50 {
        return Err("task actions request exceeds 50 actions".to_string());
    }
    let actor = optional_string(object.get("actor"), 256)?.unwrap_or_else(|| "You".to_string());
    let timestamp = chrono::Utc::now().to_rfc3339();
    let mut next_state = state.clone();
    let mut events = Vec::new();

    for (index, action) in actions.iter().enumerate() {
        let action_object = action
            .as_object()
            .ok_or_else(|| format!("action {index} must be an object"))?;
        let action_type = required_string(
            action_object.get("type"),
            32,
            &format!("action {index} type"),
        )?;
        if !matches!(
            action_type.as_str(),
            "task"
                | "status"
                | "handoff"
                | "note"
                | "pause"
                | "resume"
                | "retry"
                | "cancel"
                | "prioritize"
        ) {
            return Err(format!("action {index} has invalid type"));
        }
        let title = required_string(
            action_object.get("title"),
            256,
            &format!("action {index} title"),
        )?;
        let summary = required_string(
            action_object.get("summary"),
            1024,
            &format!("action {index} summary"),
        )?;

        if action_type == "status" {
            let target_task_id = required_string(
                action_object.get("targetTaskId"),
                256,
                &format!("action {index} targetTaskId"),
            )?;
            let status = required_string(
                action_object.get("status"),
                32,
                &format!("action {index} status"),
            )?;
            if !matches!(
                status.as_str(),
                "created" | "assigned" | "in-progress" | "done" | "paused" | "cancelled" | "queued"
            ) {
                return Err(format!("action {index} has invalid status"));
            }
            let tasks = next_state
                .get_mut("tasks")
                .and_then(|value| value.as_array_mut())
                .ok_or_else(|| "task state requires a tasks array".to_string())?;
            let task = tasks
                .iter_mut()
                .find(|task| {
                    task.get("id").and_then(|value| value.as_str()) == Some(target_task_id.as_str())
                })
                .ok_or_else(|| format!("action {index} target task not found"))?;
            let previous_task = task.clone();
            let task_object = task
                .as_object_mut()
                .ok_or_else(|| format!("action {index} target task must be an object"))?;
            task_object.insert(
                "status".to_string(),
                serde_json::Value::String(status.clone()),
            );
            task_object.insert(
                "updatedAt".to_string(),
                serde_json::Value::String(timestamp.clone()),
            );
            let next_task = task.clone();
            push_task_activity(
                group_id,
                &mut next_state,
                Some(&target_task_id),
                "status",
                &actor,
                &summary,
                &timestamp,
            )?;
            events.push(TaskActionEvent {
                task_id: Some(target_task_id),
                event_type: "task.status_changed".to_string(),
                payload: serde_json::json!({
                    "action": action,
                    "previous": previous_task,
                    "next": next_task,
                }),
            });
            continue;
        }

        if action_type == "note" {
            push_task_activity(
                group_id,
                &mut next_state,
                None,
                "note",
                &actor,
                &summary,
                &timestamp,
            )?;
            events.push(TaskActionEvent {
                task_id: None,
                event_type: "task.note_added".to_string(),
                payload: serde_json::json!({
                    "action": action,
                    "summary": summary,
                }),
            });
            continue;
        }

        if matches!(
            action_type.as_str(),
            "pause" | "resume" | "retry" | "cancel" | "prioritize"
        ) {
            let target_task_id = required_string(
                action_object.get("targetTaskId"),
                256,
                &format!("action {index} targetTaskId"),
            )?;
            let tasks = next_state
                .get_mut("tasks")
                .and_then(|value| value.as_array_mut())
                .ok_or_else(|| "task state requires a tasks array".to_string())?;
            let task = tasks
                .iter_mut()
                .find(|task| {
                    task.get("id").and_then(|value| value.as_str()) == Some(target_task_id.as_str())
                })
                .ok_or_else(|| format!("action {index} target task not found"))?;
            let previous_task = task.clone();
            let task_object = task
                .as_object_mut()
                .ok_or_else(|| format!("action {index} target task must be an object"))?;

            let event_type = match action_type.as_str() {
                "pause" => {
                    task_object.insert("status".to_string(), serde_json::json!("paused"));
                    "task.paused"
                }
                "resume" => {
                    task_object.insert("status".to_string(), serde_json::json!("in-progress"));
                    "task.resumed"
                }
                "retry" => {
                    task_object.insert("status".to_string(), serde_json::json!("in-progress"));
                    task_object.remove("latestRunStatus");
                    task_object.remove("latestRunSyncedAt");
                    task_object.remove("latestRunStepSummary");
                    "task.retried"
                }
                "cancel" => {
                    task_object.insert("status".to_string(), serde_json::json!("cancelled"));
                    "task.cancelled"
                }
                "prioritize" => {
                    let priority = optional_string(action_object.get("priority"), 32)?
                        .unwrap_or_else(|| "high".to_string());
                    if !matches!(priority.as_str(), "normal" | "high" | "urgent") {
                        return Err(format!("action {index} has invalid priority"));
                    }
                    task_object.insert("priority".to_string(), serde_json::json!(priority));
                    "task.prioritized"
                }
                _ => unreachable!(),
            };
            task_object.insert(
                "updatedAt".to_string(),
                serde_json::Value::String(timestamp.clone()),
            );
            let next_task = task.clone();
            push_task_activity(
                group_id,
                &mut next_state,
                Some(&target_task_id),
                "status",
                &actor,
                &summary,
                &timestamp,
            )?;
            events.push(TaskActionEvent {
                task_id: Some(target_task_id),
                event_type: event_type.to_string(),
                payload: serde_json::json!({
                    "action": action,
                    "previous": previous_task,
                    "next": next_task,
                }),
            });
            continue;
        }

        let assignee_id = optional_string(action_object.get("assigneeId"), 256)?;
        let status = optional_string(action_object.get("status"), 32)?.unwrap_or_else(|| {
            if assignee_id.is_some() {
                "assigned".to_string()
            } else {
                "created".to_string()
            }
        });
        if !matches!(
            status.as_str(),
            "created" | "assigned" | "in-progress" | "done" | "paused" | "cancelled" | "queued"
        ) {
            return Err(format!("action {index} has invalid status"));
        }
        let task_id = Uuid::new_v4().to_string();
        let task = serde_json::json!({
            "id": task_id,
            "groupId": group_id,
            "title": title,
            "description": null,
            "status": status,
            "assigneeId": assignee_id,
            "repo": null,
            "priority": "normal",
            "createdAt": timestamp,
            "updatedAt": timestamp,
            "createdBy": actor,
        });
        let tasks = next_state
            .get_mut("tasks")
            .and_then(|value| value.as_array_mut())
            .ok_or_else(|| "task state requires a tasks array".to_string())?;
        tasks.insert(0, task.clone());
        if assignee_id.is_some() && status == "in-progress" {
            mark_member_working(&mut next_state, assignee_id.as_deref(), &task_id)?;
        }
        let activity_kind = if action_type == "handoff" {
            "handoff"
        } else if assignee_id.is_some() {
            "assigned"
        } else {
            "created"
        };
        push_task_activity(
            group_id,
            &mut next_state,
            Some(&task_id),
            activity_kind,
            &actor,
            &summary,
            &timestamp,
        )?;
        let event_type = if action_type == "handoff" {
            "task.handoff"
        } else {
            "task.created"
        };
        events.push(TaskActionEvent {
            task_id: Some(task_id),
            event_type: event_type.to_string(),
            payload: serde_json::json!({
                "action": action,
                "task": task,
            }),
        });
    }

    next_state["updatedAt"] = serde_json::Value::String(timestamp);
    Ok((normalize_task_state(group_id, &next_state)?, events))
}

fn push_task_activity(
    group_id: &str,
    state: &mut serde_json::Value,
    task_id: Option<&str>,
    kind: &str,
    actor: &str,
    summary: &str,
    created_at: &str,
) -> Result<(), String> {
    let activity = state
        .get_mut("activity")
        .and_then(|value| value.as_array_mut())
        .ok_or_else(|| "task state requires an activity array".to_string())?;
    activity.insert(
        0,
        serde_json::json!({
            "id": Uuid::new_v4().to_string(),
            "groupId": group_id,
            "taskId": task_id,
            "kind": kind,
            "actor": actor,
            "summary": summary,
            "createdAt": created_at,
        }),
    );
    activity.truncate(500);
    Ok(())
}

fn mark_member_working(
    state: &mut serde_json::Value,
    assignee_id: Option<&str>,
    task_id: &str,
) -> Result<(), String> {
    let Some(assignee_id) = assignee_id else {
        return Ok(());
    };
    let members = state
        .get_mut("members")
        .and_then(|value| value.as_array_mut())
        .ok_or_else(|| "task state requires a members array".to_string())?;
    for member in members {
        let member_object = member
            .as_object_mut()
            .ok_or_else(|| "member must be an object".to_string())?;
        let member_id = member_object.get("id").and_then(|value| value.as_str());
        if member_id == Some(assignee_id) {
            member_object.insert(
                "status".to_string(),
                serde_json::Value::String("working".to_string()),
            );
            member_object.insert(
                "currentTaskId".to_string(),
                serde_json::Value::String(task_id.to_string()),
            );
        }
    }
    Ok(())
}

fn normalize_task(
    group_id: &str,
    value: &serde_json::Value,
    index: usize,
) -> Result<serde_json::Value, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("task {index} must be an object"))?;
    let task_group_id =
        required_string(object.get("groupId"), 256, &format!("task {index} groupId"))?;
    if task_group_id != group_id {
        return Err(format!("task {index} groupId does not match route group"));
    }
    let status = required_string(object.get("status"), 32, &format!("task {index} status"))?;
    if !matches!(
        status.as_str(),
        "created" | "assigned" | "in-progress" | "done"
    ) {
        return Err(format!("task {index} has invalid status"));
    }
    let priority = required_string(
        object.get("priority"),
        32,
        &format!("task {index} priority"),
    )?;
    if !matches!(priority.as_str(), "normal" | "high" | "urgent") {
        return Err(format!("task {index} has invalid priority"));
    }

    Ok(serde_json::json!({
        "id": required_string(object.get("id"), 256, &format!("task {index} id"))?,
        "groupId": task_group_id,
        "title": required_string(object.get("title"), 256, &format!("task {index} title"))?,
        "description": optional_string(object.get("description"), 4096)?,
        "status": status,
        "assigneeId": optional_string(object.get("assigneeId"), 256)?,
        "repo": optional_string(object.get("repo"), 512)?,
        "priority": priority,
        "createdAt": required_string(object.get("createdAt"), 64, &format!("task {index} createdAt"))?,
        "updatedAt": required_string(object.get("updatedAt"), 64, &format!("task {index} updatedAt"))?,
        "createdBy": required_string(object.get("createdBy"), 256, &format!("task {index} createdBy"))?,
        "sourceMessageId": optional_string(object.get("sourceMessageId"), 256)?,
        "projectChatConversationId": optional_string(object.get("projectChatConversationId"), 256)?,
        "projectChatLaunchedAt": optional_string(object.get("projectChatLaunchedAt"), 64)?,
        "latestRunId": optional_string(object.get("latestRunId"), 256)?,
        "latestRunStatus": optional_string(object.get("latestRunStatus"), 64)?,
        "latestRunSyncedAt": optional_string(object.get("latestRunSyncedAt"), 64)?,
        "latestRunStepSummary": normalize_step_summary(object.get("latestRunStepSummary"), index)?,
    }))
}

fn normalize_member(value: &serde_json::Value, index: usize) -> Result<serde_json::Value, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("member {index} must be an object"))?;
    let status = required_string(object.get("status"), 32, &format!("member {index} status"))?;
    if !matches!(status.as_str(), "online" | "working" | "away") {
        return Err(format!("member {index} has invalid status"));
    }

    Ok(serde_json::json!({
        "id": required_string(object.get("id"), 256, &format!("member {index} id"))?,
        "name": required_string(object.get("name"), 256, &format!("member {index} name"))?,
        "initials": required_string(object.get("initials"), 16, &format!("member {index} initials"))?,
        "status": status,
        "currentTaskId": optional_string(object.get("currentTaskId"), 256)?,
        "color": required_string(object.get("color"), 32, &format!("member {index} color"))?,
    }))
}

fn normalize_activity(
    group_id: &str,
    value: &serde_json::Value,
    index: usize,
) -> Result<serde_json::Value, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("activity {index} must be an object"))?;
    let activity_group_id = required_string(
        object.get("groupId"),
        256,
        &format!("activity {index} groupId"),
    )?;
    if activity_group_id != group_id {
        return Err(format!(
            "activity {index} groupId does not match route group"
        ));
    }
    let kind = required_string(object.get("kind"), 32, &format!("activity {index} kind"))?;
    if !matches!(
        kind.as_str(),
        "created" | "assigned" | "status" | "handoff" | "linked" | "note"
    ) {
        return Err(format!("activity {index} has invalid kind"));
    }

    Ok(serde_json::json!({
        "id": required_string(object.get("id"), 256, &format!("activity {index} id"))?,
        "groupId": activity_group_id,
        "taskId": optional_string(object.get("taskId"), 256)?,
        "kind": kind,
        "actor": required_string(object.get("actor"), 256, &format!("activity {index} actor"))?,
        "summary": required_string(object.get("summary"), 1024, &format!("activity {index} summary"))?,
        "createdAt": required_string(object.get("createdAt"), 64, &format!("activity {index} createdAt"))?,
    }))
}

fn normalize_step_summary(
    value: Option<&serde_json::Value>,
    task_index: usize,
) -> Result<serde_json::Value, String> {
    let Some(value) = value else {
        return Ok(serde_json::Value::Null);
    };
    if value.is_null() {
        return Ok(serde_json::Value::Null);
    }
    let object = value
        .as_object()
        .ok_or_else(|| format!("task {task_index} latestRunStepSummary must be an object"))?;
    Ok(serde_json::json!({
        "total": required_count(object.get("total"), &format!("task {task_index} total"))?,
        "active": required_count(object.get("active"), &format!("task {task_index} active"))?,
        "done": required_count(object.get("done"), &format!("task {task_index} done"))?,
        "failed": required_count(object.get("failed"), &format!("task {task_index} failed"))?,
    }))
}

fn required_count(value: Option<&serde_json::Value>, label: &str) -> Result<u64, String> {
    let count = value
        .and_then(|value| value.as_u64())
        .ok_or_else(|| format!("{label} must be a non-negative integer"))?;
    if count > 10_000 {
        return Err(format!("{label} is too large"));
    }
    Ok(count)
}

fn required_string(
    value: Option<&serde_json::Value>,
    max_len: usize,
    label: &str,
) -> Result<String, String> {
    optional_string(value, max_len)?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{label} is required"))
}

fn optional_string(
    value: Option<&serde_json::Value>,
    max_len: usize,
) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let string = value
        .as_str()
        .ok_or_else(|| "expected string or null".to_string())?;
    if string.len() > max_len {
        return Err(format!("string exceeds {max_len} characters"));
    }
    Ok(Some(string.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_task_state() -> serde_json::Value {
        serde_json::json!({
            "tasks": [{
                "id": "task-1",
                "groupId": "group-1",
                "title": "Ship the operation queue",
                "description": "Make the live map useful",
                "status": "in-progress",
                "assigneeId": "user-1",
                "repo": "hey-vera/heyvera",
                "priority": "high",
                "createdAt": "2026-05-25T00:00:00Z",
                "updatedAt": "2026-05-25T00:01:00Z",
                "createdBy": "You",
                "sourceMessageId": "message-1",
                "projectChatConversationId": "conversation-1",
                "projectChatLaunchedAt": "2026-05-25T00:02:00Z",
                "latestRunId": "run-1",
                "latestRunStatus": "running",
                "latestRunSyncedAt": "2026-05-25T00:03:00Z",
                "latestRunStepSummary": {
                    "total": 4,
                    "active": 1,
                    "done": 2,
                    "failed": 1
                },
                "ignored": "field"
            }],
            "members": [{
                "id": "user-1",
                "name": "You",
                "initials": "Y",
                "status": "working",
                "currentTaskId": "task-1",
                "color": "#9cc7b8",
                "ignored": "field"
            }],
            "activity": [{
                "id": "activity-1",
                "groupId": "group-1",
                "taskId": "task-1",
                "kind": "linked",
                "actor": "You",
                "summary": "Opened in Project Chat.",
                "createdAt": "2026-05-25T00:02:00Z",
                "ignored": "field"
            }],
            "updatedAt": "2026-05-25T00:03:00Z",
            "ignored": "field"
        })
    }

    #[test]
    fn normalize_task_state_accepts_known_shape_and_strips_unknown_fields() {
        let normalized = normalize_task_state("group-1", &valid_task_state()).unwrap();

        assert_eq!(normalized["tasks"][0]["id"], "task-1");
        assert_eq!(normalized["tasks"][0]["latestRunStepSummary"]["failed"], 1);
        assert!(normalized["tasks"][0].get("ignored").is_none());
        assert!(normalized.get("ignored").is_none());
    }

    #[test]
    fn normalize_task_state_rejects_cross_group_task() {
        let mut state = valid_task_state();
        state["tasks"][0]["groupId"] = serde_json::json!("other-group");

        let error = normalize_task_state("group-1", &state).unwrap_err();
        assert!(error.contains("groupId does not match"));
    }

    #[test]
    fn normalize_task_state_rejects_invalid_status() {
        let mut state = valid_task_state();
        state["tasks"][0]["status"] = serde_json::json!("paused");

        let error = normalize_task_state("group-1", &state).unwrap_err();
        assert!(error.contains("invalid status"));
    }

    #[test]
    fn normalize_task_state_rejects_oversized_task_sets() {
        let mut state = valid_task_state();
        state["tasks"] = serde_json::Value::Array(vec![state["tasks"][0].clone(); 251]);

        let error = normalize_task_state("group-1", &state).unwrap_err();
        assert!(error.contains("exceeds 250 tasks"));
    }

    #[test]
    fn apply_task_patch_updates_one_task_and_adds_activity() {
        let state = normalize_task_state("group-1", &valid_task_state()).unwrap();
        let (next, event_type, payload) = apply_task_patch(
            "group-1",
            "task-1",
            &state,
            &serde_json::json!({
                "status": "done",
                "priority": "urgent"
            }),
        )
        .unwrap();

        assert_eq!(event_type, "task.status_changed");
        assert_eq!(next["tasks"][0]["status"], "done");
        assert_eq!(next["tasks"][0]["priority"], "urgent");
        assert_eq!(next["activity"][0]["kind"], "status");
        assert_eq!(next["activity"][0]["taskId"], "task-1");
        assert_eq!(payload["previous"]["status"], "in-progress");
        assert_eq!(payload["next"]["status"], "done");
    }

    #[test]
    fn done_transition_requires_evidence_backed_completion() {
        let dir = tempfile::tempdir().unwrap().keep();
        let db = crate::db::Database::open(&dir.join("cortex.sqlite"));
        let current = normalize_task_state("group-1", &valid_task_state()).unwrap();
        db.upsert_group_task_state("user-1", "group-1", &current);
        let (next, _, _) = apply_task_patch(
            "group-1",
            "task-1",
            &current,
            &serde_json::json!({ "status": "done" }),
        )
        .unwrap();

        let error =
            require_evidence_for_done_transitions(&db, "user-1", "group-1", &current, &next)
                .unwrap_err();

        assert!(error.contains("successful verified run"));
    }

    #[test]
    fn done_transition_accepts_verified_completion() {
        let dir = tempfile::tempdir().unwrap().keep();
        let db = crate::db::Database::open(&dir.join("cortex.sqlite"));
        let current = normalize_task_state("group-1", &valid_task_state()).unwrap();
        db.upsert_group_task_state("user-1", "group-1", &current);
        let run_id = db.create_run_with_metadata(
            "user-1",
            "Ship verified task",
            "auto",
            &[],
            Some("task-1"),
            Some("group-1"),
            None,
        );
        let step_id = db.create_step(&run_id, "implement", "standard", "medium", "Ship it");
        db.register_worker("worker-1", "user-1");
        let lease_gen = db
            .lease_step(
                &step_id,
                "worker-1",
                chrono::Utc::now().timestamp_millis() + 60_000,
            )
            .unwrap();
        assert!(db.start_step(&step_id, lease_gen));
        db.record_verifier_report(
            &step_id,
            &run_id,
            lease_gen,
            Some("worker-1"),
            "test",
            "verified",
            "pass",
            "{}",
        );
        assert!(db.deliver_step(&step_id, "attempt-1", lease_gen, None, None, None, None));
        // Evidence-backed completion means *verified*, so the delivery has to
        // survive our own runner before the task may be marked done.
        assert!(db.begin_verifying_step(&step_id, "attempt-1", lease_gen, None));
        assert!(db.record_verification_outcome(&step_id, "attempt-1", lease_gen, "verified", None));
        assert!(db.update_run_status(&run_id, "succeeded", None));
        let (next, _, _) = apply_task_patch(
            "group-1",
            "task-1",
            &current,
            &serde_json::json!({ "status": "done" }),
        )
        .unwrap();

        require_evidence_for_done_transitions(&db, "user-1", "group-1", &current, &next).unwrap();
    }

    #[test]
    fn apply_task_patch_rejects_unknown_fields() {
        let state = normalize_task_state("group-1", &valid_task_state()).unwrap();
        let error = apply_task_patch(
            "group-1",
            "task-1",
            &state,
            &serde_json::json!({ "groupId": "other-group" }),
        )
        .unwrap_err();

        assert!(error.contains("not allowed"));
    }

    #[test]
    fn apply_task_create_prepends_task_and_adds_activity() {
        let state = normalize_task_state("group-1", &valid_task_state()).unwrap();
        let (next, task_id, payload) = apply_task_create(
            "group-1",
            &state,
            &serde_json::json!({
                "id": "task-2",
                "groupId": "group-1",
                "title": "Review evidence gate",
                "description": null,
                "status": "created",
                "assigneeId": null,
                "repo": "hey-vera/heyvera",
                "priority": "normal",
                "createdAt": "2026-05-25T00:04:00Z",
                "updatedAt": "2026-05-25T00:04:00Z",
                "createdBy": "You"
            }),
        )
        .unwrap();

        assert_eq!(task_id, "task-2");
        assert_eq!(next["tasks"][0]["id"], "task-2");
        assert_eq!(next["tasks"][1]["id"], "task-1");
        assert_eq!(next["activity"][0]["kind"], "created");
        assert_eq!(next["activity"][0]["taskId"], "task-2");
        assert_eq!(payload["task"]["id"], "task-2");
    }

    #[test]
    fn apply_task_create_rejects_duplicate_task_id() {
        let state = normalize_task_state("group-1", &valid_task_state()).unwrap();
        let error = apply_task_create("group-1", &state, &state["tasks"][0]).unwrap_err();

        assert!(error.contains("already exists"));
    }

    #[test]
    fn derive_task_state_reconciliation_events_covers_bulk_create_update_and_remove() {
        let current = normalize_task_state("group-1", &valid_task_state()).unwrap();
        let mut next = current.clone();
        next["tasks"] = serde_json::json!([
            {
                "id": "task-2",
                "groupId": "group-1",
                "title": "New task",
                "description": null,
                "status": "created",
                "assigneeId": null,
                "repo": null,
                "priority": "normal",
                "createdAt": "2026-05-25T00:04:00Z",
                "updatedAt": "2026-05-25T00:04:00Z",
                "createdBy": "You"
            },
            {
                "id": "task-1",
                "groupId": "group-1",
                "title": "Ship the operation queue",
                "description": "Make the live map useful",
                "status": "assigned",
                "assigneeId": "user-1",
                "repo": "hey-vera/heyvera",
                "priority": "high",
                "createdAt": "2026-05-25T00:00:00Z",
                "updatedAt": "2026-05-25T00:04:00Z",
                "createdBy": "You",
                "sourceMessageId": "message-1",
                "projectChatConversationId": "conversation-1",
                "projectChatLaunchedAt": "2026-05-25T00:02:00Z",
                "latestRunId": "run-1",
                "latestRunStatus": "running",
                "latestRunSyncedAt": "2026-05-25T00:03:00Z",
                "latestRunStepSummary": {
                    "total": 4,
                    "active": 1,
                    "done": 2,
                    "failed": 1
                }
            }
        ]);
        let next = normalize_task_state("group-1", &next).unwrap();

        let events = derive_task_state_reconciliation_events(&current, &next);

        assert_eq!(events.len(), 2);
        assert!(events.iter().any(|event| {
            event.task_id.as_deref() == Some("task-1")
                && event.event_type == "task.status_changed"
                && event.payload["previous"]["status"] == "in-progress"
                && event.payload["next"]["status"] == "assigned"
        }));
        assert!(events.iter().any(|event| {
            event.task_id.as_deref() == Some("task-2")
                && event.event_type == "task.created"
                && event.payload["task"]["title"] == "New task"
        }));
    }

    #[test]
    fn derive_task_state_reconciliation_events_records_bulk_removal() {
        let current = normalize_task_state("group-1", &valid_task_state()).unwrap();
        let mut next = current.clone();
        next["tasks"] = serde_json::json!([]);
        let next = normalize_task_state("group-1", &next).unwrap();

        let events = derive_task_state_reconciliation_events(&current, &next);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].task_id.as_deref(), Some("task-1"));
        assert_eq!(events[0].event_type, "task.removed");
        assert_eq!(
            events[0].payload["previous"]["title"],
            "Ship the operation queue"
        );
    }

    #[test]
    fn apply_task_actions_creates_tasks_and_records_events() {
        let state = normalize_task_state("group-1", &valid_task_state()).unwrap();
        let (next, events) = apply_task_actions(
            "group-1",
            &state,
            &serde_json::json!({
                "actor": "You",
                "actions": [{
                    "type": "task",
                    "title": "Add command mutation endpoint",
                    "assigneeId": "user-1",
                    "status": "assigned",
                    "summary": "Assigned command mutation endpoint to You."
                }]
            }),
        )
        .unwrap();

        assert_eq!(next["tasks"][0]["title"], "Add command mutation endpoint");
        assert_eq!(next["tasks"][0]["assigneeId"], "user-1");
        assert_eq!(next["activity"][0]["kind"], "assigned");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "task.created");
        assert_eq!(events[0].payload["action"]["type"], "task");
    }

    #[test]
    fn apply_task_actions_updates_status_without_replacing_board() {
        let state = normalize_task_state("group-1", &valid_task_state()).unwrap();
        let (next, events) = apply_task_actions(
            "group-1",
            &state,
            &serde_json::json!({
                "actor": "You",
                "actions": [{
                    "type": "status",
                    "title": "Ship the operation queue",
                    "targetTaskId": "task-1",
                    "status": "done",
                    "summary": "Marked Ship the operation queue done."
                }]
            }),
        )
        .unwrap();

        assert_eq!(next["tasks"][0]["id"], "task-1");
        assert_eq!(next["tasks"][0]["status"], "done");
        assert_eq!(next["activity"][0]["kind"], "status");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "task.status_changed");
        assert_eq!(events[0].payload["previous"]["status"], "in-progress");
        assert_eq!(events[0].payload["next"]["status"], "done");
    }

    #[test]
    fn apply_task_actions_records_manager_note_event() {
        let state = normalize_task_state("group-1", &valid_task_state()).unwrap();
        let (next, events) = apply_task_actions(
            "group-1",
            &state,
            &serde_json::json!({
                "actor": "You",
                "actions": [{
                    "type": "note",
                    "title": "Hold the rollout",
                    "summary": "Coordination note: Hold the rollout"
                }]
            }),
        )
        .unwrap();

        assert_eq!(next["tasks"].as_array().unwrap().len(), 1);
        assert_eq!(next["activity"][0]["kind"], "note");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].task_id, None);
        assert_eq!(events[0].event_type, "task.note_added");
    }
}
