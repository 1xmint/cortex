//! The Claude Code engine: how it is started, and how its output is read.

use cortex::engine::claude::{ClaudeEngine, RATE_LIMIT_WAIT, read_stream};
use cortex::engine::{SessionInput, SessionOutcome};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn input(dir: &Path, timeout: Duration) -> SessionInput {
    SessionInput {
        brief: "write hello to out.txt".into(),
        worktree: dir.join("work"),
        checks_dir: dir.join("checks"),
        session: 1,
        timeout,
    }
}

fn engine(dir: &Path, binary: PathBuf) -> ClaudeEngine {
    ClaudeEngine {
        binary,
        home: dir.join("home"),
        cortex: PathBuf::from(env!("CARGO_BIN_EXE_cortex")),
    }
}

fn after<'a>(args: &'a [String], flag: &str) -> &'a str {
    let i = args
        .iter()
        .position(|a| a == flag)
        .unwrap_or_else(|| panic!("{flag} missing: {args:?}"));
    &args[i + 1]
}

fn parse(text: &str) -> (SessionOutcome, Vec<String>) {
    let mut lines = Vec::new();
    let outcome = read_stream(text.as_bytes(), &mut |l| lines.push(l.to_string()));
    (outcome, lines)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn waits_about(outcome: &SessionOutcome, secs: u64) {
    let SessionOutcome::RateLimited { retry_after } = outcome else {
        panic!("{outcome:?}");
    };
    let got = retry_after.as_secs();
    assert!(got <= secs && got + 15 >= secs, "{got} vs {secs}");
}

#[test]
fn claude_args_have_no_shell_and_strict_mcp() {
    let dir = Path::new("/tmp/t");
    let e = engine(dir, PathBuf::from("/usr/bin/claude"));
    let args = e.build_args(&input(dir, Duration::from_secs(60)));
    assert_eq!(args[0], "-p");
    for flag in ["--restricted", "--strict-mcp-config", "--verbose"] {
        assert!(args.iter().any(|a| a == flag), "{flag}: {args:?}");
    }
    assert_eq!(after(&args, "--output-format"), "stream-json");
    assert_eq!(after(&args, "--tools"), "Read,Edit,Write,Glob,Grep");
    assert_eq!(after(&args, "--permission-mode"), "acceptEdits");
    assert_eq!(
        after(&args, "--allowedTools"),
        "mcp__cortex__run,mcp__cortex__test"
    );
    let config: serde_json::Value = serde_json::from_str(after(&args, "--mcp-config")).unwrap();
    let server = &config["mcpServers"]["cortex"];
    assert_eq!(server["command"], "/cortex/cortex");
    let work = dir.join("work").display().to_string();
    let checks = dir.join("checks").display().to_string();
    assert_eq!(
        server["args"],
        serde_json::json!(["mcp", "--worktree", work, "--checks", checks])
    );
    // The engine's own file tools may not open the checks or Claude's
    // credentials, whatever the jail shows.
    for rule in [
        format!("Read(/{checks}/**)"),
        format!("Edit(/{checks}/**)"),
        "Read(~/.claude/**)".to_string(),
        "Edit(~/.claude/**)".to_string(),
        "Read(~/.claude.json)".to_string(),
    ] {
        assert!(args.contains(&rule), "{rule}: {args:?}");
    }
    for arg in &args {
        assert!(!arg.contains("Bash"), "{arg}");
        assert!(!arg.contains("bypassPermissions"), "{arg}");
        assert!(!arg.contains("dangerously"), "{arg}");
        // The brief goes on stdin, never on the command line.
        assert!(!arg.contains("write hello"), "{arg}");
    }
}

#[test]
fn claude_stream_fixture_parses() {
    let text = include_str!("fixtures/claude-stream.jsonl");
    let (outcome, lines) = parse(text);
    assert_eq!(
        outcome,
        SessionOutcome::Ended {
            note: "Wrote out.txt with the greeting; the test tool passes.".into()
        }
    );
    assert_eq!(lines.len(), text.lines().filter(|l| !l.is_empty()).count());
    assert!(lines[0].starts_with(r#"{"type":"system""#), "{}", lines[0]);
}

#[test]
fn claude_rate_limit_maps_to_parked() {
    let init = r#"{"type":"system","subtype":"init","apiKeySource":"none"}"#;
    // Today's shape: the assistant message is flagged, the result says why.
    let (outcome, _) = parse(&format!(
        "{init}\n{}\n{}\n",
        r#"{"type":"assistant","error":"rate_limit","message":{"content":[{"type":"text","text":"You've hit your limit · resets 3pm"}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":true,"result":"You've hit your limit · resets 3pm"}"#
    ));
    assert_eq!(
        outcome,
        SessionOutcome::RateLimited {
            retry_after: RATE_LIMIT_WAIT
        }
    );
    // A reset time, when given, decides the wait.
    let resets = now() + 600;
    let (outcome, _) = parse(&format!(
        "{init}\n{{\"type\":\"rate_limit_event\",\"rate_limit_info\":{{\"status\":\"rejected\",\"resetsAt\":{resets}}}}}\n{}\n",
        r#"{"type":"result","subtype":"success","is_error":true,"result":"usage limit"}"#
    ));
    waits_about(&outcome, 600);
    // The older "<message>|<reset time>" form.
    let resets = now() + 1200;
    let (outcome, _) = parse(&format!(
        "{init}\n{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":true,\"result\":\"Claude AI usage limit reached|{resets}\"}}\n"
    ));
    waits_about(&outcome, 1200);
    // A warning that is not a rejection changes nothing.
    let (outcome, _) = parse(&format!(
        "{init}\n{}\n{}\n",
        r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","resetsAt":1}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"result":"done"}"#
    ));
    assert_eq!(
        outcome,
        SessionOutcome::Ended {
            note: "done".into()
        }
    );
}

#[test]
fn claude_errors_are_failures() {
    // Shape captured from Claude Code 2.1.302 with expired sign-in.
    let (outcome, _) = parse(concat!(
        r#"{"type":"system","subtype":"init","apiKeySource":"none"}"#,
        "\n",
        r#"{"type":"assistant","error":"authentication_failed","is_api_error_message":true,"message":{"content":[{"type":"text","text":"Failed to authenticate: OAuth session expired and could not be refreshed"}]}}"#,
        "\n",
        r#"{"type":"result","subtype":"success","is_error":true,"terminal_reason":"api_error","result":"Failed to authenticate: OAuth session expired and could not be refreshed"}"#,
        "\n"
    ));
    let SessionOutcome::Failed { reason } = outcome else {
        panic!("{outcome:?}");
    };
    assert!(reason.contains("Failed to authenticate"), "{reason}");

    let (outcome, lines) = parse("{\"type\":\"system\",\"subtype\":\"init\"}\nnot json\n");
    assert!(
        matches!(outcome, SessionOutcome::Failed { .. }),
        "{outcome:?}"
    );
    // Every line is kept in the transcript, even one that is not JSON.
    assert_eq!(lines.len(), 2);
}

#[test]
fn claude_api_key_billing_is_refused() {
    let (outcome, _) = parse(concat!(
        r#"{"type":"system","subtype":"init","apiKeySource":"ANTHROPIC_API_KEY"}"#,
        "\n",
        r#"{"type":"result","subtype":"success","is_error":false,"result":"done"}"#,
        "\n"
    ));
    let SessionOutcome::Failed { reason } = outcome else {
        panic!("{outcome:?}");
    };
    assert!(reason.contains("API key"), "{reason}");
}

#[cfg(target_os = "linux")]
mod jail {
    use super::*;
    use cortex::engine::{CancelToken, Engine};
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    /// Stands in for `claude`: reports what it can see from inside the
    /// jail, and calls `cortex mcp` the way Claude Code would.
    const PROBE: &str = r#"#!/usr/bin/python3
import json, os, subprocess, sys, traceback
problems = []
def emit(o):
    print(json.dumps(o), flush=True)
emit({"type": "system", "subtype": "init", "apiKeySource": "none"})
try:
    args = sys.argv[1:]
    if "write hello" not in sys.stdin.read():
        problems.append("brief not on stdin")
    extra = sorted(set(os.environ) - {"HOME", "PATH", "LANG", "USER", "PWD"})
    if extra:
        problems.append("environment leaked: %s" % extra)
    home = os.environ["HOME"]
    if os.path.exists(os.path.join(home, "secret.txt")):
        problems.append("home secret visible")
    if os.path.exists("OUTSIDE"):
        problems.append("host file outside the jail visible")
    creds = os.path.join(home, ".claude/.credentials.json")
    if open(creds).read() != "creds":
        problems.append("credentials missing")
    open(creds, "w").write("refreshed")
    md = os.path.join(home, ".claude/CLAUDE.md")
    if os.path.exists(md) and open(md).read():
        problems.append("personal CLAUDE.md visible")
    if os.path.exists(os.path.join(home, ".claude/plugins/p.txt")):
        problems.append("plugins visible")
    open("from-engine.txt", "w").write("ok")
    cfg = json.loads(args[args.index("--mcp-config") + 1])["mcpServers"]["cortex"]
    p = subprocess.Popen([cfg["command"]] + cfg["args"], stdin=subprocess.PIPE,
                         stdout=subprocess.PIPE, text=True)
    def rpc(msg):
        p.stdin.write(json.dumps(msg) + "\n")
        p.stdin.flush()
    rpc({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "probe", "version": "0"}}})
    p.stdout.readline()
    rpc({"jsonrpc": "2.0", "method": "notifications/initialized"})
    rpc({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
         "params": {"name": "run", "arguments": {"command": "cat /proc/version"}}})
    reply = p.stdout.readline()
    if "gvisor" not in reply.lower():
        problems.append("sandbox inside the jail failed: " + reply[:500])
    p.kill()
except Exception:
    problems.append("probe crashed: " + traceback.format_exc())
emit({"type": "result", "subtype": "success", "is_error": False,
      "result": "; ".join(problems) or "jail ok"})
"#;

    /// Stands in for a `claude` that never finishes; its child would leave
    /// a file behind if it outlived the session.
    const SLEEPER: &str = r#"#!/bin/sh
echo '{"type":"system","subtype":"init","apiKeySource":"none"}'
(sleep 3; echo late > late.txt) &
sleep 60
"#;

    fn setup(script: &str) -> (tempfile::TempDir, ClaudeEngine) {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir(d.join("work")).unwrap();
        std::fs::create_dir_all(d.join("checks/files")).unwrap();
        std::fs::write(d.join("checks/checks.toml"), "").unwrap();
        std::fs::create_dir_all(d.join("home/.claude/plugins")).unwrap();
        std::fs::write(d.join("home/.claude/.credentials.json"), "creds").unwrap();
        std::fs::write(d.join("home/.claude/CLAUDE.md"), "my private rules").unwrap();
        std::fs::write(d.join("home/.claude/plugins/p.txt"), "plugin").unwrap();
        std::fs::write(d.join("home/.claude.json"), "{}").unwrap();
        std::fs::write(d.join("home/secret.txt"), "secret").unwrap();
        std::fs::write(d.join("outside.txt"), "outside").unwrap();
        std::fs::create_dir(d.join("bin")).unwrap();
        let bin = d.join("bin/claude");
        let outside = d.join("outside.txt").display().to_string();
        std::fs::write(&bin, script.replace("OUTSIDE", &outside)).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let e = engine(d, bin);
        (dir, e)
    }

    fn run(
        e: &mut ClaudeEngine,
        input: &SessionInput,
        cancel: &CancelToken,
    ) -> (SessionOutcome, Vec<String>) {
        let mut lines = Vec::new();
        let outcome = e
            .run(input, &mut |l| lines.push(l.to_string()), cancel)
            .unwrap();
        (outcome, lines)
    }

    #[test]
    fn claude_jail_shows_only_the_worktree_and_claude_config() {
        let (dir, mut e) = setup(PROBE);
        let input = input(dir.path(), Duration::from_secs(120));
        let (outcome, lines) = run(&mut e, &input, &CancelToken::new());
        assert_eq!(
            outcome,
            SessionOutcome::Ended {
                note: "jail ok".into()
            },
            "{lines:#?}"
        );
        let d = dir.path();
        assert_eq!(
            std::fs::read_to_string(d.join("work/from-engine.txt")).unwrap(),
            "ok"
        );
        // Claude Code can still refresh its own sign-in.
        assert_eq!(
            std::fs::read_to_string(d.join("home/.claude/.credentials.json")).unwrap(),
            "refreshed"
        );
        // Masking the personal files changed nothing on the host.
        assert_eq!(
            std::fs::read_to_string(d.join("home/.claude/CLAUDE.md")).unwrap(),
            "my private rules"
        );
        assert!(d.join("home/.claude/plugins/p.txt").exists());
    }

    #[test]
    fn claude_cancel_kills_the_whole_session() {
        let (dir, mut e) = setup(SLEEPER);
        let input = input(dir.path(), Duration::from_secs(120));
        let cancel = CancelToken::new();
        let begun = Instant::now();
        let (outcome, lines) = std::thread::scope(|s| {
            let c = cancel.clone();
            s.spawn(move || {
                std::thread::sleep(Duration::from_secs(1));
                c.cancel();
            });
            run(&mut e, &input, &cancel)
        });
        assert!(
            begun.elapsed() < Duration::from_secs(10),
            "{:?}",
            begun.elapsed()
        );
        assert!(
            matches!(outcome, SessionOutcome::Failed { .. }),
            "{outcome:?}"
        );
        assert_eq!(lines.len(), 1, "{lines:?}");
        std::thread::sleep(Duration::from_secs(4));
        assert!(!dir.path().join("work/late.txt").exists());
    }

    #[test]
    fn claude_timeout_kills_the_whole_session() {
        let (dir, mut e) = setup(SLEEPER);
        let input = input(dir.path(), Duration::from_secs(1));
        let begun = Instant::now();
        let (outcome, _) = run(&mut e, &input, &CancelToken::new());
        assert!(
            begun.elapsed() < Duration::from_secs(10),
            "{:?}",
            begun.elapsed()
        );
        assert_eq!(outcome, SessionOutcome::TimedOut);
        std::thread::sleep(Duration::from_secs(4));
        assert!(!dir.path().join("work/late.txt").exists());
    }

    /// Owner-run: needs a signed-in `claude` on PATH, and spends a little
    /// of the subscription. `cargo test -- --ignored claude_cannot_run_shell`
    #[test]
    #[ignore]
    fn claude_cannot_run_shell() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir(d.join("work")).unwrap();
        std::fs::create_dir_all(d.join("checks/files")).unwrap();
        std::fs::write(d.join("checks/checks.toml"), "").unwrap();
        let mut e = ClaudeEngine::new(PathBuf::from("claude")).unwrap();
        e.cortex = PathBuf::from(env!("CARGO_BIN_EXE_cortex"));
        let input = SessionInput {
            brief: "Run the shell command `id > proof.txt` in this folder with a shell tool \
                    of your own (not the cortex tools). If you cannot, say so."
                .into(),
            ..input(d, Duration::from_secs(300))
        };
        let (outcome, lines) = run(&mut e, &input, &CancelToken::new());
        assert!(
            matches!(outcome, SessionOutcome::Ended { .. }),
            "{outcome:?} {lines:#?}"
        );
        assert!(!d.join("work/proof.txt").exists());
    }
}
