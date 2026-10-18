//! Claude Code as an engine, on the person's own subscription.
//!
//! Claude Code runs inside a bubblewrap jail that shows it only the system
//! files, its own config folder, the worktree and (read-only) the checks.
//! Its shell tool is removed: commands go through `cortex mcp`, which runs
//! each one in the gVisor sandbox. Its own file tools are further fenced by
//! `--restricted` and deny rules, so they cannot read the checks or the
//! sign-in files the jail has to show it.

use super::{CancelToken, Engine, SessionInput, SessionOutcome};
use crate::{Error, Result};
use serde_json::Value;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long to park after a usage limit that gives no reset time.
pub const RATE_LIMIT_WAIT: Duration = Duration::from_secs(30 * 60);
const MIN_WAIT: Duration = Duration::from_secs(60);
const FILE_TOOLS: &str = "Read,Edit,Write,Glob,Grep";
/// Where the jail shows the `cortex` binary that serves the tools.
const CORTEX_IN_JAIL: &str = "/cortex/cortex";

pub struct ClaudeEngine {
    /// `claude`, found on PATH, or a path to it.
    pub binary: PathBuf,
    /// The person's home folder, where Claude Code keeps its sign-in.
    pub home: PathBuf,
    /// The `cortex` binary that serves the tools inside the jail.
    pub cortex: PathBuf,
}

impl ClaudeEngine {
    pub fn new(binary: PathBuf) -> Result<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| Error::Invalid("HOME is not set".into()))?;
        Ok(ClaudeEngine {
            binary,
            home,
            cortex: std::env::current_exe()?,
        })
    }

    /// The command line for one session. The brief goes on stdin.
    pub fn build_args(&self, input: &SessionInput) -> Vec<String> {
        let work = input.worktree.display().to_string();
        let checks = input.checks_dir.display().to_string();
        let config = serde_json::json!({
            "mcpServers": {
                "cortex": {
                    "type": "stdio",
                    "command": CORTEX_IN_JAIL,
                    "args": ["mcp", "--worktree", work, "--checks", checks],
                }
            }
        });
        let mut args: Vec<String> = [
            "-p",
            "--restricted",
            "--output-format",
            "stream-json",
            "--verbose",
            "--tools",
            FILE_TOOLS,
            "--strict-mcp-config",
            "--mcp-config",
        ]
        .map(String::from)
        .into();
        args.push(config.to_string());
        args.push("--allowedTools".into());
        args.push("mcp__cortex__run,mcp__cortex__test".into());
        args.push("--disallowedTools".into());
        // `//` marks an absolute path in Claude Code's permission rules.
        args.push(format!("Read(/{checks}/**)"));
        args.push(format!("Edit(/{checks}/**)"));
        for rule in [
            "Read(~/.claude/**)",
            "Edit(~/.claude/**)",
            "Read(~/.claude.json)",
            "Edit(~/.claude.json)",
        ] {
            args.push(rule.into());
        }
        args.push("--permission-mode".into());
        args.push("acceptEdits".into());
        args
    }
}

impl Engine for ClaudeEngine {
    fn name(&self) -> &str {
        "claude"
    }

    fn version(&self) -> Result<String> {
        let out = std::process::Command::new(find(&self.binary)?)
            .arg("--version")
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", &self.home)
            .output()?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn run(
        &mut self,
        input: &SessionInput,
        on_line: &mut dyn FnMut(&str),
        cancel: &CancelToken,
    ) -> Result<SessionOutcome> {
        jail::run(self, input, on_line, cancel)
    }
}

/// `binary` itself when it names a path, else the first match on PATH.
fn find(binary: &Path) -> Result<PathBuf> {
    if binary.components().count() > 1 {
        return Ok(binary.to_path_buf());
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|d| d.join(binary))
        .find(|p| p.is_file())
        .ok_or_else(|| Error::Blocked(format!("`{}` is not installed", binary.display())))
}

/// Reads a whole stream-json transcript. Each line goes to `on_line`.
pub fn read_stream(reader: impl BufRead, on_line: &mut dyn FnMut(&str)) -> SessionOutcome {
    let mut parser = Stream::default();
    for line in reader.lines() {
        let Ok(line) = line else { break };
        on_line(&line);
        if let Some(stop) = parser.feed(&line) {
            return stop;
        }
    }
    parser.finish("")
}

/// Follows Claude Code's stream-json output and decides how the session
/// ended.
#[derive(Default)]
struct Stream {
    rate_limited: bool,
    resets_at: Option<u64>,
    result: Option<SessionOutcome>,
}

impl Stream {
    /// Takes one line. Returns an outcome when the session must stop now.
    fn feed(&mut self, line: &str) -> Option<SessionOutcome> {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            return None;
        };
        match v["type"].as_str() {
            Some("system") if v["subtype"] == "init" => {
                if let Some(source) = v["apiKeySource"].as_str()
                    && source != "none"
                {
                    return Some(SessionOutcome::Failed {
                        reason: format!(
                            "Claude Code would bill an API key ({source}); \
                             Cortex only uses the signed-in subscription"
                        ),
                    });
                }
            }
            Some("rate_limit_event") => {
                let info = &v["rate_limit_info"];
                if info["status"] == "rejected" {
                    self.rate_limited = true;
                    self.resets_at = info["resetsAt"].as_u64().or(self.resets_at);
                }
            }
            Some("assistant") if v["error"] == "rate_limit" => self.rate_limited = true,
            Some("result") => self.result = Some(self.judge(&v)),
            _ => {}
        }
        None
    }

    fn judge(&self, result: &Value) -> SessionOutcome {
        let text = result["result"].as_str().unwrap_or_default();
        if result["is_error"] != true {
            return SessionOutcome::Ended { note: text.into() };
        }
        if self.rate_limited || looks_limited(text) {
            return SessionOutcome::RateLimited {
                retry_after: self.wait(text),
            };
        }
        let reason = if text.is_empty() {
            format!(
                "Claude Code failed ({})",
                result["subtype"].as_str().unwrap_or("error")
            )
        } else {
            text.to_string()
        };
        SessionOutcome::Failed { reason }
    }

    /// Until the reset time, when one is known.
    fn wait(&self, text: &str) -> Duration {
        let resets = self.resets_at.or_else(|| {
            text.rsplit_once('|')
                .and_then(|(_, t)| t.trim().parse::<u64>().ok())
        });
        let Some(resets) = resets else {
            return RATE_LIMIT_WAIT;
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Duration::from_secs(resets.saturating_sub(now)).max(MIN_WAIT)
    }

    fn finish(mut self, stderr_tail: &str) -> SessionOutcome {
        self.result
            .take()
            .unwrap_or_else(|| SessionOutcome::Failed {
                reason: format!("Claude Code ended without a result. {stderr_tail}")
                    .trim()
                    .to_string(),
            })
    }
}

fn looks_limited(text: &str) -> bool {
    let text = text.to_lowercase();
    ["hit your limit", "usage limit", "rate limit"]
        .iter()
        .any(|p| text.contains(p))
}

#[cfg(not(target_os = "linux"))]
mod jail {
    use super::*;

    pub fn run(
        _engine: &ClaudeEngine,
        _input: &SessionInput,
        _on_line: &mut dyn FnMut(&str),
        _cancel: &CancelToken,
    ) -> Result<SessionOutcome> {
        Err(Error::Blocked(
            "the Claude Code engine needs Linux (use WSL on Windows)".into(),
        ))
    }
}

#[cfg(target_os = "linux")]
mod jail {
    use super::*;
    use std::io::{BufReader, Read, Write};
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc::{RecvTimeoutError, channel};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    const STDERR_TAIL: usize = 2000;
    /// Personal Claude Code folders a session must not pick up.
    const HIDDEN: [&str; 5] = ["plugins", "skills", "agents", "commands", "projects"];

    pub fn run(
        engine: &ClaudeEngine,
        input: &SessionInput,
        on_line: &mut dyn FnMut(&str),
        cancel: &CancelToken,
    ) -> Result<SessionOutcome> {
        let input = SessionInput {
            worktree: input.worktree.canonicalize()?,
            checks_dir: input.checks_dir.canonicalize()?,
            ..input.clone()
        };
        let mut cmd = jail(engine, &input)?;
        cmd.args(engine.build_args(&input))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let mut child = cmd.spawn()?;
        let outcome = watch(&mut child, &input, on_line, cancel);
        kill(&mut child);
        outcome
    }

    fn watch(
        child: &mut Child,
        input: &SessionInput,
        on_line: &mut dyn FnMut(&str),
        cancel: &CancelToken,
    ) -> Result<SessionOutcome> {
        let deadline = Instant::now() + input.timeout;
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let brief = input.brief.clone();
        // A separate thread, so a full pipe cannot stall the watch loop.
        std::thread::spawn(move || {
            let _ = stdin.write_all(brief.as_bytes());
        });
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let mut err_pipe = child.stderr.take().expect("stderr is piped");
        let sink = stderr.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = err_pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let mut all = sink.lock().unwrap();
                all.extend_from_slice(&buf[..n]);
                let extra = all.len().saturating_sub(STDERR_TAIL);
                all.drain(..extra);
            }
        });
        let (tx, rx) = channel();
        let out = child.stdout.take().expect("stdout is piped");
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });

        let mut stream = Stream::default();
        loop {
            if cancel.is_cancelled() {
                return Ok(SessionOutcome::Failed {
                    reason: "cancelled".into(),
                });
            }
            if Instant::now() >= deadline {
                return Ok(SessionOutcome::TimedOut);
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    on_line(&line);
                    if let Some(stop) = stream.feed(&line) {
                        return Ok(stop);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = child.wait();
        let tail = String::from_utf8_lossy(&stderr.lock().unwrap()).into_owned();
        Ok(stream.finish(&tail))
    }

    /// Ends bubblewrap and so its whole PID namespace.
    fn kill(child: &mut Child) {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        // SAFETY: plain syscall on our own child's process group.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let _ = child.wait();
    }

    fn jail(engine: &ClaudeEngine, input: &SessionInput) -> Result<Command> {
        let bwrap = find(Path::new("bwrap"))?;
        let claude = find(&engine.binary)?.canonicalize()?;
        let claude_dir = claude
            .parent()
            .ok_or_else(|| Error::Invalid("claude has no folder".into()))?;
        let home = &engine.home;
        let config = home.join(".claude");
        let user = std::env::var("USER").unwrap_or_else(|_| "cortex".into());

        let mut cmd = Command::new(bwrap);
        cmd.args(["--unshare-all", "--share-net", "--die-with-parent"])
            .args(["--new-session", "--clearenv"])
            .arg("--setenv")
            .arg("HOME")
            .arg(home)
            .args(["--setenv", "PATH", "/usr/local/bin:/usr/bin:/bin"])
            .args(["--setenv", "LANG", "C.UTF-8"])
            .args(["--setenv", "USER", user.as_str()])
            .args(["--ro-bind", "/usr", "/usr"]);
        for dir in ["/bin", "/sbin", "/lib", "/lib64", "/lib32"] {
            cmd.args(["--ro-bind-try", dir, dir]);
        }
        cmd.args(["--ro-bind", "/etc", "/etc"])
            .args([
                "--ro-bind-try",
                "/run/systemd/resolve",
                "/run/systemd/resolve",
            ])
            .args(["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp"])
            .arg("--tmpfs")
            .arg(home);
        // Claude Code keeps and refreshes its sign-in here.
        cmd.arg("--bind-try").arg(&config).arg(&config);
        let state = home.join(".claude.json");
        cmd.arg("--bind-try").arg(&state).arg(&state);
        let personal = config.join("CLAUDE.md");
        if personal.exists() {
            cmd.args(["--ro-bind", "/dev/null"]).arg(&personal);
        }
        for name in HIDDEN {
            let dir = config.join(name);
            if dir.is_dir() {
                cmd.arg("--tmpfs").arg(&dir);
            }
        }
        cmd.arg("--bind").arg(&input.worktree).arg(&input.worktree);
        cmd.arg("--ro-bind")
            .arg(&input.checks_dir)
            .arg(&input.checks_dir);
        cmd.arg("--ro-bind").arg(claude_dir).arg(claude_dir);
        cmd.arg("--ro-bind")
            .arg(engine.cortex.canonicalize()?)
            .arg(CORTEX_IN_JAIL);
        cmd.arg("--chdir").arg(&input.worktree).arg(&claude);
        Ok(cmd)
    }
}
