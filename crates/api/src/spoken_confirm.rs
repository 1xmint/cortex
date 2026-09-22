//! Pure state machine for matching a spoken "yes" (or "no") to a pending
//! confirm action during a live voice session.
//!
//! This module does no I/O and owns no clock: every method takes `now`
//! explicitly so it can be driven by tests or by a caller with a real clock.
//!
//! Rules (see plan-voice-yes.md, "Matching"):
//! - The window opens only when [`Matcher::prompt_ended`] is called; any
//!   transcript text delivered before that is ignored.
//! - An utterance starts at its first transcript delta and ends after 1.0s
//!   with no further delta, or immediately when a delegation starts.
//! - The utterance must *start* within 45s of the prompt-ended report.
//! - After lowercasing and stripping punctuation/extra whitespace, an
//!   utterance that exactly matches the yes-list confirms; the cancel-list
//!   cancels; anything else closes the spoken path.
//! - Only one action is tracked at a time; arming a new one replaces the
//!   old, discarding any in-progress utterance.
//! - A result is single-use: once the matcher resolves (Confirm, Cancel,
//!   Closed, or Expired) it yields nothing further until re-armed.

#![allow(dead_code)] // Not wired into any caller yet (see plan slice 0014d).

use std::time::{Duration, Instant};

/// How long the window stays open for an utterance to *start* after the
/// prompt-ended report.
const WINDOW: Duration = Duration::from_secs(45);

/// How long a gap with no new transcript delta ends an in-progress
/// utterance.
const UTTERANCE_GAP: Duration = Duration::from_millis(1_000);

const YES_PHRASES: &[&str] = &[
    "yes",
    "yes please",
    "yes confirm",
    "confirm",
    "confirmed",
    "yes do it",
];

const CANCEL_PHRASES: &[&str] = &["no", "cancel", "no cancel", "cancel it", "no thanks"];

/// Identifier for the action a confirm window is tracking. Kept as an owned
/// `String` because this module has no opinion on how the caller mints ids.
pub type ActionId = String;

/// What the matcher decided about the pending action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeKind {
    /// A spoken "yes" (or equivalent) matched within the window.
    Confirm,
    /// A spoken "no"/"cancel" (or equivalent) matched within the window.
    Cancel,
    /// Speech was heard in the window but did not match yes or cancel; the
    /// spoken path is closed (tapping the card still works elsewhere).
    Closed,
    /// The window closed with no utterance starting in time.
    Expired,
}

/// A terminal decision for a given action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub action_id: ActionId,
    pub kind: OutcomeKind,
}

#[derive(Debug, Clone)]
struct Utterance {
    /// When the first delta of this utterance arrived.
    start: Instant,
    /// When the most recent delta of this utterance arrived.
    last_delta: Instant,
    /// Accumulated, un-normalized transcript text.
    text: String,
}

#[derive(Debug, Clone)]
enum Phase {
    /// Nothing armed.
    Idle,
    /// An action is armed but the prompt-ended report has not arrived yet.
    /// Transcript deltas received in this phase are dropped.
    AwaitingReport { action_id: ActionId },
    /// The window is open: transcript deltas can start or extend an
    /// utterance until it resolves or the window expires.
    Open {
        action_id: ActionId,
        armed_at: Instant,
        deadline: Instant,
        utterance: Option<Utterance>,
    },
    /// A terminal outcome has already been produced; nothing further comes
    /// out until [`Matcher::arm`] starts a new cycle.
    Resolved,
}

/// The spoken-confirm matcher. One instance tracks at most one pending
/// action at a time.
#[derive(Debug, Clone)]
pub struct Matcher {
    phase: Phase,
}

impl Default for Matcher {
    fn default() -> Self {
        Self { phase: Phase::Idle }
    }
}

impl Matcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Arms the matcher for `action_id`. Replaces whatever was pending
    /// before, discarding any in-progress utterance and window. The window
    /// itself does not open until [`Matcher::prompt_ended`] is called.
    pub fn arm(&mut self, action_id: ActionId, _now: Instant) {
        self.phase = Phase::AwaitingReport { action_id };
    }

    /// Opens the matching window: the currently-armed action now has 45s
    /// for a matching utterance to *start*. A no-op if nothing is armed
    /// (e.g. a stale report after a re-arm or resolution) — returns `false`
    /// in that case (window already open, or the matcher isn't waiting on a
    /// report), `true` only when it actually opened the window.
    pub fn prompt_ended(&mut self, now: Instant) -> bool {
        if let Phase::AwaitingReport { action_id } = &self.phase {
            self.phase = Phase::Open {
                action_id: action_id.clone(),
                armed_at: now,
                deadline: now + WINDOW,
                utterance: None,
            };
            true
        } else {
            false
        }
    }

    /// Test-only: whether the window is currently open (`Phase::Open`).
    /// Lets a test confirm a rejected `prompt_ended` did not open the
    /// window, and that a re-arm after resolution put the matcher back into
    /// a state where the window is not open.
    #[cfg(test)]
    pub(crate) fn is_window_open(&self) -> bool {
        matches!(self.phase, Phase::Open { .. })
    }

    /// Feeds a transcript delta. Text before the window opens, or once the
    /// matcher has already resolved, is ignored.
    pub fn on_transcript(&mut self, text: &str, now: Instant) -> Option<Outcome> {
        let Phase::Open {
            action_id,
            deadline,
            utterance,
            ..
        } = &mut self.phase
        else {
            return None;
        };

        match utterance {
            Some(u) => {
                u.text.push(' ');
                u.text.push_str(text);
                u.last_delta = now;
                None
            }
            None => {
                if now > *deadline {
                    // No utterance ever started in time: the window is over.
                    let outcome = Outcome {
                        action_id: action_id.clone(),
                        kind: OutcomeKind::Expired,
                    };
                    self.phase = Phase::Resolved;
                    Some(outcome)
                } else {
                    *utterance = Some(Utterance {
                        start: now,
                        last_delta: now,
                        text: text.to_string(),
                    });
                    None
                }
            }
        }
    }

    /// Notifies the matcher that a delegation (agent turn) has started. If
    /// an utterance is in progress, it ends immediately and is evaluated.
    /// A no-op otherwise.
    pub fn on_delegation(&mut self, now: Instant) -> Option<Outcome> {
        let Phase::Open {
            action_id,
            utterance,
            ..
        } = &self.phase
        else {
            return None;
        };
        let Some(u) = utterance else {
            return None;
        };
        let outcome = Outcome {
            action_id: action_id.clone(),
            kind: classify(&u.text),
        };
        let _ = now;
        self.phase = Phase::Resolved;
        Some(outcome)
    }

    /// Advances time. Ends an in-progress utterance once 1.0s has passed
    /// with no new delta, or expires the window once the deadline has
    /// passed with no utterance ever starting.
    pub fn tick(&mut self, now: Instant) -> Option<Outcome> {
        let Phase::Open {
            action_id,
            deadline,
            utterance,
            ..
        } = &self.phase
        else {
            return None;
        };

        match utterance {
            Some(u) => {
                if now.saturating_duration_since(u.last_delta) >= UTTERANCE_GAP {
                    let outcome = Outcome {
                        action_id: action_id.clone(),
                        kind: classify(&u.text),
                    };
                    self.phase = Phase::Resolved;
                    Some(outcome)
                } else {
                    None
                }
            }
            None => {
                if now > *deadline {
                    let outcome = Outcome {
                        action_id: action_id.clone(),
                        kind: OutcomeKind::Expired,
                    };
                    self.phase = Phase::Resolved;
                    Some(outcome)
                } else {
                    None
                }
            }
        }
    }
}

/// Lowercases, strips punctuation, and collapses whitespace.
fn normalize(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn classify(raw: &str) -> OutcomeKind {
    let normalized = normalize(raw);
    if YES_PHRASES.contains(&normalized.as_str()) {
        OutcomeKind::Confirm
    } else if CANCEL_PHRASES.contains(&normalized.as_str()) {
        OutcomeKind::Cancel
    } else {
        OutcomeKind::Closed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Instant {
        Instant::now()
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn millis(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Arms, opens the window, and returns (matcher, armed_at).
    fn armed_and_open(action_id: &str) -> (Matcher, Instant) {
        let mut m = Matcher::new();
        let t0 = base();
        m.arm(action_id.to_string(), t0);
        m.prompt_ended(t0);
        (m, t0)
    }

    #[test]
    fn transcript_before_report_is_ignored() {
        let mut m = Matcher::new();
        let t0 = base();
        m.arm("a1".into(), t0);
        // No prompt_ended yet: still AwaitingReport.
        assert_eq!(m.on_transcript("yes", t0), None);
        // Now open the window and let the earlier ignored text have no
        // effect: a fresh "yes" utterance still matches normally.
        m.prompt_ended(t0 + millis(10));
        assert_eq!(m.on_transcript("yes", t0 + millis(20)), None);
        let out = m.tick(t0 + millis(20) + UTTERANCE_GAP);
        assert_eq!(
            out,
            Some(Outcome {
                action_id: "a1".into(),
                kind: OutcomeKind::Confirm,
            })
        );
    }

    #[test]
    fn yes_at_44_9s_is_accepted() {
        let (mut m, t0) = armed_and_open("a1");
        let start = t0 + Duration::from_millis(44_900);
        assert_eq!(m.on_transcript("yes", start), None);
        let out = m.tick(start + UTTERANCE_GAP);
        assert_eq!(
            out,
            Some(Outcome {
                action_id: "a1".into(),
                kind: OutcomeKind::Confirm,
            })
        );
    }

    #[test]
    fn yes_at_exactly_45s_is_accepted() {
        let (mut m, t0) = armed_and_open("a1");
        let start = t0 + WINDOW;
        assert_eq!(m.on_transcript("yes", start), None);
        let out = m.tick(start + UTTERANCE_GAP);
        assert_eq!(
            out,
            Some(Outcome {
                action_id: "a1".into(),
                kind: OutcomeKind::Confirm,
            })
        );
    }

    #[test]
    fn yes_at_45_1s_is_rejected() {
        let (mut m, t0) = armed_and_open("a1");
        let start = t0 + Duration::from_millis(45_100);
        // The utterance starts too late: the window expires immediately.
        let out = m.on_transcript("yes", start);
        assert_eq!(
            out,
            Some(Outcome {
                action_id: "a1".into(),
                kind: OutcomeKind::Expired,
            })
        );
        // Single-use: nothing further comes out.
        assert_eq!(m.tick(start + secs(1)), None);
    }

    #[test]
    fn window_expires_with_no_utterance() {
        let (mut m, t0) = armed_and_open("a1");
        assert_eq!(m.tick(t0 + WINDOW), None);
        let out = m.tick(t0 + WINDOW + millis(1));
        assert_eq!(
            out,
            Some(Outcome {
                action_id: "a1".into(),
                kind: OutcomeKind::Expired,
            })
        );
    }

    #[test]
    fn yes_but_wait_is_rejected_and_closes_window() {
        let (mut m, t0) = armed_and_open("a1");
        assert_eq!(m.on_transcript("yes but wait", t0 + millis(10)), None);
        let out = m.tick(t0 + millis(10) + UTTERANCE_GAP);
        assert_eq!(
            out,
            Some(Outcome {
                action_id: "a1".into(),
                kind: OutcomeKind::Closed,
            })
        );
    }

    #[test]
    fn period_only_punctuation_is_accepted() {
        let (mut m, t0) = armed_and_open("a1");
        assert_eq!(m.on_transcript("Yes.", t0 + millis(10)), None);
        let out = m.tick(t0 + millis(10) + UTTERANCE_GAP);
        assert_eq!(
            out,
            Some(Outcome {
                action_id: "a1".into(),
                kind: OutcomeKind::Confirm,
            })
        );
    }

    #[test]
    fn yes_please_with_exclamation_and_caps_is_accepted() {
        let (mut m, t0) = armed_and_open("a1");
        assert_eq!(m.on_transcript("YES please!", t0 + millis(10)), None);
        let out = m.tick(t0 + millis(10) + UTTERANCE_GAP);
        assert_eq!(
            out,
            Some(Outcome {
                action_id: "a1".into(),
                kind: OutcomeKind::Confirm,
            })
        );
    }

    #[test]
    fn all_yes_phrases_match() {
        for phrase in YES_PHRASES {
            let (mut m, t0) = armed_and_open("a1");
            assert_eq!(m.on_transcript(phrase, t0 + millis(10)), None);
            let out = m.tick(t0 + millis(10) + UTTERANCE_GAP);
            assert_eq!(
                out,
                Some(Outcome {
                    action_id: "a1".into(),
                    kind: OutcomeKind::Confirm,
                }),
                "phrase {phrase:?} should confirm"
            );
        }
    }

    #[test]
    fn all_cancel_phrases_match() {
        for phrase in CANCEL_PHRASES {
            let (mut m, t0) = armed_and_open("a1");
            assert_eq!(m.on_transcript(phrase, t0 + millis(10)), None);
            let out = m.tick(t0 + millis(10) + UTTERANCE_GAP);
            assert_eq!(
                out,
                Some(Outcome {
                    action_id: "a1".into(),
                    kind: OutcomeKind::Cancel,
                }),
                "phrase {phrase:?} should cancel"
            );
        }
    }

    #[test]
    fn yeah_yep_ok_are_not_yes() {
        for word in ["yeah", "yep", "ok"] {
            let (mut m, t0) = armed_and_open("a1");
            assert_eq!(m.on_transcript(word, t0 + millis(10)), None);
            let out = m.tick(t0 + millis(10) + UTTERANCE_GAP);
            assert_eq!(
                out,
                Some(Outcome {
                    action_id: "a1".into(),
                    kind: OutcomeKind::Closed,
                }),
                "{word:?} must not confirm"
            );
        }
    }

    #[test]
    fn multi_delta_utterance_is_joined_before_matching() {
        let (mut m, t0) = armed_and_open("a1");
        assert_eq!(m.on_transcript("yes", t0 + millis(10)), None);
        assert_eq!(m.on_transcript("please", t0 + millis(200)), None);
        // Not yet 1.0s since the last delta: no resolution.
        assert_eq!(m.tick(t0 + millis(200) + millis(500)), None);
        let out = m.tick(t0 + millis(200) + UTTERANCE_GAP);
        assert_eq!(
            out,
            Some(Outcome {
                action_id: "a1".into(),
                kind: OutcomeKind::Confirm,
            })
        );
    }

    #[test]
    fn delegation_start_ends_utterance_immediately() {
        let (mut m, t0) = armed_and_open("a1");
        assert_eq!(m.on_transcript("yes", t0 + millis(10)), None);
        // Delegation starts well before the 1.0s gap would have elapsed.
        let out = m.on_delegation(t0 + millis(50));
        assert_eq!(
            out,
            Some(Outcome {
                action_id: "a1".into(),
                kind: OutcomeKind::Confirm,
            })
        );
    }

    #[test]
    fn delegation_with_no_utterance_is_a_no_op() {
        let (mut m, t0) = armed_and_open("a1");
        assert_eq!(m.on_delegation(t0 + millis(10)), None);
        // The window is still open afterwards.
        assert_eq!(m.on_transcript("yes", t0 + millis(20)), None);
        let out = m.tick(t0 + millis(20) + UTTERANCE_GAP);
        assert_eq!(
            out,
            Some(Outcome {
                action_id: "a1".into(),
                kind: OutcomeKind::Confirm,
            })
        );
    }

    #[test]
    fn reused_yes_after_resolution_yields_nothing() {
        let (mut m, t0) = armed_and_open("a1");
        assert_eq!(m.on_transcript("yes", t0 + millis(10)), None);
        let out = m.tick(t0 + millis(10) + UTTERANCE_GAP);
        assert!(matches!(
            out,
            Some(Outcome {
                kind: OutcomeKind::Confirm,
                ..
            })
        ));
        // Further transcript, delegation, and tick calls all yield nothing.
        assert_eq!(m.on_transcript("yes", t0 + secs(5)), None);
        assert_eq!(m.on_delegation(t0 + secs(5)), None);
        assert_eq!(m.tick(t0 + secs(100)), None);
    }

    #[test]
    fn rearming_replaces_pending_action_and_drops_in_progress_utterance() {
        let (mut m, t0) = armed_and_open("a1");
        // Start an utterance on the first action, then re-arm before it
        // resolves.
        assert_eq!(m.on_transcript("yes", t0 + millis(10)), None);
        m.arm("a2".into(), t0 + millis(20));
        m.prompt_ended(t0 + millis(20));
        // The old utterance is gone; a fresh one is required for a2.
        assert_eq!(m.tick(t0 + millis(10) + UTTERANCE_GAP), None);
        assert_eq!(m.on_transcript("yes", t0 + millis(30)), None);
        let out = m.tick(t0 + millis(30) + UTTERANCE_GAP);
        assert_eq!(
            out,
            Some(Outcome {
                action_id: "a2".into(),
                kind: OutcomeKind::Confirm,
            })
        );
    }

    #[test]
    fn utterance_that_started_before_report_is_impossible_by_construction() {
        // Transcript before prompt_ended cannot start an utterance at all,
        // since on_transcript is a no-op until the window is open. This
        // documents that guarantee explicitly.
        let mut m = Matcher::new();
        let t0 = base();
        m.arm("a1".into(), t0);
        assert_eq!(m.on_transcript("yes", t0), None);
        assert_eq!(m.tick(t0 + secs(1)), None);
        // The window never opened, so there is nothing pending to expire
        // either, even long after 45s.
        assert_eq!(m.tick(t0 + secs(100)), None);
    }

    #[test]
    fn idle_matcher_ignores_everything() {
        let mut m = Matcher::new();
        let t0 = base();
        assert_eq!(m.on_transcript("yes", t0), None);
        assert_eq!(m.on_delegation(t0), None);
        assert_eq!(m.tick(t0), None);
    }
}
