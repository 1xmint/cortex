//! `cortex mcp`: the tools an engine gets besides its own file tools. Every
//! command runs in the sandbox. `test` runs the frozen checks so the engine
//! can see where it stands, but records nothing: only the checks Cortex runs
//! itself after a session count as proof.

use crate::Error;
use crate::checks::CheckSet;
use crate::sandbox::{self, RunOutput, RunSpec};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{ServiceExt, schemars, tool, tool_router};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

const DEFAULT_TIMEOUT_S: u32 = 120;
const MAX_TIMEOUT_S: u32 = 15 * 60;
const CHECK_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// How much of a command's output goes back to the engine.
const TAIL: usize = 8000;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RunArgs {
    /// Shell command, run with `/bin/sh -c` in the project folder.
    pub command: String,
    /// Seconds before the command is stopped (default 120, at most 900).
    pub timeout_s: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct TestArgs {}

#[derive(Clone)]
pub struct Tools {
    worktree: PathBuf,
    checks: PathBuf,
}

#[tool_router(server_handler)]
impl Tools {
    #[tool(
        description = "Run a shell command in the project folder, inside an isolated sandbox with no network. Returns the exit code and output."
    )]
    async fn run(
        &self,
        Parameters(RunArgs { command, timeout_s }): Parameters<RunArgs>,
    ) -> Result<String, String> {
        let secs = timeout_s
            .unwrap_or(DEFAULT_TIMEOUT_S)
            .clamp(1, MAX_TIMEOUT_S);
        let spec = RunSpec {
            cmd: command,
            workdir: self.worktree.clone(),
            checks_dir: None,
            timeout: Duration::from_secs(secs.into()),
        };
        let out = blocking(move || sandbox::run(&spec)).await?;
        Ok(describe(&out))
    }

    #[tool(
        description = "Run the project's acceptance checks against the current files and report which pass. The checks themselves are hidden."
    )]
    async fn test(&self, Parameters(TestArgs {}): Parameters<TestArgs>) -> Result<String, String> {
        let (worktree, checks) = (self.worktree.clone(), self.checks.clone());
        blocking(move || run_checks(&worktree, &checks)).await
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> crate::Result<T> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// A throwaway copy of a worktree, removed when dropped.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Copies `from` into `to` without following links: a link is recreated as
/// a link, so it still resolves only inside the sandbox, and anything that
/// is not a plain file, folder or link (a pipe, a device) is left out.
fn copy_tree(from: &Path, to: &Path, top: bool) -> std::io::Result<()> {
    std::fs::create_dir(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if top && name == ".git" {
            continue;
        }
        let (src, dst) = (entry.path(), to.join(&name));
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&src, &dst, false)?;
        } else if kind.is_file() {
            std::fs::copy(&src, &dst)?;
        } else if kind.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(std::fs::read_link(&src)?, &dst)?;
        }
    }
    Ok(())
}

/// Runs the checks on a copy of the worktree, so a check that writes (say,
/// copies a hidden file into place) cannot hand the engine the answer.
fn run_checks(worktree: &Path, checks: &Path) -> crate::Result<String> {
    let set = CheckSet::load(&checks.join("checks.toml"))?;
    let scratch =
        Scratch(std::env::temp_dir().join(format!("cortex-test-{}", uuid::Uuid::new_v4())));
    copy_tree(worktree, &scratch.0, true)?;
    let mut report = String::new();
    for check in &set.checks {
        let out = sandbox::run(&RunSpec {
            cmd: check.command.clone(),
            workdir: scratch.0.clone(),
            checks_dir: Some(checks.to_path_buf()),
            timeout: CHECK_TIMEOUT,
        })?;
        let passed = !out.timed_out && out.exit_code == check.expect_exit;
        let verdict = if passed { "passed" } else { "failed" };
        report.push_str(&format!(
            "{}: {verdict} (exit {}, expected {})\n",
            check.id, out.exit_code, check.expect_exit
        ));
        if !passed {
            let both = format!("{}{}", out.stdout, out.stderr);
            report.push_str(tail(&both, TAIL / 4));
            report.push('\n');
        }
    }
    Ok(report)
}

pub(crate) fn describe(out: &RunOutput) -> String {
    let mut text = if out.timed_out {
        "timed out; the command was stopped\n".to_string()
    } else {
        format!("exit code: {}\n", out.exit_code)
    };
    text.push_str("--- stdout\n");
    text.push_str(tail(&out.stdout, TAIL));
    text.push_str("\n--- stderr\n");
    text.push_str(tail(&out.stderr, TAIL));
    if out.truncated {
        text.push_str("\n(output was cut short)");
    }
    text
}

fn tail(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

/// Serves the tools over stdin and stdout until the client hangs up.
/// Nothing but the protocol may be written to stdout.
pub fn serve(worktree: PathBuf, checks: PathBuf) -> crate::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let tools = Tools { worktree, checks };
        let service = tools
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|e| Error::Invalid(format!("mcp: {e}")))?;
        service
            .waiting()
            .await
            .map_err(|e| Error::Invalid(format!("mcp: {e}")))?;
        Ok(())
    })
}
