//! Expertise is a property of the person *in this subsystem*.
//!
//! Phase 35.5. The brief framed the range as solo builder versus 200-developer
//! org and expected one mechanism to serve both with a different surface. The
//! literature says the framing is off by one axis.
//!
//! The **expertise reversal effect**: low-knowledge learners gain from high
//! instructional guidance, while more knowledgeable learners gain from *reduced*
//! guidance and are **actively penalised** by the redundant material. That is a
//! reversal, not a diminishing return — showing a senior engineer the scaffolded
//! explanation does not merely bore them, it measurably degrades their
//! performance, because processing redundant guidance consumes the working
//! memory the task needed.
//!
//! # Two designs this kills
//!
//! **A beginner mode and an expert mode** is a mode switch, forbidden by
//! invariant 31 — and worse, it would be a *self-declared* one. Self-assessment
//! of expertise is exactly what an expertise-reversal design must not depend on,
//! because **the developers most likely to decline scaffolding are the ones a
//! fresh codebase has just made novices again.** So [`guidance_for`] takes no
//! self-declared level, and there is no parameter for one to be threaded
//! through later.
//!
//! **A global skill score per user** is the other one, and it is subtler.
//! Expertise here is not a property of the person: **a principal engineer is a
//! novice in the payments module they have never opened.** So the only argument
//! is a [`SubsystemFamiliarity`], and there is no way to ask what guidance a
//! *user* should get without naming where.
//!
//! Phase 35.1's population correction is the same statement from data: the
//! trial's participants were mostly seven-plus-year engineers and the deficit
//! appeared anyway, because the library was new to them. The effect is about
//! domain novelty, not seniority — a much larger market and a far more familiar
//! situation. It is the position of every senior engineer opening an unfamiliar
//! framework, a legacy subsystem, or a codebase they joined last week.
//!
//! # Why the threshold must be measured rather than defaulted safely
//!
//! Most defaults have a safe side. This one does not. Under-scaffolding leaves a
//! novice without support; over-scaffolding *degrades* an expert's performance
//! rather than merely wasting their time. Both errors are harmful, so
//! [`Threshold::DEFAULT`] is a starting point to calibrate against outcomes, and
//! it is stated as one rather than presented as a considered value.

use serde::{Deserialize, Serialize};

/// What the record says about this person's history in one subsystem.
///
/// Derived, never declared. Every field is something the forge and the corpus
/// already hold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubsystemFamiliarity {
    /// Commits this person authored here.
    pub commits_here: u32,
    /// Changes this person reviewed here.
    pub reviews_here: u32,
}

impl SubsystemFamiliarity {
    /// Times this person has engaged with this subsystem at all.
    ///
    /// Reviews count. Reading a change closely is how a good deal of subsystem
    /// knowledge is acquired, and the code-review literature measures the
    /// transfer as bidirectional — author and reviewer both.
    pub fn touches(&self) -> u32 {
        self.commits_here + self.reviews_here
    }
}

/// Where the line sits between scaffolded and minimal guidance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Threshold {
    pub min_touches: u32,
}

impl Threshold {
    /// A starting point, not a considered value.
    ///
    /// Named as a default so nobody mistakes it for a measurement. Three touches
    /// is where this begins and where the calibration should start.
    pub const DEFAULT: Threshold = Threshold { min_touches: 3 };
}

impl Default for Threshold {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// How much instructional support to render.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Guidance {
    /// Full explanation. For someone who has not worked here.
    Scaffolded,
    /// The finding, without the walkthrough. For someone who has.
    Minimal,
}

/// Choose guidance from what the record says about this person, here.
///
/// Note what this function does not take: a user, a self-declared level, or a
/// mode. It takes familiarity with one subsystem, because that is the only thing
/// the expertise reversal is about.
pub fn guidance_for(familiarity: &SubsystemFamiliarity, threshold: &Threshold) -> Guidance {
    if familiarity.touches() >= threshold.min_touches {
        Guidance::Minimal
    } else {
        Guidance::Scaffolded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn familiarity(commits: u32, reviews: u32) -> SubsystemFamiliarity {
        SubsystemFamiliarity {
            commits_here: commits,
            reviews_here: reviews,
        }
    }

    #[test]
    fn the_same_person_gets_different_guidance_in_different_subsystems() {
        // The whole point, and the design a global skill score gets wrong. A
        // principal engineer is a novice in the payments module they have never
        // opened, and their seniority does not travel there with them.
        let payments = familiarity(0, 0);
        let scheduler = familiarity(40, 12);
        assert_eq!(
            guidance_for(&payments, &Threshold::DEFAULT),
            Guidance::Scaffolded
        );
        assert_eq!(
            guidance_for(&scheduler, &Threshold::DEFAULT),
            Guidance::Minimal
        );
    }

    #[test]
    fn someone_who_has_never_worked_here_gets_the_explanation() {
        assert_eq!(
            guidance_for(&SubsystemFamiliarity::default(), &Threshold::DEFAULT),
            Guidance::Scaffolded
        );
    }

    #[test]
    fn reviewing_here_counts_as_knowing_the_place() {
        // Reading a change closely is how a good deal of subsystem knowledge is
        // acquired, and the transfer is measurably bidirectional -- author and
        // reviewer both. Counting only commits would scaffold the person who
        // reviews everything and writes little, which in most teams is whoever
        // knows the subsystem best.
        assert_eq!(
            guidance_for(&familiarity(0, 5), &Threshold::DEFAULT),
            Guidance::Minimal
        );
        assert_eq!(familiarity(2, 1).touches(), 3);
    }

    #[test]
    fn the_boundary_is_inclusive_at_the_threshold() {
        assert_eq!(
            guidance_for(&familiarity(2, 0), &Threshold::DEFAULT),
            Guidance::Scaffolded
        );
        assert_eq!(
            guidance_for(&familiarity(3, 0), &Threshold::DEFAULT),
            Guidance::Minimal
        );
    }

    #[test]
    fn the_threshold_is_a_parameter_because_neither_error_is_safe() {
        // Most defaults have a safe side. This one does not: under-scaffolding
        // leaves a novice without support, and over-scaffolding degrades an
        // expert's performance rather than merely wasting their time. So the
        // line is callable with a measured value rather than baked in.
        let strict = Threshold { min_touches: 20 };
        let veteran = familiarity(10, 5);
        assert_eq!(
            guidance_for(&veteran, &Threshold::DEFAULT),
            Guidance::Minimal
        );
        assert_eq!(guidance_for(&veteran, &strict), Guidance::Scaffolded);
    }

    #[test]
    fn a_threshold_of_zero_never_scaffolds() {
        // A legitimate setting for a team that has measured the reversal to bite
        // early, and worth being reachable rather than clamped away.
        let never = Threshold { min_touches: 0 };
        assert_eq!(
            guidance_for(&SubsystemFamiliarity::default(), &never),
            Guidance::Minimal
        );
    }

    #[test]
    fn nothing_here_reads_a_self_declared_level() {
        // Self-assessment is exactly what an expertise-reversal design must not
        // depend on: the developers most likely to decline scaffolding are the
        // ones a fresh codebase has just made novices again. There is no
        // parameter for a declaration, so there is nowhere to add one without
        // changing the signature and reading this.
        let json = serde_json::to_value(familiarity(1, 2)).unwrap();
        assert_eq!(json["commits_here"], 1);
        assert_eq!(json["reviews_here"], 2);
        assert!(
            json.get("declared_level").is_none() && json.get("mode").is_none(),
            "familiarity is derived from the record and carries no declaration"
        );
    }

    #[test]
    fn guidance_is_two_values_rather_than_a_scale() {
        // The reversal is a reversal, not a diminishing return, so a gradient
        // would imply a middle that the effect does not have. Redundant
        // guidance does not become mildly redundant -- it starts costing the
        // working memory the task needed.
        let all = [Guidance::Scaffolded, Guidance::Minimal];
        assert_eq!(all.len(), 2);
    }
}
