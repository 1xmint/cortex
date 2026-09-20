use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::clerk::ClerkUser;
use crate::routes::ErrorResponse;
use crate::state::AppState;

type ApiResult<T> = Result<T, (StatusCode, Json<ErrorResponse>)>;

fn db_ref(state: &AppState) -> ApiResult<&crate::db::Database> {
    state.db.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "database not available".into(),
            }),
        )
    })
}

// --- Replit API Client ---

#[derive(Clone)]
pub struct ReplitClient {
    api_token: String,
    client: reqwest::Client,
}

impl ReplitClient {
    pub fn new(api_token: String) -> Self {
        Self {
            api_token,
            client: reqwest::Client::new(),
        }
    }

    async fn make_request<T: for<'de> serde::Deserialize<'de>>(
        &self,
        method: reqwest::Method,
        url: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, String> {
        let mut req = self
            .client
            .request(method, url)
            .header("Authorization", format!("Bearer {}", self.api_token))
            .header("Content-Type", "application/json");

        if let Some(body) = body {
            req = req.json(&body);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| format!("Replit API request failed: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("Replit API error {}: {}", status, body));
        }

        resp.json::<T>()
            .await
            .map_err(|e| format!("Failed to parse Replit API response: {}", e))
    }

    pub async fn create_workspace(
        &self,
        req: &CreateWorkspaceRequest,
    ) -> Result<ReplitWorkspace, String> {
        let body = serde_json::json!({
            "title": req.title,
            "language": req.language,
            "isPrivate": true,
            "description": req.description,
            "files": req.files
        });

        self.make_request(
            reqwest::Method::POST,
            "https://replit.com/data/repls",
            Some(body),
        )
        .await
    }

    pub async fn get_workspace(&self, repl_id: &str) -> Result<ReplitWorkspace, String> {
        self.make_request(
            reqwest::Method::GET,
            &format!("https://replit.com/data/repls/{}", repl_id),
            None,
        )
        .await
    }

    pub async fn list_user_workspaces(&self) -> Result<Vec<ReplitWorkspace>, String> {
        self.make_request(reqwest::Method::GET, "https://replit.com/data/repls", None)
            .await
    }

    pub async fn update_workspace_files(
        &self,
        repl_id: &str,
        files: &HashMap<String, String>,
    ) -> Result<(), String> {
        let body = serde_json::json!({
            "files": files
        });

        let _: serde_json::Value = self
            .make_request(
                reqwest::Method::PATCH,
                &format!("https://replit.com/data/repls/{}/files", repl_id),
                Some(body),
            )
            .await?;

        Ok(())
    }
}

// --- Types ---

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReplitWorkspace {
    pub id: String,
    pub title: String,
    pub language: String,
    pub url: String,
    #[serde(rename = "isPrivate")]
    pub is_private: bool,
    pub description: Option<String>,
    #[serde(rename = "userId")]
    pub user_id: Option<String>,
    #[serde(default)]
    pub files: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateWorkspaceRequest {
    pub title: String,
    pub language: String,
    pub description: Option<String>,
    #[serde(default)]
    pub files: HashMap<String, String>,
    #[serde(default)]
    pub project_type: Option<String>,
    #[serde(default)]
    pub github_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ImportProjectRequest {
    pub name: String,
    pub source_type: String, // "github", "upload", "template"
    pub source_url: Option<String>,
    #[serde(default)]
    pub files: HashMap<String, String>,
}

#[derive(Debug, Serialize)]
pub struct ProjectWorkspaceResponse {
    pub project_id: String,
    pub workspace: ReplitWorkspace,
    pub chat_endpoint: String,
    pub access_token: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceListResponse {
    pub workspaces: Vec<ProjectWorkspace>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProjectWorkspace {
    pub id: String,
    pub user_id: String,
    pub project_name: String,
    pub workspace_id: String,
    pub workspace_url: String,
    pub chat_endpoint: String,
    pub created_at: String,
    pub updated_at: String,
    pub status: String, // "creating", "active", "error"
    pub metadata: Option<serde_json::Value>,
}

// --- Database Operations ---

impl crate::db::Database {
    pub fn create_project_workspace(&self, workspace: &ProjectWorkspace) -> bool {
        let conn = self.conn();
        let result = conn.execute(
            "INSERT INTO project_workspaces (
                id, user_id, project_name, workspace_id, workspace_url,
                chat_endpoint, created_at, updated_at, status, metadata
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                &workspace.id,
                &workspace.user_id,
                &workspace.project_name,
                &workspace.workspace_id,
                &workspace.workspace_url,
                &workspace.chat_endpoint,
                &workspace.created_at,
                &workspace.updated_at,
                &workspace.status,
                workspace
                    .metadata
                    .as_ref()
                    .map(|m| serde_json::to_string(m).unwrap_or_default()),
            ],
        );
        result.is_ok()
    }

    pub fn get_project_workspace(
        &self,
        user_id: &str,
        project_id: &str,
    ) -> Option<ProjectWorkspace> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, project_name, workspace_id, workspace_url,
                    chat_endpoint, created_at, updated_at, status, metadata
             FROM project_workspaces
             WHERE user_id = ? AND id = ?",
            )
            .ok()?;

        let mut rows = stmt
            .query_map(
                rusqlite::params![user_id, project_id],
                |row: &rusqlite::Row| {
                    let metadata_str: Option<String> = row.get(9)?;
                    let metadata = metadata_str
                        .as_deref()
                        .and_then(|s| serde_json::from_str(s).ok());

                    Ok(ProjectWorkspace {
                        id: row.get(0)?,
                        user_id: row.get(1)?,
                        project_name: row.get(2)?,
                        workspace_id: row.get(3)?,
                        workspace_url: row.get(4)?,
                        chat_endpoint: row.get(5)?,
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                        status: row.get(8)?,
                        metadata,
                    })
                },
            )
            .ok()?;

        rows.next()
            .and_then(|row: Result<ProjectWorkspace, _>| row.ok())
    }

    pub fn list_user_project_workspaces(&self, user_id: &str) -> Vec<ProjectWorkspace> {
        let conn = self.conn();
        let mut stmt = match conn.prepare(
            "SELECT id, user_id, project_name, workspace_id, workspace_url,
                    chat_endpoint, created_at, updated_at, status, metadata
             FROM project_workspaces
             WHERE user_id = ?
             ORDER BY updated_at DESC",
        ) {
            Ok(stmt) => stmt,
            Err(_) => return Vec::new(),
        };

        let rows_result = stmt.query_map(rusqlite::params![user_id], |row: &rusqlite::Row| {
            let metadata_str: Option<String> = row.get(9)?;
            let metadata = metadata_str
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok());

            Ok(ProjectWorkspace {
                id: row.get(0)?,
                user_id: row.get(1)?,
                project_name: row.get(2)?,
                workspace_id: row.get(3)?,
                workspace_url: row.get(4)?,
                chat_endpoint: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
                status: row.get(8)?,
                metadata,
            })
        });

        match rows_result {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn update_project_workspace_status(
        &self,
        user_id: &str,
        project_id: &str,
        status: &str,
    ) -> bool {
        let conn = self.conn();
        let now = chrono::Utc::now().to_rfc3339();
        let result = conn.execute(
            "UPDATE project_workspaces SET status = ?, updated_at = ? WHERE user_id = ? AND id = ?",
            rusqlite::params![status, &now, user_id, project_id],
        );
        result.map(|affected| affected > 0).unwrap_or(false)
    }

    pub fn delete_project_workspace(&self, user_id: &str, project_id: &str) -> bool {
        let conn = self.conn();
        let result = conn.execute(
            "DELETE FROM project_workspaces WHERE user_id = ? AND id = ?",
            rusqlite::params![user_id, project_id],
        );
        result.map(|affected| affected > 0).unwrap_or(false)
    }
}

// --- HTTP Handlers ---

/// POST /api/projects/create - Create new project with Replit workspace
pub async fn create_project(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Json(req): Json<CreateWorkspaceRequest>,
) -> ApiResult<Json<ProjectWorkspaceResponse>> {
    let replit_token = std::env::var("REPLIT_API_TOKEN").map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "Replit integration not configured".into(),
            }),
        )
    })?;

    let replit_client = ReplitClient::new(replit_token);
    let db = db_ref(&state)?;

    // Create workspace on Replit
    let workspace = replit_client.create_workspace(&req).await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(ErrorResponse {
                error: format!("Failed to create Replit workspace: {}", e),
            }),
        )
    })?;

    // Generate project ID and chat endpoint
    let project_id = Uuid::new_v4().to_string();
    let chat_endpoint = format!("wss://proxy.heyvera.org/api/projects/{}/chat", project_id);
    let now = chrono::Utc::now().to_rfc3339();

    // Store in database
    let project_workspace = ProjectWorkspace {
        id: project_id.clone(),
        user_id: user.user_id.clone(),
        project_name: req.title.clone(),
        workspace_id: workspace.id.clone(),
        workspace_url: workspace.url.clone(),
        chat_endpoint: chat_endpoint.clone(),
        created_at: now.clone(),
        updated_at: now,
        status: "active".into(),
        metadata: Some(serde_json::json!({
            "language": req.language,
            "description": req.description,
            "source_type": "create"
        })),
    };

    if !db.create_project_workspace(&project_workspace) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Failed to save project workspace".into(),
            }),
        ));
    }

    Ok(Json(ProjectWorkspaceResponse {
        project_id,
        workspace,
        chat_endpoint,
        access_token: None, // TODO: Generate workspace-specific token
    }))
}

/// POST /api/projects/import - Import project from GitHub/files
pub async fn import_project(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Json(req): Json<ImportProjectRequest>,
) -> ApiResult<Json<ProjectWorkspaceResponse>> {
    let replit_token = std::env::var("REPLIT_API_TOKEN").map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "Replit integration not configured".into(),
            }),
        )
    })?;

    let replit_client = ReplitClient::new(replit_token);
    let db = db_ref(&state)?;

    // Determine language from source
    let language = match req.source_type.as_str() {
        "github" => detect_language_from_github(req.source_url.as_deref().unwrap_or("")),
        _ => "javascript".to_string(), // Default fallback
    };

    // Create workspace request
    let create_req = CreateWorkspaceRequest {
        title: req.name.clone(),
        language,
        description: Some(format!("Imported from {}", req.source_type)),
        files: req.files,
        project_type: None,
        github_url: req.source_url.clone(),
    };

    // Create workspace on Replit
    let workspace = replit_client
        .create_workspace(&create_req)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: format!("Failed to create Replit workspace: {}", e),
                }),
            )
        })?;

    // If GitHub URL provided, clone it to the workspace
    if let Some(github_url) = &req.source_url {
        if req.source_type == "github" {
            let _ = clone_github_to_workspace(&replit_client, &workspace.id, github_url).await;
        }
    }

    // Generate project ID and chat endpoint
    let project_id = Uuid::new_v4().to_string();
    let chat_endpoint = format!("wss://proxy.heyvera.org/api/projects/{}/chat", project_id);
    let now = chrono::Utc::now().to_rfc3339();

    // Store in database
    let project_workspace = ProjectWorkspace {
        id: project_id.clone(),
        user_id: user.user_id.clone(),
        project_name: req.name.clone(),
        workspace_id: workspace.id.clone(),
        workspace_url: workspace.url.clone(),
        chat_endpoint: chat_endpoint.clone(),
        created_at: now.clone(),
        updated_at: now,
        status: "active".into(),
        metadata: Some(serde_json::json!({
            "source_type": req.source_type,
            "source_url": req.source_url,
            "language": create_req.language
        })),
    };

    if !db.create_project_workspace(&project_workspace) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Failed to save project workspace".into(),
            }),
        ));
    }

    Ok(Json(ProjectWorkspaceResponse {
        project_id,
        workspace,
        chat_endpoint,
        access_token: None,
    }))
}

/// GET /api/projects - List user's project workspaces
pub async fn list_projects(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> ApiResult<Json<WorkspaceListResponse>> {
    let db = db_ref(&state)?;
    let workspaces = db.list_user_project_workspaces(&user.user_id);

    Ok(Json(WorkspaceListResponse { workspaces }))
}

/// GET /api/projects/{id} - Get specific project workspace
pub async fn get_project(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(project_id): Path<String>,
) -> ApiResult<Json<ProjectWorkspace>> {
    let db = db_ref(&state)?;
    let workspace = db
        .get_project_workspace(&user.user_id, &project_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "Project not found".into(),
                }),
            )
        })?;

    Ok(Json(workspace))
}

/// DELETE /api/projects/{id} - Delete project workspace
pub async fn delete_project(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(project_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let db = db_ref(&state)?;

    if !db.delete_project_workspace(&user.user_id, &project_id) {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Project not found".into(),
            }),
        ));
    }

    Ok(Json(serde_json::json!({"success": true})))
}

/// POST /api/projects/{id}/chat - Proxy chat to workspace
pub async fn proxy_chat_to_workspace(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Path(project_id): Path<String>,
    Json(chat_req): Json<crate::chat::ChatRequest>,
) -> Result<
    axum::response::sse::Sse<
        impl futures_core::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
    >,
    (StatusCode, Json<ErrorResponse>),
> {
    use axum::response::sse::{Event, KeepAlive};
    use std::convert::Infallible;
    use tokio::sync::mpsc;
    use tokio_stream::wrappers::ReceiverStream;
    use tokio_stream::StreamExt;

    let db = db_ref(&state)?;
    let workspace = db
        .get_project_workspace(&user.user_id, &project_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "Project workspace not found".into(),
                }),
            )
        })?;

    let (tx, rx) = mpsc::channel::<crate::state::StepEvent>(64);
    let workspace_clone = workspace.clone();

    tokio::spawn(async move {
        let _ = tx
            .send(crate::state::StepEvent::Started {
                step_id: "workspace-chat".into(),
                provider: "replit".into(),
                model: "workspace".into(),
            })
            .await;

        match proxy_to_replit_workspace(&workspace_clone, &chat_req).await {
            Ok(response) => {
                let _ = tx
                    .send(crate::state::StepEvent::Output {
                        step_id: "workspace-chat".into(),
                        line: response,
                    })
                    .await;
                let _ = tx
                    .send(crate::state::StepEvent::Completed {
                        step_id: "workspace-chat".into(),
                        exit_code: 0,
                    })
                    .await;
            }
            Err(error) => {
                let _ = tx
                    .send(crate::state::StepEvent::Failed {
                        step_id: "workspace-chat".into(),
                        error,
                    })
                    .await;
            }
        }
    });

    fn step_event_to_sse(event: crate::state::StepEvent) -> Result<Event, Infallible> {
        let data = serde_json::to_string(&event).unwrap_or_default();
        Ok(Event::default().data(data))
    }

    let stream = ReceiverStream::new(rx).map(step_event_to_sse);
    Ok(axum::response::sse::Sse::new(stream).keep_alive(KeepAlive::default()))
}

// --- Helper Functions ---

fn detect_language_from_github(github_url: &str) -> String {
    // Simple language detection from repo name/path
    if github_url.contains("rust") || github_url.ends_with(".rs") {
        "rust".to_string()
    } else if github_url.contains("python") || github_url.contains("py") {
        "python".to_string()
    } else if github_url.contains("typescript") || github_url.contains("ts") {
        "typescript".to_string()
    } else if github_url.contains("go") {
        "go".to_string()
    } else {
        "javascript".to_string() // Default
    }
}

async fn clone_github_to_workspace(
    replit_client: &ReplitClient,
    workspace_id: &str,
    github_url: &str,
) -> Result<(), String> {
    // Create a basic shell script to clone the repo
    let clone_script = format!(
        "git clone {} /tmp/repo && cp -r /tmp/repo/* . && rm -rf /tmp/repo",
        github_url
    );

    let mut files = HashMap::new();
    files.insert("clone.sh".to_string(), clone_script);

    replit_client
        .update_workspace_files(workspace_id, &files)
        .await
}

async fn proxy_to_replit_workspace(
    workspace: &ProjectWorkspace,
    chat_req: &crate::chat::ChatRequest,
) -> Result<String, String> {
    // TODO: Implement actual Replit workspace execution proxy
    // This would connect to the workspace's runtime and execute the chat command

    // For now, return a placeholder response
    Ok(format!(
        "🔧 **Workspace Response** (Replit: {})\n\n\
         I received your message: \"{}\"\n\n\
         This workspace is running at: {}\n\n\
         *Note: Full workspace execution proxy is being implemented.*",
        workspace.workspace_id, chat_req.message, workspace.workspace_url
    ))
}
