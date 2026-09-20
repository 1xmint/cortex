//! Reading a delivered tree into [`EcosystemFacts`].
//!
//! The edge of task **V2**. Every rule about *what must pass* lives in
//! `cortex_core::check_derivation` and is unit tested without a disk; this is
//! the one piece that needs a filesystem, so it is kept as small and as dull
//! as possible.
//!
//! It reads the delivered tree rather than `allowed_paths`, which is the
//! correction VERIFIER.md asks for: path-sniffing misses repo-level effects,
//! so a change that broke a crate it never named went unchecked.

use std::path::Path;

use cortex_core::check_derivation::EcosystemFacts;
use cortex_core::egress::EcosystemManifests;

/// Scripts a placeholder generator writes, which must never become a required
/// check: `npm test` on a freshly generated package fails by design, and a
/// floor that always fails is as useless as one that always passes.
fn is_placeholder_script(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    normalized.contains("no test specified") || normalized.is_empty()
}

/// Script names the ecosystem floor knows how to use. Anything else in
/// `package.json` is a project's own business.
const KNOWN_SCRIPTS: [&str; 3] = ["build", "test", "lint"];

pub fn probe_ecosystem(workspace_dir: &Path) -> EcosystemFacts {
    let has_cargo_manifest = workspace_dir.join("Cargo.toml").is_file();

    let package_json_path = workspace_dir.join("package.json");
    let has_package_json = package_json_path.is_file();

    let npm_scripts = if has_package_json {
        read_npm_scripts(&package_json_path)
    } else {
        Vec::new()
    };

    EcosystemFacts {
        has_cargo_manifest,
        has_package_json,
        npm_scripts,
    }
}

/// Which dependency manifests the tree contains.
///
/// A second, deliberately separate walk from [`probe_ecosystem`]. They answer
/// different questions — what must pass, versus what must be reachable — and a
/// `go.mod` is the clearest case: it justifies the Go module proxy and
/// contributes nothing to the check floor. Sharing one struct would mean a new
/// ecosystem could only be granted egress by also changing what verification
/// requires.
///
/// Existence checks only. Nothing here parses a manifest, because a grant is
/// justified by "this repository uses cargo", not by anything inside the file.
pub fn probe_manifests(workspace_dir: &Path) -> EcosystemManifests {
    EcosystemManifests {
        cargo: workspace_dir.join("Cargo.toml").is_file(),
        npm: workspace_dir.join("package.json").is_file(),
        // Either of the two ways a Python project declares dependencies.
        pypi: workspace_dir.join("pyproject.toml").is_file()
            || workspace_dir.join("requirements.txt").is_file(),
        go: workspace_dir.join("go.mod").is_file(),
    }
}

fn read_npm_scripts(package_json_path: &Path) -> Vec<String> {
    // A malformed or unreadable package.json yields no scripts rather than an
    // error. The floor still requires `npm ci`, which will fail loudly and
    // truthfully if the manifest is broken — that is a real verdict about the
    // delivered work, not a derivation failure.
    let Ok(raw) = std::fs::read_to_string(package_json_path) else {
        return Vec::new();
    };
    let Ok(package) = serde_json::from_str::<serde_json::Value>(&raw) else {
        tracing::warn!(
            path = %package_json_path.display(),
            "package.json did not parse; deriving no script checks from it"
        );
        return Vec::new();
    };
    let Some(scripts) = package.get("scripts").and_then(|value| value.as_object()) else {
        return Vec::new();
    };

    KNOWN_SCRIPTS
        .iter()
        .filter(|name| {
            scripts
                .get(**name)
                .and_then(|value| value.as_str())
                .is_some_and(|value| !is_placeholder_script(value))
        })
        .map(|name| (*name).to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cortex-probe-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn an_empty_tree_owns_nothing() {
        let dir = tempdir();
        let facts = probe_ecosystem(&dir);
        assert!(facts.is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_cargo_manifest_is_detected() {
        let dir = tempdir();
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let facts = probe_ecosystem(&dir);
        assert!(facts.has_cargo_manifest);
        assert!(!facts.has_package_json);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn real_scripts_are_read_and_placeholders_are_not() {
        let dir = tempdir();
        fs::write(
            dir.join("package.json"),
            r#"{"scripts":{"build":"vite build","test":"echo \"Error: no test specified\" && exit 1","lint":"eslint ."}}"#,
        )
        .unwrap();
        let facts = probe_ecosystem(&dir);
        assert!(facts.has_package_json);
        assert_eq!(facts.npm_scripts, vec!["build", "lint"]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_broken_manifest_yields_no_scripts_rather_than_panicking() {
        let dir = tempdir();
        fs::write(dir.join("package.json"), "{ not json").unwrap();
        let facts = probe_ecosystem(&dir);
        assert!(facts.has_package_json);
        assert!(facts.npm_scripts.is_empty());
        fs::remove_dir_all(&dir).ok();
    }
}
