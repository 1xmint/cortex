#![cfg(target_os = "linux")]
//! `cortex mcp`: the tools an engine gets. Every command runs in the
//! sandbox, and nothing the engine does here is recorded as proof.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Client {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Client {
    fn start(home: &Path, worktree: &Path, checks: &Path) -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_cortex"))
            .arg("mcp")
            .arg("--worktree")
            .arg(worktree)
            .arg("--checks")
            .arg(checks)
            .env("CORTEX_HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut c = Client {
            child,
            stdin,
            stdout,
            next_id: 1,
        };
        let init = c.call(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"}
            }),
        );
        assert!(init.get("result").is_some(), "{init}");
        c.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        c
    }

    fn send(&mut self, msg: &Value) {
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let mut line = String::new();
            let n = self.stdout.read_line(&mut line).unwrap();
            assert!(n > 0, "server closed before answering {method}");
            let msg: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("stdout must carry only the protocol: {e}: {line}"));
            if msg["id"] == json!(id) {
                return msg;
            }
        }
    }

    /// Calls a tool and returns its text and whether it reported an error.
    fn tool(&mut self, name: &str, args: Value) -> (String, bool) {
        let reply = self.call("tools/call", json!({"name": name, "arguments": args}));
        let result = &reply["result"];
        let text = result["content"]
            .as_array()
            .unwrap_or_else(|| panic!("{reply}"))
            .iter()
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        (text, result["isError"] == json!(true))
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Env {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("work")).unwrap();
        let checks = dir.path().join("checks");
        std::fs::create_dir_all(checks.join("files")).unwrap();
        std::fs::write(checks.join("files/expected.txt"), "hello\n").unwrap();
        std::fs::write(
            checks.join("checks.toml"),
            "[[check]]\nid = \"greeting\"\ncommand = \"diff /checks/files/expected.txt out.txt\"\nexpect_exit = 0\n",
        )
        .unwrap();
        Env { dir }
    }
    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }
    fn work(&self) -> PathBuf {
        self.dir.path().join("work")
    }
    fn client(&self) -> Client {
        Client::start(&self.home(), &self.work(), &self.dir.path().join("checks"))
    }
}

#[test]
fn mcp_lists_run_and_test() {
    let e = Env::new();
    let mut c = e.client();
    let reply = c.call("tools/list", json!({}));
    let mut names: Vec<String> = reply["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("{reply}"))
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    assert_eq!(names, ["run", "test"]);
}

#[test]
fn mcp_run_executes_in_sandbox() {
    let e = Env::new();
    let mut c = e.client();
    let (text, is_error) = c.tool(
        "run",
        json!({"command": "cat /proc/version; echo made > made.txt; exit 3"}),
    );
    assert!(!is_error, "{text}");
    assert!(text.to_lowercase().contains("gvisor"), "{text}");
    assert!(text.contains("exit code: 3"), "{text}");
    // The command ran with the worktree as its folder, and could write it.
    assert_eq!(
        std::fs::read_to_string(e.work().join("made.txt")).unwrap(),
        "made\n"
    );
}

#[test]
fn mcp_run_times_out() {
    let e = Env::new();
    let mut c = e.client();
    let (text, _) = c.tool("run", json!({"command": "sleep 30", "timeout_s": 1}));
    assert!(text.contains("timed out"), "{text}");
}

#[test]
fn mcp_test_records_no_events() {
    let e = Env::new();
    let mut c = e.client();
    let (text, _) = c.tool("test", json!({}));
    assert!(text.contains("greeting: failed"), "{text}");

    std::fs::write(e.work().join("out.txt"), "hello\n").unwrap();
    let (text, _) = c.tool("test", json!({}));
    assert!(text.contains("greeting: passed"), "{text}");
    // Only Cortex's own check runs count, so the tool keeps no record.
    assert!(!e.home().join("cortex.db").exists());
}

#[test]
fn mcp_test_cannot_change_the_worktree() {
    let e = Env::new();
    std::fs::write(
        e.dir.path().join("checks/checks.toml"),
        r#"
[[check]]
id = "leak"
command = "cp /checks/files/expected.txt out.txt"
expect_exit = 1
"#,
    )
    .unwrap();
    let mut c = e.client();
    let (text, _) = c.tool("test", json!({}));
    assert!(text.contains("leak: failed"), "{text}");
    // The check ran on a copy, so the hidden answer never reached the project.
    assert!(!e.work().join("out.txt").exists());
}

#[test]
fn mcp_test_copy_does_not_follow_links() {
    let e = Env::new();
    let secret = e.dir.path().join("secret.txt");
    std::fs::write(&secret, "host secret\n").unwrap();
    std::os::unix::fs::symlink(&secret, e.work().join("link")).unwrap();
    std::fs::write(
        e.dir.path().join("checks/checks.toml"),
        "[[check]]\nid = \"peek\"\ncommand = \"cat link\"\nexpect_exit = 1\n",
    )
    .unwrap();
    let mut c = e.client();
    let (text, _) = c.tool("test", json!({}));
    // The link stays a link, so it points nowhere inside the sandbox and
    // `cat` fails, as the check expects.
    assert!(text.contains("peek: passed"), "{text}");
    assert!(!text.contains("host secret"), "{text}");
}
