use cortex_core::evaluator::{classify_risk, parse_intent};
use cortex_core::routing::{Intent, RiskLevel};
use cortex_core::task::{AcceptanceCriterion, AcceptanceVerification, WorkKind, WorkRecipeSeed};

use crate::captain::{RunBuilder, StepKind};

pub fn decompose_goal(
    user_id: &str,
    goal: &str,
    file_paths: &[String],
    profile: &str,
) -> Result<RunBuilder, String> {
    let splits = split_compound(goal);

    if splits.is_empty() {
        return Err("empty goal".into());
    }
    if splits.len() > 5 {
        return Err(
            "too many segments (max 5) — please clarify or split into separate requests".into(),
        );
    }

    let mut builder = RunBuilder::new(user_id.to_string(), goal.to_string(), profile.to_string());

    let mut step_indices: Vec<usize> = Vec::new();
    let mut any_sequential = false;

    for (i, split) in splits.iter().enumerate() {
        let intent_ev = parse_intent(&split.text);
        let risk_ev = classify_risk(
            tier_to_static_risk(intent_ev.default_tier),
            file_paths,
            None,
        );

        let kind = intent_to_step_kind(intent_ev.intent);
        let tier = intent_ev.default_tier.to_string();
        let risk = risk_level_str(risk_ev.level);

        let work_kind = intent_to_work_kind(intent_ev.intent);
        let idx = builder.add_step_with_work_kind(kind, work_kind, &tier, &risk, &split.text);
        builder.set_step_recipe_seed(
            idx,
            build_recipe_seed(work_kind, &split.text, file_paths, risk_ev.level),
        );
        step_indices.push(idx);

        if i > 0 && split.ordering == SplitOrdering::Sequential {
            builder.add_sequential(step_indices[i - 1], idx);
            any_sequential = true;
        }
    }

    // If no explicit ordering, check for implicit dependencies
    if !any_sequential && splits.len() > 1 {
        infer_dependencies(&mut builder, &step_indices, &splits);
    }

    builder.validate()?;
    Ok(builder)
}

// --- Split types ---

#[derive(Debug, Clone, PartialEq, Eq)]
enum SplitOrdering {
    First,
    Sequential,
    Parallel,
}

#[derive(Debug, Clone)]
struct Split {
    text: String,
    ordering: SplitOrdering,
}

// --- Compound splitting ---

const SEQUENTIAL_MARKERS: &[&str] = &[
    " and then ",
    " then ",
    " after that ",
    " after ",
    " finally ",
];

const PARALLEL_MARKERS: &[&str] = &[" and also ", " also ", " plus "];

const ACTION_VERBS: &[&str] = &[
    "fix",
    "add",
    "create",
    "implement",
    "build",
    "write",
    "find",
    "explore",
    "search",
    "grep",
    "test",
    "verify",
    "validate",
    "check",
    "review",
    "audit",
    "inspect",
    "refactor",
    "clean",
    "simplify",
    "remove",
    "delete",
    "update",
    "rename",
    "deploy",
    "ship",
    "release",
    "run",
];

fn split_compound(input: &str) -> Vec<Split> {
    let raw_splits = split_on_markers(input);
    let mut result = Vec::new();

    for split in raw_splits {
        let sub = split_on_comma_verb(&split.text);
        if sub.len() > 1 {
            for (i, text) in sub.into_iter().enumerate() {
                let ordering = if i == 0 {
                    split.ordering.clone()
                } else {
                    SplitOrdering::Parallel
                };
                result.push(Split { text, ordering });
            }
        } else {
            result.push(split);
        }
    }

    result
}

fn split_on_markers(input: &str) -> Vec<Split> {
    let mut splits = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut in_backtick = false;
    let mut next_ordering = SplitOrdering::First;

    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let c = chars[i];

        if c == '"' || c == '\'' {
            in_quotes = !in_quotes;
            current.push(c);
            i += 1;
            continue;
        }
        if c == '`' {
            in_backtick = !in_backtick;
            current.push(c);
            i += 1;
            continue;
        }

        if in_quotes || in_backtick {
            current.push(c);
            i += 1;
            continue;
        }

        let remaining = &input[i..];
        let lower_remaining = remaining.to_lowercase();

        // Check sequential markers first (longer matches)
        if let Some(marker) = find_marker(&lower_remaining, SEQUENTIAL_MARKERS) {
            let trimmed = clean_segment(&current);
            if !trimmed.is_empty() {
                splits.push(Split {
                    text: trimmed,
                    ordering: next_ordering,
                });
                next_ordering = SplitOrdering::Sequential;
            }
            current.clear();
            i += marker.len();
            continue;
        }

        // Check parallel markers
        if let Some(marker) = find_marker(&lower_remaining, PARALLEL_MARKERS) {
            let trimmed = clean_segment(&current);
            if !trimmed.is_empty() {
                splits.push(Split {
                    text: trimmed,
                    ordering: next_ordering,
                });
                next_ordering = SplitOrdering::Parallel;
            }
            current.clear();
            i += marker.len();
            continue;
        }

        // " and " — only split at clause boundaries
        if lower_remaining.starts_with(" and ") {
            let before = clean_segment(&current);
            if !before.is_empty() && is_clause_boundary(&before) {
                splits.push(Split {
                    text: before,
                    ordering: next_ordering,
                });
                next_ordering = SplitOrdering::Parallel;
                current.clear();
                i += 5;
                continue;
            }
        }

        current.push(c);
        i += 1;
    }

    let trimmed = clean_segment(&current);
    if !trimmed.is_empty() {
        splits.push(Split {
            text: trimmed,
            ordering: next_ordering,
        });
    }

    splits
}

fn clean_segment(s: &str) -> String {
    s.trim().trim_end_matches(',').trim().to_string()
}

fn find_marker<'a>(text: &str, markers: &[&'a str]) -> Option<&'a str> {
    markers.iter().copied().find(|m| text.starts_with(m))
}

fn split_on_comma_verb(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if bytes[i] == b',' && i + 2 < len && bytes[i + 1] == b' ' {
            let rest = &text[i + 2..];
            let first_word = rest.split_whitespace().next().unwrap_or("");
            let first_lower = first_word.to_lowercase();

            if ACTION_VERBS.iter().any(|v| first_lower == *v) {
                let trimmed = current.trim().trim_end_matches(',').trim().to_string();
                if !trimmed.is_empty() {
                    parts.push(trimmed);
                }
                current.clear();
                i += 2; // skip ", "
                continue;
            }
        }

        current.push(text[i..].chars().next().unwrap());
        i += text[i..].chars().next().unwrap().len_utf8();
    }

    let trimmed = current.trim().trim_end_matches(',').trim().to_string();
    if !trimmed.is_empty() {
        parts.push(trimmed);
    }

    parts
}

fn is_clause_boundary(s: &str) -> bool {
    let last_word = s.split_whitespace().last().unwrap_or("");
    ![
        "the", "a", "an", "is", "are", "was", "in", "on", "to", "for", "with",
    ]
    .contains(&last_word)
}

fn intent_to_step_kind(intent: Intent) -> StepKind {
    match intent {
        Intent::Explore => StepKind::Search,
        Intent::Fix | Intent::Add | Intent::Refactor | Intent::Ship => StepKind::Execute,
        Intent::Test => StepKind::Test,
        Intent::Review | Intent::Think => StepKind::Think,
    }
}

fn intent_to_work_kind(intent: Intent) -> WorkKind {
    match intent {
        Intent::Explore | Intent::Think => WorkKind::Explore,
        Intent::Fix => WorkKind::Modify,
        Intent::Add => WorkKind::Add,
        Intent::Refactor => WorkKind::Refactor,
        Intent::Ship => WorkKind::Ship,
        Intent::Test => WorkKind::Test,
        Intent::Review => WorkKind::Review,
    }
}

fn build_recipe_seed(
    work_kind: WorkKind,
    objective: &str,
    file_paths: &[String],
    risk: RiskLevel,
) -> WorkRecipeSeed {
    let mut constraints = vec![
        "Use the planner-selected work kind as the source of truth for this step".to_string(),
        format!("planner_risk={}", risk_level_str(risk)),
    ];
    if file_paths.is_empty() {
        constraints.push("No explicit target paths were provided by the user".to_string());
    } else {
        constraints.push("Stay inside the user-selected target paths unless the task proves a related file is required".to_string());
    }

    WorkRecipeSeed {
        target_paths: file_paths.to_vec(),
        acceptance: vec![AcceptanceCriterion {
            id: "planner-objective-satisfied".to_string(),
            text: format!(
                "{} step satisfies planner objective: {}",
                work_kind.as_str(),
                objective
            ),
            verification: AcceptanceVerification::Manual,
        }],
        constraints,
    }
}

fn risk_level_str(level: RiskLevel) -> String {
    match level {
        RiskLevel::Low => "low",
        RiskLevel::Medium => "medium",
        RiskLevel::High => "high",
        RiskLevel::Critical => "critical",
    }
    .to_string()
}

fn infer_dependencies(builder: &mut RunBuilder, indices: &[usize], splits: &[Split]) {
    // Search/explore steps naturally come before execute steps
    for (i, split_i) in splits.iter().enumerate() {
        let intent_i = parse_intent(&split_i.text).intent;
        if intent_i == Intent::Explore {
            for (j, split_j) in splits.iter().enumerate() {
                if j == i {
                    continue;
                }
                let intent_j = parse_intent(&split_j.text).intent;
                if matches!(intent_j, Intent::Fix | Intent::Add | Intent::Refactor) {
                    builder.add_sequential(indices[i], indices[j]);
                }
            }
        }
    }

    // Test/build steps should depend on execute steps
    for (i, split_i) in splits.iter().enumerate() {
        let intent_i = parse_intent(&split_i.text).intent;
        if matches!(intent_i, Intent::Fix | Intent::Add | Intent::Refactor) {
            for (j, split_j) in splits.iter().enumerate() {
                if j == i {
                    continue;
                }
                let intent_j = parse_intent(&split_j.text).intent;
                if intent_j == Intent::Test && j > i {
                    builder.add_sequential(indices[i], indices[j]);
                }
            }
        }
    }
}

fn tier_to_static_risk(tier: cortex_core::provider::Tier) -> RiskLevel {
    match tier {
        cortex_core::provider::Tier::Search => RiskLevel::Low,
        cortex_core::provider::Tier::Execute => RiskLevel::Medium,
        cortex_core::provider::Tier::Think => RiskLevel::Medium,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_task() {
        let builder = decompose_goal("u1", "fix the login bug", &[], "auto").unwrap();
        assert_eq!(builder.steps().len(), 1);
        assert_eq!(builder.edges().len(), 0);
    }

    #[test]
    fn compound_with_and() {
        let builder =
            decompose_goal("u1", "fix the login bug and add dark mode", &[], "auto").unwrap();
        assert_eq!(builder.steps().len(), 2);
    }

    #[test]
    fn compound_with_then() {
        let builder = decompose_goal(
            "u1",
            "explore the auth module then fix the bug",
            &[],
            "auto",
        )
        .unwrap();
        assert_eq!(builder.steps().len(), 2);
        assert_eq!(builder.edges().len(), 1);
    }

    #[test]
    fn compound_with_and_then_creates_dependency() {
        let builder = decompose_goal(
            "u1",
            "explore the auth module and then fix the login bug",
            &[],
            "auto",
        )
        .unwrap();
        assert_eq!(builder.steps().len(), 2);
        assert!(
            !builder.edges().is_empty(),
            "and then should create sequential edge"
        );
    }

    #[test]
    fn compound_with_also() {
        let builder =
            decompose_goal("u1", "fix the login bug, also add tests", &[], "auto").unwrap();
        assert_eq!(builder.steps().len(), 2);
    }

    #[test]
    fn preserves_quoted_strings() {
        let builder =
            decompose_goal("u1", "fix the \"foo and bar\" function", &[], "auto").unwrap();
        assert_eq!(builder.steps().len(), 1);
    }

    #[test]
    fn too_many_segments_rejected() {
        let result = decompose_goal(
            "u1",
            "do a and then b and then c and then d and then e and then f",
            &[],
            "auto",
        );
        assert!(result.is_err());
    }

    #[test]
    fn explore_before_fix_inferred() {
        let builder = decompose_goal(
            "u1",
            "find where auth is defined and fix the login bug",
            &[],
            "auto",
        )
        .unwrap();
        assert_eq!(builder.steps().len(), 2);
        assert!(!builder.edges().is_empty());
    }

    #[test]
    fn step_kinds_match_intent() {
        let builder = decompose_goal(
            "u1",
            "explore the codebase then fix the bug then test it",
            &[],
            "auto",
        )
        .unwrap();
        assert_eq!(builder.steps().len(), 3);

        let kinds: Vec<_> = builder.steps().iter().map(|s| s.kind).collect();
        assert_eq!(kinds[0], StepKind::Search);
        assert_eq!(kinds[1], StepKind::Execute);
        assert_eq!(kinds[2], StepKind::Test);

        // All sequential via "then"
        assert_eq!(builder.edges().len(), 2);
    }

    #[test]
    fn step_work_kinds_preserve_planner_intent() {
        let builder = decompose_goal(
            "u1",
            "add login flow then refactor auth module then ship the pull request",
            &[],
            "auto",
        )
        .unwrap();

        let work_kinds: Vec<_> = builder.steps().iter().map(|s| s.work_kind).collect();
        assert_eq!(
            work_kinds,
            vec![WorkKind::Add, WorkKind::Refactor, WorkKind::Ship]
        );
    }

    #[test]
    fn step_recipe_seed_carries_paths_and_acceptance() {
        let builder = decompose_goal(
            "u1",
            "refactor auth module",
            &["crates/api/src/auth.rs".to_string()],
            "auto",
        )
        .unwrap();

        let seed = builder.steps()[0].recipe_seed.as_ref().unwrap();
        assert_eq!(seed.target_paths, vec!["crates/api/src/auth.rs"]);
        assert!(seed.acceptance[0]
            .text
            .contains("refactor step satisfies planner objective"));
        assert!(seed
            .constraints
            .iter()
            .any(|c| c.starts_with("planner_risk=")));
    }

    #[test]
    fn file_path_risk_propagates() {
        let builder = decompose_goal(
            "u1",
            "fix the middleware",
            &["src/auth/middleware.rs".to_string()],
            "auto",
        )
        .unwrap();
        assert_eq!(builder.steps()[0].risk, "critical");
    }

    #[test]
    fn comma_verb_splits_tasks() {
        let builder = decompose_goal(
            "u1",
            "find where the config is loaded, fix the timeout bug, run the tests",
            &[],
            "auto",
        )
        .unwrap();
        assert_eq!(builder.steps().len(), 3);

        let kinds: Vec<_> = builder.steps().iter().map(|s| s.kind).collect();
        assert_eq!(kinds[0], StepKind::Search);
        assert_eq!(kinds[1], StepKind::Execute);
        assert_eq!(kinds[2], StepKind::Test);
    }

    #[test]
    fn comma_verb_with_and_then() {
        let builder = decompose_goal(
            "u1",
            "find where the config is loaded, fix the timeout bug, and then run the tests",
            &[],
            "auto",
        )
        .unwrap();
        assert_eq!(builder.steps().len(), 3);

        // "and then" should create sequential dependency
        // and the comma-verb split should also work
        assert!(!builder.edges().is_empty());
    }

    #[test]
    fn comma_doesnt_split_on_non_verbs() {
        let builder = decompose_goal(
            "u1",
            "fix the bug, which is in the auth module",
            &[],
            "auto",
        )
        .unwrap();
        assert_eq!(builder.steps().len(), 1);
    }

    #[test]
    fn fix_then_test_inferred_dependency() {
        let builder = decompose_goal(
            "u1",
            "fix the login bug and test the auth flow",
            &[],
            "auto",
        )
        .unwrap();
        assert_eq!(builder.steps().len(), 2);
        // execute → test should be inferred
        assert!(!builder.edges().is_empty());
    }
}
