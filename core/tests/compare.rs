//! Paired comparison of two arms: the same tasks and tries, a sign test on
//! the pairs where only one arm was accepted, and a verdict decided by the
//! margin and cost cap stated before the run.

use cortex::bench::compare::{Ratio, Rule, Verdict, compare, sign_test};
use cortex::bench::score::{Attempt, Story, summarize};
use cortex::bench::task::Kind;
use cortex::bench::{self, Report};
use cortex::payload::Spend;
use std::path::Path;

/// An arm that ran `tasks` once each; `accepted[i]` and `cost[i]` describe
/// task i.
fn report(arm: &str, accepted: &[bool], cost: &[Option<u64>]) -> Report {
    let attempts: Vec<Attempt> = accepted
        .iter()
        .zip(cost)
        .enumerate()
        .map(|(i, (&ok, &usd))| Attempt {
            task: format!("t{i:02}"),
            kind: Kind::Fresh,
            try_no: 1,
            mission_id: format!("{arm}-{i}"),
            mission_done: ok,
            accepted: ok,
            hidden_failed: Vec::new(),
            sessions: 1,
            seconds: 1.0,
            tokens: 10,
            estimated_usd_micros: usd,
            spent: Spend::Subscription,
            story: Story::default(),
        })
        .collect();
    Report {
        schema: 2,
        arm: arm.into(),
        tier: "multi".into(),
        worker_profile: "fake".into(),
        engine: "fake".into(),
        engine_version: "0".into(),
        provisional: false,
        cortex_commit: "abc".into(),
        started: "2026-10-19T00:00:00Z".into(),
        summary: summarize(&attempts, 1),
        attempts,
    }
}

const RULE: Rule = Rule {
    win_margin: 2,
    max_cost_ratio: 1.5,
};

#[test]
fn compare_refuses_different_task_sets() {
    let a = report("baseline", &[true, false], &[Some(1), Some(1)]);
    let b = report("planner", &[true, false, true], &[Some(1); 3]);
    assert!(compare(&a, &b, &RULE).is_err());

    // Same tasks, but a different number of tries.
    let mut c = report("planner", &[true, false], &[Some(1), Some(1)]);
    c.attempts[1].try_no = 2;
    assert!(compare(&a, &c, &RULE).is_err());
}

#[test]
fn sign_test_known_values() {
    assert!((sign_test(0, 6) - 0.03125).abs() < 1e-9);
    assert_eq!(sign_test(3, 3), 1.0);
    assert_eq!(sign_test(0, 0), 1.0);
}

#[test]
fn unknown_cost_makes_verdict_undecided() {
    let a = report("baseline", &[true, false, false], &[Some(10); 3]);
    let b = report("planner", &[true, true, true], &[Some(10), None, Some(10)]);
    let c = compare(&a, &b, &RULE).unwrap();
    assert_eq!(c.discordant, (0, 2));
    assert_eq!(c.cost_ratio, Ratio::Unknown);
    assert_eq!(c.verdict, Verdict::Undecided);
}

#[test]
fn win_needs_margin_and_cost_cap() {
    // A accepts 2 of 4 for 40 total (20 per accepted). B accepts all 4.
    let a = report("baseline", &[true, true, false, false], &[Some(10); 4]);
    let cheap = report("planner", &[true; 4], &[Some(28); 4]);
    let c = compare(&a, &cheap, &RULE).unwrap();
    assert_eq!(c.accepted, (2, 4));
    assert_eq!(c.discordant, (0, 2));
    assert_eq!(c.cost_ratio, Ratio::Times(1.4));
    assert_eq!(c.verdict, Verdict::Win);

    let dear = report("planner", &[true; 4], &[Some(32); 4]);
    let c = compare(&a, &dear, &RULE).unwrap();
    assert_eq!(c.cost_ratio, Ratio::Times(1.6));
    assert_eq!(c.verdict, Verdict::NoWin);

    // One task better is under the margin, however cheap.
    let close = report("planner", &[true, true, true, false], &[Some(1); 4]);
    assert_eq!(compare(&a, &close, &RULE).unwrap().verdict, Verdict::NoWin);
}

#[test]
fn fixture_files_compare() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/results");
    let a = bench::load(&dir.join("2026-10-19-baseline-fake.json")).unwrap();
    let b = bench::load(&dir.join("2026-10-19-planner-fake.json")).unwrap();
    let c = compare(&a, &b, &RULE).unwrap();
    assert_eq!(c.accepted, (1, 3));
    assert_eq!(c.discordant, (0, 2));
    assert_eq!(c.verdict, Verdict::Win);
}
