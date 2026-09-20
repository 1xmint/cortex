use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::clerk::ClerkUser;
use crate::routes::ErrorResponse;
use crate::state::AppState;

#[derive(Serialize)]
pub struct UserProfile {
    pub user_id: String,
    pub github_linked: bool,
    pub selected_repos: Vec<String>,
}

#[derive(Serialize)]
pub struct GitHubStatus {
    pub linked: bool,
    pub username: Option<String>,
    pub repos: Vec<GitHubRepo>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct GitHubRepo {
    pub id: u64,
    pub full_name: String,
    pub description: Option<String>,
    pub html_url: String,
    pub private: bool,
    pub language: Option<String>,
    #[serde(default)]
    pub default_branch: Option<String>,
    #[serde(default)]
    pub owner: Option<GitHubRepoOwner>,
    #[serde(default)]
    pub permissions: Option<GitHubRepoPermissions>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct GitHubRepoOwner {
    pub login: String,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct GitHubRepoPermissions {
    #[serde(default)]
    pub admin: bool,
    #[serde(default)]
    pub maintain: bool,
    #[serde(default)]
    pub push: bool,
    #[serde(default)]
    pub triage: bool,
    #[serde(default)]
    pub pull: bool,
}

#[derive(Deserialize)]
pub struct SelectReposRequest {
    pub repo_ids: Vec<u64>,
}

#[derive(Serialize, Deserialize, Default)]
struct UserData {
    #[serde(default)]
    selected_repos: Vec<u64>,
}

fn user_data_path(state: &AppState, user_id: &str) -> std::path::PathBuf {
    state
        .workspace_dir
        .join(".cortex")
        .join("users")
        .join(format!("{user_id}.json"))
}

fn read_user_data(state: &AppState, user_id: &str) -> UserData {
    let path = user_data_path(state, user_id);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_user_data(state: &AppState, user_id: &str, data: &UserData) -> Result<(), String> {
    let path = user_data_path(state, user_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir failed: {e}"))?;
    }
    let json = serde_json::to_string_pretty(data).map_err(|e| format!("serialize failed: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("write failed: {e}"))?;
    Ok(())
}

async fn github_oauth_token(state: &AppState, user_id: &str) -> Result<Option<String>, String> {
    crate::github::github_oauth_token(state.clerk_secret_key.as_deref(), user_id).await
}

async fn fetch_github_repos(github_token: &str) -> Vec<GitHubRepo> {
    let client = reqwest::Client::new();
    let repos_res = client
        .get("https://api.github.com/user/repos?affiliation=owner,collaborator,organization_member&per_page=100&sort=updated")
        .header("Authorization", format!("Bearer {github_token}"))
        .header("User-Agent", "Cortex/1.0")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await;

    match repos_res {
        Ok(res) if res.status().is_success() => res.json().await.unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn repo_authority_access(repo: &GitHubRepo) -> (&'static str, &'static str) {
    let permissions = repo.permissions.clone().unwrap_or_default();
    if permissions.admin || permissions.maintain {
        ("write", "admin")
    } else if permissions.push {
        ("write", "member")
    } else {
        ("read", "viewer")
    }
}

fn strongest_authority_role(current: Option<&str>, candidate: &str) -> &'static str {
    let rank = |role: &str| match role {
        "owner" => 4,
        "admin" => 3,
        "member" => 2,
        "viewer" => 1,
        _ => 0,
    };
    let current_role = current.unwrap_or("viewer");
    if rank(candidate) > rank(current_role) {
        match candidate {
            "owner" => "owner",
            "admin" => "admin",
            "member" => "member",
            _ => "viewer",
        }
    } else {
        match current_role {
            "owner" => "owner",
            "admin" => "admin",
            "member" => "member",
            _ => "viewer",
        }
    }
}

fn sync_selected_github_authority(state: &AppState, user_id: &str, selected_repos: &[GitHubRepo]) {
    let Some(db) = &state.db else {
        return;
    };
    let mut org_roles: HashMap<String, &'static str> = HashMap::new();
    for repo in selected_repos {
        let Some(owner) = repo.owner.as_ref() else {
            continue;
        };
        if owner.kind != "Organization" {
            continue;
        }

        let org_login = owner.login.trim();
        if org_login.is_empty() {
            continue;
        }

        let scope_id = format!("org:github:{org_login}");
        let repo_key = format!("github:{}", repo.full_name);
        let (access, role) = repo_authority_access(repo);
        let current_role = org_roles.get(&scope_id).copied();
        org_roles.insert(
            scope_id.clone(),
            strongest_authority_role(current_role, role),
        );
        db.upsert_authority_scope(
            user_id,
            &scope_id,
            "org",
            org_login,
            "GitHub organization authority discovered from selected repositories",
            "github",
            Some(org_login),
            serde_json::json!({
                "requires_org_handoff": true,
                "allowed_actions": ["read", "plan", "queue", "run_org"],
                "write_policy": "repo_permission_required",
                "source": "github_oauth_repo_selection"
            }),
        );
        db.upsert_authority_resource(
            &scope_id,
            "github_repo",
            &repo_key,
            access,
            serde_json::json!({
                "repo_id": repo.id,
                "full_name": repo.full_name.clone(),
                "private": repo.private,
                "html_url": repo.html_url.clone(),
                "language": repo.language.clone(),
                "default_branch": repo.default_branch.clone(),
                "permissions": repo.permissions.clone(),
                "requires_lease": true,
                "source": "github_oauth_repo_selection"
            }),
        );
    }

    for (scope_id, role) in org_roles {
        db.add_authority_membership(&scope_id, user_id, role);
    }
}

pub async fn get_profile(State(state): State<Arc<AppState>>, user: ClerkUser) -> Json<UserProfile> {
    let data = read_user_data(&state, &user.user_id);
    Json(UserProfile {
        user_id: user.user_id,
        github_linked: false,
        selected_repos: data
            .selected_repos
            .iter()
            .map(|id| id.to_string())
            .collect(),
    })
}

pub async fn github_status(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> Result<Json<GitHubStatus>, (StatusCode, Json<ErrorResponse>)> {
    let github_token = github_oauth_token(&state, &user.user_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error: e }),
            )
        })?;

    let github_token = match github_token {
        Some(t) => t,
        None => {
            return Ok(Json(GitHubStatus {
                linked: false,
                username: None,
                repos: Vec::new(),
            }));
        }
    };

    let client = reqwest::Client::new();
    let repos = fetch_github_repos(&github_token).await;

    let user_res = client
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {github_token}"))
        .header("User-Agent", "Cortex/1.0")
        .send()
        .await;

    let username = match user_res {
        Ok(res) if res.status().is_success() => {
            let v: serde_json::Value = res.json().await.unwrap_or_default();
            v.get("login")
                .and_then(|l| l.as_str())
                .map(|s| s.to_string())
        }
        _ => None,
    };

    Ok(Json(GitHubStatus {
        linked: true,
        username,
        repos,
    }))
}

pub async fn select_repos(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Json(req): Json<SelectReposRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let mut data = read_user_data(&state, &user.user_id);
    data.selected_repos = req.repo_ids.clone();
    write_user_data(&state, &user.user_id, &data).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { error: e }),
        )
    })?;

    let selected_count = match github_oauth_token(&state, &user.user_id).await {
        Ok(Some(token)) => {
            let repos = fetch_github_repos(&token).await;
            let selected: Vec<GitHubRepo> = repos
                .into_iter()
                .filter(|repo| req.repo_ids.contains(&repo.id))
                .collect();
            let count = selected.len();
            sync_selected_github_authority(&state, &user.user_id, &selected);
            count
        }
        _ => 0,
    };

    Ok(Json(serde_json::json!({
        "ok": true,
        "authority_synced": selected_count
    })))
}

// --- Routing profile ---

#[derive(Deserialize)]
pub struct UpdateProfileRequest {
    pub profile: String,
}

pub async fn update_profile(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
    Json(req): Json<UpdateProfileRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    use cortex_core::evaluator::Profile;

    let profile = Profile::from_alias(&req.profile).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!(
                    "unknown profile '{}' — use: auto, balanced, cost-saver, quality-first",
                    req.profile
                ),
            }),
        )
    })?;

    let canonical = match profile {
        Profile::Auto => "auto",
        Profile::Balanced => "balanced",
        Profile::CostSaver => "cost-saver",
        Profile::QualityFirst => "quality-first",
    };

    if let Some(db) = &state.db {
        db.upsert_user_profile(&user.user_id, canonical);
    }

    Ok(Json(serde_json::json!({
        "profile": canonical,
        "user_id": user.user_id,
    })))
}

pub async fn get_routing_profile(
    State(state): State<Arc<AppState>>,
    user: ClerkUser,
) -> Json<serde_json::Value> {
    // If using local auth (no real user), return empty profile to prevent frontend crashes
    if user.user_id == "local" && state.clerk_secret_key.is_none() {
        return Json(serde_json::json!({
            "user_id": "local",
            "profile": "auto",
            "auto_mode": "normal",
            "pressure": []
        }));
    }

    let (profile, auto_mode) = state
        .db
        .as_ref()
        .and_then(|db| db.get_full_user_profile(&user.user_id))
        .unwrap_or_else(|| ("auto".into(), "normal".into()));

    let pressure = state
        .db
        .as_ref()
        .map(|db| {
            let raw =
                db.pressure_for_user(&user.user_id, cortex_core::evaluator::WINDOW_SECS * 1000);
            raw.into_iter()
                .map(|(provider, tier, tokens)| {
                    serde_json::json!({
                        "provider": provider,
                        "tier": tier,
                        "tokens": tokens,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Json(serde_json::json!({
        "user_id": user.user_id,
        "profile": profile,
        "auto_mode": auto_mode,
        "pressure": pressure,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_with_permissions(
        id: u64,
        full_name: &str,
        owner_kind: &str,
        permissions: GitHubRepoPermissions,
    ) -> GitHubRepo {
        let owner_login = full_name.split('/').next().unwrap_or("owner").to_string();
        GitHubRepo {
            id,
            full_name: full_name.to_string(),
            description: None,
            html_url: format!("https://github.com/{full_name}"),
            private: true,
            language: Some("TypeScript".to_string()),
            default_branch: Some("main".to_string()),
            owner: Some(GitHubRepoOwner {
                login: owner_login,
                kind: owner_kind.to_string(),
            }),
            permissions: Some(permissions),
        }
    }

    #[test]
    fn repo_authority_access_uses_github_permissions() {
        let admin = repo_with_permissions(
            1,
            "hey-vera/heyvera",
            "Organization",
            GitHubRepoPermissions {
                admin: true,
                ..GitHubRepoPermissions::default()
            },
        );
        let push = repo_with_permissions(
            2,
            "hey-vera/cortex",
            "Organization",
            GitHubRepoPermissions {
                push: true,
                ..GitHubRepoPermissions::default()
            },
        );
        let read = repo_with_permissions(
            3,
            "hey-vera/read-only",
            "Organization",
            GitHubRepoPermissions {
                pull: true,
                ..GitHubRepoPermissions::default()
            },
        );

        assert_eq!(repo_authority_access(&admin), ("write", "admin"));
        assert_eq!(repo_authority_access(&push), ("write", "member"));
        assert_eq!(repo_authority_access(&read), ("read", "viewer"));
    }

    #[test]
    fn strongest_authority_role_does_not_downgrade() {
        assert_eq!(strongest_authority_role(Some("admin"), "viewer"), "admin");
        assert_eq!(strongest_authority_role(Some("viewer"), "member"), "member");
        assert_eq!(strongest_authority_role(None, "viewer"), "viewer");
    }
}
