//! The `cortex-server-bin` package builds a second `cortex-server` binary
//! (`crates/cortex-server/src/main.rs`) that duplicates the fail-closed
//! startup check in `crates/api/src/main.rs` rather than sharing it. That
//! duplication is exactly how the second binary went fail-open once already
//! (see the commit that introduced this file's sibling in
//! `crates/api/tests/auth_fail_closed_startup.rs`) -- nothing forced the two
//! copies to be kept in sync. This test mirrors that one, against this
//! package's own binary, so a regression here is caught the same way.
//!
//! Spawns the real `cortex-server` binary as a subprocess -- no network
//! traffic, no shared state with other tests, and no dependency on the
//! library's internal types.

use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn production_without_clerk_trust_anchors_exits_nonzero_and_says_why() {
    let bin = env!("CARGO_BIN_EXE_cortex-server");
    let dir = tempfile::tempdir().unwrap();
    let ledger_path = dir.path().join("ledger.jsonl");

    let mut child = Command::new(bin)
        .env("CORTEX_ENV", "production")
        .env("CORTEX_SINGLE_NODE", "1")
        .env("CORTEX_PORT", "0")
        .env("CORTEX_LEDGER_PATH", &ledger_path)
        .env("CORTEX_WORKSPACE", dir.path())
        .env_remove("CLERK_SECRET_KEY")
        .env_remove("CLERK_ISSUER")
        .env_remove("CLERK_AUTHORIZED_PARTY")
        .env_remove("CORTEX_AUTH_DISABLED")
        .env_remove("HEYVERA_REQUIRE_AUTH")
        .env_remove("CORTEX_ALLOWED_ORIGINS")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn cortex-server binary");

    // A fail-open binary would keep listening forever instead of exiting;
    // poll rather than block so that regression can't hang the test suite.
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().expect("failed to poll child status") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "cortex-server did not exit within 30s in production without Clerk trust \
                 anchors; a fail-open server must not keep listening"
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    use std::io::Read;
    let mut stdout = String::new();
    let mut stderr = String::new();
    child
        .stdout
        .take()
        .expect("child stdout was not piped")
        .read_to_string(&mut stdout)
        .expect("failed to read child stdout");
    child
        .stderr
        .take()
        .expect("child stderr was not piped")
        .read_to_string(&mut stderr)
        .expect("failed to read child stderr");

    let output = std::process::Output {
        status,
        stdout: stdout.into_bytes(),
        stderr: stderr.into_bytes(),
    };

    assert!(
        !output.status.success(),
        "the server must not start without Clerk trust anchors in production; \
         status={:?} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("CLERK_SECRET_KEY is required in HeyVera production"),
        "stderr did not explain the failure: {stderr}"
    );
}
