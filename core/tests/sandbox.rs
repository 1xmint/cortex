#![cfg(target_os = "linux")]

use cortex::sandbox::{RunSpec, run, self_test};
use std::path::Path;
use std::time::{Duration, Instant};

fn spec(cmd: &str, workdir: &Path) -> RunSpec {
    RunSpec {
        cmd: cmd.to_string(),
        workdir: workdir.to_path_buf(),
        checks_dir: None,
        timeout: Duration::from_secs(30),
    }
}

#[test]
fn self_test_passes() {
    self_test().expect("sandbox self test");
}

#[test]
fn cannot_read_planted_key() {
    let home = std::env::var("HOME").unwrap();
    let key = Path::new(&home).join(".cortex-test-key");
    std::fs::write(&key, "SECRET-123").unwrap();
    let work = tempfile::tempdir().unwrap();
    // The search pattern is built at run time so the literal secret never
    // appears in the command line, which gVisor keeps in its own state files.
    let out = run(&spec(
        &format!(
            "echo canary; cat ~/.cortex-test-key; cat /root/.cortex-test-key; cat {}; \
             grep -rs \"SECRET-$((100 + 23))\" /home /root /tmp /etc /work /var /opt /mnt /srv /run",
            key.display()
        ),
        work.path(),
    ))
    .unwrap();
    // Positive control: the sandbox really ran the command.
    assert!(out.stdout.contains("canary"), "{out:?}");
    assert!(!out.timed_out, "{out:?}");
    assert!(!out.stdout.contains("SECRET-123"), "{out:?}");
}

#[test]
fn cannot_reach_network() {
    let probe = "import socket; socket.create_connection(('1.1.1.1', 53), 3); print('NET OPEN')";
    let host = std::process::Command::new("python3")
        .args(["-c", probe])
        .output()
        .unwrap();
    if !host.status.success() {
        eprintln!("note: host itself is offline; network refusal not compared");
    }
    let work = tempfile::tempdir().unwrap();
    // Positive control: python runs inside the sandbox.
    let ok = run(&spec("python3 -c \"print('py ok')\"", work.path())).unwrap();
    assert!(ok.stdout.contains("py ok"), "{ok:?}");
    let out = run(&spec(&format!("python3 -c \"{probe}\""), work.path())).unwrap();
    assert_ne!(out.exit_code, 0, "{out:?}");
    assert!(!out.stdout.contains("NET OPEN"), "{out:?}");
    // Refused by the network, not by a startup failure.
    assert!(
        out.stderr.contains("OSError") || out.stderr.contains("unreachable"),
        "{out:?}"
    );
}

#[test]
fn writes_land_in_workdir() {
    let work = tempfile::tempdir().unwrap();
    let out = run(&spec("echo hi > out.txt", work.path())).unwrap();
    assert_eq!(out.exit_code, 0, "{out:?}");
    let got = std::fs::read_to_string(work.path().join("out.txt")).unwrap();
    assert_eq!(got.trim(), "hi");
}

#[test]
fn checks_are_read_only() {
    let work = tempfile::tempdir().unwrap();
    let checks = tempfile::tempdir().unwrap();
    std::fs::write(checks.path().join("known.txt"), "hello").unwrap();
    let mut s = spec("cat /checks/known.txt", work.path());
    s.checks_dir = Some(checks.path().to_path_buf());
    let read = run(&s).unwrap();
    assert!(read.stdout.contains("hello"), "{read:?}");
    s.cmd = "touch /checks/x".to_string();
    let out = run(&s).unwrap();
    assert_ne!(out.exit_code, 0, "{out:?}");
    assert!(!checks.path().join("x").exists());
}

#[test]
fn runaway_command_is_killed_and_capped() {
    let work = tempfile::tempdir().unwrap();
    let mut s = spec("yes", work.path());
    s.timeout = Duration::from_secs(2);
    let start = Instant::now();
    let out = run(&s).unwrap();
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "{:?}",
        start.elapsed()
    );
    assert!(out.timed_out);
    assert!(out.truncated);
    assert_eq!(out.exit_code, 124);
    assert!(out.stdout.len() <= 1024 * 1024);
}

#[test]
fn uses_gvisor_kernel() {
    let work = tempfile::tempdir().unwrap();
    let out = run(&spec("cat /proc/version", work.path())).unwrap();
    assert!(out.stdout.contains("gvisor"), "{out:?}");
}

#[test]
fn doctor_reports_ok() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cortex"))
        .arg("doctor")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("sandbox: ok"), "{stdout}");
}
