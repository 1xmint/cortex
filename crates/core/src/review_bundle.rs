//! What a receipt is allowed to retire, and what it never is.
//!
//! Phase 31.6, which the plan calls the finding that most changes what "success"
//! means at team scale.
//!
//! If every verified change still requires a full human review, then **human
//! review capacity — not Cortex's throughput — caps the value Cortex can
//! deliver.** A team of 200 developers has a fixed number of reviewer-hours per
//! day. Doubling the rate of incoming changes against a fixed review budget does
//! not double delivered value; past a point it *reduces* it, by lengthening
//! queues, ageing branches, and pushing base drift onto every open change.
//!
//! So the win condition is not more pull requests. It is **less review per pull
//! request**, and that requires saying explicitly what a receipt may retire:
//!
//! | A strong receipt retires | It does not retire |
//! |---|---|
//! | did they run the tests, and did they pass | is this the right design |
//! | does it build, typecheck, and lint | does this belong in this subsystem |
//! | is there a regression test for the reported defect | is this maintainable by this team |
//! | did it break something in the blast radius | is the public API shape right |
//! | was the exam surface left intact | is this the problem worth solving |
//!
//! The right-hand column is irreducibly human and Cortex should stop pretending
//! otherwise. [`ReviewQuestion::retired_by_receipt`] is that table, and
//! [`bundle_for`] is the consequence: the bundle is ordered by what the receipt
//! *cannot* answer, and visibly shrinks as the verdict class strengthens.
//!
//! # The metric that matters more than pass rate
//!
//! **Review-minutes per merged change, trending down.** That is the number that
//! proves Cortex works at team scale. A pass rate that rises while review-minutes
//! hold flat has bought nothing at a team level, because the constraint was never
//! Cortex's throughput.
//!
//! # The hazard on the other side of it
//!
//! Review time falling is the goal *and* the failure mode, and they look
//! identical for a while.
//!
//! > If reviewers begin rubber-stamping, the human gate has silently disappeared
//! > and every false accept from Phase 27 reaches production unimpeded. A product
//! > that earns trust faster than it earns precision has built a hazard.
//!
//! [`trust_ratio`] is the instrument. A receipt genuinely earns less review, so a
//! ratio below 1 is expected and desirable; what is not is a ratio falling faster
//! than measured precision rises. The floor belongs to policy, and the trend is
//! the thing to watch — a single reading proves nothing either way.

use serde::{Deserialize, Serialize};

use crate::verification::Verdict;

/// A question a reviewer might have to answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewQuestion {
    // --- What executed checks can settle ---
    /// Did they run the tests, and did they pass.
    TestsRunAndPassed,
    /// Does it build, typecheck, and lint.
    BuildsTypechecksLints,
    /// Is there a regression test for the reported defect (27.5's differential
    /// control).
    RegressionTestForTheDefect,
    /// Did it break something elsewhere in the blast radius.
    BlastRadiusUnbroken,
    /// Was the exam surface left intact (27.2).
    ExamSurfaceIntact,

    // --- What no receipt settles ---
    /// Is this the right design.
    IsTheDesignRight,
    /// Does this belong in this subsystem.
    BelongsInThisSubsystem,
    /// Is this maintainable by this team.
    MaintainableByThisTeam,
    /// Is the public API shape right.
    PublicApiShapeIsRight,
    /// Is this the problem worth solving.
    WorthSolvingAtAll,
}

/// Every question, in the order a reviewer should meet them: what the receipt
/// cannot answer first.
pub const ALL: &[ReviewQuestion] = &[
    ReviewQuestion::IsTheDesignRight,
    ReviewQuestion::WorthSolvingAtAll,
    ReviewQuestion::BelongsInThisSubsystem,
    ReviewQuestion::PublicApiShapeIsRight,
    ReviewQuestion::MaintainableByThisTeam,
    ReviewQuestion::TestsRunAndPassed,
    ReviewQuestion::BuildsTypechecksLints,
    ReviewQuestion::RegressionTestForTheDefect,
    ReviewQuestion::BlastRadiusUnbroken,
    ReviewQuestion::ExamSurfaceIntact,
];

impl ReviewQuestion {
    /// Whether an executed battery can settle this.
    pub fn retired_by_receipt(&self) -> bool {
        match self {
            Self::TestsRunAndPassed
            | Self::BuildsTypechecksLints
            | Self::RegressionTestForTheDefect
            | Self::BlastRadiusUnbroken
            | Self::ExamSurfaceIntact => true,
            Self::IsTheDesignRight
            | Self::BelongsInThisSubsystem
            | Self::MaintainableByThisTeam
            | Self::PublicApiShapeIsRight
            | Self::WorthSolvingAtAll => false,
        }
    }
}

/// The review bundle for a change with this verdict.
///
/// Ordered by what the receipt cannot answer, and shrinking as the verdict
/// strengthens. Only [`Verdict::Verified`] retires anything: `Unverified` means
/// the work was sold without a verification promise, so it retires nothing, and
/// a reviewer handed a short bundle there would be reading a promise that was
/// never made.
pub fn bundle_for(verdict: Verdict) -> Vec<ReviewQuestion> {
    let retires = verdict == Verdict::Verified;
    ALL.iter()
        .copied()
        .filter(|q| !(retires && q.retired_by_receipt()))
        .collect()
}

/// How much review one population of changes received, per unit of diff.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReviewIntensity {
    pub minutes: f64,
    pub diff_lines: u32,
}

impl ReviewIntensity {
    /// `None` for an empty diff — no denominator, and a zero-line change tells
    /// you nothing about review effort.
    pub fn minutes_per_hundred_lines(&self) -> Option<f64> {
        if self.diff_lines == 0 {
            return None;
        }
        Some(self.minutes * 100.0 / self.diff_lines as f64)
    }
}

/// Review intensity on Cortex changes relative to human changes.
///
/// Below 1 means Cortex changes get less scrutiny per line. That is the intended
/// effect of a receipt, not evidence of a problem — the problem is this number
/// falling faster than measured precision rises, which is why it belongs on a
/// dashboard as a trend rather than in a gate as a threshold.
///
/// `None` when either population has no diff to measure.
pub fn trust_ratio(cortex: &ReviewIntensity, human: &ReviewIntensity) -> Option<f64> {
    let c = cortex.minutes_per_hundred_lines()?;
    let h = human.minutes_per_hundred_lines()?;
    if h == 0.0 {
        return None;
    }
    Some(c / h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intensity(minutes: f64, diff_lines: u32) -> ReviewIntensity {
        ReviewIntensity {
            minutes,
            diff_lines,
        }
    }

    #[test]
    fn a_verified_change_asks_only_the_questions_no_receipt_answers() {
        // The point of the whole phase. Review capacity is the binding
        // constraint at team scale, and this is where a receipt converts
        // reviewer minutes into a glance.
        let bundle = bundle_for(Verdict::Verified);
        assert_eq!(bundle.len(), 5);
        assert!(
            bundle.iter().all(|q| !q.retired_by_receipt()),
            "a verified receipt retires every executable question"
        );
        assert!(bundle.contains(&ReviewQuestion::IsTheDesignRight));
        assert!(bundle.contains(&ReviewQuestion::WorthSolvingAtAll));
    }

    #[test]
    fn unverified_work_retires_nothing() {
        // `Unverified` means the work was sold without a verification promise.
        // A reviewer handed a short bundle here would be reading a promise
        // nobody made.
        let bundle = bundle_for(Verdict::Unverified);
        assert_eq!(bundle.len(), ALL.len());
    }

    #[test]
    fn the_bundle_shrinks_as_the_verdict_strengthens() {
        assert!(bundle_for(Verdict::Verified).len() < bundle_for(Verdict::Unverified).len());
        for weaker in [Verdict::Failed, Verdict::Inconclusive, Verdict::Unverified] {
            assert_eq!(
                bundle_for(weaker).len(),
                ALL.len(),
                "{weaker:?} retires nothing"
            );
        }
    }

    #[test]
    fn the_bundle_leads_with_what_the_receipt_cannot_answer() {
        // Phase 3.3 builds the surface; this is the ordering principle that
        // should govern its layout. A reviewer who opens with "did the tests
        // pass" spends their attention on the one question already answered.
        let bundle = bundle_for(Verdict::Unverified);
        let first_retirable = bundle
            .iter()
            .position(|q| q.retired_by_receipt())
            .expect("some questions are retirable");
        let last_human = bundle
            .iter()
            .rposition(|q| !q.retired_by_receipt())
            .expect("some questions are human");
        assert!(
            last_human < first_retirable,
            "every human question precedes every retirable one"
        );
    }

    #[test]
    fn design_and_taste_are_never_retired_by_any_verdict() {
        // Cortex should stop pretending otherwise. No verdict, however strong,
        // establishes that a change belongs in this subsystem.
        for verdict in [
            Verdict::Verified,
            Verdict::Failed,
            Verdict::Inconclusive,
            Verdict::Unverified,
        ] {
            let bundle = bundle_for(verdict);
            for human in [
                ReviewQuestion::IsTheDesignRight,
                ReviewQuestion::BelongsInThisSubsystem,
                ReviewQuestion::MaintainableByThisTeam,
                ReviewQuestion::PublicApiShapeIsRight,
                ReviewQuestion::WorthSolvingAtAll,
            ] {
                assert!(
                    bundle.contains(&human),
                    "{verdict:?} must not retire {human:?}"
                );
            }
        }
    }

    #[test]
    fn every_question_is_in_the_canonical_order_exactly_once() {
        // `ALL` is what a renderer iterates, so a question missing from it is a
        // question that silently stops being asked.
        let unique: std::collections::HashSet<_> = ALL.iter().collect();
        assert_eq!(unique.len(), ALL.len());
        assert_eq!(ALL.len(), 10);
    }

    #[test]
    fn less_review_per_line_on_cortex_changes_is_the_intended_effect() {
        // Below 1 is the goal. A receipt genuinely earns less scrutiny, and a
        // gate that fired here would be a gate against the product working.
        let cortex = intensity(10.0, 200);
        let human = intensity(30.0, 200);
        assert_eq!(trust_ratio(&cortex, &human), Some(1.0 / 3.0));
    }

    #[test]
    fn intensity_is_per_line_so_a_bigger_diff_is_not_mistaken_for_more_care() {
        // Twice the minutes on twice the diff is the same scrutiny. Comparing
        // raw minutes would read a large change as a careful review.
        let small = intensity(10.0, 100);
        let large = intensity(20.0, 200);
        assert_eq!(
            small.minutes_per_hundred_lines(),
            large.minutes_per_hundred_lines()
        );
    }

    #[test]
    fn an_empty_diff_has_no_intensity_and_no_ratio() {
        assert_eq!(intensity(5.0, 0).minutes_per_hundred_lines(), None);
        assert_eq!(trust_ratio(&intensity(5.0, 0), &intensity(5.0, 100)), None);
        assert_eq!(trust_ratio(&intensity(5.0, 100), &intensity(5.0, 0)), None);
    }

    #[test]
    fn a_human_population_that_spent_no_time_yields_no_ratio() {
        // Not infinity, and not zero. Nothing to compare against.
        assert_eq!(
            trust_ratio(&intensity(5.0, 100), &intensity(0.0, 100)),
            None
        );
    }
}
