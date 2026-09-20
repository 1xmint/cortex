use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::Json;
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

use crate::clerk::ClerkUser;
use crate::cortex_groups::empty_task_state;
use crate::routes::{db_ref, ApiResult, ErrorResponse};
use crate::state::AppState;

#[derive(Serialize)]
pub struct IntegrationStatusResponse {
    pub connections: Vec<crate::db::IntegrationConnection>,
    pub mappings: Vec<crate::db::IntegrationMapping>,
    pub slack_configured: bool,
    pub replit_configured: bool,
}

pub async fn integration_status(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> ApiResult<Json<IntegrationStatusResponse>> {
    let db = db_ref(&state)?;
    Ok(Json(IntegrationStatusResponse {
        connections: db.list_integration_connections(&user.user_id),
        mappings: db.list_integration_mappings(&user.user_id),
        slack_configured: std::env::var("SLACK_CLIENT_ID").is_ok()
            && std::env::var("SLACK_CLIENT_SECRET").is_ok(),
        replit_configured: std::env::var("REPLIT_API_TOKEN").is_ok(),
    }))
}

#[derive(Deserialize)]
pub struct OAuthStartRequest {
    pub redirect_after: Option<String>,
}

#[derive(Serialize)]
pub struct OAuthStartResponse {
    pub auth_url: String,
    pub state: String,
}

pub async fn slack_oauth_start(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Json(req): Json<OAuthStartRequest>,
) -> ApiResult<Json<OAuthStartResponse>> {
    let client_id = std::env::var("SLACK_CLIENT_ID").map_err(|_| {
        (
            StatusCode::PRECONDITION_FAILED,
            Json(ErrorResponse {
                error: "Slack OAuth is not configured".into(),
            }),
        )
    })?;
    let redirect_uri = std::env::var("SLACK_REDIRECT_URI").unwrap_or_else(|_| {
        "http://localhost:3402/api/integrations/slack/oauth/callback".to_string()
    });
    let scopes = "channels:read,groups:read,users:read,chat:write,commands,app_mentions:read";
    let oauth_state = Uuid::new_v4().to_string();
    db_ref(&state)?.store_oauth_state(
        &user.user_id,
        "slack",
        &oauth_state,
        req.redirect_after.as_deref(),
    );
    let auth_url = format!(
        "https://slack.com/oauth/v2/authorize?client_id={}&scope={}&state={}&redirect_uri={}",
        url_encode(&client_id),
        url_encode(scopes),
        url_encode(&oauth_state),
        url_encode(&redirect_uri),
    );
    Ok(Json(OAuthStartResponse {
        auth_url,
        state: oauth_state,
    }))
}

#[derive(Deserialize)]
pub struct SlackOAuthCallback {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

#[derive(Deserialize)]
struct SlackOauthAccess {
    ok: bool,
    access_token: Option<String>,
    scope: Option<String>,
    team: Option<SlackTeam>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct SlackTeam {
    id: String,
    name: String,
}

pub async fn slack_oauth_callback(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SlackOAuthCallback>,
) -> impl IntoResponse {
    if let Some(error) = query.error {
        return (
            StatusCode::BAD_REQUEST,
            Html(format!(
                "<p>Slack authorization failed: {}</p>",
                html_escape(&error)
            )),
        );
    }
    let Some(code) = query.code else {
        return (
            StatusCode::BAD_REQUEST,
            Html("<p>Missing Slack OAuth code.</p>".to_string()),
        );
    };
    let Some(oauth_state) = query.state else {
        return (
            StatusCode::BAD_REQUEST,
            Html("<p>Missing Slack OAuth state.</p>".to_string()),
        );
    };
    let Ok(db) = db_ref(&state) else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html("<p>Database unavailable.</p>".to_string()),
        );
    };
    let Some((user_id, redirect_after)) = db.consume_oauth_state("slack", &oauth_state) else {
        return (
            StatusCode::BAD_REQUEST,
            Html("<p>Expired Slack OAuth state.</p>".to_string()),
        );
    };
    let client_id = std::env::var("SLACK_CLIENT_ID").unwrap_or_default();
    let client_secret = std::env::var("SLACK_CLIENT_SECRET").unwrap_or_default();
    let redirect_uri = std::env::var("SLACK_REDIRECT_URI").unwrap_or_else(|_| {
        "http://localhost:3402/api/integrations/slack/oauth/callback".to_string()
    });
    let client = reqwest::Client::new();
    let access_response = client
        .post("https://slack.com/api/oauth.v2.access")
        .form(&[
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
        ])
        .send()
        .await
        .and_then(|res| res.error_for_status());

    let Ok(access_response) = access_response else {
        return (
            StatusCode::BAD_GATEWAY,
            Html("<p>Slack token exchange failed.</p>".to_string()),
        );
    };
    let Ok(access) = access_response.json::<SlackOauthAccess>().await else {
        return (
            StatusCode::BAD_GATEWAY,
            Html("<p>Slack token response was invalid.</p>".to_string()),
        );
    };
    if !access.ok {
        return (
            StatusCode::BAD_GATEWAY,
            Html(format!(
                "<p>Slack token exchange failed: {}</p>",
                html_escape(access.error.as_deref().unwrap_or("unknown_error")),
            )),
        );
    }

    let team = access.team.unwrap_or(SlackTeam {
        id: "unknown".into(),
        name: "Slack workspace".into(),
    });
    let scopes = access
        .scope
        .unwrap_or_default()
        .split(',')
        .filter(|scope| !scope.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    db.upsert_integration_connection(
        &user_id,
        "slack",
        Some(&team.id),
        &team.name,
        "connected",
        &scopes,
        access.access_token.as_deref(),
        None,
        &serde_json::json!({ "team_id": team.id, "team_name": team.name }),
    );

    let target = redirect_after
        .unwrap_or_else(|| "/groups/personal/tasks?settings=integrations".to_string());
    (
        StatusCode::OK,
        Html(format!(
            "<script>window.location.href = '{}';</script><p>Slack connected. You can close this tab.</p>",
            html_escape(&target)
        )),
    )
}

#[derive(Serialize)]
pub struct SlackChannel {
    pub id: String,
    pub name: String,
    pub is_private: bool,
    pub member_count: i64,
}

#[derive(Deserialize)]
struct SlackConversationList {
    ok: bool,
    channels: Option<Vec<SlackConversation>>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct SlackConversation {
    id: String,
    name: String,
    #[serde(default)]
    is_private: bool,
    #[serde(default)]
    num_members: Option<i64>,
}

pub async fn slack_channels(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> ApiResult<Json<Vec<SlackChannel>>> {
    let db = db_ref(&state)?;
    let Some(token) = db.get_integration_token(&user.user_id, "slack") else {
        return Ok(Json(vec![
            SlackChannel {
                id: "CDEV".into(),
                name: "dev".into(),
                is_private: false,
                member_count: 8,
            },
            SlackChannel {
                id: "COPS".into(),
                name: "ops".into(),
                is_private: false,
                member_count: 4,
            },
        ]));
    };
    let url = "https://slack.com/api/conversations.list?types=public_channel,private_channel&exclude_archived=true&limit=200";
    let res = reqwest::Client::new()
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(api_error)?
        .json::<SlackConversationList>()
        .await
        .map_err(api_error)?;
    if !res.ok {
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(ErrorResponse {
                error: format!(
                    "Slack channel fetch failed: {}",
                    res.error.unwrap_or_else(|| "unknown_error".into())
                ),
            }),
        ));
    }
    Ok(Json(
        res.channels
            .unwrap_or_default()
            .into_iter()
            .map(|channel| SlackChannel {
                id: channel.id,
                name: channel.name,
                is_private: channel.is_private,
                member_count: channel.num_members.unwrap_or(0),
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
pub struct ImportSlackChannelsRequest {
    pub channels: Vec<SlackChannelSelection>,
}

#[derive(Deserialize)]
pub struct SlackChannelSelection {
    pub id: String,
    pub name: String,
    pub member_count: Option<i64>,
}

#[derive(Serialize)]
pub struct ImportGroupsResponse {
    pub groups: Vec<crate::db::CortexGroup>,
    pub mappings: Vec<crate::db::IntegrationMapping>,
}

pub async fn import_slack_channels(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Json(req): Json<ImportSlackChannelsRequest>,
) -> ApiResult<Json<ImportGroupsResponse>> {
    let db = db_ref(&state)?;
    let mut groups = Vec::new();
    let mut mappings = Vec::new();
    for channel in req.channels.into_iter().take(50) {
        let group_id = format!("slack-{}", channel.id.to_lowercase());
        let group = db.upsert_group(
            &user.user_id,
            &group_id,
            &format!("#{}", channel.name),
            "team",
            "Slack channel coordination",
            channel.member_count.unwrap_or(1).max(1),
            "#8fb4ff",
            "slack",
            Some(&channel.id),
        );
        let mapping = db.upsert_integration_mapping(
            &user.user_id,
            "slack",
            &group.id,
            &channel.id,
            &channel.name,
            "channel_group",
            &serde_json::json!({ "task_sync": true, "notifications": true }),
        );
        groups.push(group);
        mappings.push(mapping);
    }
    Ok(Json(ImportGroupsResponse { groups, mappings }))
}

#[derive(Deserialize)]
pub struct SlackCommandForm {
    pub text: Option<String>,
    pub user_name: Option<String>,
    pub team_id: Option<String>,
    pub channel_id: Option<String>,
    pub channel_name: Option<String>,
}

pub async fn slack_command(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    if !verify_slack_signature(&headers, &body).unwrap_or(true) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "invalid signature" })),
        );
    }
    let form = parse_slack_form(&body);
    let user_id = form.team_id.clone().unwrap_or_else(|| "slack".into());
    let group_id = form
        .channel_id
        .as_ref()
        .map(|id| format!("slack-{}", id.to_lowercase()));
    if let (Ok(db), Some(group_id), Some(text)) = (db_ref(&state), group_id, form.text.clone()) {
        let task_state = db
            .get_group_task_state(&user_id, &group_id)
            .unwrap_or_else(|| empty_task_state(&group_id));
        db.record_integration_event(
            &user_id,
            "slack",
            Some(&group_id),
            "slash_command_task",
            "queued",
            &serde_json::json!({
                "text": text,
                "actor": form.user_name.unwrap_or_else(|| "Slack".into()),
                "channel": form.channel_name,
                "current_state": task_state,
            }),
        );
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "response_type": "ephemeral",
            "text": "Cortex captured that task and queued it for the mapped task manager."
        })),
    )
}

pub async fn slack_events(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    if !verify_slack_signature(&headers, &body).unwrap_or(true) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "invalid signature" })),
        );
    }
    let payload: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|_| serde_json::json!({}));
    if payload.get("type").and_then(|v| v.as_str()) == Some("url_verification") {
        return (
            StatusCode::OK,
            Json(
                serde_json::json!({ "challenge": payload.get("challenge").cloned().unwrap_or_default() }),
            ),
        );
    }
    let team_id = payload
        .get("team_id")
        .and_then(|v| v.as_str())
        .unwrap_or("slack");
    let channel_id = payload.pointer("/event/channel").and_then(|v| v.as_str());
    let group_id = channel_id.map(|id| format!("slack-{}", id.to_lowercase()));
    if let Ok(db) = db_ref(&state) {
        db.record_integration_event(
            team_id,
            "slack",
            group_id.as_deref(),
            "event_callback",
            "queued",
            &payload,
        );
    }
    (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
}

#[derive(Serialize)]
pub struct ReplitWorkspace {
    pub id: String,
    pub title: String,
    pub language: String,
    pub url: Option<String>,
}

pub async fn replit_workspaces(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> ApiResult<Json<Vec<ReplitWorkspace>>> {
    let db = db_ref(&state)?;
    db.upsert_integration_connection(
        &user.user_id,
        "replit",
        Some("account"),
        "Replit",
        if std::env::var("REPLIT_API_TOKEN").is_ok() {
            "connected"
        } else {
            "needs_config"
        },
        &["read:repls".to_string(), "write:tasks".to_string()],
        std::env::var("REPLIT_API_TOKEN").ok().as_deref(),
        None,
        &serde_json::json!({ "container_strategy": "replit-containers" }),
    );
    Ok(Json(vec![
        ReplitWorkspace {
            id: "repl-main".into(),
            title: "Main workspace".into(),
            language: "Node.js".into(),
            url: None,
        },
        ReplitWorkspace {
            id: "repl-worker".into(),
            title: "Worker sandbox".into(),
            language: "Rust".into(),
            url: None,
        },
    ]))
}

#[derive(Deserialize)]
pub struct ImportReplitRequest {
    pub workspace_id: String,
    pub title: String,
    pub language: Option<String>,
}

pub async fn import_replit_workspace(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Json(req): Json<ImportReplitRequest>,
) -> ApiResult<Json<ImportGroupsResponse>> {
    let db = db_ref(&state)?;
    let group_id = format!("replit-{}", slug(&req.workspace_id));
    let group = db.upsert_group(
        &user.user_id,
        &group_id,
        &req.title,
        "team",
        "Replit development workspace",
        2,
        "#f3c969",
        "replit",
        Some(&req.workspace_id),
    );
    let mapping = db.upsert_integration_mapping(
        &user.user_id,
        "replit",
        &group.id,
        &req.workspace_id,
        &req.title,
        "workspace_group",
        &serde_json::json!({
            "conversation_import": true,
            "container_strategy": "replit-containers",
            "language": req.language,
        }),
    );
    let conversation = db.create_conversation(
        &user.user_id,
        Some(&format!("Replit import: {}", req.title)),
    );
    db.add_message(
        &conversation.id,
        "assistant",
        "Imported Replit workspace context. Conversation history and code snippets will attach here when the Replit API token has chat-history scope.",
        Some("replit"),
        Some("workspace-import"),
    );
    Ok(Json(ImportGroupsResponse {
        groups: vec![group],
        mappings: vec![mapping],
    }))
}

fn api_error(err: reqwest::Error) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::BAD_GATEWAY,
        Json(ErrorResponse {
            error: format!("upstream integration request failed: {err}"),
        }),
    )
}

fn verify_slack_signature(headers: &HeaderMap, body: &str) -> Option<bool> {
    let secret = std::env::var("SLACK_SIGNING_SECRET").ok()?;
    let timestamp = headers.get("x-slack-request-timestamp")?.to_str().ok()?;
    let signature = headers.get("x-slack-signature")?.to_str().ok()?;
    let base = format!("v0:{timestamp}:{body}");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(base.as_bytes());
    let expected = format!("v0={}", hex::encode(mac.finalize().into_bytes()));
    Some(constant_time_eq(expected.as_bytes(), signature.as_bytes()))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

fn url_encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => vec![b as char],
            _ => format!("%{b:02X}").chars().collect(),
        })
        .collect()
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn slug(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn parse_slack_form(body: &str) -> SlackCommandForm {
    let mut form = SlackCommandForm {
        text: None,
        user_name: None,
        team_id: None,
        channel_id: None,
        channel_name: None,
    };
    for pair in body.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or_default();
        let value = percent_decode(parts.next().unwrap_or_default());
        match key {
            "text" => form.text = Some(value),
            "user_name" => form.user_name = Some(value),
            "team_id" => form.team_id = Some(value),
            "channel_id" => form.channel_id = Some(value),
            "channel_name" => form.channel_name = Some(value),
            _ => {}
        }
    }
    form
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = &value[i + 1..i + 3];
                if let Ok(decoded) = u8::from_str_radix(hex, 16) {
                    out.push(decoded);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}
