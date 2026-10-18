use std::process::Command;

#[test]
fn version_prints() {
    let out = Command::new(env!("CARGO_BIN_EXE_cortex"))
        .arg("--version")
        .output()
        .expect("run cortex");
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.starts_with("cortex 0.1.0"), "got {stdout:?}");
}
