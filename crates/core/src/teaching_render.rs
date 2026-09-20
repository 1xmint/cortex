//! A thin record produces a *shorter* account, never a vaguer one.
//!
//! Phase 35's exit gate names two properties that nothing else in the phase
//! enforces, and they are the two a renderer gets wrong under pressure.
//!
//! **Shorter, not vaguer.** The tempting failure is to keep the artifact the
//! same length by hedging: "the checks broadly passed", "this looks correct".
//! Hedged prose is the same claim with the evidence filed off, and it reads as
//! *more* authoritative than the sourced sentence it replaced, because it has
//! stopped saying anything that could be checked. [`render`] drops sentences it
//! cannot source rather than softening them, so a run with less behind it
//! produces strictly fewer sentences.
//!
//! **"Not recorded" and "did not happen" are different sentences.** Cortex not
//! holding a fact and Cortex recording that something did not occur are
//! completely different states, and a reader acts on them differently: the first
//! is a gap in Cortex, the second is a fact about the work. Collapsing them into
//! "no" is the small dishonesty that makes a receipt untrustworthy in aggregate.
//!
//! # No model in the rendering path
//!
//! This module takes a record and returns sentences. There is no inference, no
//! provider, and no async — which is the structural form of the gate's "no model
//! is invoked anywhere in the rendering path". A synchronous pure function over
//! owned data has nowhere to put a call.
//!
//! # The `always` section stands alone
//!
//! It must be complete on its own, contain no vocabulary defined in the plan,
//! and need no operator-tier fact to make sense. [`VOCABULARY`] is that list and
//! [`Artifact::vocabulary_in_always`] is the check — a term like "battery power"
//! reaching the always tier is exactly how a correct product becomes unreadable
//! to the person it was simplified for.

use serde::{Deserialize, Serialize};

/// Terms this plan defines. None of them may appear in the `always` section.
///
/// Lower-case, and matched case-insensitively, because the failure is the
/// concept arriving rather than a particular capitalisation of it.
pub const VOCABULARY: &[&str] = &[
    "verdict class",
    "battery power",
    "race agreement",
    "comprehension state",
    "diff surface",
    "loss ratio",
    "false-accept",
    "p_fa",
    "blast radius",
    "idempotency",
];

/// Who a sentence is for.
///
/// A local two-value view of the plan's disclosure tiers, which are richer. Only
/// the always/on-request split matters to a renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Audience {
    /// Plain language, everyone, no vocabulary.
    Always,
    /// Anyone who opens the detail.
    OnRequest,
}

/// One sentence, and the record field it reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sentence {
    pub text: String,
    /// The field this came from. A sentence without one does not exist here.
    pub source: String,
    pub audience: Audience,
}

/// Why something the reader might expect is not in the artifact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Absence {
    /// Cortex does not hold this fact. A gap in Cortex.
    NotRecorded,
    /// Cortex holds that this did not occur. A fact about the work.
    DidNotHappen,
}

impl Absence {
    /// The sentence a reader sees. Distinct wording, deliberately: these are
    /// different states and a reader acts on them differently.
    pub fn phrasing(&self) -> &'static str {
        match self {
            Self::NotRecorded => "Cortex did not record this",
            Self::DidNotHappen => "this did not happen",
        }
    }
}

/// What one completed task can say for itself.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub sentences: Vec<Sentence>,
    /// Named gaps, each with its kind. Present rather than silent, because a
    /// reader needs to know the difference between a short account and an
    /// incomplete one.
    pub absences: Vec<(String, Absence)>,
}

impl Artifact {
    pub fn always(&self) -> impl Iterator<Item = &Sentence> {
        self.sentences
            .iter()
            .filter(|s| s.audience == Audience::Always)
    }

    /// Vocabulary that leaked into the `always` section.
    ///
    /// Anything this returns is a term the person it was simplified for now has
    /// to look up.
    pub fn vocabulary_in_always(&self) -> Vec<&'static str> {
        let always: String = self
            .always()
            .map(|s| s.text.to_lowercase())
            .collect::<Vec<_>>()
            .join(" ");
        VOCABULARY
            .iter()
            .copied()
            .filter(|term| always.contains(term))
            .collect()
    }
}

/// What the record holds about a completed task.
///
/// Each field is `Option` because the question is not "what was the value" but
/// "is there one" — and that is the distinction the whole module turns on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// What was attempted.
    pub objective: Option<String>,
    /// Why each check was in the exam.
    pub check_derivation: Option<String>,
    /// What failed. `Some(None)` means recorded, and nothing failed.
    pub failure: Option<Option<String>>,
    /// What the checks could not establish.
    pub not_established: Option<String>,
}

/// Render the account, dropping what cannot be sourced.
///
/// Sentences are emitted only for fields the record actually holds. A missing
/// field becomes a named [`Absence::NotRecorded`] rather than a hedged sentence,
/// and a recorded non-event becomes [`Absence::DidNotHappen`].
pub fn render(record: &Record) -> Artifact {
    let mut sentences = Vec::new();
    let mut absences = Vec::new();

    let mut emit =
        |field: &str, audience, value: Option<&str>, phrase: &dyn Fn(&str) -> String| match value {
            Some(v) => sentences.push(Sentence {
                text: phrase(v),
                source: field.to_string(),
                audience,
            }),
            None => absences.push((field.to_string(), Absence::NotRecorded)),
        };

    emit(
        "objective",
        Audience::Always,
        record.objective.as_deref(),
        &|v| format!("Cortex was asked to {v}."),
    );
    emit(
        "check_derivation",
        Audience::OnRequest,
        record.check_derivation.as_deref(),
        &|v| format!("The checks were chosen because {v}."),
    );
    emit(
        "not_established",
        Audience::Always,
        record.not_established.as_deref(),
        &|v| format!("These checks do not show {v}."),
    );

    match &record.failure {
        Some(Some(what)) => sentences.push(Sentence {
            text: format!("A check failed: {what}."),
            source: "failure".to_string(),
            audience: Audience::Always,
        }),
        // Recorded, and nothing failed. A fact about the work, not a gap.
        Some(None) => absences.push(("failure".to_string(), Absence::DidNotHappen)),
        None => absences.push(("failure".to_string(), Absence::NotRecorded)),
    }

    Artifact {
        sentences,
        absences,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> Record {
        Record {
            objective: Some("add a retry to the payments client".to_string()),
            check_derivation: Some("Cargo.toml declares a test target".to_string()),
            failure: Some(Some("the timeout test timed out".to_string())),
            not_established: Some("that the retry is correct under concurrent load".to_string()),
        }
    }

    #[test]
    fn a_thinner_record_produces_strictly_fewer_sentences() {
        // Shorter, not vaguer. The tempting failure is to hold the length
        // steady by hedging -- "the checks broadly passed" -- which is the same
        // claim with the evidence filed off, and reads as *more* authoritative
        // because it has stopped saying anything checkable.
        let rich = render(&full());
        let thin = render(&Record {
            objective: Some("add a retry".to_string()),
            ..Record::default()
        });
        assert!(
            thin.sentences.len() < rich.sentences.len(),
            "thin: {}, rich: {}",
            thin.sentences.len(),
            rich.sentences.len()
        );
        assert_eq!(thin.sentences.len(), 1);
    }

    #[test]
    fn an_empty_record_says_nothing_rather_than_something_vague() {
        let nothing = render(&Record::default());
        assert!(nothing.sentences.is_empty());
        assert_eq!(nothing.absences.len(), 4, "and names every gap");
    }

    #[test]
    fn not_recorded_and_did_not_happen_are_different_sentences() {
        // The distinction the gate names. Cortex not holding a fact is a gap in
        // Cortex; Cortex recording that nothing failed is a fact about the work.
        // A reader acts on them differently, and "no" for both is the small
        // dishonesty that makes a receipt untrustworthy in aggregate.
        let nothing_failed = render(&Record {
            failure: Some(None),
            ..full()
        });
        assert!(nothing_failed
            .absences
            .contains(&("failure".to_string(), Absence::DidNotHappen)));

        let unknown = render(&Record {
            failure: None,
            ..full()
        });
        assert!(unknown
            .absences
            .contains(&("failure".to_string(), Absence::NotRecorded)));

        assert_ne!(
            Absence::NotRecorded.phrasing(),
            Absence::DidNotHappen.phrasing()
        );
    }

    #[test]
    fn every_sentence_names_the_field_it_reads() {
        let artifact = render(&full());
        assert!(!artifact.sentences.is_empty());
        for sentence in &artifact.sentences {
            assert!(!sentence.source.is_empty(), "{sentence:?}");
        }
    }

    #[test]
    fn a_gap_is_named_rather_than_passed_over_in_silence() {
        // A short account and an incomplete one look identical unless the gaps
        // are listed. The absence list is what stops a reader assuming the
        // silence was completeness.
        let artifact = render(&Record {
            check_derivation: None,
            ..full()
        });
        assert!(artifact
            .absences
            .iter()
            .any(|(field, kind)| field == "check_derivation" && *kind == Absence::NotRecorded));
    }

    #[test]
    fn the_always_section_carries_no_vocabulary_from_the_plan() {
        // A term like "battery power" reaching the always tier is exactly how a
        // correct product becomes unreadable to the person it was simplified
        // for.
        let artifact = render(&full());
        assert!(
            artifact.vocabulary_in_always().is_empty(),
            "leaked: {:?}",
            artifact.vocabulary_in_always()
        );
    }

    #[test]
    fn the_vocabulary_check_actually_catches_a_leak() {
        // A guard nobody has seen fire is a guard nobody should trust.
        let leaky = Artifact {
            sentences: vec![Sentence {
                text: "The battery power here was weak.".to_string(),
                source: "battery_power".to_string(),
                audience: Audience::Always,
            }],
            absences: Vec::new(),
        };
        assert_eq!(leaky.vocabulary_in_always(), vec!["battery power"]);
    }

    #[test]
    fn vocabulary_below_the_always_tier_is_fine() {
        // The detail is where the terms belong. The rule is that they are never
        // required reading, not that they are never written.
        let detailed = Artifact {
            sentences: vec![Sentence {
                text: "Battery power: weak.".to_string(),
                source: "battery_power".to_string(),
                audience: Audience::OnRequest,
            }],
            absences: Vec::new(),
        };
        assert!(detailed.vocabulary_in_always().is_empty());
    }

    #[test]
    fn the_always_section_stands_alone() {
        // Complete on its own: what was attempted, what failed, and what was
        // not established, with no on-request fact needed to make sense of it.
        let artifact = render(&full());
        let always: Vec<&str> = artifact.always().map(|s| s.text.as_str()).collect();
        assert_eq!(always.len(), 3);
        assert!(always.iter().any(|t| t.contains("asked to")));
        assert!(always.iter().any(|t| t.contains("A check failed")));
        assert!(always.iter().any(|t| t.contains("do not show")));
    }
}
