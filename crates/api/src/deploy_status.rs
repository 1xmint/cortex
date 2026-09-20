use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::Json;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::lock::LockRecovering;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeploySurfaceStatus {
    Match,
    Drift,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommitAlignmentStatus {
    Match,
    Mismatch,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FrontendAssets {
    pub js: Option<String>,
    pub css: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployMeta {
    pub service: Option<String>,
    pub branch: Option<String>,
    pub commit: Option<String>,
    #[serde(rename = "commitShort")]
    pub commit_short: Option<String>,
    #[serde(rename = "deployedAt")]
    pub deployed_at: Option<String>,
    #[serde(rename = "githubActions")]
    pub github_actions: Option<GitHubActionsDeployMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubActionsDeployMeta {
    pub workflow: Option<String>,
    #[serde(rename = "runId")]
    pub run_id: Option<String>,
    #[serde(rename = "runNumber")]
    pub run_number: Option<String>,
    #[serde(rename = "runAttempt")]
    pub run_attempt: Option<String>,
    pub event: Option<String>,
    pub repository: Option<String>,
    pub actor: Option<String>,
    #[serde(rename = "headBranch")]
    pub head_branch: Option<String>,
    #[serde(rename = "headSha")]
    pub head_sha: Option<String>,
    #[serde(rename = "htmlUrl")]
    pub html_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackendDeployStatus {
    pub service: &'static str,
    pub version: &'static str,
    pub commit: Option<String>,
    pub commit_short: Option<String>,
    pub branch: Option<String>,
    pub deployed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FrontendDeployStatus {
    pub public_url: String,
    pub local_root: String,
    pub expected_source: String,
    pub expected_assets: FrontendAssets,
    pub live_assets: Option<FrontendAssets>,
    pub status: DeploySurfaceStatus,
    pub drift: bool,
    pub checked_at: String,
    pub error: Option<String>,
    pub cloudflare_pages: CloudflarePagesStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct CloudflarePagesStatus {
    pub configured: bool,
    pub project: String,
    pub status: DeploySurfaceStatus,
    pub deployment_id: Option<String>,
    pub environment: Option<String>,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub url: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitHubActionsStatus {
    pub configured: bool,
    pub source: String,
    pub owner: String,
    pub repo: String,
    pub workflow: String,
    pub workflow_id: Option<i64>,
    pub workflow_name: Option<String>,
    pub workflow_path: Option<String>,
    pub branch: String,
    pub status: DeploySurfaceStatus,
    pub run_id: Option<i64>,
    pub run_number: Option<i64>,
    pub run_attempt: Option<i64>,
    pub run_status: Option<String>,
    pub conclusion: Option<String>,
    pub event: Option<String>,
    pub head_branch: Option<String>,
    pub head_sha: Option<String>,
    pub head_sha_short: Option<String>,
    pub html_url: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub run_started_at: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
struct ResolvedGitHubWorkflow {
    id: i64,
    name: Option<String>,
    path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeploymentCommitStatus {
    pub status: CommitAlignmentStatus,
    pub backend_commit: Option<String>,
    pub backend_commit_short: Option<String>,
    pub frontend_commit: Option<String>,
    pub frontend_commit_short: Option<String>,
    pub backend_branch: Option<String>,
    pub frontend_branch: Option<String>,
    pub branch_match: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeployStatusResponse {
    pub status: DeploySurfaceStatus,
    pub service: &'static str,
    pub observed_at: String,
    pub generated_at: String,
    pub backend: BackendDeployStatus,
    pub frontend: FrontendDeployStatus,
    pub github_actions: GitHubActionsStatus,
    pub commits: DeploymentCommitStatus,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeploymentEventsQuery {
    #[serde(default = "default_deployment_events_limit")]
    pub limit: usize,
}

fn default_deployment_events_limit() -> usize {
    25
}

pub async fn deploy_status(State(state): State<Arc<AppState>>) -> Json<DeployStatusResponse> {
    let status = build_deploy_status().await;
    record_deploy_inspected_event(&state, &status);
    Json(status)
}

pub async fn deployment_events(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DeploymentEventsQuery>,
) -> Json<Value> {
    let limit = query.limit.clamp(1, 100);
    let events = state
        .db
        .as_ref()
        .map(|db| db.list_deployment_operations_events(limit))
        .unwrap_or_default();

    Json(serde_json::json!({
        "scope_id": "cortex",
        "entity_type": "deployment",
        "generated_at": Utc::now().to_rfc3339(),
        "limit": limit,
        "events": events,
    }))
}

async fn build_deploy_status() -> DeployStatusResponse {
    let generated_at = Utc::now().to_rfc3339();
    let meta = read_deploy_meta();
    let github_actions_meta = meta.as_ref().and_then(|m| m.github_actions.clone());
    let backend = BackendDeployStatus {
        service: "cortex",
        version: env!("CARGO_PKG_VERSION"),
        commit: option_env!("GITHUB_SHA")
            .map(str::to_string)
            .or_else(|| std::env::var("GITHUB_SHA").ok())
            .or_else(|| std::env::var("DEPLOY_COMMIT").ok())
            .or_else(|| meta.as_ref().and_then(|m| m.commit.clone())),
        commit_short: std::env::var("DEPLOY_COMMIT_SHORT")
            .ok()
            .or_else(|| meta.as_ref().and_then(|m| m.commit_short.clone())),
        branch: std::env::var("GIT_BRANCH")
            .ok()
            .or_else(|| meta.as_ref().and_then(|m| m.branch.clone())),
        deployed_at: std::env::var("DEPLOY_TIMESTAMP")
            .ok()
            .or_else(|| meta.as_ref().and_then(|m| m.deployed_at.clone())),
    };

    let frontend = build_frontend_status().await;
    let github_actions = inspect_github_actions(
        backend.branch.as_deref().unwrap_or("main"),
        github_actions_meta.as_ref(),
    )
    .await;
    let commits = build_commit_status(&backend, &frontend);
    let status = if frontend.status == DeploySurfaceStatus::Drift {
        DeploySurfaceStatus::Drift
    } else if backend.commit.is_none() || frontend.status == DeploySurfaceStatus::Unknown {
        DeploySurfaceStatus::Unknown
    } else {
        DeploySurfaceStatus::Match
    };

    DeployStatusResponse {
        status,
        service: "cortex",
        observed_at: generated_at.clone(),
        generated_at,
        backend,
        frontend,
        github_actions,
        commits,
    }
}

fn build_commit_status(
    backend: &BackendDeployStatus,
    frontend: &FrontendDeployStatus,
) -> DeploymentCommitStatus {
    let backend_commit = backend.commit.clone();
    let frontend_commit = frontend.cloudflare_pages.commit.clone();
    let status = match (backend_commit.as_deref(), frontend_commit.as_deref()) {
        (Some(backend_commit), Some(frontend_commit)) if backend_commit == frontend_commit => {
            CommitAlignmentStatus::Match
        }
        (Some(_), Some(_)) => CommitAlignmentStatus::Mismatch,
        _ => CommitAlignmentStatus::Unknown,
    };
    let backend_branch = backend.branch.clone();
    let frontend_branch = frontend.cloudflare_pages.branch.clone();
    let branch_match = match (backend_branch.as_deref(), frontend_branch.as_deref()) {
        (Some(backend_branch), Some(frontend_branch)) => Some(backend_branch == frontend_branch),
        _ => None,
    };

    DeploymentCommitStatus {
        status,
        backend_commit_short: backend
            .commit_short
            .clone()
            .or_else(|| backend_commit.as_deref().map(short_commit)),
        frontend_commit_short: frontend_commit.as_deref().map(short_commit),
        backend_commit,
        frontend_commit,
        backend_branch,
        frontend_branch,
        branch_match,
    }
}

fn short_commit(commit: &str) -> String {
    commit.chars().take(7).collect()
}

fn record_deploy_inspected_event(state: &AppState, status: &DeployStatusResponse) {
    let Some(db) = &state.db else {
        return;
    };
    let fingerprint = deployment_event_fingerprint(status);
    {
        let mut recorded = state.recorded_deploy_events.lock_recovering();
        if !recorded.insert(fingerprint) {
            return;
        }
    }

    db.record_deployment_event(
        "deploy.inspected",
        "cortex-production",
        &deployment_event_payload(status),
    );
}

fn deployment_event_fingerprint(status: &DeployStatusResponse) -> String {
    format!(
        "{}:{}:{}:{}:{}",
        status.status_string(),
        status.backend.commit.as_deref().unwrap_or("unknown"),
        status
            .frontend
            .cloudflare_pages
            .deployment_id
            .as_deref()
            .unwrap_or("unknown"),
        status
            .github_actions
            .run_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "unknown".to_string()),
        status
            .frontend
            .live_assets
            .as_ref()
            .and_then(|assets| assets.js.as_deref())
            .unwrap_or("unknown"),
    )
}

fn deployment_event_payload(status: &DeployStatusResponse) -> serde_json::Value {
    serde_json::json!({
        "service": status.service,
        "status": status.status.clone(),
        "backend": {
            "commit": status.backend.commit.clone(),
            "commit_short": status.backend.commit_short.clone(),
            "branch": status.backend.branch.clone(),
            "deployed_at": status.backend.deployed_at.clone(),
        },
        "frontend": {
            "status": status.frontend.status.clone(),
            "drift": status.frontend.drift,
            "expected_source": status.frontend.expected_source.clone(),
            "expected_assets": status.frontend.expected_assets.clone(),
            "live_assets": status.frontend.live_assets.clone(),
            "cloudflare_pages": status.frontend.cloudflare_pages.clone(),
        },
        "commits": status.commits.clone(),
        "github_actions": status.github_actions.clone(),
        "observed_at": status.observed_at.clone(),
    })
}

impl DeployStatusResponse {
    fn status_string(&self) -> &'static str {
        match self.status {
            DeploySurfaceStatus::Match => "match",
            DeploySurfaceStatus::Drift => "drift",
            DeploySurfaceStatus::Unknown => "unknown",
        }
    }
}

async fn build_frontend_status() -> FrontendDeployStatus {
    let public_url = std::env::var("CORTEX_FRONTEND_PUBLIC_URL")
        .unwrap_or_else(|_| "https://cortex.heyvera.org/".to_string());
    let local_root = frontend_root();
    let checked_at = Utc::now().to_rfc3339();
    let cloudflare_pages = inspect_cloudflare_pages().await;
    let (expected_source, expected_assets) =
        expected_frontend_assets(&cloudflare_pages, &local_root).await;

    let live_result = if should_check_live_frontend() {
        fetch_live_assets(&public_url).await
    } else {
        Err("live frontend check disabled for this build".to_string())
    };
    let (live_assets, status, error) = match live_result {
        Ok(assets) => {
            let status = if expected_assets.js.is_some()
                && assets.js.is_some()
                && expected_assets.js != assets.js
            {
                DeploySurfaceStatus::Drift
            } else if expected_assets.js.is_some() && assets.js.is_some() {
                DeploySurfaceStatus::Match
            } else {
                DeploySurfaceStatus::Unknown
            };
            (Some(assets), status, None)
        }
        Err(err) => (None, DeploySurfaceStatus::Unknown, Some(err)),
    };

    let drift = status == DeploySurfaceStatus::Drift;
    FrontendDeployStatus {
        public_url,
        local_root: local_root.display().to_string(),
        expected_source,
        expected_assets,
        live_assets,
        status,
        drift,
        checked_at,
        error,
        cloudflare_pages,
    }
}

fn should_check_live_frontend() -> bool {
    match std::env::var("CORTEX_DEPLOY_STATUS_LIVE_CHECK") {
        Ok(value) => value != "0" && !value.eq_ignore_ascii_case("false"),
        Err(_) => !cfg!(debug_assertions),
    }
}

fn read_deploy_meta() -> Option<DeployMeta> {
    let path = std::env::var("CORTEX_DEPLOY_META_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("deploy-meta.json"));
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

fn frontend_root() -> PathBuf {
    std::env::var("CORTEX_FRONTEND_DIST")
        .or_else(|_| std::env::var("CORTEX_STATIC_DIR"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/var/www/cortex"))
}

fn read_frontend_assets(root: &Path) -> FrontendAssets {
    let html = std::fs::read_to_string(root.join("index.html")).unwrap_or_default();
    parse_frontend_assets(&html)
}

async fn expected_frontend_assets(
    cloudflare_pages: &CloudflarePagesStatus,
    local_root: &Path,
) -> (String, FrontendAssets) {
    if let Some(url) = cloudflare_pages.url.as_deref() {
        if let Ok(assets) = fetch_live_assets(url).await {
            return ("cloudflare_pages".to_string(), assets);
        }
    }
    (
        "vps_static_fallback".to_string(),
        read_frontend_assets(local_root),
    )
}

async fn inspect_cloudflare_pages() -> CloudflarePagesStatus {
    let project = std::env::var("CLOUDFLARE_PAGES_PROJECT")
        .or_else(|_| std::env::var("CF_PAGES_PROJECT"))
        .unwrap_or_else(|_| "cortex".to_string());
    let account_id = std::env::var("CLOUDFLARE_ACCOUNT_ID")
        .or_else(|_| std::env::var("CF_ACCOUNT_ID"))
        .ok();
    let api_token = std::env::var("CLOUDFLARE_API_TOKEN")
        .or_else(|_| std::env::var("CF_API_TOKEN"))
        .ok();

    let (Some(account_id), Some(api_token)) = (account_id, api_token) else {
        return CloudflarePagesStatus {
            configured: false,
            project,
            status: DeploySurfaceStatus::Unknown,
            deployment_id: None,
            environment: None,
            branch: None,
            commit: None,
            url: None,
            error: Some("Cloudflare Pages read credentials are not configured".to_string()),
        };
    };

    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/pages/projects/{}",
        account_id, project
    );
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .user_agent("cortex-deploy-status/0.1")
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            return CloudflarePagesStatus {
                configured: true,
                project,
                status: DeploySurfaceStatus::Unknown,
                deployment_id: None,
                environment: None,
                branch: None,
                commit: None,
                url: None,
                error: Some(err.to_string()),
            };
        }
    };

    let response = client
        .get(url)
        .bearer_auth(api_token)
        .send()
        .await
        .and_then(|res| res.error_for_status());

    let body = match response {
        Ok(res) => match res.json::<Value>().await {
            Ok(body) => body,
            Err(err) => {
                return cloudflare_error_status(
                    project,
                    true,
                    redact_cloudflare_error(err, &account_id),
                );
            }
        },
        Err(err) => {
            return cloudflare_error_status(
                project,
                true,
                redact_cloudflare_error(err, &account_id),
            );
        }
    };

    let deployment = body
        .pointer("/result/canonical_deployment")
        .or_else(|| body.pointer("/result/latest_deployment"));

    match deployment {
        Some(deployment) => CloudflarePagesStatus {
            configured: true,
            project,
            status: deployment_status_from_json(deployment),
            deployment_id: json_string(deployment, &["id"]),
            environment: json_string(deployment, &["environment"]),
            branch: deployment_branch(deployment),
            commit: deployment_commit(deployment),
            url: json_string(deployment, &["url"]),
            error: None,
        },
        None => CloudflarePagesStatus {
            configured: true,
            project,
            status: DeploySurfaceStatus::Unknown,
            deployment_id: None,
            environment: None,
            branch: None,
            commit: None,
            url: None,
            error: Some(
                "Cloudflare Pages project response did not include a deployment".to_string(),
            ),
        },
    }
}

async fn inspect_github_actions(
    branch: &str,
    deploy_meta: Option<&GitHubActionsDeployMeta>,
) -> GitHubActionsStatus {
    let (owner, repo) = github_repo();
    let workflow =
        std::env::var("GITHUB_DEPLOY_WORKFLOW").unwrap_or_else(|_| "Deploy Production".to_string());
    let branch = std::env::var("GITHUB_DEPLOY_BRANCH").unwrap_or_else(|_| branch.to_string());
    let api_token = std::env::var("GITHUB_TOKEN")
        .or_else(|_| std::env::var("GH_TOKEN"))
        .ok();

    let Some(api_token) = api_token else {
        if let Some(deploy_meta) = deploy_meta {
            return github_status_from_deploy_meta(owner, repo, workflow, branch, deploy_meta);
        }
        return GitHubActionsStatus {
            configured: false,
            source: "not_configured".to_string(),
            owner,
            repo,
            workflow,
            workflow_id: None,
            workflow_name: None,
            workflow_path: None,
            branch,
            status: DeploySurfaceStatus::Unknown,
            run_id: None,
            run_number: None,
            run_attempt: None,
            run_status: None,
            conclusion: None,
            event: None,
            head_branch: None,
            head_sha: None,
            head_sha_short: None,
            html_url: None,
            created_at: None,
            updated_at: None,
            run_started_at: None,
            error: Some("GitHub Actions read token is not configured".to_string()),
        };
    };

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .user_agent("cortex-deploy-status/0.1")
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            return github_error_status(owner, repo, workflow, branch, true, err.to_string());
        }
    };
    let resolved_workflow =
        match resolve_github_workflow(&client, &api_token, &owner, &repo, &workflow).await {
            Ok(workflow) => workflow,
            Err(err) => {
                return github_error_status(owner, repo, workflow, branch, true, err);
            }
        };
    let url = format!(
        "https://api.github.com/repos/{}/{}/actions/workflows/{}/runs",
        owner, repo, resolved_workflow.id
    );
    let response = client
        .get(url)
        .bearer_auth(api_token)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .query(&[("branch", branch.as_str()), ("per_page", "1")])
        .send()
        .await
        .and_then(|res| res.error_for_status());

    let body = match response {
        Ok(res) => match res.json::<Value>().await {
            Ok(body) => body,
            Err(err) => {
                return github_error_status(owner, repo, workflow, branch, true, err.to_string());
            }
        },
        Err(err) => {
            return github_error_status(owner, repo, workflow, branch, true, err.to_string());
        }
    };

    let run = body.pointer("/workflow_runs/0");
    match run {
        Some(run) => github_status_from_run(owner, repo, workflow, resolved_workflow, branch, run),
        None => GitHubActionsStatus {
            configured: true,
            source: "github_api".to_string(),
            owner,
            repo,
            workflow,
            workflow_id: Some(resolved_workflow.id),
            workflow_name: resolved_workflow.name,
            workflow_path: resolved_workflow.path,
            branch,
            status: DeploySurfaceStatus::Unknown,
            run_id: None,
            run_number: None,
            run_attempt: None,
            run_status: None,
            conclusion: None,
            event: None,
            head_branch: None,
            head_sha: None,
            head_sha_short: None,
            html_url: None,
            created_at: None,
            updated_at: None,
            run_started_at: None,
            error: Some("GitHub Actions response did not include workflow runs".to_string()),
        },
    }
}

fn github_repo() -> (String, String) {
    let raw = std::env::var("GITHUB_REPOSITORY").unwrap_or_else(|_| "hey-vera/heyvera".to_string());
    parse_owner_repo(&raw).unwrap_or(("hey-vera".to_string(), "heyvera".to_string()))
}

fn parse_owner_repo(raw: &str) -> Option<(String, String)> {
    let mut parts = raw.splitn(2, '/');
    let owner = parts.next()?.trim();
    let repo = parts.next()?.trim();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

async fn resolve_github_workflow(
    client: &reqwest::Client,
    api_token: &str,
    owner: &str,
    repo: &str,
    requested_workflow: &str,
) -> Result<ResolvedGitHubWorkflow, String> {
    if let Ok(id) = requested_workflow.parse::<i64>() {
        return Ok(ResolvedGitHubWorkflow {
            id,
            name: None,
            path: None,
        });
    }

    let url = format!("https://api.github.com/repos/{owner}/{repo}/actions/workflows");
    let body = client
        .get(url)
        .bearer_auth(api_token)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .and_then(|res| res.error_for_status())
        .map_err(|err| err.to_string())?
        .json::<Value>()
        .await
        .map_err(|err| err.to_string())?;

    let workflows = body
        .get("workflows")
        .and_then(Value::as_array)
        .ok_or_else(|| "GitHub Actions response did not include workflows".to_string())?;
    for workflow in workflows {
        let id = json_i64(workflow, &["id"]);
        let name = json_string(workflow, &["name"]);
        let path = json_string(workflow, &["path"]);
        let filename = path.as_deref().and_then(|path| path.rsplit('/').next());
        let matches = name.as_deref() == Some(requested_workflow)
            || path.as_deref() == Some(requested_workflow)
            || filename == Some(requested_workflow);
        if matches {
            let Some(id) = id else {
                return Err("Matched GitHub workflow did not include an id".to_string());
            };
            return Ok(ResolvedGitHubWorkflow { id, name, path });
        }
    }

    Err(format!(
        "GitHub Actions workflow `{requested_workflow}` was not found"
    ))
}

fn github_status_from_run(
    owner: String,
    repo: String,
    workflow: String,
    resolved_workflow: ResolvedGitHubWorkflow,
    branch: String,
    run: &Value,
) -> GitHubActionsStatus {
    let head_sha = json_string(run, &["head_sha"]);
    let conclusion = json_string(run, &["conclusion"]);
    let status = match conclusion.as_deref() {
        Some("success") => DeploySurfaceStatus::Match,
        _ => DeploySurfaceStatus::Unknown,
    };

    GitHubActionsStatus {
        configured: true,
        source: "github_api".to_string(),
        owner,
        repo,
        workflow,
        workflow_id: Some(resolved_workflow.id),
        workflow_name: resolved_workflow.name,
        workflow_path: resolved_workflow.path,
        branch,
        status,
        run_id: json_i64(run, &["id"]),
        run_number: json_i64(run, &["run_number"]),
        run_attempt: json_i64(run, &["run_attempt"]),
        run_status: json_string(run, &["status"]),
        conclusion,
        event: json_string(run, &["event"]),
        head_branch: json_string(run, &["head_branch"]),
        head_sha_short: head_sha.as_deref().map(short_commit),
        head_sha,
        html_url: json_string(run, &["html_url"]),
        created_at: json_string(run, &["created_at"]),
        updated_at: json_string(run, &["updated_at"]),
        run_started_at: json_string(run, &["run_started_at"]),
        error: None,
    }
}

fn github_status_from_deploy_meta(
    fallback_owner: String,
    fallback_repo: String,
    fallback_workflow: String,
    fallback_branch: String,
    deploy_meta: &GitHubActionsDeployMeta,
) -> GitHubActionsStatus {
    let (owner, repo) = deploy_meta
        .repository
        .as_deref()
        .and_then(parse_owner_repo)
        .unwrap_or((fallback_owner, fallback_repo));
    let workflow = non_empty_string(deploy_meta.workflow.clone()).unwrap_or(fallback_workflow);
    let head_branch =
        non_empty_string(deploy_meta.head_branch.clone()).or(Some(fallback_branch.clone()));
    let head_sha = non_empty_string(deploy_meta.head_sha.clone());
    let run_id = deploy_meta.run_id.as_deref().and_then(parse_optional_i64);
    let run_number = deploy_meta
        .run_number
        .as_deref()
        .and_then(parse_optional_i64);
    let run_attempt = deploy_meta
        .run_attempt
        .as_deref()
        .and_then(parse_optional_i64);
    let html_url = non_empty_string(deploy_meta.html_url.clone());

    GitHubActionsStatus {
        configured: false,
        source: "deploy_meta".to_string(),
        owner,
        repo,
        workflow,
        workflow_id: None,
        workflow_name: None,
        workflow_path: None,
        branch: fallback_branch,
        status: if run_id.is_some() {
            DeploySurfaceStatus::Match
        } else {
            DeploySurfaceStatus::Unknown
        },
        run_id,
        run_number,
        run_attempt,
        run_status: if run_id.is_some() {
            Some("completed".to_string())
        } else {
            None
        },
        conclusion: if run_id.is_some() {
            Some("success".to_string())
        } else {
            None
        },
        event: non_empty_string(deploy_meta.event.clone()),
        head_branch,
        head_sha_short: head_sha.as_deref().map(short_commit),
        head_sha,
        html_url,
        created_at: None,
        updated_at: None,
        run_started_at: None,
        error: Some(
            "GitHub Actions read token is not configured; using deploy metadata fallback"
                .to_string(),
        ),
    }
}

fn github_error_status(
    owner: String,
    repo: String,
    workflow: String,
    branch: String,
    configured: bool,
    error: String,
) -> GitHubActionsStatus {
    GitHubActionsStatus {
        configured,
        source: if configured {
            "error"
        } else {
            "not_configured"
        }
        .to_string(),
        owner,
        repo,
        workflow,
        workflow_id: None,
        workflow_name: None,
        workflow_path: None,
        branch,
        status: DeploySurfaceStatus::Unknown,
        run_id: None,
        run_number: None,
        run_attempt: None,
        run_status: None,
        conclusion: None,
        event: None,
        head_branch: None,
        head_sha: None,
        head_sha_short: None,
        html_url: None,
        created_at: None,
        updated_at: None,
        run_started_at: None,
        error: Some(error),
    }
}

fn cloudflare_error_status(
    project: String,
    configured: bool,
    error: String,
) -> CloudflarePagesStatus {
    CloudflarePagesStatus {
        configured,
        project,
        status: DeploySurfaceStatus::Unknown,
        deployment_id: None,
        environment: None,
        branch: None,
        commit: None,
        url: None,
        error: Some(error),
    }
}

fn redact_cloudflare_error(err: reqwest::Error, account_id: &str) -> String {
    err.to_string().replace(account_id, "[cloudflare-account]")
}

fn deployment_status_from_json(deployment: &Value) -> DeploySurfaceStatus {
    match json_string(deployment, &["latest_stage", "status"])
        .or_else(|| json_string(deployment, &["stages", "0", "status"]))
        .as_deref()
    {
        Some("success") => DeploySurfaceStatus::Match,
        Some(_) => DeploySurfaceStatus::Unknown,
        None => DeploySurfaceStatus::Unknown,
    }
}

fn deployment_branch(deployment: &Value) -> Option<String> {
    json_string(deployment, &["deployment_trigger", "metadata", "branch"])
        .or_else(|| json_string(deployment, &["source", "config", "branch"]))
}

fn deployment_commit(deployment: &Value) -> Option<String> {
    json_string(
        deployment,
        &["deployment_trigger", "metadata", "commit_hash"],
    )
    .or_else(|| json_string(deployment, &["source", "config", "commit_hash"]))
    .or_else(|| json_string(deployment, &["source", "config", "commit"]))
}

fn json_string(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for segment in path {
        if let Ok(index) = segment.parse::<usize>() {
            current = current.get(index)?;
        } else {
            current = current.get(*segment)?;
        }
    }
    current.as_str().map(ToString::to_string)
}

fn json_i64(value: &Value, path: &[&str]) -> Option<i64> {
    let mut current = value;
    for segment in path {
        if let Ok(index) = segment.parse::<usize>() {
            current = current.get(index)?;
        } else {
            current = current.get(*segment)?;
        }
    }
    current.as_i64()
}

fn parse_optional_i64(value: &str) -> Option<i64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        trimmed.parse::<i64>().ok()
    }
}

fn non_empty_string(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

async fn fetch_live_assets(public_url: &str) -> Result<FrontendAssets, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .user_agent("cortex-deploy-status/0.1")
        .build()
        .map_err(|err| err.to_string())?;
    let html = client
        .get(public_url)
        .header("Cache-Control", "no-cache")
        .send()
        .await
        .map_err(|err| err.to_string())?
        .error_for_status()
        .map_err(|err| err.to_string())?
        .text()
        .await
        .map_err(|err| err.to_string())?;
    Ok(parse_frontend_assets(&html))
}

pub fn parse_frontend_assets(html: &str) -> FrontendAssets {
    FrontendAssets {
        js: find_asset(html, "/assets/index-", ".js"),
        css: find_asset(html, "/assets/index-", ".css"),
    }
}

fn find_asset(html: &str, prefix: &str, suffix: &str) -> Option<String> {
    for (start, _) in html.match_indices(prefix) {
        let rest = &html[start..];
        let end = rest
            .find(|ch: char| ch == '"' || ch == '\'' || ch.is_whitespace() || ch == '>')
            .unwrap_or(rest.len());
        let candidate = &rest[..end];
        if candidate.ends_with(suffix) {
            return Some(candidate.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vite_index_assets() {
        let html = r#"
            <link rel="stylesheet" crossorigin href="/assets/index-DCTLbVzF.css">
            <script type="module" crossorigin src="/assets/index-8M8EThK7.js"></script>
        "#;

        let assets = parse_frontend_assets(html);

        assert_eq!(assets.js.as_deref(), Some("/assets/index-8M8EThK7.js"));
        assert_eq!(assets.css.as_deref(), Some("/assets/index-DCTLbVzF.css"));
    }

    #[test]
    fn parses_cloudflare_pages_deployment_metadata() {
        let deployment = serde_json::json!({
            "id": "deployment-123",
            "environment": "production",
            "url": "https://deployment.cortex.pages.dev",
            "latest_stage": { "status": "success" },
            "deployment_trigger": {
                "metadata": {
                    "branch": "main",
                    "commit_hash": "abc123"
                }
            }
        });

        assert_eq!(
            deployment_status_from_json(&deployment),
            DeploySurfaceStatus::Match
        );
        assert_eq!(deployment_branch(&deployment).as_deref(), Some("main"));
        assert_eq!(deployment_commit(&deployment).as_deref(), Some("abc123"));
        assert_eq!(
            json_string(&deployment, &["id"]).as_deref(),
            Some("deployment-123")
        );
    }

    #[test]
    fn parses_github_actions_run_metadata() {
        let run = serde_json::json!({
            "id": 26413241249i64,
            "run_number": 42,
            "run_attempt": 1,
            "status": "completed",
            "conclusion": "success",
            "event": "workflow_dispatch",
            "head_branch": "main",
            "head_sha": "15f5b31746b3547142b566cb08067ed80256c41b",
            "html_url": "https://github.com/hey-vera/heyvera/actions/runs/26413241249",
            "created_at": "2026-05-25T17:53:21Z",
            "updated_at": "2026-05-25T17:56:42Z",
            "run_started_at": "2026-05-25T17:53:24Z"
        });

        let status = github_status_from_run(
            "hey-vera".to_string(),
            "heyvera".to_string(),
            "Deploy Production".to_string(),
            test_resolved_workflow(),
            "main".to_string(),
            &run,
        );

        assert!(status.configured);
        assert_eq!(status.workflow_id, Some(259523968));
        assert_eq!(status.workflow_name.as_deref(), Some("Deploy Production"));
        assert_eq!(
            status.workflow_path.as_deref(),
            Some(".github/workflows/deploy-production.yml")
        );
        assert_eq!(status.status, DeploySurfaceStatus::Match);
        assert_eq!(status.run_id, Some(26413241249));
        assert_eq!(status.run_number, Some(42));
        assert_eq!(status.run_attempt, Some(1));
        assert_eq!(status.run_status.as_deref(), Some("completed"));
        assert_eq!(status.conclusion.as_deref(), Some("success"));
        assert_eq!(status.event.as_deref(), Some("workflow_dispatch"));
        assert_eq!(status.head_branch.as_deref(), Some("main"));
        assert_eq!(status.head_sha_short.as_deref(), Some("15f5b31"));
        assert_eq!(
            status.html_url.as_deref(),
            Some("https://github.com/hey-vera/heyvera/actions/runs/26413241249")
        );
    }

    #[test]
    fn github_actions_non_success_run_is_unknown_not_asset_drift() {
        let run = serde_json::json!({
            "id": 26413241249i64,
            "status": "completed",
            "conclusion": "failure",
            "head_sha": "15f5b31746b3547142b566cb08067ed80256c41b"
        });

        let status = github_status_from_run(
            "hey-vera".to_string(),
            "heyvera".to_string(),
            "Deploy Production".to_string(),
            test_resolved_workflow(),
            "main".to_string(),
            &run,
        );

        assert_eq!(status.status, DeploySurfaceStatus::Unknown);
        assert_eq!(status.conclusion.as_deref(), Some("failure"));
    }

    #[test]
    fn github_actions_can_fall_back_to_deploy_metadata_without_token() {
        let deploy_meta = GitHubActionsDeployMeta {
            workflow: Some("Deploy Production".to_string()),
            run_id: Some("26414165500".to_string()),
            run_number: Some("78".to_string()),
            run_attempt: Some("1".to_string()),
            event: Some("workflow_dispatch".to_string()),
            repository: Some("hey-vera/heyvera".to_string()),
            actor: Some("1xmint".to_string()),
            head_branch: Some("main".to_string()),
            head_sha: Some("07bab45d044ea4e73ab6e83532101c76490cb102".to_string()),
            html_url: Some(
                "https://github.com/hey-vera/heyvera/actions/runs/26414165500".to_string(),
            ),
        };

        let status = github_status_from_deploy_meta(
            "fallback".to_string(),
            "repo".to_string(),
            "Fallback Workflow".to_string(),
            "main".to_string(),
            &deploy_meta,
        );

        assert!(!status.configured);
        assert_eq!(status.source, "deploy_meta");
        assert_eq!(status.owner, "hey-vera");
        assert_eq!(status.repo, "heyvera");
        assert_eq!(status.status, DeploySurfaceStatus::Match);
        assert_eq!(status.run_id, Some(26414165500));
        assert_eq!(status.run_number, Some(78));
        assert_eq!(status.run_attempt, Some(1));
        assert_eq!(status.run_status.as_deref(), Some("completed"));
        assert_eq!(status.conclusion.as_deref(), Some("success"));
        assert_eq!(status.head_sha_short.as_deref(), Some("07bab45"));
        assert!(status
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("deploy metadata fallback"));
    }

    fn test_resolved_workflow() -> ResolvedGitHubWorkflow {
        ResolvedGitHubWorkflow {
            id: 259523968,
            name: Some("Deploy Production".to_string()),
            path: Some(".github/workflows/deploy-production.yml".to_string()),
        }
    }

    #[test]
    fn commit_alignment_matches_equal_commits() {
        let backend = test_backend(Some("abc123456789"), Some("main"));
        let frontend = test_frontend(Some("abc123456789"), Some("main"));

        let commits = build_commit_status(&backend, &frontend);

        assert_eq!(commits.status, CommitAlignmentStatus::Match);
        assert_eq!(commits.backend_commit_short.as_deref(), Some("abc1234"));
        assert_eq!(commits.frontend_commit_short.as_deref(), Some("abc1234"));
        assert_eq!(commits.branch_match, Some(true));
    }

    #[test]
    fn commit_alignment_reports_mismatched_commits() {
        let backend = test_backend(Some("abc123456789"), Some("main"));
        let frontend = test_frontend(Some("def987654321"), Some("main"));

        let commits = build_commit_status(&backend, &frontend);

        assert_eq!(commits.status, CommitAlignmentStatus::Mismatch);
        assert_eq!(commits.branch_match, Some(true));
    }

    #[test]
    fn commit_alignment_is_unknown_when_a_surface_is_missing_commit() {
        let backend = test_backend(Some("abc123456789"), Some("main"));
        let frontend = test_frontend(None, Some("main"));

        let commits = build_commit_status(&backend, &frontend);

        assert_eq!(commits.status, CommitAlignmentStatus::Unknown);
        assert_eq!(commits.branch_match, Some(true));
    }

    #[test]
    fn deployment_event_fingerprint_changes_for_new_release_evidence() {
        let first = test_deploy_response(
            "abc123456789",
            "deployment-123",
            Some(42),
            "/assets/index-a.js",
        );
        let same = test_deploy_response(
            "abc123456789",
            "deployment-123",
            Some(42),
            "/assets/index-a.js",
        );
        let next_run = test_deploy_response(
            "abc123456789",
            "deployment-123",
            Some(43),
            "/assets/index-a.js",
        );
        let next_asset = test_deploy_response(
            "abc123456789",
            "deployment-123",
            Some(42),
            "/assets/index-b.js",
        );

        assert_eq!(
            deployment_event_fingerprint(&first),
            deployment_event_fingerprint(&same)
        );
        assert_ne!(
            deployment_event_fingerprint(&first),
            deployment_event_fingerprint(&next_run)
        );
        assert_ne!(
            deployment_event_fingerprint(&first),
            deployment_event_fingerprint(&next_asset)
        );
    }

    #[test]
    fn deployment_event_payload_contains_release_evidence() {
        let response = test_deploy_response(
            "afb8c255913910d0d84e5a126dca3ac9bed0015e",
            "deployment-123",
            Some(26414597373),
            "/assets/index-Bu3uhHyL.js",
        );

        let payload = deployment_event_payload(&response);

        assert_eq!(payload["status"], "match");
        assert_eq!(
            payload["backend"]["commit"],
            "afb8c255913910d0d84e5a126dca3ac9bed0015e"
        );
        assert_eq!(
            payload["frontend"]["cloudflare_pages"]["deployment_id"],
            "deployment-123"
        );
        assert_eq!(payload["github_actions"]["run_id"], 26414597373i64);
        assert_eq!(payload["commits"]["status"], "match");
    }

    fn test_backend(commit: Option<&str>, branch: Option<&str>) -> BackendDeployStatus {
        BackendDeployStatus {
            service: "cortex",
            version: "0.1.0",
            commit: commit.map(ToString::to_string),
            commit_short: None,
            branch: branch.map(ToString::to_string),
            deployed_at: None,
        }
    }

    fn test_frontend(commit: Option<&str>, branch: Option<&str>) -> FrontendDeployStatus {
        FrontendDeployStatus {
            public_url: "https://cortex.heyvera.org/".to_string(),
            local_root: "/var/www/cortex".to_string(),
            expected_source: "cloudflare_pages".to_string(),
            expected_assets: FrontendAssets {
                js: Some("/assets/index-test.js".to_string()),
                css: Some("/assets/index-test.css".to_string()),
            },
            live_assets: Some(FrontendAssets {
                js: Some("/assets/index-test.js".to_string()),
                css: Some("/assets/index-test.css".to_string()),
            }),
            status: DeploySurfaceStatus::Match,
            drift: false,
            checked_at: "2026-05-25T00:00:00Z".to_string(),
            error: None,
            cloudflare_pages: CloudflarePagesStatus {
                configured: true,
                project: "cortex".to_string(),
                status: DeploySurfaceStatus::Match,
                deployment_id: Some("deployment-123".to_string()),
                environment: Some("production".to_string()),
                branch: branch.map(ToString::to_string),
                commit: commit.map(ToString::to_string),
                url: Some("https://deployment.cortex.pages.dev".to_string()),
                error: None,
            },
        }
    }

    fn test_deploy_response(
        commit: &str,
        cloudflare_deployment_id: &str,
        run_id: Option<i64>,
        live_js: &str,
    ) -> DeployStatusResponse {
        let backend = test_backend(Some(commit), Some("main"));
        let mut frontend = test_frontend(Some(commit), Some("main"));
        frontend.live_assets = Some(FrontendAssets {
            js: Some(live_js.to_string()),
            css: Some("/assets/index-test.css".to_string()),
        });
        frontend.cloudflare_pages.deployment_id = Some(cloudflare_deployment_id.to_string());
        let commits = build_commit_status(&backend, &frontend);
        DeployStatusResponse {
            status: DeploySurfaceStatus::Match,
            service: "cortex",
            observed_at: "2026-05-25T00:00:00Z".to_string(),
            generated_at: "2026-05-25T00:00:00Z".to_string(),
            backend,
            frontend,
            github_actions: GitHubActionsStatus {
                configured: false,
                source: "deploy_meta".to_string(),
                owner: "hey-vera".to_string(),
                repo: "heyvera".to_string(),
                workflow: "Deploy Production".to_string(),
                workflow_id: None,
                workflow_name: None,
                workflow_path: None,
                branch: "main".to_string(),
                status: DeploySurfaceStatus::Match,
                run_id,
                run_number: run_id,
                run_attempt: Some(1),
                run_status: Some("completed".to_string()),
                conclusion: Some("success".to_string()),
                event: Some("workflow_dispatch".to_string()),
                head_branch: Some("main".to_string()),
                head_sha: Some(commit.to_string()),
                head_sha_short: Some(short_commit(commit)),
                html_url: run_id
                    .map(|id| format!("https://github.com/hey-vera/heyvera/actions/runs/{id}")),
                created_at: None,
                updated_at: None,
                run_started_at: None,
                error: None,
            },
            commits,
        }
    }
}
