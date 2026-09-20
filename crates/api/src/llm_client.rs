use serde::{Deserialize, Serialize};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use uuid::Uuid;

/// Resolve the CLI binary path from an env var, falling back to a default name.
/// Logs a warning on first use if the binary cannot be found on $PATH.
fn resolve_cli_path(env_var: &str, default: &str) -> String {
    std::env::var(env_var).unwrap_or_else(|_| default.to_string())
}

/// Check that a CLI binary exists and warn once if not.
/// Also checks for authentication and provides helpful setup guidance.
fn check_cli_exists(binary: &str, label: &str) {
    static CLAUDE_CHECKED: OnceLock<()> = OnceLock::new();
    static CODEX_CHECKED: OnceLock<()> = OnceLock::new();

    let lock = match label {
        "claude" => &CLAUDE_CHECKED,
        _ => &CODEX_CHECKED,
    };

    lock.get_or_init(|| {
        if which::which(binary).is_err() {
            tracing::warn!(
                binary = binary,
                "'{binary}' not found on $PATH. BYOS chat will fail. \
                 Install {label} CLI or set the override env var \
                 (CORTEX_CLAUDE_PATH / CORTEX_CODEX_PATH) to the correct path."
            );
            log_installation_guidance(label);
        } else {
            tracing::info!(binary = binary, "{label} CLI found");
            // Check authentication asynchronously
            tokio::spawn(check_cli_auth_async(binary.to_string(), label.to_string()));
        }
    });
}

/// Log installation guidance for missing CLI tools
fn log_installation_guidance(label: &str) {
    match label {
        "claude" => {
            tracing::info!("To install Claude CLI: npm install -g @anthropic-ai/claude-cli");
            if std::env::var("ANTHROPIC_API_KEY").is_ok() {
                tracing::info!(
                    "ANTHROPIC_API_KEY detected - will auto-configure auth after install"
                );
            } else {
                tracing::info!("Set ANTHROPIC_API_KEY environment variable for headless auth");
            }
        }
        "codex" => {
            tracing::info!("To install Codex CLI: npm install -g @openai/codex");
            if std::env::var("OPENAI_API_KEY").is_ok() {
                tracing::info!("OPENAI_API_KEY detected - will auto-configure auth after install");
            } else {
                tracing::info!("Set OPENAI_API_KEY environment variable for headless auth");
            }
        }
        _ => {}
    }
}

/// Check CLI authentication status asynchronously
async fn check_cli_auth_async(_binary: String, label: String) {
    let auth_status = match label.as_str() {
        "claude" => check_claude_auth_status().await,
        "codex" => check_codex_auth_status().await,
        _ => AuthStatus::Unknown,
    };

    match auth_status {
        AuthStatus::Authenticated => {
            tracing::info!("{label} CLI authenticated and ready");
        }
        AuthStatus::NotAuthenticated => {
            tracing::warn!("{label} CLI found but not authenticated. BYOS chat will fail.");
            log_auth_guidance(&label);
        }
        AuthStatus::Unknown => {
            tracing::warn!("Could not determine {label} CLI auth status");
        }
    }
}

#[derive(Debug, Clone)]
enum AuthStatus {
    Authenticated,
    NotAuthenticated,
    Unknown,
}

/// Check Claude CLI authentication status
async fn check_claude_auth_status() -> AuthStatus {
    match tokio::process::Command::new("claude")
        .args(["auth", "status", "--json"])
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("\"loggedIn\":true") {
                AuthStatus::Authenticated
            } else {
                AuthStatus::NotAuthenticated
            }
        }
        _ => AuthStatus::Unknown,
    }
}

/// Check Codex CLI authentication status
async fn check_codex_auth_status() -> AuthStatus {
    match tokio::process::Command::new("codex")
        .args(["login", "status"])
        .output()
        .await
    {
        Ok(output) => {
            let all_output = format!(
                "{} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            if all_output.contains("Logged in") {
                AuthStatus::Authenticated
            } else {
                AuthStatus::NotAuthenticated
            }
        }
        _ => AuthStatus::Unknown,
    }
}

/// Log authentication setup guidance
fn log_auth_guidance(label: &str) {
    match label {
        "claude" => {
            if std::env::var("ANTHROPIC_API_KEY").is_ok() {
                tracing::info!(
                    "Run: bash scripts/setup-headless-auth.sh to auto-configure Claude auth"
                );
            } else {
                tracing::info!("To authenticate Claude:");
                tracing::info!("  1. Set ANTHROPIC_API_KEY environment variable, or");
                tracing::info!("  2. Run: claude auth login (requires browser)");
            }
        }
        "codex" => {
            if std::env::var("OPENAI_API_KEY").is_ok() {
                tracing::info!(
                    "Run: bash scripts/setup-headless-auth.sh to auto-configure Codex auth"
                );
            } else {
                tracing::info!("To authenticate Codex:");
                tracing::info!("  1. Set OPENAI_API_KEY environment variable, or");
                tracing::info!("  2. Run: codex login --device-auth");
            }
        }
        _ => {}
    }
}

fn cli_timeout() -> Duration {
    let secs: u64 = std::env::var("CORTEX_CLI_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    Duration::from_secs(secs)
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Copy)]
pub enum Provider {
    Claude,
    Openai,
    /// OpenCode Zen — OpenAI-compatible gateway (GLM, Kimi, Qwen, Grok under
    /// one key). API-only: no CLI, no subscription auth. Every CLI path below
    /// must reject it; only the API paths may serve it.
    Zen,
}

impl Provider {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "claude" | "anthropic" => Some(Self::Claude),
            "openai" | "codex" | "gpt" => Some(Self::Openai),
            "zen" | "opencode" | "opencode-zen" => Some(Self::Zen),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Openai => "openai",
            Self::Zen => "zen",
        }
    }

    pub fn default_model(&self) -> &'static str {
        match self {
            Self::Claude => "claude-sonnet-4-6",
            Self::Openai => "gpt-4.1-mini",
            Self::Zen => "glm-5.2",
        }
    }

    /// Environment variable holding the OPERATOR's API key for this provider.
    pub fn api_key_env(&self) -> &'static str {
        match self {
            Self::Claude => "ANTHROPIC_API_KEY",
            Self::Openai => "OPENAI_API_KEY",
            Self::Zen => "OPENCODE_ZEN_API_KEY",
        }
    }
}

/// OpenAI-compatible chat-completions endpoints, one per provider that
/// speaks the dialect. Claude is deliberately absent — it uses /v1/messages.
const OPENAI_CHAT_COMPLETIONS_URL: &str = "https://api.openai.com/v1/chat/completions";
const ZEN_CHAT_COMPLETIONS_URL: &str = "https://opencode.ai/zen/v1/chat/completions";

// --- tmpfs credential isolation ---

struct TmpfsCredentialDir {
    path: std::path::PathBuf,
}

impl TmpfsCredentialDir {
    fn create(user_id: &str) -> Result<Self, String> {
        let dir_name = format!("cortex-{}-{}", user_id, Uuid::new_v4().simple());
        let path = std::path::PathBuf::from("/dev/shm").join(&dir_name);

        std::fs::create_dir_all(&path)
            .map_err(|e| format!("failed to create tmpfs dir {}: {e}", path.display()))?;

        // Restrict permissions to owner only
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| format!("failed to set tmpfs permissions: {e}"))?;
        }

        Ok(Self { path })
    }

    fn write_credential_file(&self, filename: &str, content: &str) -> Result<(), String> {
        let file_path = self.path.join(filename);
        std::fs::write(&file_path, content)
            .map_err(|e| format!("failed to write credential to tmpfs: {e}"))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("failed to set credential file permissions: {e}"))?;
        }

        Ok(())
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TmpfsCredentialDir {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.path) {
            tracing::warn!(
                "failed to clean up tmpfs credential dir {}: {e}",
                self.path.display()
            );
        } else {
            tracing::debug!("cleaned up tmpfs credential dir {}", self.path.display());
        }
    }
}

/// Stream a chat response using the CLI tools (BYOS — uses subscription auth).
/// Calls `claude` or `codex` as a subprocess, streaming output line by line.
/// Falls back to API mode if CLI tools are not available/authenticated.
pub async fn stream_chat_cli(
    provider: &Provider,
    model: Option<&str>,
    system_prompt: &str,
    user_message: &str,
    tx: mpsc::Sender<String>,
) -> Result<(), String> {
    // Check if we should try API fallback for this provider.
    //
    // ANTHROPIC_API_KEY / OPENAI_API_KEY here are the OPERATOR's keys, not the
    // caller's. This is the subscription path, so falling back to them means
    // Cortex pays for a request the user intended to run on their own
    // subscription — triggered precisely when that user's auth is broken.
    //
    // Nothing on this path calls check_budget_before_request (chat.rs only
    // budgets ProviderPath::ApiKey), no cost session is opened, and
    // CostEstimator::get_cost_type has no value to record operator spend under.
    // So this is uncapped, unattributed, and invisible.
    //
    // It is currently dormant only because neither key is set in production.
    // It arms the moment they are. Require an explicit opt-in so that adding
    // keys — for the credit product, on a different code path — does not
    // silently turn user auth failures into operator spend.
    let fallback_enabled = std::env::var("CORTEX_PLATFORM_FALLBACK")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let api_key_available = fallback_enabled && std::env::var(provider.api_key_env()).is_ok();

    let result = match provider {
        Provider::Claude => stream_claude_cli(model, system_prompt, user_message, tx.clone()).await,
        Provider::Openai => stream_codex_cli(model, system_prompt, user_message, tx.clone()).await,
        // API-only: falls through to the fallback arm below, which is the
        // only way a Zen request can be served on this path.
        Provider::Zen => Err("OpenCode Zen is API-only — no CLI to spawn".to_string()),
    };

    // If CLI failed and operator-funded fallback is explicitly enabled, use it.
    if let Err(cli_error) = result {
        if api_key_available {
            // Logged at warn, not debug: every line here is money leaving the
            // operator's account for a request that was supposed to be on the
            // user's own subscription. Until platform spend is metered, this
            // log is the only record that it happened.
            tracing::warn!(
                provider = provider.name(),
                cost_type = "platform",
                "CLI auth failed; falling back to OPERATOR-FUNDED API key (unmetered): {cli_error}"
            );

            let api_key = std::env::var(provider.api_key_env()).unwrap();

            let messages = vec![ChatMessage {
                role: "user".to_string(),
                content: user_message.to_string(),
            }];

            stream_chat_api(provider, &api_key, model, system_prompt, &messages, tx).await
        } else {
            if matches!(provider, Provider::Zen) {
                return Err(format!(
                    "{cli_error} - Set OPENCODE_ZEN_API_KEY together with \
                     CORTEX_PLATFORM_FALLBACK=1 to run Zen models on the \
                     operator's key, or supply a Zen key via the BYOK API path."
                ));
            }
            let (env_var, cli_login) = match provider {
                Provider::Claude => ("ANTHROPIC_API_KEY", "claude auth login"),
                Provider::Openai => ("OPENAI_API_KEY", "codex login --device-auth"),
                Provider::Zen => unreachable!("returned above: Zen has no CLI"),
            };
            // Distinguish "no key" from "key present but fallback withheld", so
            // the operator is not told to set a variable that is already set.
            if std::env::var(env_var).is_ok() {
                Err(format!(
                    "{cli_error} - Operator-funded fallback is disabled. \
                     Re-authenticate the CLI with `{cli_login}`, or set \
                     CORTEX_PLATFORM_FALLBACK=1 to bill this request to the \
                     operator's {env_var}."
                ))
            } else {
                Err(format!(
                    "{cli_error} - No API key available for fallback. \
                     Authenticate the CLI with `{cli_login}`, or set {env_var} \
                     together with CORTEX_PLATFORM_FALLBACK=1."
                ))
            }
        }
    } else {
        result
    }
}

/// Stream a chat response using CLI tools with tmpfs credential isolation.
/// Decrypted credentials are written to a tmpfs directory, CLI is spawned with
/// HOME pointing there, and the directory is wiped after the process exits.
pub async fn stream_chat_cli_isolated(
    provider: &Provider,
    model: Option<&str>,
    credential_data: &str,
    user_id: &str,
    system_prompt: &str,
    user_message: &str,
    tx: mpsc::Sender<String>,
) -> Result<(), String> {
    let tmpdir = TmpfsCredentialDir::create(user_id)?;

    // Write credential data to tmpfs based on provider
    match provider {
        Provider::Claude => {
            // Claude CLI expects credentials in ~/.claude/ or ANTHROPIC_API_KEY
            // For subscription auth, write the session data
            let claude_dir = tmpdir.path().join(".claude");
            std::fs::create_dir_all(&claude_dir)
                .map_err(|e| format!("failed to create .claude dir: {e}"))?;
            tmpdir.write_credential_file(".claude/credentials.json", credential_data)?;
        }
        Provider::Openai => {
            // Codex CLI uses OPENAI_API_KEY or ~/.codex/auth.json
            let codex_dir = tmpdir.path().join(".codex");
            std::fs::create_dir_all(&codex_dir)
                .map_err(|e| format!("failed to create .codex dir: {e}"))?;
            tmpdir.write_credential_file(".codex/auth.json", credential_data)?;
        }
        Provider::Zen => {
            return Err("OpenCode Zen is API-only — no CLI credentials to isolate".to_string());
        }
    }

    let model_str = model.unwrap_or(provider.default_model());
    let prompt = if system_prompt.is_empty() {
        user_message.to_string()
    } else {
        format!("{system_prompt}\n\n{user_message}")
    };

    let timeout = cli_timeout();
    let home_path = tmpdir.path().to_string_lossy().to_string();

    let result = match provider {
        Provider::Claude => {
            let cli_path = resolve_cli_path("CORTEX_CLAUDE_PATH", "claude");
            let mut cmd = Command::new(&cli_path);
            cmd.args([
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--no-session-persistence",
                "--model",
                model_str,
            ])
            .arg(&prompt)
            .env("HOME", &home_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

            spawn_and_stream_cli(cmd, &cli_path, "claude", timeout, tx).await
        }
        Provider::Openai => {
            let cli_path = resolve_cli_path("CORTEX_CODEX_PATH", "codex");
            let mut cmd = Command::new(&cli_path);
            cmd.args([
                "exec",
                "-c",
                &format!("model={model_str}"),
                "-c",
                "approval_policy=never",
            ])
            .arg(&prompt)
            .env("HOME", &home_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

            spawn_and_stream_codex(cmd, &cli_path, timeout, tx).await
        }
        Provider::Zen => unreachable!("rejected above: Zen has no CLI"),
    };

    // tmpdir drops here, wiping credentials from tmpfs
    // (Drop impl handles cleanup even if result is Err)

    result
}

pub async fn stream_chat_via_container(
    provider: &Provider,
    model: &str,
    system_prompt: &str,
    user_message: &str,
    container_manager: &crate::docker::ContainerManager,
    container_id: &str,
    tx: mpsc::Sender<String>,
) -> Result<(), String> {
    let prompt = if system_prompt.is_empty() {
        user_message.to_string()
    } else {
        format!("{system_prompt}\n\n{user_message}")
    };

    let timeout = cli_timeout();

    let cmd: Vec<String> = match provider {
        Provider::Zen => {
            return Err("OpenCode Zen is API-only — it cannot run in a CLI container".to_string())
        }
        Provider::Claude => vec![
            "claude".into(),
            "-p".into(),
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
            "--no-session-persistence".into(),
            "--model".into(),
            model.into(),
            prompt,
        ],
        Provider::Openai => vec![
            "codex".into(),
            "exec".into(),
            "-c".into(),
            format!("model={model}"),
            "-c".into(),
            "approval_policy=never".into(),
            prompt,
        ],
    };

    let cmd_refs: Vec<&str> = cmd.iter().map(|s| s.as_str()).collect();

    // For Claude, we get stream-json and need to parse it
    // For Codex, raw lines are fine
    let (raw_tx, mut raw_rx) = mpsc::channel::<String>(64);

    let parse_handle = {
        let provider = *provider;
        let tx = tx.clone();
        tokio::spawn(async move {
            match provider {
                Provider::Claude => {
                    while let Some(chunk) = raw_rx.recv().await {
                        for line in chunk.lines() {
                            let trimmed = line.trim();
                            if trimmed.is_empty() {
                                continue;
                            }
                            if let Ok(event) = serde_json::from_str::<serde_json::Value>(trimmed) {
                                let text = event
                                    .get("result")
                                    .and_then(|r| r.get("content"))
                                    .and_then(|c| c.as_array())
                                    .and_then(|arr| {
                                        arr.iter()
                                            .find_map(|b| b.get("text").and_then(|t| t.as_str()))
                                    })
                                    .or_else(|| {
                                        event
                                            .get("delta")
                                            .and_then(|d| d.get("text").and_then(|t| t.as_str()))
                                    })
                                    .or_else(|| {
                                        event
                                            .get("message")
                                            .and_then(|m| m.get("content"))
                                            .and_then(|c| c.as_array())
                                            .and_then(|arr| {
                                                arr.iter().find_map(|b| {
                                                    b.get("text").and_then(|t| t.as_str())
                                                })
                                            })
                                    });
                                if let Some(text) = text {
                                    if tx.send(text.to_string()).await.is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }
                // Zen is unreachable (rejected before spawn); raw lines is the
                // safe behavior for any OpenAI-compatible output regardless.
                Provider::Openai | Provider::Zen => {
                    while let Some(chunk) = raw_rx.recv().await {
                        for line in chunk.lines() {
                            let trimmed = line.trim();
                            if !trimmed.is_empty() && tx.send(format!("{trimmed}\n")).await.is_err()
                            {
                                return;
                            }
                        }
                    }
                }
            }
        })
    };

    let exit_code = container_manager
        .exec_stream(container_id, &cmd_refs, raw_tx, timeout)
        .await?;
    drop(tx); // signal parser we're done
    let _ = parse_handle.await;

    if exit_code != 0 {
        return Err(format!(
            "{} CLI exited with code {exit_code}",
            provider.name()
        ));
    }
    Ok(())
}

async fn spawn_and_stream_cli(
    mut cmd: Command,
    cli_path: &str,
    label: &str,
    timeout: Duration,
    tx: mpsc::Sender<String>,
) -> Result<(), String> {
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn {label} CLI at '{cli_path}': {e}"))?;

    let stdout = child.stdout.take().ok_or("failed to capture stdout")?;

    let stream_fut = async {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(event) = serde_json::from_str::<ClaudeStreamEvent>(&line) {
                match event {
                    ClaudeStreamEvent::Assistant { message } => {
                        if let Some(content) = extract_claude_text(&message) {
                            if tx.send(content).await.is_err() {
                                break;
                            }
                        }
                    }
                    ClaudeStreamEvent::ContentBlockDelta { delta } => {
                        if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                            if tx.send(text.to_string()).await.is_err() {
                                break;
                            }
                        }
                    }
                    ClaudeStreamEvent::Result { result } => {
                        if let Some(text) = extract_claude_text(&result) {
                            if tx.send(text).await.is_err() {
                                break;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok::<(), String>(())
    };

    match tokio::time::timeout(timeout, stream_fut).await {
        Ok(result) => {
            result?;
            let status = child
                .wait()
                .await
                .map_err(|e| format!("{label} process error: {e}"))?;
            if !status.success() {
                return Err(format!("{label} exited with status {status}"));
            }
            Ok(())
        }
        Err(_) => {
            let _ = child.kill().await;
            Err(format!(
                "{label} CLI timed out after {} seconds",
                timeout.as_secs()
            ))
        }
    }
}

async fn spawn_and_stream_codex(
    mut cmd: Command,
    cli_path: &str,
    timeout: Duration,
    tx: mpsc::Sender<String>,
) -> Result<(), String> {
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn codex CLI at '{cli_path}': {e}"))?;

    let stdout = child.stdout.take().ok_or("failed to capture stdout")?;

    let stream_fut = async {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            let trimmed = line.trim();
            if !trimmed.is_empty() && tx.send(format!("{trimmed}\n")).await.is_err() {
                break;
            }
        }
        Ok::<(), String>(())
    };

    match tokio::time::timeout(timeout, stream_fut).await {
        Ok(result) => {
            result?;
            let status = child
                .wait()
                .await
                .map_err(|e| format!("codex process error: {e}"))?;
            if !status.success() {
                return Err(format!("codex exited with status {status}"));
            }
            Ok(())
        }
        Err(_) => {
            let _ = child.kill().await;
            Err(format!(
                "codex CLI timed out after {} seconds",
                timeout.as_secs()
            ))
        }
    }
}

async fn stream_claude_cli(
    model: Option<&str>,
    system_prompt: &str,
    user_message: &str,
    tx: mpsc::Sender<String>,
) -> Result<(), String> {
    let model = model.unwrap_or("claude-sonnet-4-6");
    let cli_path = resolve_cli_path("CORTEX_CLAUDE_PATH", "claude");
    check_cli_exists(&cli_path, "claude");

    let prompt = if system_prompt.is_empty() {
        user_message.to_string()
    } else {
        format!("{system_prompt}\n\n{user_message}")
    };

    let mut cmd = Command::new(&cli_path);
    cmd.args([
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--no-session-persistence",
        "--model",
        model,
    ])
    .arg(&prompt)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!(
            "Claude CLI not found at '{cli_path}'. Install it with: \
                 npm install -g @anthropic-ai/claude-cli — or set CORTEX_CLAUDE_PATH \
                 to the full path of the binary. For headless auth, set ANTHROPIC_API_KEY \
                 and run: bash scripts/setup-headless-auth.sh"
        ),
        std::io::ErrorKind::PermissionDenied => format!(
            "Permission denied running '{cli_path}'. Check file permissions \
                 (chmod +x) or set CORTEX_CLAUDE_PATH to an accessible binary."
        ),
        _ => format!("Failed to spawn claude CLI at '{cli_path}': {e}"),
    })?;

    let stdout = child.stdout.take().ok_or("failed to capture stdout")?;
    let timeout = cli_timeout();

    let stream_fut = async {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(event) = serde_json::from_str::<ClaudeStreamEvent>(&line) {
                match event {
                    ClaudeStreamEvent::Assistant { message } => {
                        if let Some(content) = extract_claude_text(&message) {
                            if tx.send(content).await.is_err() {
                                break;
                            }
                        }
                    }
                    ClaudeStreamEvent::ContentBlockDelta { delta } => {
                        if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                            if tx.send(text.to_string()).await.is_err() {
                                break;
                            }
                        }
                    }
                    ClaudeStreamEvent::Result { result } => {
                        if let Some(text) = extract_claude_text(&result) {
                            if tx.send(text).await.is_err() {
                                break;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok::<(), String>(())
    };

    match tokio::time::timeout(timeout, stream_fut).await {
        Ok(result) => {
            result?;
            let status = child
                .wait()
                .await
                .map_err(|e| format!("claude process error: {e}"))?;
            if !status.success() {
                let error_msg = format!("claude exited with status {status}");
                tracing::error!("{error_msg}");

                // Check if this might be an auth issue
                if status.code() == Some(1) {
                    return Err(format!(
                        "{error_msg} - This may indicate authentication failure. \
                         Check auth with: claude auth status --json. \
                         For headless setup: bash scripts/setup-headless-auth.sh"
                    ));
                }
                return Err(error_msg);
            }
            Ok(())
        }
        Err(_) => {
            tracing::error!(
                timeout_secs = timeout.as_secs(),
                "claude CLI timed out — killing process"
            );
            let _ = child.kill().await;
            Err(format!(
                "Claude CLI timed out after {} seconds. Increase CORTEX_CLI_TIMEOUT_SECS \
                 or simplify the prompt.",
                timeout.as_secs()
            ))
        }
    }
}

async fn stream_codex_cli(
    model: Option<&str>,
    system_prompt: &str,
    user_message: &str,
    tx: mpsc::Sender<String>,
) -> Result<(), String> {
    let model = model.unwrap_or("gpt-4.1-mini");
    let cli_path = resolve_cli_path("CORTEX_CODEX_PATH", "codex");
    check_cli_exists(&cli_path, "codex");

    let prompt = if system_prompt.is_empty() {
        user_message.to_string()
    } else {
        format!("{system_prompt}\n\n{user_message}")
    };

    let mut cmd = Command::new(&cli_path);
    cmd.args([
        "exec",
        "-c",
        &format!("model={model}"),
        "-c",
        "approval_policy=never",
    ])
    .arg(&prompt)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!(
            "Codex CLI not found at '{cli_path}'. Install it with: \
                 npm install -g @openai/codex — or set CORTEX_CODEX_PATH \
                 to the full path of the binary. For headless auth, set OPENAI_API_KEY \
                 and run: bash scripts/setup-headless-auth.sh"
        ),
        std::io::ErrorKind::PermissionDenied => format!(
            "Permission denied running '{cli_path}'. Check file permissions \
                 (chmod +x) or set CORTEX_CODEX_PATH to an accessible binary."
        ),
        _ => format!("Failed to spawn codex CLI at '{cli_path}': {e}"),
    })?;

    let stdout = child.stdout.take().ok_or("failed to capture stdout")?;
    let timeout = cli_timeout();

    let stream_fut = async {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            let trimmed = line.trim();
            if !trimmed.is_empty() && tx.send(format!("{trimmed}\n")).await.is_err() {
                break;
            }
        }
        Ok::<(), String>(())
    };

    match tokio::time::timeout(timeout, stream_fut).await {
        Ok(result) => {
            result?;
            let status = child
                .wait()
                .await
                .map_err(|e| format!("codex process error: {e}"))?;
            if !status.success() {
                let error_msg = format!("codex exited with status {status}");
                tracing::error!("{error_msg}");

                // Check if this might be an auth issue
                if status.code() == Some(1) {
                    return Err(format!(
                        "{error_msg} - This may indicate authentication failure. \
                         Check auth with: codex login status. \
                         For headless setup: bash scripts/setup-headless-auth.sh"
                    ));
                }
                return Err(error_msg);
            }
            Ok(())
        }
        Err(_) => {
            tracing::error!(
                timeout_secs = timeout.as_secs(),
                "codex CLI timed out — killing process"
            );
            let _ = child.kill().await;
            Err(format!(
                "Codex CLI timed out after {} seconds. Increase CORTEX_CLI_TIMEOUT_SECS \
                 or simplify the prompt.",
                timeout.as_secs()
            ))
        }
    }
}

// --- BYOK direct API path (for users who bring their own API keys) ---

/// Non-streaming chat completion (single full response). Used by Pulse tools_v2.
pub async fn chat_completion(
    provider: &Provider,
    api_key: &str,
    model: Option<&str>,
    system_prompt: &str,
    messages: &[ChatMessage],
) -> Result<String, String> {
    match provider {
        Provider::Claude => complete_anthropic_api(api_key, model, system_prompt, messages).await,
        Provider::Openai | Provider::Zen => {
            complete_openai_compatible_api(provider, api_key, model, system_prompt, messages).await
        }
    }
}

/// Resolve server-side LLM credentials for Pulse tools_v2 (honest: None if no keys).
/// Order is preference order: Claude, then OpenAI, then Zen.
pub fn resolve_server_llm() -> Option<(Provider, String)> {
    for provider in [Provider::Claude, Provider::Openai, Provider::Zen] {
        if let Ok(key) = std::env::var(provider.api_key_env()) {
            let key = key.trim().to_string();
            if !key.is_empty() {
                return Some((provider, key));
            }
        }
    }
    None
}

async fn complete_anthropic_api(
    api_key: &str,
    model: Option<&str>,
    system_prompt: &str,
    messages: &[ChatMessage],
) -> Result<String, String> {
    let client = reqwest::Client::new();
    let model = model.unwrap_or("claude-haiku-4-5");

    let api_messages: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| serde_json::json!({"role": m.role, "content": m.content}))
        .collect();

    let body = serde_json::json!({
        "model": model,
        "max_tokens": 1024,
        "system": system_prompt,
        "messages": api_messages,
    });

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .timeout(Duration::from_secs(45))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("anthropic request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        tracing::error!(provider = "anthropic", %status, "API error: {body}");
        return Err(format!("Anthropic API error ({status})"));
    }

    let value: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("anthropic parse failed: {e}"))?;

    let text: String = value
        .get("content")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();

    if text.trim().is_empty() {
        return Err("anthropic returned empty content".to_string());
    }
    Ok(text)
}

/// Chat-completions endpoint for a provider that speaks the OpenAI dialect.
/// Panics on Claude by design — that is a wiring bug, not a runtime condition.
fn openai_compat_endpoint(provider: &Provider) -> &'static str {
    match provider {
        Provider::Openai => OPENAI_CHAT_COMPLETIONS_URL,
        Provider::Zen => ZEN_CHAT_COMPLETIONS_URL,
        Provider::Claude => unreachable!("Claude speaks /v1/messages, not chat/completions"),
    }
}

async fn complete_openai_compatible_api(
    provider: &Provider,
    api_key: &str,
    model: Option<&str>,
    system_prompt: &str,
    messages: &[ChatMessage],
) -> Result<String, String> {
    let client = reqwest::Client::new();
    let label = provider.name();
    let model = model.unwrap_or(provider.default_model());

    let mut api_messages = vec![serde_json::json!({"role": "system", "content": system_prompt})];
    for m in messages {
        api_messages.push(serde_json::json!({"role": m.role, "content": m.content}));
    }

    let body = serde_json::json!({
        "model": model,
        "messages": api_messages,
        "max_tokens": 1024,
    });

    let resp = client
        .post(openai_compat_endpoint(provider))
        .header("Authorization", format!("Bearer {api_key}"))
        .header("content-type", "application/json")
        .timeout(Duration::from_secs(45))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("{label} request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        tracing::error!(provider = label, %status, "API error: {body}");
        return Err(format!("{label} API error ({status})"));
    }

    let value: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("{label} parse failed: {e}"))?;

    let text = value
        .pointer("/choices/0/message/content")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();

    if text.trim().is_empty() {
        return Err(format!("{label} returned empty content"));
    }
    Ok(text)
}

/// Stream a chat response using a raw API key (BYOK path).
/// Use with caution — this burns per-token credits.
pub async fn stream_chat_api(
    provider: &Provider,
    api_key: &str,
    model: Option<&str>,
    system_prompt: &str,
    messages: &[ChatMessage],
    tx: mpsc::Sender<String>,
) -> Result<(), String> {
    match provider {
        Provider::Claude => stream_anthropic_api(api_key, model, system_prompt, messages, tx).await,
        Provider::Openai | Provider::Zen => {
            stream_openai_compatible_api(provider, api_key, model, system_prompt, messages, tx)
                .await
        }
    }
}

async fn stream_anthropic_api(
    api_key: &str,
    model: Option<&str>,
    system_prompt: &str,
    messages: &[ChatMessage],
    tx: mpsc::Sender<String>,
) -> Result<(), String> {
    use futures_util::StreamExt;

    let client = reqwest::Client::new();
    let model = model.unwrap_or("claude-haiku-4-5");

    let api_messages: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| serde_json::json!({"role": m.role, "content": m.content}))
        .collect();

    let body = serde_json::json!({
        "model": model,
        "max_tokens": 4096,
        "system": system_prompt,
        "messages": api_messages,
        "stream": true,
    });

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("anthropic request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        tracing::error!(provider = "anthropic", %status, "API error: {body}");
        let user_msg = match status.as_u16() {
            401 => "Invalid API key — check your Anthropic key in Settings.",
            429 => "Rate limited by Anthropic — try again in a moment.",
            _ => "Anthropic API returned an error. Check your key and try again.",
        };
        return Err(user_msg.to_string());
    }

    let mut stream = resp.bytes_stream();
    let mut buffer = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("stream error: {e}"))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(line_end) = buffer.find('\n') {
            let line = buffer[..line_end].to_string();
            buffer = buffer[line_end + 1..].to_string();

            if let Some(data) = line.strip_prefix("data: ") {
                if data.trim() == "[DONE]" {
                    return Ok(());
                }
                if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                    if event.get("type").and_then(|t| t.as_str()) == Some("content_block_delta") {
                        if let Some(text) = event.pointer("/delta/text").and_then(|t| t.as_str()) {
                            if tx.send(text.to_string()).await.is_err() {
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

async fn stream_openai_compatible_api(
    provider: &Provider,
    api_key: &str,
    model: Option<&str>,
    system_prompt: &str,
    messages: &[ChatMessage],
    tx: mpsc::Sender<String>,
) -> Result<(), String> {
    use futures_util::StreamExt;

    let client = reqwest::Client::new();
    let label = provider.name();
    let display = match provider {
        Provider::Openai => "OpenAI",
        Provider::Zen => "OpenCode Zen",
        Provider::Claude => unreachable!("Claude does not stream chat/completions"),
    };
    let model = model.unwrap_or(provider.default_model());

    let mut api_messages = vec![serde_json::json!({"role": "system", "content": system_prompt})];
    for m in messages {
        api_messages.push(serde_json::json!({"role": m.role, "content": m.content}));
    }

    let body = serde_json::json!({
        "model": model,
        "messages": api_messages,
        "stream": true,
    });

    let resp = client
        .post(openai_compat_endpoint(provider))
        .header("Authorization", format!("Bearer {api_key}"))
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("{label} request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        tracing::error!(provider = label, %status, "API error: {body}");
        let user_msg = match status.as_u16() {
            401 => format!("Invalid API key — check your {display} key in Settings."),
            429 => format!("Rate limited by {display} — try again in a moment."),
            _ => format!("{display} API returned an error. Check your key and try again."),
        };
        return Err(user_msg);
    }

    let mut stream = resp.bytes_stream();
    let mut buffer = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("stream error: {e}"))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(line_end) = buffer.find('\n') {
            let line = buffer[..line_end].to_string();
            buffer = buffer[line_end + 1..].to_string();

            if let Some(data) = line.strip_prefix("data: ") {
                if data.trim() == "[DONE]" {
                    return Ok(());
                }
                if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                    if let Some(content) = event
                        .pointer("/choices/0/delta/content")
                        .and_then(|t| t.as_str())
                    {
                        if tx.send(content.to_string()).await.is_err() {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

// --- Claude stream-json event types ---

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ClaudeStreamEvent {
    #[serde(rename = "assistant")]
    Assistant { message: serde_json::Value },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { delta: serde_json::Value },
    #[serde(rename = "result")]
    Result { result: serde_json::Value },
    #[serde(other)]
    Other,
}

fn extract_claude_text(value: &serde_json::Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    if let Some(content) = value.get("content") {
        if let Some(arr) = content.as_array() {
            let text: String = arr
                .iter()
                .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("");
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

#[cfg(test)]
mod provider_tests {
    use super::*;

    #[test]
    fn from_str_parses_zen_aliases() {
        assert!(matches!(Provider::from_str("zen"), Some(Provider::Zen)));
        assert!(matches!(
            Provider::from_str("opencode"),
            Some(Provider::Zen)
        ));
        assert!(matches!(
            Provider::from_str("opencode-zen"),
            Some(Provider::Zen)
        ));
        assert!(Provider::from_str("not-a-provider").is_none());
    }

    #[test]
    fn openai_compatible_endpoints_are_distinct() {
        assert_eq!(
            openai_compat_endpoint(&Provider::Zen),
            ZEN_CHAT_COMPLETIONS_URL
        );
        assert_eq!(
            openai_compat_endpoint(&Provider::Openai),
            OPENAI_CHAT_COMPLETIONS_URL
        );
    }

    #[test]
    fn api_key_env_is_distinct_per_provider() {
        assert_eq!(Provider::Claude.api_key_env(), "ANTHROPIC_API_KEY");
        assert_eq!(Provider::Openai.api_key_env(), "OPENAI_API_KEY");
        assert_eq!(Provider::Zen.api_key_env(), "OPENCODE_ZEN_API_KEY");
    }

    #[test]
    fn zen_default_model_is_a_zen_model() {
        assert_eq!(Provider::Zen.default_model(), "glm-5.2");
    }
}
