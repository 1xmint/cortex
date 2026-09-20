use cortex_core::contamination::{EvidenceSignal, EvidenceSource, SignalTier};
use cortex_core::protocol::StepOutput;
use cortex_core::routing::RiskLevel;
use cortex_core::task::TaskContract;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use uuid::Uuid;

use crate::evidence_floor::{check_floor, FloorVerdict};
use crate::risk::classify_risk;

const REQUIRED_CHECK_PREFIX: &str = "required_check:";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerifierWorkContract {
    pub task_id: Option<Uuid>,
    pub objective: String,
    pub risk: RiskLevel,
    pub acceptance_criteria: Vec<String>,
    pub allowed_paths: Vec<String>,
    pub expected_base_commit: Option<String>,
}

impl VerifierWorkContract {
    pub fn from_task_contract(
        task: &TaskContract,
        allowed_paths: Vec<String>,
        expected_base_commit: Option<String>,
    ) -> Self {
        let task_value = serde_json::to_value(task).unwrap_or(Value::Null);
        let allowed_paths = if task.allowed_paths.is_empty() {
            string_array_field(&task_value, "allowed_paths").unwrap_or(allowed_paths)
        } else {
            task.allowed_paths.clone()
        };
        let expected_base_commit = task
            .expected_base_commit
            .clone()
            .or_else(|| optional_string_field(&task_value, "expected_base_commit").flatten())
            .or(expected_base_commit);
        let mut acceptance_criteria = task.acceptance_criteria.clone();
        acceptance_criteria.extend(task.required_checks.iter().filter_map(|check| {
            check
                .required
                .then(|| required_check_name_from_parts(&check.name, &check.command))
                .flatten()
                .map(encode_required_check_criterion)
        }));

        Self {
            task_id: Some(task.id),
            objective: task.objective.clone(),
            risk: task.risk,
            acceptance_criteria,
            allowed_paths,
            expected_base_commit,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StructuredStepEvidence {
    pub step_id: Option<String>,
    pub attempt_id: Option<String>,
    pub summary: String,
    pub exit_code: Option<i32>,
    pub files_changed: Vec<String>,
    pub base_commit: Option<String>,
    pub head_commit: Option<String>,
    pub commands: Vec<CommandEvidence>,
    pub checks: Vec<CheckEvidence>,
    pub signals: Vec<EvidenceSignal>,
}

impl StructuredStepEvidence {
    pub fn from_step_output(
        output: StepOutput,
        exit_code: i32,
        base_commit: Option<String>,
        head_commit: Option<String>,
    ) -> Self {
        Self {
            step_id: None,
            attempt_id: None,
            summary: output.summary,
            exit_code: Some(exit_code),
            files_changed: output.files_changed,
            base_commit,
            head_commit,
            commands: Vec::new(),
            checks: Vec::new(),
            signals: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerifierInput {
    pub contract: VerifierWorkContract,
    pub evidence: StructuredStepEvidence,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommandEvidence {
    pub command: String,
    pub exit_code: Option<i32>,
    pub summary: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckEvidence {
    pub name: String,
    pub status: CheckStatus,
    pub summary: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Passed,
    Failed,
    Skipped,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifierVerdict {
    Success,
    NeedsEvidence,
    Failed,
    Blocked,
}

impl VerifierVerdict {
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifierNextAction {
    Accept,
    AddEvidence,
    FixAndRetry,
    RebaseAndRetry,
    NarrowScope,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerifierReport {
    pub verdict: VerifierVerdict,
    pub risk: RiskLevel,
    pub evidence_signals: Vec<EvidenceSignal>,
    pub evidence_floor: VerifierFloorSummary,
    pub allowed_path_violations: Vec<AllowedPathViolation>,
    pub command_summary: CommandSummary,
    pub check_summary: CheckSummary,
    #[serde(default)]
    pub required_check_summary: RequiredCheckSummary,
    pub acceptance_coverage: AcceptanceCoverage,
    pub stale_base_notes: Vec<StaleBaseNote>,
    pub next_action: VerifierNextAction,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerifierFloorSummary {
    pub satisfied: bool,
    pub signals_met: Vec<String>,
    pub missing: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedPathViolation {
    pub path: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommandSummary {
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub unknown: usize,
    pub failures: Vec<CommandFailure>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommandFailure {
    pub command: String,
    pub exit_code: Option<i32>,
    pub summary: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub unknown: usize,
    pub failures: Vec<CheckFailure>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckFailure {
    pub name: String,
    pub summary: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RequiredCheckSummary {
    pub total: usize,
    pub satisfied: usize,
    pub expected: Vec<CheckEvidence>,
    pub missing: Vec<CheckEvidence>,
    pub failed: Vec<CheckEvidence>,
    pub skipped: Vec<CheckEvidence>,
    pub unknown: Vec<CheckEvidence>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptanceCoverage {
    pub evaluated: bool,
    pub total: usize,
    pub covered: usize,
    pub uncovered: Vec<String>,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaleBaseNote {
    pub expected_base_commit: Option<String>,
    pub observed_base_commit: Option<String>,
    pub note: String,
}

pub fn verify_step(input: VerifierInput) -> VerifierReport {
    verify_contract_evidence(&input.contract, &input.evidence)
}

pub fn verify_contract_evidence(
    contract: &VerifierWorkContract,
    evidence: &StructuredStepEvidence,
) -> VerifierReport {
    let risk = observed_risk(contract.risk, &evidence.files_changed);
    let mut evidence_signals = evidence.signals.clone();
    evidence_signals.extend(signals_from_evidence(evidence));

    let evidence_floor = summarize_floor(check_floor(risk, &evidence_signals));
    let allowed_path_violations =
        find_allowed_path_violations(&contract.allowed_paths, &evidence.files_changed);
    let command_summary = summarize_commands(&evidence.commands);
    let check_summary = summarize_checks(&evidence.checks);
    let required_check_expectations = required_check_expectations(contract);
    let required_check_summary =
        summarize_required_checks(&required_check_expectations, &evidence.checks);
    let acceptance_coverage = placeholder_acceptance_coverage(&contract.acceptance_criteria);
    let stale_base_notes = stale_base_notes(contract, evidence);

    let (verdict, next_action) = decide_verdict(
        evidence.exit_code,
        &allowed_path_violations,
        &command_summary,
        &check_summary,
        &required_check_summary,
        &evidence_floor,
        &stale_base_notes,
    );

    VerifierReport {
        verdict,
        risk,
        evidence_signals,
        evidence_floor,
        allowed_path_violations,
        command_summary,
        check_summary,
        required_check_summary,
        acceptance_coverage,
        stale_base_notes,
        next_action,
    }
}

pub fn find_allowed_path_violations(
    allowed_paths: &[String],
    changed_paths: &[String],
) -> Vec<AllowedPathViolation> {
    if allowed_paths.is_empty() || allowed_paths.iter().any(|p| p.trim() == "*") {
        return Vec::new();
    }

    let normalized_allowed: Vec<String> = allowed_paths
        .iter()
        .filter_map(|path| normalize_repo_path(path).ok())
        .collect();

    changed_paths
        .iter()
        .filter_map(|path| {
            let normalized = match normalize_repo_path(path) {
                Ok(path) => path,
                Err(reason) => {
                    return Some(AllowedPathViolation {
                        path: path.clone(),
                        reason,
                    });
                }
            };

            if normalized_allowed
                .iter()
                .any(|allowed| path_matches_allowed(&normalized, allowed))
            {
                None
            } else {
                Some(AllowedPathViolation {
                    path: path.clone(),
                    reason: "changed path is outside the allowed path contract".into(),
                })
            }
        })
        .collect()
}

pub fn summarize_commands(commands: &[CommandEvidence]) -> CommandSummary {
    let mut summary = CommandSummary {
        total: commands.len(),
        succeeded: 0,
        failed: 0,
        unknown: 0,
        failures: Vec::new(),
    };

    for command in commands {
        match command.exit_code {
            Some(0) => summary.succeeded += 1,
            Some(_) => {
                summary.failed += 1;
                summary.failures.push(CommandFailure {
                    command: command.command.clone(),
                    exit_code: command.exit_code,
                    summary: command.summary.clone(),
                });
            }
            None => summary.unknown += 1,
        }
    }

    summary
}

pub fn summarize_checks(checks: &[CheckEvidence]) -> CheckSummary {
    let mut summary = CheckSummary {
        total: checks.len(),
        passed: 0,
        failed: 0,
        skipped: 0,
        unknown: 0,
        failures: Vec::new(),
    };

    for check in checks {
        match check.status {
            CheckStatus::Passed => summary.passed += 1,
            CheckStatus::Failed => {
                summary.failed += 1;
                summary.failures.push(CheckFailure {
                    name: check.name.clone(),
                    summary: check.summary.clone(),
                });
            }
            CheckStatus::Skipped => summary.skipped += 1,
            CheckStatus::Unknown => summary.unknown += 1,
        }
    }

    summary
}

pub fn summarize_required_checks(
    expectations: &[CheckEvidence],
    checks: &[CheckEvidence],
) -> RequiredCheckSummary {
    let mut summary = RequiredCheckSummary {
        total: expectations.len(),
        expected: expectations.to_vec(),
        ..RequiredCheckSummary::default()
    };

    for expectation in expectations {
        match required_check_match(expectation, checks) {
            Some(check) if check.status == CheckStatus::Passed => {
                summary.satisfied += 1;
            }
            Some(check) if check.status == CheckStatus::Failed => {
                summary.failed.push(check);
            }
            Some(check) if check.status == CheckStatus::Skipped => {
                summary.skipped.push(check);
            }
            Some(check) => {
                summary.unknown.push(check);
            }
            None => {
                summary.missing.push(CheckEvidence {
                    name: expectation.name.clone(),
                    status: CheckStatus::Unknown,
                    summary: Some("required check was not reported".into()),
                });
            }
        }
    }

    summary
}

fn decide_verdict(
    exit_code: Option<i32>,
    allowed_path_violations: &[AllowedPathViolation],
    command_summary: &CommandSummary,
    check_summary: &CheckSummary,
    required_check_summary: &RequiredCheckSummary,
    evidence_floor: &VerifierFloorSummary,
    stale_base_notes: &[StaleBaseNote],
) -> (VerifierVerdict, VerifierNextAction) {
    if !stale_base_notes.is_empty() {
        return (VerifierVerdict::Blocked, VerifierNextAction::RebaseAndRetry);
    }

    if !allowed_path_violations.is_empty() {
        return (VerifierVerdict::Blocked, VerifierNextAction::NarrowScope);
    }

    if exit_code.is_some_and(|code| code != 0)
        || command_summary.failed > 0
        || check_summary.failed > 0
        || !required_check_summary.failed.is_empty()
    {
        return (VerifierVerdict::Failed, VerifierNextAction::FixAndRetry);
    }

    if !required_check_summary.missing.is_empty()
        || !required_check_summary.skipped.is_empty()
        || !required_check_summary.unknown.is_empty()
    {
        return (
            VerifierVerdict::NeedsEvidence,
            VerifierNextAction::AddEvidence,
        );
    }

    if exit_code.is_none() {
        return (
            VerifierVerdict::NeedsEvidence,
            VerifierNextAction::AddEvidence,
        );
    }

    if !evidence_floor.satisfied {
        return (
            VerifierVerdict::NeedsEvidence,
            VerifierNextAction::AddEvidence,
        );
    }

    (VerifierVerdict::Success, VerifierNextAction::Accept)
}

fn observed_risk(contract_risk: RiskLevel, changed_paths: &[String]) -> RiskLevel {
    let changed: Vec<&str> = changed_paths.iter().map(String::as_str).collect();
    contract_risk.max(classify_risk(&changed))
}

fn summarize_floor(verdict: FloorVerdict) -> VerifierFloorSummary {
    match verdict {
        FloorVerdict::Satisfied { signals_met } => VerifierFloorSummary {
            satisfied: true,
            signals_met,
            missing: Vec::new(),
        },
        FloorVerdict::Blocked { missing, .. } => VerifierFloorSummary {
            satisfied: false,
            signals_met: Vec::new(),
            missing,
        },
    }
}

fn signals_from_evidence(evidence: &StructuredStepEvidence) -> Vec<EvidenceSignal> {
    let mut signals = Vec::new();

    for command in &evidence.commands {
        if command.exit_code == Some(0) {
            signals.push(EvidenceSignal::new(
                SignalTier::HardObjective,
                source_for_command(&command.command),
                None,
                None,
                1.0,
                format!("command passed: {}", command.command),
            ));
        }
    }

    for check in &evidence.checks {
        if check.status == CheckStatus::Passed {
            signals.push(EvidenceSignal::new(
                SignalTier::HardObjective,
                source_for_command(&check.name),
                None,
                None,
                1.0,
                format!("check passed: {}", check.name),
            ));
        }
    }

    signals
}

fn source_for_command(command: &str) -> EvidenceSource {
    let lower = command.to_lowercase();
    if lower.contains("lint") || lower.contains("clippy") {
        EvidenceSource::Linter
    } else if lower.contains("test") || lower.contains("spec") {
        EvidenceSource::ExistingTestSuite
    } else if lower.contains("build") || lower.contains("check") || lower.contains("cargo") {
        EvidenceSource::CompilerOutput
    } else {
        EvidenceSource::RuntimeBehavior
    }
}

fn stale_base_notes(
    contract: &VerifierWorkContract,
    evidence: &StructuredStepEvidence,
) -> Vec<StaleBaseNote> {
    match (&contract.expected_base_commit, &evidence.base_commit) {
        (Some(expected), Some(observed)) if expected != observed => vec![StaleBaseNote {
            expected_base_commit: Some(expected.clone()),
            observed_base_commit: Some(observed.clone()),
            note: "step ran against a different base commit than the work contract expected".into(),
        }],
        (Some(expected), None) => vec![StaleBaseNote {
            expected_base_commit: Some(expected.clone()),
            observed_base_commit: None,
            note: "work contract expected a base commit but step evidence did not report one"
                .into(),
        }],
        _ => Vec::new(),
    }
}

fn placeholder_acceptance_coverage(criteria: &[String]) -> AcceptanceCoverage {
    let free_text_criteria: Vec<String> = criteria
        .iter()
        .filter(|criterion| parse_required_check_criterion(criterion).is_none())
        .cloned()
        .collect();

    AcceptanceCoverage {
        evaluated: false,
        total: free_text_criteria.len(),
        covered: 0,
        uncovered: free_text_criteria,
        note: "free-text acceptance criteria are not machine-verified; required checks are reported separately".into(),
    }
}

fn required_check_expectations(contract: &VerifierWorkContract) -> Vec<CheckEvidence> {
    let mut seen = BTreeSet::new();
    contract
        .acceptance_criteria
        .iter()
        .filter_map(|criterion| parse_required_check_criterion(criterion))
        .filter(|name| seen.insert(normalize_check_name(name)))
        .map(|name| CheckEvidence {
            name,
            status: CheckStatus::Unknown,
            summary: Some("required by work contract".into()),
        })
        .collect()
}

fn required_check_match(
    expectation: &CheckEvidence,
    checks: &[CheckEvidence],
) -> Option<CheckEvidence> {
    let expected_name = normalize_check_name(&expectation.name);
    let matches: Vec<&CheckEvidence> = checks
        .iter()
        .filter(|check| normalize_check_name(&check.name) == expected_name)
        .collect();

    matches
        .iter()
        .find(|check| check.status == CheckStatus::Failed)
        .or_else(|| {
            matches
                .iter()
                .find(|check| check.status == CheckStatus::Passed)
        })
        .or_else(|| {
            matches
                .iter()
                .find(|check| check.status == CheckStatus::Skipped)
        })
        .or_else(|| {
            matches
                .iter()
                .find(|check| check.status == CheckStatus::Unknown)
        })
        .map(|check| (*check).clone())
}

fn encode_required_check_criterion(name: String) -> String {
    format!("{REQUIRED_CHECK_PREFIX}{name}")
}

fn parse_required_check_criterion(criterion: &str) -> Option<String> {
    criterion
        .trim()
        .strip_prefix(REQUIRED_CHECK_PREFIX)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

fn normalize_check_name(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

fn string_array_field(value: &Value, field: &str) -> Option<Vec<String>> {
    let array = value.get(field)?.as_array()?;
    Some(
        array
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
    )
}

fn optional_string_field(value: &Value, field: &str) -> Option<Option<String>> {
    match value.get(field)? {
        Value::Null => Some(None),
        Value::String(value) => Some(Some(value.clone())),
        _ => None,
    }
}

fn clean_required_check_name(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn required_check_name_from_parts(name: &str, command: &str) -> Option<String> {
    clean_required_check_name(name).or_else(|| clean_required_check_name(command))
}

fn path_matches_allowed(path: &str, allowed: &str) -> bool {
    if allowed.is_empty() {
        return false;
    }

    path == allowed || path.starts_with(&format!("{}/", allowed.trim_end_matches('/')))
}

fn normalize_repo_path(path: &str) -> Result<String, String> {
    let path = path.trim().replace('\\', "/");
    if path.is_empty() {
        return Err("path is empty".into());
    }

    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err("path escapes the workspace".into());
                }
            }
            part => parts.push(part),
        }
    }

    if parts.is_empty() {
        Err("path is empty after normalization".into())
    } else {
        Ok(parts.join("/"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_core::provider::Tier;
    use cortex_core::task::{RequiredCheck, TaskContract};

    fn contract() -> VerifierWorkContract {
        VerifierWorkContract {
            task_id: None,
            objective: "change engine verifier".into(),
            risk: RiskLevel::Low,
            acceptance_criteria: vec!["tests pass".into()],
            allowed_paths: vec!["crates/engine/src".into()],
            expected_base_commit: None,
        }
    }

    fn evidence() -> StructuredStepEvidence {
        StructuredStepEvidence {
            step_id: Some("step-1".into()),
            attempt_id: Some("attempt-1".into()),
            summary: "done".into(),
            exit_code: Some(0),
            files_changed: vec!["crates/engine/src/verifier.rs".into()],
            base_commit: None,
            head_commit: Some("abc123".into()),
            commands: vec![CommandEvidence {
                command: "cargo test -p cortex-engine verifier".into(),
                exit_code: Some(0),
                summary: None,
            }],
            checks: Vec::new(),
            signals: Vec::new(),
        }
    }

    fn required_check(name: &str) -> String {
        encode_required_check_criterion(name.to_string())
    }

    #[test]
    fn task_contract_fields_feed_verifier_contract() {
        let mut task = TaskContract::new(
            "change engine verifier".into(),
            Tier::Execute,
            RiskLevel::Medium,
        )
        .with_dispatch_contract(vec!["crates/engine/src".into()], Some("base-a".into()));
        task.acceptance_criteria.push("free-text acceptance".into());
        task.required_checks.push(RequiredCheck {
            name: "cargo test -p cortex-engine verifier".into(),
            command: "cargo test -p cortex-engine verifier".into(),
            required: true,
        });
        task.required_checks.push(RequiredCheck {
            name: "optional lint".into(),
            command: "cargo clippy -p cortex-engine".into(),
            required: false,
        });

        let contract = VerifierWorkContract::from_task_contract(
            &task,
            vec!["fallback/path".into()],
            Some("fallback-base".into()),
        );

        assert_eq!(
            contract.allowed_paths,
            vec!["crates/engine/src".to_string()]
        );
        assert_eq!(contract.expected_base_commit, Some("base-a".into()));
        assert!(contract
            .acceptance_criteria
            .contains(&"free-text acceptance".to_string()));

        let required = required_check_expectations(&contract);
        assert_eq!(required.len(), 1);
        assert_eq!(required[0].name, "cargo test -p cortex-engine verifier");
    }

    #[test]
    fn allowed_path_violation_blocks_report() {
        let mut evidence = evidence();
        evidence.files_changed = vec!["crates/api/src/ws.rs".into()];

        let report = verify_contract_evidence(&contract(), &evidence);

        assert_eq!(report.verdict, VerifierVerdict::Blocked);
        assert_eq!(report.next_action, VerifierNextAction::NarrowScope);
        assert_eq!(report.allowed_path_violations.len(), 1);
        assert_eq!(
            report.allowed_path_violations[0].path,
            "crates/api/src/ws.rs"
        );
    }

    #[test]
    fn failed_command_causes_non_success_verdict() {
        let mut evidence = evidence();
        evidence.commands = vec![CommandEvidence {
            command: "cargo test -p cortex-engine".into(),
            exit_code: Some(101),
            summary: Some("test failure".into()),
        }];

        let report = verify_contract_evidence(&contract(), &evidence);

        assert_eq!(report.verdict, VerifierVerdict::Failed);
        assert!(!report.verdict.is_success());
        assert_eq!(report.next_action, VerifierNextAction::FixAndRetry);
        assert_eq!(report.command_summary.failed, 1);
    }

    #[test]
    fn stale_base_blocks_report() {
        let mut contract = contract();
        contract.expected_base_commit = Some("base-a".into());
        let mut evidence = evidence();
        evidence.base_commit = Some("base-b".into());

        let report = verify_contract_evidence(&contract, &evidence);

        assert_eq!(report.verdict, VerifierVerdict::Blocked);
        assert_eq!(report.next_action, VerifierNextAction::RebaseAndRetry);
        assert_eq!(report.stale_base_notes.len(), 1);
        assert_eq!(
            report.stale_base_notes[0].note,
            "step ran against a different base commit than the work contract expected"
        );
    }

    #[test]
    fn missing_required_check_needs_evidence() {
        let mut contract = contract();
        contract
            .acceptance_criteria
            .push(required_check("cargo test -p cortex-engine verifier"));
        let mut evidence = evidence();
        evidence.checks = Vec::new();

        let report = verify_contract_evidence(&contract, &evidence);

        assert_eq!(report.verdict, VerifierVerdict::NeedsEvidence);
        assert_eq!(report.next_action, VerifierNextAction::AddEvidence);
        assert_eq!(report.required_check_summary.total, 1);
        assert_eq!(report.required_check_summary.missing.len(), 1);
        assert_eq!(
            report.required_check_summary.missing[0].name,
            "cargo test -p cortex-engine verifier"
        );
        assert_eq!(report.acceptance_coverage.total, 1);
        assert_eq!(
            report.acceptance_coverage.uncovered,
            vec!["tests pass".to_string()]
        );
    }

    #[test]
    fn failed_required_check_fails_report() {
        let mut contract = contract();
        contract
            .acceptance_criteria
            .push(required_check("cargo test -p cortex-engine verifier"));
        let mut evidence = evidence();
        evidence.checks = vec![CheckEvidence {
            name: "cargo test -p cortex-engine verifier".into(),
            status: CheckStatus::Failed,
            summary: Some("unit failure".into()),
        }];

        let report = verify_contract_evidence(&contract, &evidence);

        assert_eq!(report.verdict, VerifierVerdict::Failed);
        assert_eq!(report.next_action, VerifierNextAction::FixAndRetry);
        assert_eq!(report.required_check_summary.failed.len(), 1);
    }

    #[test]
    fn high_risk_needs_hard_objective_beyond_worker_exit() {
        let mut contract = contract();
        contract.risk = RiskLevel::High;
        let mut evidence = evidence();
        evidence.commands = Vec::new();
        evidence.checks = Vec::new();
        evidence.signals = Vec::new();

        let report = verify_contract_evidence(&contract, &evidence);

        assert_eq!(report.verdict, VerifierVerdict::NeedsEvidence);
        assert!(report
            .evidence_floor
            .missing
            .contains(&"Objective verification required".to_string()));
    }

    #[test]
    fn critical_risk_needs_independent_verify() {
        let mut contract = contract();
        contract.risk = RiskLevel::Critical;

        let report = verify_contract_evidence(&contract, &evidence());

        assert_eq!(report.verdict, VerifierVerdict::NeedsEvidence);
        assert!(report
            .evidence_floor
            .missing
            .contains(&"Independent review required for critical code".to_string()));
    }
}

// ─── V5: verdicts from Cortex-run executions, not from worker claims ───

use cortex_core::verification::{
    compute_verdict, CheckExecution, CheckSpec, Verdict, VerdictReport,
};

/// A verdict computed from checks Cortex executed itself, with the worker's
/// own account of events retained only as a hint.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutedVerification {
    pub verdict: VerifierVerdict,
    pub next_action: VerifierNextAction,
    /// The deterministic result, with its reasoning, exactly as
    /// `compute_verdict` produced it.
    pub gate: VerdictReport,
    /// Everything the worker said, summarized. Present because it is useful
    /// for debugging and short-circuiting, and *only* for that: nothing in
    /// here can change `verdict`.
    pub worker_hints: VerifierReport,
}

/// Verify a step from executed checks.
///
/// This is the V5 boundary. `verify_contract_evidence` grades what the worker
/// reported about itself — the least trustworthy witness available, since in
/// the server-held-key model that worker is executing LLM-authored output.
/// This function grades what Cortex ran, and demotes the worker's account to
/// a hint that travels alongside the verdict without influencing it.
///
/// The mapping to the existing `VerifierVerdict` vocabulary is deliberate:
///
/// - `Verified` → `Success`, accept.
/// - `Failed` → `Failed`, fix and retry.
/// - `Inconclusive` → `Blocked`. Not `Failed`: a dead runner must never refund
///   a customer or charge one. Blocked is the only existing variant that means
///   "stop, this is ours to fix", which is exactly right.
/// - `Unverified` → `NeedsEvidence`. The work may still be sold at the normal
///   rate, but without the badge and without the refund promise, and calling
///   it `Success` here would quietly launder an unverifiable task into a
///   verified one.
pub fn verify_from_executions(
    contract: &VerifierWorkContract,
    worker_evidence: &StructuredStepEvidence,
    specs: &[CheckSpec],
    executions: &[CheckExecution],
) -> ExecutedVerification {
    let gate = compute_verdict(specs, executions);

    let (verdict, next_action) = match gate.verdict {
        Verdict::Verified => (VerifierVerdict::Success, VerifierNextAction::Accept),
        Verdict::Failed => (VerifierVerdict::Failed, VerifierNextAction::FixAndRetry),
        Verdict::Inconclusive => (VerifierVerdict::Blocked, VerifierNextAction::FixAndRetry),
        Verdict::Unverified => (
            VerifierVerdict::NeedsEvidence,
            VerifierNextAction::AddEvidence,
        ),
    };

    ExecutedVerification {
        verdict,
        next_action,
        gate,
        worker_hints: verify_contract_evidence(contract, worker_evidence),
    }
}

#[cfg(test)]
mod executed_tests {
    use super::*;
    use cortex_core::routing::RiskLevel;
    use cortex_core::verification::{CheckOutcome, CheckSource};

    fn contract() -> VerifierWorkContract {
        VerifierWorkContract {
            task_id: None,
            objective: "do the thing".into(),
            risk: RiskLevel::Low,
            acceptance_criteria: Vec::new(),
            allowed_paths: Vec::new(),
            expected_base_commit: None,
        }
    }

    fn spec(id: &str) -> CheckSpec {
        CheckSpec {
            id: id.into(),
            source: CheckSource::Ecosystem,
            command: vec!["cargo".into(), "test".into()],
            timeout_secs: 600,
            required: true,
        }
    }

    fn execution(id: &str, outcome: CheckOutcome) -> CheckExecution {
        CheckExecution {
            spec_id: id.into(),
            exit_code: Some(if matches!(outcome, CheckOutcome::Passed) {
                0
            } else {
                1
            }),
            outcome,
            duration_ms: 10,
            output_digest: "sha256:x".into(),
            output_tail: String::new(),
            runner_image: "img@sha256:y".into(),
        }
    }

    /// The whole point of V5: a worker claiming success cannot produce one.
    #[test]
    fn a_lying_worker_cannot_manufacture_a_pass() {
        let evidence = StructuredStepEvidence {
            exit_code: Some(0),
            checks: vec![CheckEvidence {
                name: "cargo:test".into(),
                status: CheckStatus::Passed,
                summary: Some("all green, honest".into()),
            }],
            ..Default::default()
        };

        let result = verify_from_executions(
            &contract(),
            &evidence,
            &[spec("ecosystem:cargo-test")],
            &[execution("ecosystem:cargo-test", CheckOutcome::Failed)],
        );

        assert_eq!(result.verdict, VerifierVerdict::Failed);
        assert_eq!(result.gate.failed, vec!["ecosystem:cargo-test".to_string()]);
    }

    #[test]
    fn executed_passes_produce_success() {
        let result = verify_from_executions(
            &contract(),
            &StructuredStepEvidence::default(),
            &[spec("a")],
            &[execution("a", CheckOutcome::Passed)],
        );
        assert_eq!(result.verdict, VerifierVerdict::Success);
        assert_eq!(result.next_action, VerifierNextAction::Accept);
    }

    #[test]
    fn an_unrunnable_check_blocks_rather_than_failing() {
        // A dead runner must never refund a customer or charge one.
        let result = verify_from_executions(
            &contract(),
            &StructuredStepEvidence::default(),
            &[spec("a")],
            &[execution("a", CheckOutcome::NotExecuted)],
        );
        assert_eq!(result.verdict, VerifierVerdict::Blocked);
        assert!(!result.gate.verdict.has_billing_effect());
    }

    #[test]
    fn nothing_derivable_is_not_quietly_a_success() {
        let result =
            verify_from_executions(&contract(), &StructuredStepEvidence::default(), &[], &[]);
        assert_eq!(result.verdict, VerifierVerdict::NeedsEvidence);
        assert_eq!(result.gate.verdict, Verdict::Unverified);
    }

    #[test]
    fn worker_evidence_survives_as_a_hint() {
        let evidence = StructuredStepEvidence {
            commands: vec![CommandEvidence {
                command: "cargo test".into(),
                exit_code: Some(0),
                summary: None,
            }],
            ..Default::default()
        };

        let result = verify_from_executions(
            &contract(),
            &evidence,
            &[spec("a")],
            &[execution("a", CheckOutcome::Passed)],
        );

        // Retained for debugging, and demonstrably not load-bearing: the
        // verdict above came from the execution, not from this.
        assert_eq!(result.worker_hints.command_summary.total, 1);
    }
}
