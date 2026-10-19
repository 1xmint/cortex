// Stamps the binary with the commit it was built from (CORTEX_BUILD), so
// every event can name the build that wrote it.
use std::process::Command;

fn main() {
    let build = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=CORTEX_BUILD={build}");
    for path in ["../.git/HEAD", "../.git/refs", "../.git/packed-refs"] {
        if std::path::Path::new(path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}
