//! GitHub repository import + sync.
//!
//! Lets a user browse their GitHub repositories, import (clone) one into their
//! personal BYOS container, track import progress, and run bidirectional sync
//! (commit/push local changes, pull remote changes) against GitHub.
//!
//! The GitHub OAuth token is brokered by Clerk and fetched on demand — it is
//! never persisted by Cortex. For each git network operation the token is passed
//! in-memory via an `http.extraheader` config flag (`-c`), so it never lands on
//! disk in the container's `.git/config` or in the remote URL.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use base64::Engine;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::billing::PremiumUser;
use crate::routes::ErrorResponse;
use crate::state::AppState;
use crate::user::GitHubRepo;

/// Where repos are cloned inside the container.
const REPOS_ROOT: &str = "/home/sandbox/repos";
/// Provider label used when ensuring the container exists for import work.
const IMPORT_PROVIDER: &str = "claude";

const CLONE_TIMEOUT: Duration = Duration::from_secs(120);
const GIT_OP_TIMEOUT: Duration = Duration::from_secs(90);

fn err(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (status, Json(ErrorResponse { error: msg.into() }))
}

fn db_unavailable() -> (StatusCode, Json<ErrorResponse>) {
    err(StatusCode::INTERNAL_SERVER_ERROR, "database not available")
}

/// Build the `http.<base>.extraheader` value that authenticates a git HTTPS
/// request without writing the token to disk. GitHub accepts a Basic auth
/// header of `x-access-token:<token>`.
fn auth_extraheader(token: &str) -> String {
    let encoded =
        base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"));
    format!("http.https://github.com/.extraheader=Authorization: Basic {encoded}")
}

/// Filesystem-safe directory name for a repo (`owner/repo` -> `owner__repo`).
fn clone_dir_name(full_name: &str) -> String {
    full_name.replace('/', "__")
}

// ─── GET /api/github/repos ──────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ReposResponse {
    pub linked: bool,
    pub repos: Vec<GitHubRepo>,
}

/// List the authenticated user's GitHub repositories.
pub async fn list_repos(
    State(state): State<Arc<AppState>>,
    user: PremiumUser,
) -> Result<Json<ReposResponse>, (StatusCode, Json<ErrorResponse>)> {
    let token = crate::github::github_oauth_token(state.clerk_secret_key.as_deref(), &user.user_id)
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, e))?;

    let Some(token) = token else {
        return Ok(Json(ReposResponse {
            linked: false,
            repos: Vec::new(),
        }));
    };

    let repos = fetch_repos(&token).await;
    Ok(Json(ReposResponse {
        linked: true,
        repos,
    }))
}

async fn fetch_repos(token: &str) -> Vec<GitHubRepo> {
    let client = reqwest::Client::new();
    let res = client
        .get("https://api.github.com/user/repos?affiliation=owner,collaborator,organization_member&per_page=100&sort=updated")
        .header("Authorization", format!("Bearer {token}"))
        .header("User-Agent", "Cortex/1.0")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await;
    match res {
        Ok(r) if r.status().is_success() => r.json().await.unwrap_or_default(),
        _ => Vec::new(),
    }
}

// ─── POST /api/github/import ─────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ImportRequest {
    /// `owner/repo` slug to import.
    pub repo_full_name: String,
}

#[derive(Serialize)]
pub struct ImportResponse {
    pub import_id: String,
    pub status: String,
    pub clone_path: String,
}

/// Begin importing (cloning) a repo into the user's container. Returns
/// immediately with an `import_id`; the clone runs in the background and
/// progress is observable via `GET /api/github/status/{import_id}`.
pub async fn import_repo(
    State(state): State<Arc<AppState>>,
    user: PremiumUser,
    Json(req): Json<ImportRequest>,
) -> Result<Json<ImportResponse>, (StatusCode, Json<ErrorResponse>)> {
    let repo_full_name = req.repo_full_name.trim().to_string();
    if !crate::github::is_valid_repo_full_name(&repo_full_name) {
        return Err(err(StatusCode::BAD_REQUEST, "invalid repo_full_name"));
    }

    let db = state.db.as_ref().ok_or_else(db_unavailable)?;

    if state.container_manager.is_none() {
        return Err(err(
            StatusCode::SERVICE_UNAVAILABLE,
            "container runtime not available — import requires BYOS containers",
        ));
    }

    // Resolve the GitHub token + repo metadata up front so we fail fast on auth
    // problems before creating any background work.
    let token = crate::github::github_oauth_token(state.clerk_secret_key.as_deref(), &user.user_id)
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, e))?
        .ok_or_else(|| {
            err(
                StatusCode::FORBIDDEN,
                "GitHub is not linked — connect GitHub before importing",
            )
        })?;

    let meta = fetch_repo_meta(&token, &repo_full_name)
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, e))?
        .ok_or_else(|| {
            err(
                StatusCode::NOT_FOUND,
                "repository not found or not accessible with the linked GitHub account",
            )
        })?;

    let clone_dir = clone_dir_name(&repo_full_name);
    let clone_path = format!("{REPOS_ROOT}/{clone_dir}");

    let import_id = db.upsert_github_import(
        &Uuid::new_v4().to_string(),
        &user.user_id,
        meta.id,
        &repo_full_name,
        &meta.default_branch,
        &clone_path,
        meta.private,
    );

    db.audit_log(
        &user.user_id,
        "user",
        "github.import.started",
        Some("github_import"),
        Some(&import_id),
        Some(&format!("{{\"repo\":\"{repo_full_name}\"}}")),
        None,
    );

    // Run the clone in the background so the request returns fast.
    let state_bg = state.clone();
    let user_id = user.user_id.clone();
    let import_id_bg = import_id.clone();
    let clone_path_bg = clone_path.clone();
    let default_branch = meta.default_branch.clone();
    tokio::spawn(async move {
        run_import(
            state_bg,
            user_id,
            import_id_bg,
            repo_full_name,
            default_branch,
            clone_path_bg,
            token,
        )
        .await;
    });

    Ok(Json(ImportResponse {
        import_id,
        status: "pending".to_string(),
        clone_path,
    }))
}

struct RepoMeta {
    id: i64,
    default_branch: String,
    private: bool,
}

async fn fetch_repo_meta(token: &str, full_name: &str) -> Result<Option<RepoMeta>, String> {
    let client = reqwest::Client::new();
    let res = client
        .get(format!("https://api.github.com/repos/{full_name}"))
        .header("Authorization", format!("Bearer {token}"))
        .header("User-Agent", "Cortex/1.0")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("GitHub API error: {e}"))?;

    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !res.status().is_success() {
        return Err(format!("GitHub API returned {}", res.status()));
    }

    let v: serde_json::Value = res
        .json()
        .await
        .map_err(|e| format!("failed to parse repo metadata: {e}"))?;

    Ok(Some(RepoMeta {
        id: v.get("id").and_then(|x| x.as_i64()).unwrap_or(0),
        default_branch: v
            .get("default_branch")
            .and_then(|x| x.as_str())
            .unwrap_or("main")
            .to_string(),
        private: v.get("private").and_then(|x| x.as_bool()).unwrap_or(false),
    }))
}

/// Background worker that performs the clone and records progress.
async fn run_import(
    state: Arc<AppState>,
    user_id: String,
    import_id: String,
    repo_full_name: String,
    default_branch: String,
    clone_path: String,
    token: String,
) {
    let Some(cm) = state.container_manager.as_ref() else {
        update_progress(
            &state,
            &import_id,
            "failed",
            0,
            "container runtime unavailable",
            Some("container runtime unavailable"),
        );
        return;
    };
    let Some(db) = state.db.as_ref() else {
        return;
    };

    update_progress(
        &state,
        &import_id,
        "importing",
        10,
        "provisioning container",
        None,
    );

    let container_id = match cm.ensure_container(db, &user_id, IMPORT_PROVIDER).await {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(user_id, import_id, error = %e, "github import: container provisioning failed");
            update_progress(
                &state,
                &import_id,
                "failed",
                10,
                "container provisioning failed",
                Some(&e),
            );
            return;
        }
    };
    db.touch_container_activity(&user_id);

    update_progress(
        &state,
        &import_id,
        "importing",
        30,
        "cloning repository",
        None,
    );

    // Ensure the repos root exists and is owned by the sandbox user.
    let _ = cm
        .exec_in_container(
            &container_id,
            &["mkdir", "-p", REPOS_ROOT],
            None,
            GIT_OP_TIMEOUT,
        )
        .await;

    // Fresh clone: if the directory already exists (re-import), remove it first
    // so the clone starts clean.
    let _ = cm
        .exec_in_container(
            &container_id,
            &["rm", "-rf", &clone_path],
            None,
            GIT_OP_TIMEOUT,
        )
        .await;

    let extraheader = auth_extraheader(&token);
    let clone_url = format!("https://github.com/{repo_full_name}.git");
    let clone_cmd = vec![
        "git",
        "-c",
        extraheader.as_str(),
        "clone",
        "--depth",
        "50",
        "--branch",
        default_branch.as_str(),
        clone_url.as_str(),
        clone_path.as_str(),
    ];

    let clone_res = cm
        .exec_in_container(&container_id, &clone_cmd, None, CLONE_TIMEOUT)
        .await;

    match clone_res {
        Ok(r) if r.exit_code == 0 => {}
        Ok(r) => {
            let detail = sanitize_git_error(&r.stderr);
            tracing::warn!(
                user_id,
                import_id,
                exit = r.exit_code,
                "github import: clone failed"
            );
            update_progress(
                &state,
                &import_id,
                "failed",
                30,
                "clone failed",
                Some(&detail),
            );
            return;
        }
        Err(e) => {
            update_progress(&state, &import_id, "failed", 30, "clone failed", Some(&e));
            return;
        }
    }

    update_progress(
        &state,
        &import_id,
        "importing",
        80,
        "configuring repository",
        None,
    );

    // Identify Cortex as the committer for any future sync commits.
    let _ = cm
        .exec_in_container(
            &container_id,
            &["git", "-C", &clone_path, "config", "user.name", "Cortex"],
            None,
            GIT_OP_TIMEOUT,
        )
        .await;
    let _ = cm
        .exec_in_container(
            &container_id,
            &[
                "git",
                "-C",
                &clone_path,
                "config",
                "user.email",
                "bot@cortex.heyvera.ai",
            ],
            None,
            GIT_OP_TIMEOUT,
        )
        .await;

    // Capture the cloned HEAD commit for provenance.
    let head_commit = cm
        .exec_in_container(
            &container_id,
            &["git", "-C", &clone_path, "rev-parse", "HEAD"],
            None,
            GIT_OP_TIMEOUT,
        )
        .await
        .ok()
        .filter(|r| r.exit_code == 0)
        .map(|r| r.stdout.trim().to_string());

    db.mark_github_import_synced(&import_id, head_commit.as_deref());
    update_progress(&state, &import_id, "ready", 100, "ready", None);
    db.audit_log(
        &user_id,
        "user",
        "github.import.completed",
        Some("github_import"),
        Some(&import_id),
        Some(&format!("{{\"repo\":\"{repo_full_name}\"}}")),
        None,
    );
    tracing::info!(user_id, import_id, repo = %repo_full_name, "github import complete");
}

fn update_progress(
    state: &AppState,
    import_id: &str,
    status: &str,
    progress: i64,
    stage: &str,
    error: Option<&str>,
) {
    if let Some(db) = state.db.as_ref() {
        db.update_github_import_progress(import_id, status, progress, stage, error);
    }
}

/// Strip any leaked auth material from git stderr before surfacing to the user.
fn sanitize_git_error(stderr: &str) -> String {
    let cleaned = stderr
        .lines()
        .filter(|l| !l.to_lowercase().contains("authorization"))
        .collect::<Vec<_>>()
        .join("\n");
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "git operation failed".to_string()
    } else if trimmed.len() > 500 {
        format!("{}...", &trimmed[..500])
    } else {
        trimmed.to_string()
    }
}

// ─── GET /api/github/status/{import_id} ──────────────────────────────────────

#[derive(Serialize)]
pub struct ImportStatus {
    pub import_id: String,
    pub repo_full_name: String,
    pub status: String,
    pub progress: i64,
    pub stage: String,
    pub clone_path: String,
    pub error: Option<String>,
    pub last_synced_at: Option<i64>,
    pub head_commit: Option<String>,
}

/// Report the status/progress of an import.
pub async fn import_status(
    State(state): State<Arc<AppState>>,
    user: PremiumUser,
    Path(import_id): Path<String>,
) -> Result<Json<ImportStatus>, (StatusCode, Json<ErrorResponse>)> {
    let db = state.db.as_ref().ok_or_else(db_unavailable)?;
    let imp = db
        .get_github_import(&user.user_id, &import_id)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "import not found"))?;

    Ok(Json(ImportStatus {
        import_id: imp.id,
        repo_full_name: imp.repo_full_name,
        status: imp.status,
        progress: imp.progress,
        stage: imp.stage,
        clone_path: imp.clone_path,
        error: imp.error,
        last_synced_at: imp.last_synced_at,
        head_commit: imp.head_commit,
    }))
}

/// List all of a user's imports (for the repo manager UI).
pub async fn list_imports(
    State(state): State<Arc<AppState>>,
    user: PremiumUser,
) -> Result<Json<Vec<ImportStatus>>, (StatusCode, Json<ErrorResponse>)> {
    let db = state.db.as_ref().ok_or_else(db_unavailable)?;
    let imports = db
        .list_github_imports(&user.user_id)
        .into_iter()
        .map(|imp| ImportStatus {
            import_id: imp.id,
            repo_full_name: imp.repo_full_name,
            status: imp.status,
            progress: imp.progress,
            stage: imp.stage,
            clone_path: imp.clone_path,
            error: imp.error,
            last_synced_at: imp.last_synced_at,
            head_commit: imp.head_commit,
        })
        .collect();
    Ok(Json(imports))
}

// ─── POST /api/github/sync/{import_id} ───────────────────────────────────────

#[derive(Deserialize, Default)]
pub struct SyncRequest {
    /// Optional commit message for local changes. Defaults to a timestamped
    /// "Cortex sync" message.
    #[serde(default)]
    pub message: Option<String>,
    /// When true (default), push committed local changes to GitHub after pulling.
    #[serde(default = "default_true")]
    pub push: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Serialize)]
pub struct SyncResponse {
    pub status: String,
    /// One of: synced | conflict | up_to_date | pushed
    pub committed: bool,
    pub pushed: bool,
    pub conflict: bool,
    pub head_commit: Option<String>,
    pub detail: String,
}

/// Bidirectional sync: commit local changes, pull remote (rebase), push back.
///
/// Merge conflicts are handled gracefully — a conflicting rebase is aborted and
/// the endpoint reports `conflict: true` with the conflicting files, leaving the
/// working tree clean rather than half-merged.
pub async fn sync_repo(
    State(state): State<Arc<AppState>>,
    user: PremiumUser,
    Path(import_id): Path<String>,
    body: Option<Json<SyncRequest>>,
) -> Result<Json<SyncResponse>, (StatusCode, Json<ErrorResponse>)> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let db = state.db.as_ref().ok_or_else(db_unavailable)?;

    let imp = db
        .get_github_import(&user.user_id, &import_id)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "import not found"))?;

    if imp.status != "ready" {
        return Err(err(
            StatusCode::CONFLICT,
            format!("import is not ready to sync (status: {})", imp.status),
        ));
    }

    let cm = state.container_manager.as_ref().ok_or_else(|| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "container runtime not available",
        )
    })?;

    let token = crate::github::github_oauth_token(state.clerk_secret_key.as_deref(), &user.user_id)
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, e))?
        .ok_or_else(|| err(StatusCode::FORBIDDEN, "GitHub is not linked"))?;

    let container_id = cm
        .ensure_container(db, &user.user_id, IMPORT_PROVIDER)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    db.touch_container_activity(&user.user_id);

    let path = &imp.clone_path;
    let extraheader = auth_extraheader(&token);

    // 1. Stage + commit any local working-tree changes.
    let mut committed = false;
    let status_out = cm
        .exec_in_container(
            &container_id,
            &["git", "-C", path, "status", "--porcelain"],
            None,
            GIT_OP_TIMEOUT,
        )
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    if !status_out.stdout.trim().is_empty() {
        let _ = cm
            .exec_in_container(
                &container_id,
                &["git", "-C", path, "add", "-A"],
                None,
                GIT_OP_TIMEOUT,
            )
            .await;
        let msg = req
            .message
            .clone()
            .unwrap_or_else(|| format!("Cortex sync {}", chrono::Utc::now().to_rfc3339()));
        let commit = cm
            .exec_in_container(
                &container_id,
                &["git", "-C", path, "commit", "-m", &msg],
                None,
                GIT_OP_TIMEOUT,
            )
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        committed = commit.exit_code == 0;
    }

    // 2. Pull remote with rebase (auth via in-memory extraheader).
    let pull = cm
        .exec_in_container(
            &container_id,
            &[
                "git",
                "-c",
                &extraheader,
                "-C",
                path,
                "pull",
                "--rebase",
                "origin",
                &imp.default_branch,
            ],
            None,
            GIT_OP_TIMEOUT,
        )
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    if pull.exit_code != 0 {
        // Detect a conflicting rebase and abort cleanly so the working tree is
        // never left in a half-merged state.
        let combined = format!("{}{}", pull.stdout, pull.stderr).to_lowercase();
        if combined.contains("conflict") || combined.contains("rebase") {
            let conflicts = cm
                .exec_in_container(
                    &container_id,
                    &["git", "-C", path, "diff", "--name-only", "--diff-filter=U"],
                    None,
                    GIT_OP_TIMEOUT,
                )
                .await
                .ok()
                .map(|r| r.stdout.trim().to_string())
                .unwrap_or_default();
            let _ = cm
                .exec_in_container(
                    &container_id,
                    &["git", "-C", path, "rebase", "--abort"],
                    None,
                    GIT_OP_TIMEOUT,
                )
                .await;

            db.audit_log(
                &user.user_id,
                "user",
                "github.sync.conflict",
                Some("github_import"),
                Some(&import_id),
                None,
                None,
            );

            let detail = if conflicts.is_empty() {
                "remote changes conflict with local changes; rebase aborted".to_string()
            } else {
                format!("merge conflict in: {}", conflicts.replace('\n', ", "))
            };
            return Ok(Json(SyncResponse {
                status: "conflict".to_string(),
                committed,
                pushed: false,
                conflict: true,
                head_commit: None,
                detail,
            }));
        }
        return Err(err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("git pull failed: {}", sanitize_git_error(&pull.stderr)),
        ));
    }

    // 3. Push back to GitHub if requested.
    let mut pushed = false;
    if req.push {
        let push = cm
            .exec_in_container(
                &container_id,
                &[
                    "git",
                    "-c",
                    &extraheader,
                    "-C",
                    path,
                    "push",
                    "origin",
                    &imp.default_branch,
                ],
                None,
                GIT_OP_TIMEOUT,
            )
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        if push.exit_code != 0 {
            return Err(err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("git push failed: {}", sanitize_git_error(&push.stderr)),
            ));
        }
        pushed = true;
    }

    let head_commit = cm
        .exec_in_container(
            &container_id,
            &["git", "-C", path, "rev-parse", "HEAD"],
            None,
            GIT_OP_TIMEOUT,
        )
        .await
        .ok()
        .filter(|r| r.exit_code == 0)
        .map(|r| r.stdout.trim().to_string());

    db.mark_github_import_synced(&import_id, head_commit.as_deref());
    db.audit_log(
        &user.user_id,
        "user",
        "github.sync.completed",
        Some("github_import"),
        Some(&import_id),
        None,
        None,
    );

    let detail = match (committed, pushed) {
        (true, true) => "local changes committed and pushed; remote merged",
        (true, false) => "local changes committed; remote merged",
        (false, true) => "remote merged; nothing local to push",
        (false, false) => "already up to date",
    };

    Ok(Json(SyncResponse {
        status: "synced".to_string(),
        committed,
        pushed,
        conflict: false,
        head_commit,
        detail: detail.to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_dir_name_replaces_slash() {
        assert_eq!(clone_dir_name("hey-vera/cortex"), "hey-vera__cortex");
    }

    #[test]
    fn extraheader_is_basic_auth() {
        let h = auth_extraheader("secrettoken");
        assert!(h.starts_with("http.https://github.com/.extraheader=Authorization: Basic "));
        // Decodes back to x-access-token:<token>
        let b64 = h.rsplit(' ').next().unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        assert_eq!(
            String::from_utf8(decoded).unwrap(),
            "x-access-token:secrettoken"
        );
    }

    #[test]
    fn sanitize_strips_auth_lines() {
        let raw = "fatal: clone failed\nAuthorization: Basic abcd1234\nremote error";
        let out = sanitize_git_error(raw);
        assert!(!out.to_lowercase().contains("authorization"));
        assert!(out.contains("clone failed"));
    }
}
