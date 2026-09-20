//! Rate is a control, and spend is not the one that binds.
//!
//! Phase 33.3. At 200 developers, Cortex's failure mode is not spending too much
//! — it is **producing changes faster than the team can absorb them**. Open
//! changes that outrun review capacity age, drift off their base, collide with
//! each other, and consume more review time than they save. Phase 20's controls
//! govern money, effort, and authority; none of them governs rate.
//!
//! > A queue that Cortex fills faster than it drains is a product that makes a
//! > good team slower, which is the most expensive possible way to be wrong
//! > here.
//!
//! # Little's law is the whole argument
//!
//! `throughput = work_in_progress / cycle_time`. Review capacity caps
//! throughput, so past the point where review is saturated, adding
//! work-in-progress adds only latency and conflict — the *same* number of
//! changes land, later, having collided more. [`saturating_wip`] is where that
//! point is, computed from Phase 31.6's measured review-minutes-per-change
//! rather than guessed.
//!
//! # Where the queue drains
//!
//! Any repository at this size runs a merge queue, and it is precisely the
//! mechanism that already solves "two changes each pass alone and fail
//! together". Cortex participates in it rather than around it:
//!
//! - Cortex submits to the queue and never merges directly — Phase 26.6's
//!   no-bypass rule applied to the integration point.
//! - Where a queue exists, **its speculative-batch verification is the team's
//!   implementation of invariant 16**, so Cortex defers to it rather than
//!   duplicating the cost, and records the queue's result on the receipt.
//! - A rejection is a first-class outcome that flows back as a failed
//!   integration and a learning signal, **not as a stuck run**.

use serde::{Deserialize, Serialize};

/// A deliberately low starting point for open Cortex changes per repository.
///
/// Low because the cost of being wrong is asymmetric: too low leaves capacity
/// unused and is visible immediately, while too high ages branches and consumes
/// the review budget it was supposed to save, and looks like the product working
/// until it does not.
pub const CONSERVATIVE_DEFAULT_LIMIT: u32 = 3;

/// Changes landed per day at this work-in-progress and cycle time.
///
/// Little's law. `None` for a non-positive cycle time, which is not a very fast
/// team.
pub fn throughput_per_day(work_in_progress: u32, cycle_time_days: f64) -> Option<f64> {
    if cycle_time_days <= 0.0 {
        return None;
    }
    Some(work_in_progress as f64 / cycle_time_days)
}

/// The work-in-progress at which review saturates.
///
/// Beyond this, more open changes do not produce more merged changes — they
/// produce the same number, later, having drifted and collided in the meantime.
///
/// `None` when review capacity or cycle time is not positive, or when a single
/// change costs more review than a day contains: that is not a work-in-progress
/// limit problem, it is a change-size problem.
pub fn saturating_wip(
    review_minutes_per_day: f64,
    review_minutes_per_change: f64,
    cycle_time_days: f64,
) -> Option<u32> {
    if review_minutes_per_day <= 0.0 || review_minutes_per_change <= 0.0 || cycle_time_days <= 0.0 {
        return None;
    }
    let reviews_per_day = review_minutes_per_day / review_minutes_per_change;
    if reviews_per_day < 1.0 {
        return None;
    }
    Some((reviews_per_day * cycle_time_days) as u32)
}

/// Whether another change may be opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Admission {
    Admit,
    /// The limit is reached. Not an error — the work waits, which is the entire
    /// point of a work-in-progress limit.
    Hold {
        open: u32,
        limit: u32,
    },
}

/// Apply the work-in-progress limit.
pub fn admit(open_cortex_changes: u32, limit: u32) -> Admission {
    if open_cortex_changes < limit {
        Admission::Admit
    } else {
        Admission::Hold {
            open: open_cortex_changes,
            limit,
        }
    }
}

/// How a change reaches the trunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Integration {
    /// The team runs a merge queue. Submit, and never merge directly.
    SubmitToQueue,
    /// No queue. Cortex integrates and re-verifies against the integrated tree
    /// itself, per invariant 16.
    SelfIntegrateAndReverify,
}

/// Choose the integration route.
///
/// Where a queue exists Cortex defers to it. Merging directly would bypass the
/// team's own gate, and re-running a full battery alongside the queue's
/// speculative batch pays twice for one answer.
pub fn integration_route(merge_queue_available: bool) -> Integration {
    if merge_queue_available {
        Integration::SubmitToQueue
    } else {
        Integration::SelfIntegrateAndReverify
    }
}

/// What the queue did with a submitted change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum QueueOutcome {
    Merged {
        batch: String,
    },
    /// The queue refused it. A real integration failure with a reason, and a
    /// signal to learn from — never a run that just stops.
    Rejected {
        reason: String,
    },
}

impl QueueOutcome {
    pub fn integrated(&self) -> bool {
        matches!(self, Self::Merged { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Option<f64>, b: f64) -> bool {
        matches!(a, Some(v) if (v - b).abs() < 1e-9)
    }

    #[test]
    fn throughput_is_work_in_progress_over_cycle_time() {
        // Little's law, which is the whole argument for a rate limit.
        assert!(close(throughput_per_day(40, 2.0), 20.0));
        assert!(close(throughput_per_day(0, 2.0), 0.0));
    }

    #[test]
    fn a_zero_cycle_time_is_not_a_very_fast_team() {
        assert_eq!(throughput_per_day(10, 0.0), None);
        assert_eq!(throughput_per_day(10, -1.0), None);
    }

    #[test]
    fn saturation_is_computed_from_measured_review_capacity() {
        // 400 reviewer-minutes a day at 20 minutes a change is 20 reviews a
        // day; at a two-day cycle time that is 40 open changes. The
        // review-minutes figure is Phase 31.6's measurement, not a guess.
        assert_eq!(saturating_wip(400.0, 20.0, 2.0), Some(40));
    }

    #[test]
    fn a_slower_review_lowers_the_ceiling() {
        // The same team reviewing more carefully absorbs fewer open changes,
        // and a limit tuned to the old number would age branches.
        assert_eq!(saturating_wip(400.0, 40.0, 2.0), Some(20));
    }

    #[test]
    fn a_change_costing_more_review_than_a_day_has_is_not_a_wip_problem() {
        // Fewer than one review a day means the limit is not the constraint --
        // the size of a change is. Returning 0 would read as "stop
        // dispatching", which fixes nothing.
        assert_eq!(saturating_wip(120.0, 240.0, 2.0), None);
    }

    #[test]
    fn no_review_capacity_has_no_saturation_point() {
        assert_eq!(saturating_wip(0.0, 20.0, 2.0), None);
        assert_eq!(saturating_wip(400.0, 0.0, 2.0), None);
        assert_eq!(saturating_wip(400.0, 20.0, 0.0), None);
    }

    #[test]
    fn the_limit_holds_work_rather_than_failing_it() {
        // A hold is not an error. The work waits, which is the entire point --
        // past saturation, opening it anyway adds latency and conflict and
        // lands the same number of changes.
        assert_eq!(admit(2, 3), Admission::Admit);
        assert_eq!(admit(3, 3), Admission::Hold { open: 3, limit: 3 });
        assert_eq!(admit(9, 3), Admission::Hold { open: 9, limit: 3 });
    }

    #[test]
    fn a_zero_limit_admits_nothing() {
        // Which is a legitimate setting: an org pausing Cortex on a repository
        // without engaging an operator stop.
        assert_eq!(admit(0, 0), Admission::Hold { open: 0, limit: 0 });
    }

    #[test]
    fn the_default_limit_is_low_on_purpose() {
        // The cost of being wrong is asymmetric. Too low leaves capacity unused
        // and is visible immediately; too high ages branches and eats the
        // review budget it was meant to save, and looks like the product
        // working until it does not.
        assert_eq!(
            admit(CONSERVATIVE_DEFAULT_LIMIT, CONSERVATIVE_DEFAULT_LIMIT),
            Admission::Hold {
                open: CONSERVATIVE_DEFAULT_LIMIT,
                limit: CONSERVATIVE_DEFAULT_LIMIT
            }
        );
    }

    #[test]
    fn cortex_submits_to_a_merge_queue_and_never_merges_around_it() {
        // Phase 26.6's no-bypass rule applied to the integration point. Where a
        // queue exists it is the team's gate, and its speculative-batch
        // verification is their implementation of invariant 16 -- running a
        // full battery alongside it pays twice for one answer.
        assert_eq!(integration_route(true), Integration::SubmitToQueue);
        assert_eq!(
            integration_route(false),
            Integration::SelfIntegrateAndReverify
        );
    }

    #[test]
    fn a_queue_rejection_is_an_outcome_rather_than_a_stuck_run() {
        // There is no third variant meaning "nothing happened", so a submitted
        // change always comes back with something a router and a corpus can
        // learn from. Dropping a rejection on the floor is how a run becomes
        // stuck instead of finished, and this type gives it nowhere to be
        // dropped.
        let rejected = QueueOutcome::Rejected {
            reason: "batch failed with #904".to_string(),
        };
        assert!(!rejected.integrated());

        let merged = QueueOutcome::Merged {
            batch: "batch-17".to_string(),
        };
        assert!(merged.integrated());
    }

    #[test]
    fn a_rejection_carries_its_reason_to_the_receipt() {
        let rejected = QueueOutcome::Rejected {
            reason: "conflicts with #904".to_string(),
        };
        let json = serde_json::to_value(&rejected).unwrap();
        assert_eq!(json["kind"], "rejected");
        assert_eq!(json["reason"], "conflicts with #904");
    }
}
