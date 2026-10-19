#![cfg(target_os = "linux")]

// Browser checks (ADR 0006): headless Chromium driven by Playwright, run
// inside the same bubblewrap + gVisor sandbox as every other check. Needs
// `scripts/install-browser-tools.sh` to have run on the host.

use cortex::sandbox::{RunOutput, RunSpec, run, self_test};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const NODE: &str = "/usr/local/lib/cortex-browser/node/bin/node";

// One browser at a time, so the leftover-process check in the hang test
// cannot see another test's live browser.
static ONE_BROWSER: Mutex<()> = Mutex::new(());

fn fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/browser")
}

/// Runs `script` as a check against a copy of the fixture page `page`.
fn run_check(page: &str, script: &str, timeout: Duration) -> RunOutput {
    let work = tempfile::tempdir().unwrap();
    std::fs::copy(
        fixtures().join(page).join("index.html"),
        work.path().join("index.html"),
    )
    .unwrap();
    let checks = tempfile::tempdir().unwrap();
    std::fs::create_dir(checks.path().join("lib")).unwrap();
    let helper = Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench/browser/cortex-check.mjs");
    std::fs::copy(&helper, checks.path().join("lib/cortex-check.mjs"))
        .unwrap_or_else(|e| panic!("copy {}: {e}", helper.display()));
    std::fs::write(checks.path().join("check.mjs"), script).unwrap();
    run(&RunSpec {
        cmd: format!("{NODE} /checks/check.mjs"),
        workdir: work.path().to_path_buf(),
        checks_dir: Some(checks.path().to_path_buf()),
        timeout,
    })
    .unwrap()
}

#[test]
fn browser_loads_page_and_clicks() {
    let _one = ONE_BROWSER.lock().unwrap_or_else(|e| e.into_inner());
    let out = run_check(
        "ok",
        r##"import { withPage, check } from "./lib/cortex-check.mjs";
await withPage(async (page) => {
  check("starts waiting", (await page.textContent("#out")) === "waiting");
  await page.click("#go");
  const text = await page.textContent("#out");
  check("clicked", text === "clicked", `text was ${text}`);
});
"##,
        Duration::from_secs(120),
    );
    assert_eq!(out.exit_code, 0, "{out:?}");
    assert!(out.stdout.contains("PASS starts waiting"), "{out:?}");
    assert!(out.stdout.contains("PASS clicked"), "{out:?}");
    assert!(out.stdout.contains("PASS no page errors"), "{out:?}");
}

#[test]
fn page_error_fails_check() {
    let _one = ONE_BROWSER.lock().unwrap_or_else(|e| e.into_inner());
    let out = run_check(
        "error",
        r##"import { withPage, check } from "./lib/cortex-check.mjs";
await withPage(async (page) => {
  check("page loaded", (await page.textContent("#out")) === "loaded");
});
"##,
        Duration::from_secs(120),
    );
    // Positive control: the script's own assertion ran and passed.
    assert!(out.stdout.contains("PASS page loaded"), "{out:?}");
    assert!(out.stdout.contains("FAIL no page errors"), "{out:?}");
    assert!(out.stdout.contains("boom on load"), "{out:?}");
    assert_ne!(out.exit_code, 0, "{out:?}");
}

#[test]
fn hung_page_is_killed_at_timeout() {
    let _one = ONE_BROWSER.lock().unwrap_or_else(|e| e.into_inner());
    let started = Instant::now();
    let out = run_check(
        "hang",
        r##"import { withPage, check } from "./lib/cortex-check.mjs";
console.log("check started");
await withPage(async () => {
  check("never reached", false, "the hung page should never finish loading");
});
"##,
        Duration::from_secs(20),
    );
    let took = started.elapsed();
    assert!(out.timed_out, "{out:?}");
    assert!(took < Duration::from_secs(40), "took {took:?}: {out:?}");
    assert!(out.stdout.contains("check started"), "{out:?}");
    let left = std::process::Command::new("pgrep")
        .args(["-af", "cortex-browser"])
        .output()
        .unwrap();
    let left = String::from_utf8_lossy(&left.stdout);
    assert!(left.trim().is_empty(), "browser processes left: {left}");
}

#[test]
fn browser_has_no_network() {
    let _one = ONE_BROWSER.lock().unwrap_or_else(|e| e.into_inner());
    let out = run_check(
        "ok",
        r##"import { withPage, check } from "./lib/cortex-check.mjs";
await withPage(async (page) => {
  check("loopback page loaded", (await page.textContent("#out")) === "waiting");
  try {
    await page.goto("http://1.1.1.1", { timeout: 15000 });
    check("offline", false, "reached 1.1.1.1");
  } catch (e) {
    check("offline", true);
  }
});
"##,
        Duration::from_secs(120),
    );
    assert!(out.stdout.contains("PASS loopback page loaded"), "{out:?}");
    assert!(out.stdout.contains("PASS offline"), "{out:?}");
    assert!(!out.stdout.contains("FAIL offline"), "{out:?}");
}

#[test]
fn sandbox_self_test_still_passes() {
    self_test().expect("sandbox self test");
}
