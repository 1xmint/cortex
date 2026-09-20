//! Every external effect has a declared inverse, or is declared irreversible.
//!
//! Phase 32.3. Phase 24.3 made *internal* state survive partial delivery.
//! External effects had no such treatment, and they are the ones a customer
//! sees: branches pushed, pull requests opened, review comments posted, issues
//! transitioned, checks reported, deploys triggered.
//!
//! | Effect | Inverse | Class |
//! |---|---|---|
//! | branch pushed | delete branch | compensable |
//! | pull request opened | close, with a reason | compensable |
//! | check run reported | report superseded | compensable |
//! | review comment posted | delete or amend with a correction | compensable, **visibly** |
//! | issue transitioned | restore prior state | compensable |
//! | deploy triggered | nothing Cortex may assume | **irreversible — gated, never autonomous** |
//! | notification sent to a human | nothing | **irreversible — cheap, but it happened** |
//!
//! # The rule worth stating on its own
//!
//! **Irreversible external effects are ordered last.**
//!
//! > A plan that posts a comment before it knows the outcome has converted a
//! > recoverable failure into a visible one for no benefit.
//!
//! [`plan_order`] enforces it, and it is a sort rather than advice because the
//! natural order to write a plan in is the order the work happens, which puts
//! the notification exactly where it does the most damage.
//!
//! # Abandonment is not always clean, and says so
//!
//! [`abandon`] returns the inverses to run **and** the residue that no inverse
//! reaches. Two kinds: effects with no inverse at all, and effects whose inverse
//! runs but which a human already saw — a deleted review comment was still read.
//! An abandonment that reported only the inverses would be claiming the world
//! was restored when it was not, and somebody downstream would believe it.

use serde::{Deserialize, Serialize};

/// Something Cortex did that the outside world can see.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "effect")]
pub enum Effect {
    BranchPushed { branch: String },
    PullRequestOpened { number: u64 },
    CheckRunReported { id: String },
    ReviewCommentPosted { id: String },
    IssueTransitioned { id: String, prior_state: String },
    DeployTriggered { target: String },
    NotificationSent { recipient: String },
}

/// The action that undoes an effect.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "inverse")]
pub enum Inverse {
    DeleteBranch { branch: String },
    ClosePullRequest { number: u64, reason: String },
    ReportSuperseded { id: String },
    DeleteOrAmendComment { id: String },
    RestoreIssueState { id: String, state: String },
}

/// What undoing an effect can achieve.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// An inverse exists and restores the prior state.
    Compensable,
    /// An inverse exists, and a human may already have seen the effect. The
    /// state is restored; the fact of it is not.
    CompensableVisibly,
    /// Nothing Cortex may assume undoes this.
    Irreversible,
}

impl Effect {
    pub fn class(&self) -> Class {
        match self {
            Self::BranchPushed { .. }
            | Self::PullRequestOpened { .. }
            | Self::CheckRunReported { .. }
            | Self::IssueTransitioned { .. } => Class::Compensable,
            Self::ReviewCommentPosted { .. } => Class::CompensableVisibly,
            Self::DeployTriggered { .. } | Self::NotificationSent { .. } => Class::Irreversible,
        }
    }

    /// The declared inverse, where one exists.
    pub fn inverse(&self, reason: &str) -> Option<Inverse> {
        match self {
            Self::BranchPushed { branch } => Some(Inverse::DeleteBranch {
                branch: branch.clone(),
            }),
            Self::PullRequestOpened { number } => Some(Inverse::ClosePullRequest {
                number: *number,
                reason: reason.to_string(),
            }),
            Self::CheckRunReported { id } => Some(Inverse::ReportSuperseded { id: id.clone() }),
            Self::ReviewCommentPosted { id } => {
                Some(Inverse::DeleteOrAmendComment { id: id.clone() })
            }
            Self::IssueTransitioned { id, prior_state } => Some(Inverse::RestoreIssueState {
                id: id.clone(),
                state: prior_state.clone(),
            }),
            Self::DeployTriggered { .. } | Self::NotificationSent { .. } => None,
        }
    }

    /// Whether this may never happen without a human saying so.
    ///
    /// A deploy is gated, never autonomous. The plan is explicit that there is
    /// nothing Cortex may assume undoes one, and an effect with no inverse and
    /// no gate is an effect nobody agreed to.
    pub fn requires_human_gate(&self) -> bool {
        matches!(self, Self::DeployTriggered { .. })
    }
}

/// Order a plan's external effects so the irreversible ones come last.
///
/// Stable within each class, so a plan's own ordering survives where it is not
/// dangerous. The natural order to write a plan in is the order the work
/// happens, which is exactly the order that puts a notification before the
/// outcome it is announcing.
pub fn plan_order(effects: &[Effect]) -> Vec<Effect> {
    let mut ordered = effects.to_vec();
    ordered.sort_by_key(|e| e.class() == Class::Irreversible);
    ordered
}

/// What abandoning a run can and cannot undo.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Abandonment {
    /// Inverses to execute, in the order to execute them: the reverse of the
    /// order the effects happened.
    pub inverses: Vec<Inverse>,
    /// Effects no inverse reaches, and effects a human may already have seen.
    /// Reported rather than dropped.
    pub residue: Vec<Effect>,
}

/// Compute the compensation for a run being abandoned.
///
/// `performed` is in the order the effects happened. Inverses run in reverse:
/// closing a pull request before deleting the branch it points at leaves a
/// closed pull request referencing a branch that is about to vanish.
pub fn abandon(performed: &[Effect], reason: &str) -> Abandonment {
    let inverses = performed
        .iter()
        .rev()
        .filter_map(|e| e.inverse(reason))
        .collect();
    let residue = performed
        .iter()
        .filter(|e| matches!(e.class(), Class::Irreversible | Class::CompensableVisibly))
        .cloned()
        .collect();
    Abandonment { inverses, residue }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch() -> Effect {
        Effect::BranchPushed {
            branch: "cortex/fix-auth".to_string(),
        }
    }

    fn pr() -> Effect {
        Effect::PullRequestOpened { number: 42 }
    }

    fn comment() -> Effect {
        Effect::ReviewCommentPosted {
            id: "c-1".to_string(),
        }
    }

    fn notification() -> Effect {
        Effect::NotificationSent {
            recipient: "alice".to_string(),
        }
    }

    fn deploy() -> Effect {
        Effect::DeployTriggered {
            target: "staging".to_string(),
        }
    }

    #[test]
    fn irreversible_effects_are_ordered_last() {
        // The rule the plan states on its own. A plan that notifies before it
        // knows the outcome has converted a recoverable failure into a visible
        // one for no benefit.
        let written = [notification(), branch(), deploy(), pr()];
        let ordered = plan_order(&written);
        assert_eq!(ordered[0], branch());
        assert_eq!(ordered[1], pr());
        assert_eq!(ordered[2], notification());
        assert_eq!(ordered[3], deploy());
    }

    #[test]
    fn ordering_is_stable_within_a_class() {
        // A plan's own ordering survives where it is not dangerous. Reordering
        // the reversible effects would break plans that depend on a branch
        // existing before a pull request points at it.
        let written = [branch(), pr(), comment()];
        assert_eq!(plan_order(&written), written.to_vec());
    }

    #[test]
    fn a_plan_of_only_irreversible_effects_is_unchanged() {
        let written = [notification(), deploy()];
        assert_eq!(plan_order(&written), written.to_vec());
    }

    #[test]
    fn inverses_run_in_the_reverse_of_the_order_the_effects_happened() {
        // Closing the pull request before deleting the branch it points at
        // leaves a closed pull request referencing a branch about to vanish.
        let performed = [branch(), pr()];
        let a = abandon(&performed, "run abandoned");
        assert_eq!(
            a.inverses,
            vec![
                Inverse::ClosePullRequest {
                    number: 42,
                    reason: "run abandoned".to_string()
                },
                Inverse::DeleteBranch {
                    branch: "cortex/fix-auth".to_string()
                },
            ]
        );
    }

    #[test]
    fn a_closed_pull_request_carries_the_reason() {
        // A pull request that closes itself with no explanation is a support
        // ticket. The inverse takes the reason rather than inventing one.
        let a = abandon(&[pr()], "the battery could not be derived");
        assert_eq!(
            a.inverses,
            vec![Inverse::ClosePullRequest {
                number: 42,
                reason: "the battery could not be derived".to_string()
            }]
        );
    }

    #[test]
    fn what_no_inverse_reaches_is_reported_rather_than_dropped() {
        // An abandonment reporting only its inverses would be claiming the
        // world was restored when it was not, and somebody downstream would
        // believe it.
        let performed = [branch(), notification(), deploy()];
        let a = abandon(&performed, "abandoned");
        assert_eq!(a.inverses.len(), 1, "only the branch can be undone");
        assert_eq!(a.residue, vec![notification(), deploy()]);
    }

    #[test]
    fn a_deleted_comment_is_still_residue() {
        // The inverse runs and the state is restored; the fact of it is not. A
        // reviewer who read the comment read it.
        let a = abandon(&[comment()], "abandoned");
        assert_eq!(
            a.inverses,
            vec![Inverse::DeleteOrAmendComment {
                id: "c-1".to_string()
            }]
        );
        assert_eq!(a.residue, vec![comment()]);
    }

    #[test]
    fn a_fully_compensable_run_leaves_no_residue() {
        let performed = [
            branch(),
            pr(),
            Effect::CheckRunReported {
                id: "check-9".to_string(),
            },
            Effect::IssueTransitioned {
                id: "ISSUE-3".to_string(),
                prior_state: "open".to_string(),
            },
        ];
        let a = abandon(&performed, "abandoned");
        assert_eq!(a.inverses.len(), 4);
        assert!(a.residue.is_empty());
    }

    #[test]
    fn an_issue_inverse_carries_the_state_to_restore() {
        // "Restore prior state" needs the prior state, and the only moment it
        // is knowable is when the transition is recorded.
        let a = abandon(
            &[Effect::IssueTransitioned {
                id: "ISSUE-3".to_string(),
                prior_state: "in-review".to_string(),
            }],
            "abandoned",
        );
        assert_eq!(
            a.inverses,
            vec![Inverse::RestoreIssueState {
                id: "ISSUE-3".to_string(),
                state: "in-review".to_string()
            }]
        );
    }

    #[test]
    fn a_deploy_is_gated_and_nothing_else_is() {
        // Gated, never autonomous. An effect with no inverse and no gate is an
        // effect nobody agreed to -- and a notification, which also has no
        // inverse, is cheap enough that gating every one would make the gate
        // meaningless.
        assert!(deploy().requires_human_gate());
        for ungated in [branch(), pr(), comment(), notification()] {
            assert!(
                !ungated.requires_human_gate(),
                "{ungated:?} does not need a human before it happens"
            );
        }
    }

    #[test]
    fn the_effects_with_no_inverse_are_exactly_the_irreversible_ones() {
        // The class and the inverse must agree, or an effect could be filed as
        // compensable with nothing to compensate it.
        for effect in [
            branch(),
            pr(),
            comment(),
            notification(),
            deploy(),
            Effect::CheckRunReported {
                id: "c".to_string(),
            },
            Effect::IssueTransitioned {
                id: "i".to_string(),
                prior_state: "open".to_string(),
            },
        ] {
            assert_eq!(
                effect.inverse("reason").is_none(),
                effect.class() == Class::Irreversible,
                "{effect:?} disagrees with itself about being reversible"
            );
        }
    }

    #[test]
    fn abandoning_a_run_that_did_nothing_is_a_no_op() {
        let a = abandon(&[], "abandoned");
        assert!(a.inverses.is_empty());
        assert!(a.residue.is_empty());
    }
}
