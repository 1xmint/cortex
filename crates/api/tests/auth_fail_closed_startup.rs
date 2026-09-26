//! The server binary must refuse to start in production when the Clerk
//! trust anchors are missing, rather than silently falling back to treating
//! every request as user "local" (the fail-open bug this loader replaces).
//! Spawns the real `cortex-server` binary as a subprocess — no network
//! traffic, no shared state with other tests, and no dependency on the
//! library's internal types.

use std::process::Command;

#[test]
fn production_without_clerk_trust_anchors_exits_nonzero_and_says_why() {
    let bin = env!("CARGO_BIN_EXE_cortex-server");
    let dir = tempfile::tempdir().unwrap();
    let ledger_path = dir.path().join("ledger.jsonl");

    let output = Command::new(bin)
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
        .output()
        .expect("failed to spawn cortex-server binary");

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
