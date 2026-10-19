//! The results page: one self-contained HTML file per run. Everything a
//! model wrote passes through one escape, and costs that are not known are
//! said in words, never shown as $0.

use cortex::bench::Report;
use cortex::bench::page::render;
use cortex::bench::score::{Attempt, Failure, Story, summarize};
use cortex::bench::task::Kind;
use cortex::payload::Spend;

fn report(attempts: Vec<Attempt>) -> Report {
    Report {
        schema: 2,
        arm: "baseline".into(),
        tier: "multi".into(),
        worker_profile: "claude-cli-sonnet".into(),
        engine: "claude".into(),
        engine_version: "1".into(),
        provisional: true,
        cortex_commit: "abc".into(),
        started: "2026-10-19T00:00:00Z".into(),
        summary: summarize(&attempts, 1),
        attempts,
    }
}

fn attempt(accepted: bool, cost: Option<u64>, story: Story) -> Attempt {
    Attempt {
        task: "m01-snake".into(),
        kind: Kind::Fresh,
        try_no: 1,
        mission_id: "m".into(),
        mission_done: true,
        accepted,
        hidden_failed: Vec::new(),
        sessions: 2,
        seconds: 61.0,
        tokens: 1234,
        estimated_usd_micros: cost,
        spent: Spend::Subscription,
        story,
    }
}

#[test]
fn page_escapes_model_text() {
    let story = Story {
        request: "Make a game & win".into(),
        commit: Some("deadbeef".into()),
        files_changed: vec!["<script>alert(1)</script>.js".into()],
        failures: vec![Failure {
            check_id: "board".into(),
            hidden: true,
            output_tail: "<img src=x onerror=1>".into(),
        }],
        stuck_reason: Some("ran out of \"sessions\"".into()),
    };
    let html = render(&report(vec![attempt(false, Some(5), story)]));
    assert!(!html.contains("<script>"), "{html}");
    assert!(!html.contains("<img"), "{html}");
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;.js"));
    assert!(html.contains("&lt;img src=x onerror=1&gt;"));
    assert!(html.contains("Make a game &amp; win"));
    assert!(html.contains("hidden"));
    assert!(html.contains("deadbeef"));
    assert!(html.contains("ran out of &quot;sessions&quot;"));
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains("prefers-color-scheme: dark"));
}

#[test]
fn page_shows_unbounded_and_unknown_in_words() {
    let nothing = render(&report(vec![attempt(false, Some(5), Story::default())]));
    assert!(
        nothing.contains("unbounded (nothing accepted)"),
        "{nothing}"
    );

    let unpriced = render(&report(vec![attempt(true, None, Story::default())]));
    assert!(unpriced.contains("unknown"), "{unpriced}");
    assert!(!unpriced.contains("$0"), "{unpriced}");
}
