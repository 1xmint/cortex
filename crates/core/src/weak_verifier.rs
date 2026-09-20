//! Invariant 26: a model may rank, and may never pass.
//!
//! Phase 6.7 rejected multi-model debate and consensus on measured grounds, and
//! that rejection stands. But it was expressed as *models propose, never grade*,
//! and taken absolutely that rule forecloses the best-measured result in the
//! area: an ensemble of **weak** verifiers is a materially stronger selector
//! than any one of them, and beats both self-consistency and a single reward
//! model at shrinking the generation–verification gap.
//!
//! The two positions reconcile on a clean boundary:
//!
//! > Executed checks are the only thing that can create a pass. A model acting
//! > as a verifier is a **weak** signal: it may rank candidates *within* the set
//! > that already passed the executed battery, it may raise a finding for a
//! > human, and it may lower confidence. It can never move a `failed` to a
//! > `verified`, never create a badge, and never emit a positive routing reward.
//!
//! Under that boundary a weak verifier is safe by construction. The worst a
//! compromised or simply wrong one can do is pick a poorer member of a set whose
//! every member already passed — and it attacks exactly the residual executed
//! checks cannot reach, which is choosing between five candidates that all pass
//! the battery. Phase 27.4's cross-attempt agreement is the cheapest weak
//! verifier available and needs no extra inference at all.
//!
//! # Where the boundary is enforced
//!
//! Not in review. [`PassedCandidate`] is constructible only from a
//! [`Verdict::Verified`], and [`select`] returns a borrow *out of the slice it
//! was given*. So the signature already proves the two things that matter: a
//! failing candidate cannot enter the set, and nothing outside the set can come
//! out. There is no code path to test for, because there is no code path.
//!
//! What still needs tests is the behaviour at the edges — a model that votes for
//! something that did not pass, and a model that says nothing at all. Both must
//! be inert, and the second is the one people get wrong: a weak verifier that
//! could withhold its opinion to block a delivery would be grading by omission.
//!
//! This is also where the EvilGenie result lives. An inspecting judge beat
//! held-out tests at *detecting* reward hacks; as a detector feeding Phase
//! 27.2's disclosure list that is a weak verifier doing what weak verifiers are
//! good at, and as a grader it would be a model-authored verdict. The
//! distinction is the whole difference between a useful signal and a laundered
//! opinion.

use serde::{Deserialize, Serialize};

use crate::verification::Verdict;

/// A candidate that has already passed the executed battery.
///
/// The only constructor takes a [`Verdict`] and refuses anything but
/// [`Verdict::Verified`]. That refusal is the invariant: a weak verifier is
/// never handed a failing candidate, so it can never promote one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PassedCandidate {
    id: String,
}

impl PassedCandidate {
    /// `Some` only for a verdict the executed battery produced.
    ///
    /// `Unverified` is refused too, and deliberately: it means no required
    /// checks were derivable, so there is no battery for a model to rank
    /// *within*. Ranking there would be the model supplying the grade.
    pub fn from_verdict(id: impl Into<String>, verdict: Verdict) -> Option<Self> {
        match verdict {
            Verdict::Verified => Some(Self { id: id.into() }),
            Verdict::Failed | Verdict::Inconclusive | Verdict::Unverified => None,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

/// Everything a model acting as a verifier is permitted to say.
///
/// There is no variant that passes, fails, or badges anything, which is why
/// this is an enum and not a struct with a verdict field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeakSignal {
    /// "Of the ones that passed, this is the better one."
    Prefer { candidate_id: String },
    /// "A human should look at this." Never blocks on its own.
    Finding { note: String },
    /// "I would not lean on this." Lowers confidence, changes no verdict.
    Doubt { note: String },
}

/// Pick a winner from candidates that have already passed.
///
/// Preferences for anything outside `candidates` are discarded rather than
/// honoured, and the fallback when the signals are silent, name nobody valid, or
/// tie is the first candidate in input order. That fallback is the point: every
/// member of this slice passed the battery and is deliverable on that basis
/// alone, so a weak verifier must not be able to withhold delivery by declining
/// to speak.
///
/// Ties break on input order, so the same inputs pick the same winner. A
/// selector that depended on iteration order would make a receipt unreproducible
/// for no gain.
pub fn select<'a>(
    candidates: &'a [PassedCandidate],
    signals: &[WeakSignal],
) -> Option<&'a PassedCandidate> {
    let mut best = candidates.first()?;
    let mut best_votes = 0usize;
    for candidate in candidates {
        let votes = signals
            .iter()
            .filter(|s| match s {
                WeakSignal::Prefer { candidate_id } => candidate_id == &candidate.id,
                _ => false,
            })
            .count();
        if votes > best_votes {
            best = candidate;
            best_votes = votes;
        }
    }
    Some(best)
}

/// The findings a weak verifier raised, for the disclosure list.
///
/// Carried, never acted on. A finding that could fail a step would be a
/// model-authored verdict wearing a different word.
pub fn findings(signals: &[WeakSignal]) -> Vec<&str> {
    signals
        .iter()
        .filter_map(|s| match s {
            WeakSignal::Finding { note } => Some(note.as_str()),
            _ => None,
        })
        .collect()
}

/// A routing reward derived from weak signals. Never positive.
///
/// Feed this to `EvidenceSignal::new` as `raw_reward` instead of a bare float.
/// Routing rewards steer future dispatch, so a positive one sourced from a model
/// would let models teach the router which models to prefer — a loop that closes
/// on itself with no executed check anywhere inside it. Doubt may still push a
/// provider down, because a wrongly-lowered reward costs a little routing
/// quality and a wrongly-raised one costs the guarantee.
pub fn weak_routing_reward(signals: &[WeakSignal]) -> f64 {
    let doubts = signals
        .iter()
        .filter(|s| matches!(s, WeakSignal::Doubt { .. }))
        .count();
    if doubts == 0 {
        // Not `-0.0`, which is what negating a zero product would put on a
        // receipt.
        return 0.0;
    }
    -(doubts as f64 * 0.1).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passed(id: &str) -> PassedCandidate {
        PassedCandidate::from_verdict(id, Verdict::Verified).expect("verified is admissible")
    }

    fn prefer(id: &str) -> WeakSignal {
        WeakSignal::Prefer {
            candidate_id: id.to_string(),
        }
    }

    fn doubt(note: &str) -> WeakSignal {
        WeakSignal::Doubt {
            note: note.to_string(),
        }
    }

    #[test]
    fn only_a_verified_candidate_can_be_ranked() {
        // The invariant, at its only entry point. Every non-passing verdict is
        // refused, including `Unverified` -- with no derivable battery there is
        // nothing for a model to rank *within*, so ranking would be grading.
        for refused in [Verdict::Failed, Verdict::Inconclusive, Verdict::Unverified] {
            assert!(
                PassedCandidate::from_verdict("c", refused).is_none(),
                "{refused:?} must not be admissible to a weak ranking"
            );
        }
        assert!(PassedCandidate::from_verdict("c", Verdict::Verified).is_some());
    }

    #[test]
    fn a_silent_weak_verifier_cannot_block_delivery() {
        // The failure people miss. If saying nothing withheld the winner, a
        // model could veto a passing delivery by staying quiet, which is
        // grading by omission.
        let candidates = [passed("a"), passed("b")];
        assert_eq!(select(&candidates, &[]).map(|c| c.id()), Some("a"));
    }

    #[test]
    fn a_vote_for_something_that_did_not_pass_is_discarded() {
        // "ghost" never cleared the battery, so it is not in the set and cannot
        // be voted into it. The vote is dropped, not honoured, and delivery
        // continues from the candidates that did pass.
        let candidates = [passed("a"), passed("b")];
        let winner = select(&candidates, &[prefer("ghost"), prefer("ghost")]);
        assert_eq!(winner.map(|c| c.id()), Some("a"));
    }

    #[test]
    fn preference_moves_the_winner_within_the_passing_set() {
        // The thing weak verifiers are actually for: three candidates all pass,
        // and executed checks have nothing left to say about which is better.
        let candidates = [passed("a"), passed("b"), passed("c")];
        let winner = select(&candidates, &[prefer("c"), prefer("c"), prefer("b")]);
        assert_eq!(winner.map(|c| c.id()), Some("c"));
    }

    #[test]
    fn a_tie_breaks_on_input_order_so_the_receipt_reproduces() {
        let candidates = [passed("a"), passed("b")];
        let winner = select(&candidates, &[prefer("b"), prefer("a")]);
        assert_eq!(winner.map(|c| c.id()), Some("a"));
    }

    #[test]
    fn nothing_passed_means_nothing_is_selected() {
        assert!(select(&[], &[prefer("a"), prefer("b")]).is_none());
    }

    #[test]
    fn findings_are_carried_and_change_no_selection() {
        let candidates = [passed("a"), passed("b")];
        let signals = [
            WeakSignal::Finding {
                note: "the fix special-cases the test input".to_string(),
            },
            prefer("b"),
        ];
        assert_eq!(
            findings(&signals),
            vec!["the fix special-cases the test input"]
        );
        assert_eq!(select(&candidates, &signals).map(|c| c.id()), Some("b"));
    }

    #[test]
    fn a_weak_routing_reward_is_never_positive() {
        // No arrangement of signals produces a reward that would teach the
        // router to prefer a provider on a model's say-so.
        let cases: [&[WeakSignal]; 4] = [
            &[],
            &[prefer("a"), prefer("a"), prefer("a")],
            &[WeakSignal::Finding {
                note: "n".to_string(),
            }],
            &[doubt("d"), prefer("a")],
        ];
        for signals in cases {
            assert!(
                weak_routing_reward(signals) <= 0.0,
                "weak signals must never reward: {signals:?}"
            );
        }
    }

    #[test]
    fn doubt_lowers_the_reward_and_stays_bounded() {
        let one = weak_routing_reward(&[doubt("a")]);
        let three = weak_routing_reward(&[doubt("a"), doubt("b"), doubt("c")]);
        assert!(three < one && one < 0.0);
        let many: Vec<WeakSignal> = (0..100).map(|i| doubt(&i.to_string())).collect();
        assert!(weak_routing_reward(&many) >= -1.0);
    }
}
