use std::path::Path;

use serde::{Deserialize, Serialize};

/// Lightweight GitHub API client for PR creation.
/// Falls back to `gh` CLI if no token is available.
pub struct GitHubClient {
    token: String,
    client: reqwest::Client,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PrResponse {
    pub html_url: String,
    pub number: i64,
}

impl GitHubClient {
    /// Create a client from the `GITHUB_TOKEN` environment variable.
    pub fn from_env() -> Option<Self> {
        let token = std::env::var("GITHUB_TOKEN").ok()?;
        if token.is_empty() {
            return None;
        }
        let client = reqwest::Client::builder()
            .user_agent("cortex-api")
            .build()
            .ok()?;
        Some(Self { token, client })
    }

    /// Create a pull request via the GitHub REST API.
    pub async fn create_pull_request(
        &self,
        owner: &str,
        repo: &str,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
    ) -> Result<PrResponse, String> {
        let url = format!("https://api.github.com/repos/{owner}/{repo}/pulls");

        let payload = serde_json::json!({
            "title": title,
            "body": body,
            "head": head,
            "base": base,
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("GitHub API request failed: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("GitHub API returned {status}: {text}"));
        }

        resp.json::<PrResponse>()
            .await
            .map_err(|e| format!("failed to parse GitHub PR response: {e}"))
    }
}

/// Fetch a user's GitHub OAuth access token from Clerk.
///
/// Clerk brokers the GitHub OAuth connection, so the token is retrieved from
/// Clerk's `oauth_access_tokens` endpoint rather than stored locally. Returns
/// `Ok(None)` when Clerk is not configured or the user hasn't linked GitHub.
pub async fn github_oauth_token(
    clerk_secret_key: Option<&str>,
    user_id: &str,
) -> Result<Option<String>, String> {
    let Some(clerk_secret) = clerk_secret_key else {
        return Ok(None);
    };
    let client = reqwest::Client::new();
    let token_res = client
        .get(format!(
            "https://api.clerk.com/v1/users/{user_id}/oauth_access_tokens/oauth_github"
        ))
        .bearer_auth(clerk_secret)
        .send()
        .await
        .map_err(|e| format!("Clerk API error: {e}"))?;

    if !token_res.status().is_success() {
        return Ok(None);
    }

    let tokens: Vec<serde_json::Value> = token_res.json().await.unwrap_or_default();
    Ok(tokens
        .first()
        .and_then(|t| t.get("token"))
        .and_then(|t| t.as_str())
        .map(String::from))
}

/// Validate that a GitHub `owner/repo` slug is well-formed and safe to embed in
/// a shell command or filesystem path. Rejects anything outside the GitHub
/// naming charset to prevent command/path injection.
pub fn is_valid_repo_full_name(full_name: &str) -> bool {
    let mut parts = full_name.splitn(2, '/');
    let (owner, repo) = match (parts.next(), parts.next()) {
        (Some(o), Some(r)) => (o, r),
        _ => return false,
    };
    if owner.is_empty() || repo.is_empty() || full_name.len() > 200 {
        return false;
    }
    let valid_segment = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            && s != "."
            && s != ".."
    };
    valid_segment(owner) && valid_segment(repo)
}

/// Parse `owner` and `repo` from a git remote URL.
///
/// Supports both SSH (`git@github.com:owner/repo.git`) and HTTPS
/// (`https://github.com/owner/repo.git`) formats.
pub fn parse_github_remote(workspace_dir: &Path) -> Option<(String, String)> {
    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(workspace_dir)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    parse_owner_repo(&url)
}

/// Extract `(owner, repo)` from a GitHub remote URL string.
fn parse_owner_repo(url: &str) -> Option<(String, String)> {
    // SSH: git@github.com:owner/repo.git
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        let rest = rest.strip_suffix(".git").unwrap_or(rest);
        let mut parts = rest.splitn(2, '/');
        let owner = parts.next()?.to_string();
        let repo = parts.next()?.to_string();
        if !owner.is_empty() && !repo.is_empty() {
            return Some((owner, repo));
        }
    }

    // HTTPS: https://github.com/owner/repo.git
    if url.contains("github.com/") {
        let after = url.split("github.com/").nth(1)?;
        let after = after.strip_suffix(".git").unwrap_or(after);
        let mut parts = after.splitn(2, '/');
        let owner = parts.next()?.to_string();
        let repo = parts.next()?.to_string();
        if !owner.is_empty() && !repo.is_empty() {
            return Some((owner, repo));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ssh_remote() {
        let (owner, repo) = parse_owner_repo("git@github.com:acme/widgets.git").unwrap();
        assert_eq!(owner, "acme");
        assert_eq!(repo, "widgets");
    }

    #[test]
    fn parse_https_remote() {
        let (owner, repo) = parse_owner_repo("https://github.com/acme/widgets.git").unwrap();
        assert_eq!(owner, "acme");
        assert_eq!(repo, "widgets");
    }

    #[test]
    fn parse_https_no_dotgit() {
        let (owner, repo) = parse_owner_repo("https://github.com/acme/widgets").unwrap();
        assert_eq!(owner, "acme");
        assert_eq!(repo, "widgets");
    }

    #[test]
    fn parse_invalid_url() {
        assert!(parse_owner_repo("https://gitlab.com/foo/bar").is_none());
        assert!(parse_owner_repo("not-a-url").is_none());
    }

    #[test]
    fn valid_repo_names() {
        assert!(is_valid_repo_full_name("hey-vera/cortex"));
        assert!(is_valid_repo_full_name("octocat/Hello-World.js"));
    }

    #[test]
    fn rejects_injection_and_traversal() {
        assert!(!is_valid_repo_full_name("foo/bar; rm -rf /"));
        assert!(!is_valid_repo_full_name("../../etc/passwd"));
        assert!(!is_valid_repo_full_name("owner/.."));
        assert!(!is_valid_repo_full_name("noslash"));
        assert!(!is_valid_repo_full_name("owner/"));
        assert!(!is_valid_repo_full_name("/repo"));
        assert!(!is_valid_repo_full_name("a b/c"));
    }
}
