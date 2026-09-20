//! Deriving the required check set.
//!
//! Task **V2** of `cortex/plan/VERIFIER.md`. Replaces the rule that made
//! "verified" hollow: `infer_required_checks` returned an empty vec for every
//! Execute step below High risk, so for most billable work the verdict meant
//! "the CLI exited 0, says the CLI".
//!
//! Three sources, unioned, in this order:
//!
//! 1. **Ecosystem floor** — from what the delivered tree *is*. Mandatory.
//! 2. **Contract checks** — acceptance criteria, frozen before execution.
//! 3. **Risk overlays** — allowed-path enforcement and denied lint warnings at
//!    High and Critical.
//!
//! Kept pure and filesystem-free on purpose: it takes facts about a tree, not
//! a path. The probe that gathers those facts is small, lives at the edge, and
//! is the only part that needs a disk — which means every rule below is unit
//! testable, and the rules are what decide whether a customer is charged.

use crate::routing::RiskLevel;
use crate::verification::{CheckSource, CheckSpec};
use serde::{Deserialize, Serialize};

const DEFAULT_BUILD_TIMEOUT_SECS: u64 = 900;
const DEFAULT_TEST_TIMEOUT_SECS: u64 = 1_800;
const DEFAULT_LINT_TIMEOUT_SECS: u64 = 600;
const DEFAULT_CONTRACT_TIMEOUT_SECS: u64 = 900;

/// What the delivered tree contains. Gathered by a probe at the edge; every
/// rule in this module reads these facts rather than a filesystem.
///
/// Deliberately derived from the tree and **not** from `allowed_paths`: the
/// old path-sniffing missed repo-level effects, so a change that broke a
/// crate it never named went unchecked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EcosystemFacts {
    pub has_cargo_manifest: bool,
    pub has_package_json: bool,
    /// Script names present in `package.json` and not placeholders — an
    /// `"echo no test specified && exit 1"` script is worse than no script,
    /// because it would make the floor fail on every run.
    pub npm_scripts: Vec<String>,
}

impl EcosystemFacts {
    fn has_npm_script(&self, name: &str) -> bool {
        self.npm_scripts.iter().any(|script| script == name)
    }

    /// True when no ecosystem owns anything here, so no floor can be derived.
    pub fn is_empty(&self) -> bool {
        !self.has_cargo_manifest && !self.has_package_json
    }
}

/// Everything needed to decide what must pass.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivationInput {
    pub facts: EcosystemFacts,
    /// Commands from the task contract's acceptance criteria, frozen at plan
    /// time. A check added after the worker has seen the task is advisory
    /// only — otherwise the optimizing process picks its own exam.
    pub contract_commands: Vec<Vec<String>>,
    pub risk: RiskLevel,
    /// Whether the step declared a path allowlist worth enforcing.
    pub has_allowed_paths: bool,
}

/// `RiskLevel` has no `Default` in `routing`, and giving it one there would be
/// a semantic claim about routing rather than about derivation. Low is the
/// right default here specifically: it is the level at which the old rule
/// derived nothing, so tests that omit risk exercise the case V2 exists to fix.
impl Default for DerivationInput {
    fn default() -> Self {
        Self {
            facts: EcosystemFacts::default(),
            contract_commands: Vec::new(),
            risk: RiskLevel::Low,
            has_allowed_paths: false,
        }
    }
}

fn spec(
    id: &str,
    source: CheckSource,
    command: &[&str],
    timeout_secs: u64,
    required: bool,
) -> CheckSpec {
    CheckSpec {
        id: id.to_string(),
        source,
        command: command.iter().map(|part| (*part).to_string()).collect(),
        timeout_secs,
        required,
    }
}

/// The ecosystem floor. Mandatory for any tree an ecosystem owns.
///
/// `--locked` on both cargo commands is not decoration: without it a
/// verification run may resolve different dependency versions than the ones
/// the work was written against, which makes a green verdict a statement
/// about a tree nobody delivered.
fn ecosystem_floor(facts: &EcosystemFacts) -> Vec<CheckSpec> {
    let mut checks = Vec::new();

    if facts.has_cargo_manifest {
        checks.push(spec(
            "ecosystem:cargo-check",
            CheckSource::Ecosystem,
            &["cargo", "check", "--locked", "--workspace"],
            DEFAULT_BUILD_TIMEOUT_SECS,
            true,
        ));
        checks.push(spec(
            "ecosystem:cargo-test",
            CheckSource::Ecosystem,
            &["cargo", "test", "--locked", "--workspace"],
            DEFAULT_TEST_TIMEOUT_SECS,
            true,
        ));
    }

    if facts.has_package_json {
        // `npm ci` rather than `npm install`, for the same reason as
        // `--locked`: verify the tree that was delivered.
        checks.push(spec(
            "ecosystem:npm-ci",
            CheckSource::Ecosystem,
            &["npm", "ci"],
            DEFAULT_BUILD_TIMEOUT_SECS,
            true,
        ));
        if facts.has_npm_script("build") {
            checks.push(spec(
                "ecosystem:npm-build",
                CheckSource::Ecosystem,
                &["npm", "run", "build"],
                DEFAULT_BUILD_TIMEOUT_SECS,
                true,
            ));
        }
        if facts.has_npm_script("test") {
            checks.push(spec(
                "ecosystem:npm-test",
                CheckSource::Ecosystem,
                &["npm", "test"],
                DEFAULT_TEST_TIMEOUT_SECS,
                true,
            ));
        }
    }

    checks
}

/// Overlays that only apply where a mistake is expensive.
fn risk_overlays(input: &DerivationInput) -> Vec<CheckSpec> {
    if input.risk < RiskLevel::High {
        return Vec::new();
    }

    let mut checks = Vec::new();

    if input.has_allowed_paths {
        // Path-allowlist enforcement already exists in verifier.rs as
        // `find_allowed_path_violations`. At High risk it stops being an
        // advisory summary and becomes a check that can fail a verdict.
        checks.push(spec(
            "risk:allowed-paths",
            CheckSource::Risk,
            &["cortex-verify", "allowed-paths"],
            DEFAULT_LINT_TIMEOUT_SECS,
            true,
        ));
    }

    if input.facts.has_cargo_manifest {
        checks.push(spec(
            "risk:cargo-clippy",
            CheckSource::Risk,
            &[
                "cargo",
                "clippy",
                "--locked",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
            DEFAULT_LINT_TIMEOUT_SECS,
            true,
        ));
    }

    if input.facts.has_package_json && input.facts.has_npm_script("lint") {
        checks.push(spec(
            "risk:npm-lint",
            CheckSource::Risk,
            &["npm", "run", "lint"],
            DEFAULT_LINT_TIMEOUT_SECS,
            true,
        ));
    }

    checks
}

fn contract_checks(commands: &[Vec<String>]) -> Vec<CheckSpec> {
    commands
        .iter()
        .filter(|command| !command.is_empty())
        .enumerate()
        .map(|(index, command)| CheckSpec {
            id: format!("contract:{index}"),
            source: CheckSource::Contract,
            command: command.clone(),
            timeout_secs: DEFAULT_CONTRACT_TIMEOUT_SECS,
            required: true,
        })
        .collect()
}

/// Derive the required check set. The union of all three sources.
///
/// An empty result is meaningful rather than exceptional: the step is
/// UNVERIFIED **by construction**, and must be labelled that way at plan time
/// so the user knows what they are buying before pressing go — not after.
/// See [`is_unverified_by_construction`].
pub fn derive_checks(input: &DerivationInput) -> Vec<CheckSpec> {
    let mut checks = ecosystem_floor(&input.facts);
    checks.extend(contract_checks(&input.contract_commands));
    checks.extend(risk_overlays(input));

    // Sources can collide — a contract that names `cargo test` duplicates the
    // floor. Deduplicate on the command itself, keeping the first occurrence,
    // so the floor's version wins and a check never runs twice on one tree.
    let mut seen: Vec<Vec<String>> = Vec::new();
    checks.retain(|check| {
        if seen.contains(&check.command) {
            false
        } else {
            seen.push(check.command.clone());
            true
        }
    });

    checks
}

/// Whether this step can produce a verdict at all. False means the work is
/// still sellable — at normal rates, labelled UNVERIFIED, without the badge
/// and without the refund promise.
pub fn is_unverified_by_construction(checks: &[CheckSpec]) -> bool {
    !checks.iter().any(|check| check.required)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cargo_tree() -> EcosystemFacts {
        EcosystemFacts {
            has_cargo_manifest: true,
            has_package_json: false,
            npm_scripts: Vec::new(),
        }
    }

    fn npm_tree(scripts: &[&str]) -> EcosystemFacts {
        EcosystemFacts {
            has_cargo_manifest: false,
            has_package_json: true,
            npm_scripts: scripts.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    fn ids(checks: &[CheckSpec]) -> Vec<String> {
        checks.iter().map(|check| check.id.clone()).collect()
    }

    #[test]
    fn a_cargo_tree_gets_check_and_test_as_a_floor() {
        let input = DerivationInput {
            facts: cargo_tree(),
            risk: RiskLevel::Low,
            ..Default::default()
        };
        let checks = derive_checks(&input);
        assert_eq!(
            ids(&checks),
            vec!["ecosystem:cargo-check", "ecosystem:cargo-test"]
        );
        assert!(checks.iter().all(|check| check.required));
    }

    #[test]
    fn low_risk_work_is_no_longer_unchecked() {
        // The whole point of V2: below High risk the old rule returned an
        // empty vec, so "verified" meant the CLI exited zero.
        let input = DerivationInput {
            facts: cargo_tree(),
            risk: RiskLevel::Low,
            ..Default::default()
        };
        assert!(!is_unverified_by_construction(&derive_checks(&input)));
    }

    #[test]
    fn cargo_checks_are_locked() {
        let input = DerivationInput {
            facts: cargo_tree(),
            ..Default::default()
        };
        for check in derive_checks(&input) {
            assert!(
                check.command.contains(&"--locked".to_string()),
                "{:?} must pin dependencies or the verdict describes a different tree",
                check.command
            );
        }
    }

    #[test]
    fn npm_floor_follows_the_scripts_that_exist() {
        let input = DerivationInput {
            facts: npm_tree(&["build"]),
            ..Default::default()
        };
        let checks = derive_checks(&input);
        assert_eq!(
            ids(&checks),
            vec!["ecosystem:npm-ci", "ecosystem:npm-build"]
        );
    }

    #[test]
    fn a_missing_npm_script_is_not_invented() {
        let input = DerivationInput {
            facts: npm_tree(&[]),
            ..Default::default()
        };
        assert_eq!(ids(&derive_checks(&input)), vec!["ecosystem:npm-ci"]);
    }

    #[test]
    fn a_polyglot_tree_gets_both_floors() {
        let input = DerivationInput {
            facts: EcosystemFacts {
                has_cargo_manifest: true,
                has_package_json: true,
                npm_scripts: vec!["test".to_string()],
            },
            ..Default::default()
        };
        let checks = derive_checks(&input);
        assert!(checks.iter().any(|c| c.id == "ecosystem:cargo-test"));
        assert!(checks.iter().any(|c| c.id == "ecosystem:npm-test"));
    }

    #[test]
    fn risk_overlays_apply_only_at_high_and_above() {
        let low = DerivationInput {
            facts: cargo_tree(),
            risk: RiskLevel::Medium,
            has_allowed_paths: true,
            ..Default::default()
        };
        assert!(!ids(&derive_checks(&low)).contains(&"risk:allowed-paths".to_string()));

        let high = DerivationInput {
            risk: RiskLevel::High,
            ..low
        };
        let checks = ids(&derive_checks(&high));
        assert!(checks.contains(&"risk:allowed-paths".to_string()));
        assert!(checks.contains(&"risk:cargo-clippy".to_string()));
    }

    #[test]
    fn contract_checks_are_included_and_required() {
        let input = DerivationInput {
            facts: EcosystemFacts::default(),
            contract_commands: vec![vec!["./scripts/acceptance.sh".to_string()]],
            ..Default::default()
        };
        let checks = derive_checks(&input);
        assert_eq!(ids(&checks), vec!["contract:0"]);
        assert_eq!(checks[0].source, CheckSource::Contract);
        assert!(checks[0].required);
    }

    #[test]
    fn an_empty_contract_command_is_dropped_not_run() {
        let input = DerivationInput {
            contract_commands: vec![Vec::new()],
            ..Default::default()
        };
        assert!(derive_checks(&input).is_empty());
    }

    #[test]
    fn a_duplicate_command_runs_once() {
        let input = DerivationInput {
            facts: cargo_tree(),
            contract_commands: vec![vec![
                "cargo".to_string(),
                "test".to_string(),
                "--locked".to_string(),
                "--workspace".to_string(),
            ]],
            ..Default::default()
        };
        let checks = derive_checks(&input);
        assert_eq!(
            ids(&checks),
            vec!["ecosystem:cargo-check", "ecosystem:cargo-test"],
            "the floor's version should win and the contract duplicate should drop"
        );
    }

    #[test]
    fn a_tree_no_ecosystem_owns_is_unverified_by_construction() {
        // "Rewrite this README" — genuinely unverifiable, still sellable,
        // labelled honestly.
        let input = DerivationInput::default();
        let checks = derive_checks(&input);
        assert!(checks.is_empty());
        assert!(is_unverified_by_construction(&checks));
    }
}
