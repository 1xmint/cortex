//! Finding every receipt a bad artifact touched, and saying so.
//!
//! Phase 32.5. Once receipts are sold as compliance evidence, Cortex acquires an
//! obligation it has no way to discharge. A check-derivation bug, a bad
//! method-library entry, a mis-pinned runner, or a discovered verifier-gaming
//! pattern does not affect one receipt — it affects **every receipt produced
//! while it was live**, and Cortex must be able to find them and say so.
//!
//! > This is unglamorous and it is the difference between a receipt being
//! > evidence and a receipt being a claim. **A vendor that cannot recall a bad
//! > receipt does not have an audit trail; it has a marketing surface with
//! > timestamps.**
//!
//! The capability is a query plus a procedure, and the data is already there
//! because every receipt names its exact versions (invariant 23):
//!
//! 1. Identify the blast radius by artifact version — [`blast_radius`].
//! 2. Re-execute where re-execution is still meaningful; Phase 24.4's shelf life
//!    decides where it is not — [`RecallPlan`].
//! 3. **Notify affected customers with the corrected verdict, whether or not it
//!    changes the outcome**, and reverse the ledger where it does — [`recall`].
//! 4. Publish the correction against the artifact version, so the version
//!    history carries its own errata.
//!
//! # Step 3 is the one with a shape worth enforcing
//!
//! Notification is unconditional. [`recall`] emits one [`Notification`] per
//! affected receipt and there is no branch that skips the ones whose verdict
//! came back the same — because "we checked and it was fine" is exactly the
//! message that makes the next recall believable, and suppressing it converts an
//! audit trail back into a marketing surface.
//!
//! Ledger reversal is conditional, and on the *billing* change rather than the
//! verdict change: a correction from `Inconclusive` to `Failed` moves no money,
//! because neither charged.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::verification::Verdict;

/// A versioned artifact a receipt names.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "artifact")]
pub enum ArtifactVersion {
    CheckDerivationRule { id: String, version: u32 },
    MethodLibraryEntry { id: String, version: u32 },
    ProfessionalPack { id: String, version: u32 },
    RunnerImage { digest: String },
    ExecutionPolicy { version: u32 },
}

/// What a recall needs to know about one receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptRef {
    pub id: String,
    /// Every artifact version that produced it.
    pub artifacts: Vec<ArtifactVersion>,
    pub verdict: Verdict,
    /// Whether re-execution would still mean anything. Phase 24.4's shelf life:
    /// a tree whose dependencies have all moved cannot be re-run into the same
    /// answer, and a re-execution that proves nothing is worse than none because
    /// it looks like diligence.
    pub within_shelf_life: bool,
}

/// The receipts a faulty artifact version produced.
pub fn blast_radius<'a>(
    receipts: &'a [ReceiptRef],
    faulty: &ArtifactVersion,
) -> Vec<&'a ReceiptRef> {
    receipts
        .iter()
        .filter(|r| r.artifacts.contains(faulty))
        .collect()
}

/// What to do about the blast radius, split by whether re-execution is
/// meaningful.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecallPlan {
    pub re_execute: Vec<String>,
    /// Past shelf life. These are still notified — the correction is what the
    /// customer is owed, not the re-run.
    pub notify_without_re_execution: Vec<String>,
}

/// Plan the recall for one faulty artifact version.
pub fn plan(receipts: &[ReceiptRef], faulty: &ArtifactVersion) -> RecallPlan {
    let affected = blast_radius(receipts, faulty);
    RecallPlan {
        re_execute: affected
            .iter()
            .filter(|r| r.within_shelf_life)
            .map(|r| r.id.clone())
            .collect(),
        notify_without_re_execution: affected
            .iter()
            .filter(|r| !r.within_shelf_life)
            .map(|r| r.id.clone())
            .collect(),
    }
}

/// What one affected customer is told.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    pub receipt_id: String,
    pub original: Verdict,
    /// `None` where re-execution was not meaningful. The customer is still told
    /// the artifact was faulty.
    pub corrected: Option<Verdict>,
}

impl Notification {
    /// Whether the verdict actually moved.
    ///
    /// A notification is sent either way; this only says which kind it is.
    pub fn verdict_changed(&self) -> bool {
        self.corrected.is_some_and(|c| c != self.original)
    }
}

/// A ledger entry the correction requires.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerReversal {
    pub receipt_id: String,
    pub from: Verdict,
    pub to: Verdict,
}

/// Everything a recall produces.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recall {
    /// One per affected receipt. Always.
    pub notifications: Vec<Notification>,
    pub ledger_reversals: Vec<LedgerReversal>,
}

/// Whether a verdict caused money to move.
fn charged(verdict: Verdict) -> bool {
    matches!(verdict, Verdict::Verified | Verdict::Unverified)
}

/// Execute the recall.
///
/// `re_executed` maps receipt id to the verdict a re-run produced; receipts
/// absent from it were not re-executed, and are notified with no corrected
/// verdict.
pub fn recall(
    receipts: &[ReceiptRef],
    faulty: &ArtifactVersion,
    re_executed: &BTreeMap<String, Verdict>,
) -> Recall {
    let affected = blast_radius(receipts, faulty);

    let notifications: Vec<Notification> = affected
        .iter()
        .map(|r| Notification {
            receipt_id: r.id.clone(),
            original: r.verdict,
            corrected: re_executed.get(&r.id).copied(),
        })
        .collect();

    let ledger_reversals = notifications
        .iter()
        .filter_map(|n| {
            let corrected = n.corrected?;
            // The ledger moves on the *billing* change, not the verdict change.
            // `Inconclusive` to `Failed` is a real correction that moves no
            // money, because neither charged.
            if corrected != n.original && charged(n.original) != charged(corrected) {
                Some(LedgerReversal {
                    receipt_id: n.receipt_id.clone(),
                    from: n.original,
                    to: corrected,
                })
            } else {
                None
            }
        })
        .collect();

    Recall {
        notifications,
        ledger_reversals,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(version: u32) -> ArtifactVersion {
        ArtifactVersion::CheckDerivationRule {
            id: "cargo-test".to_string(),
            version,
        }
    }

    fn runner(digest: &str) -> ArtifactVersion {
        ArtifactVersion::RunnerImage {
            digest: digest.to_string(),
        }
    }

    fn receipt(id: &str, verdict: Verdict, artifacts: Vec<ArtifactVersion>) -> ReceiptRef {
        ReceiptRef {
            id: id.to_string(),
            artifacts,
            verdict,
            within_shelf_life: true,
        }
    }

    fn corrections(pairs: &[(&str, Verdict)]) -> BTreeMap<String, Verdict> {
        pairs.iter().map(|(id, v)| (id.to_string(), *v)).collect()
    }

    #[test]
    fn the_blast_radius_is_every_receipt_the_faulty_version_produced() {
        // Not one receipt. A bad derivation rule affects everything it graded
        // while it was live, and the data to find them is already on every
        // receipt.
        let receipts = [
            receipt("r1", Verdict::Verified, vec![rule(3)]),
            receipt("r2", Verdict::Verified, vec![rule(4)]),
            receipt("r3", Verdict::Failed, vec![rule(3), runner("sha256:a")]),
        ];
        let hit = blast_radius(&receipts, &rule(3));
        assert_eq!(hit.len(), 2);
        assert_eq!(hit[0].id, "r1");
        assert_eq!(hit[1].id, "r3");
    }

    #[test]
    fn a_different_version_of_the_same_artifact_is_not_affected() {
        // Version 4 fixed the bug. Recalling its receipts would be recalling
        // work that was graded correctly.
        let receipts = [receipt("r2", Verdict::Verified, vec![rule(4)])];
        assert!(blast_radius(&receipts, &rule(3)).is_empty());
    }

    #[test]
    fn a_receipt_past_its_shelf_life_is_notified_without_re_execution() {
        // A tree whose dependencies have all moved cannot be re-run into the
        // same answer, and a re-execution that proves nothing is worse than
        // none because it looks like diligence.
        let stale = ReceiptRef {
            within_shelf_life: false,
            ..receipt("old", Verdict::Verified, vec![rule(3)])
        };
        let receipts = [stale, receipt("fresh", Verdict::Verified, vec![rule(3)])];
        let p = plan(&receipts, &rule(3));
        assert_eq!(p.re_execute, vec!["fresh".to_string()]);
        assert_eq!(p.notify_without_re_execution, vec!["old".to_string()]);
    }

    #[test]
    fn every_affected_customer_is_notified_even_when_nothing_changed() {
        // The rule with a shape worth enforcing. "We checked and it was fine"
        // is exactly the message that makes the next recall believable, and
        // there is no branch here that skips it.
        let receipts = [
            receipt("r1", Verdict::Verified, vec![rule(3)]),
            receipt("r2", Verdict::Verified, vec![rule(3)]),
        ];
        let unchanged = corrections(&[("r1", Verdict::Verified), ("r2", Verdict::Verified)]);
        let r = recall(&receipts, &rule(3), &unchanged);
        assert_eq!(r.notifications.len(), 2);
        assert!(r.notifications.iter().all(|n| !n.verdict_changed()));
        assert!(r.ledger_reversals.is_empty());
    }

    #[test]
    fn a_verified_that_should_have_failed_reverses_the_ledger() {
        // The expensive case, and the reason this capability exists: Cortex
        // charged for work its own bug certified.
        let receipts = [receipt("r1", Verdict::Verified, vec![rule(3)])];
        let r = recall(
            &receipts,
            &rule(3),
            &corrections(&[("r1", Verdict::Failed)]),
        );
        assert_eq!(
            r.ledger_reversals,
            vec![LedgerReversal {
                receipt_id: "r1".to_string(),
                from: Verdict::Verified,
                to: Verdict::Failed
            }]
        );
        assert!(r.notifications[0].verdict_changed());
    }

    #[test]
    fn a_failed_that_should_have_passed_also_reverses() {
        // Corrections run in Cortex's favour too, and a recall that only fixed
        // the ones costing money would be a refund process rather than an audit
        // trail.
        let receipts = [receipt("r1", Verdict::Failed, vec![rule(3)])];
        let r = recall(
            &receipts,
            &rule(3),
            &corrections(&[("r1", Verdict::Verified)]),
        );
        assert_eq!(r.ledger_reversals.len(), 1);
    }

    #[test]
    fn a_correction_that_moves_no_money_notifies_without_touching_the_ledger() {
        // `Inconclusive` to `Failed` is a real correction and neither charged.
        // Writing a reversal for it would put a zero-value entry on the one
        // table that must never be wrong.
        let receipts = [receipt("r1", Verdict::Inconclusive, vec![rule(3)])];
        let r = recall(
            &receipts,
            &rule(3),
            &corrections(&[("r1", Verdict::Failed)]),
        );
        assert_eq!(r.notifications.len(), 1);
        assert!(r.notifications[0].verdict_changed());
        assert!(r.ledger_reversals.is_empty());
    }

    #[test]
    fn a_receipt_that_was_not_re_executed_is_notified_with_no_corrected_verdict() {
        // Past shelf life. The customer is owed the correction, not the re-run,
        // and claiming a verdict nobody produced would be worse than saying so.
        let stale = ReceiptRef {
            within_shelf_life: false,
            ..receipt("old", Verdict::Verified, vec![rule(3)])
        };
        let r = recall(&[stale], &rule(3), &BTreeMap::new());
        assert_eq!(r.notifications.len(), 1);
        assert_eq!(r.notifications[0].corrected, None);
        assert!(!r.notifications[0].verdict_changed());
        assert!(r.ledger_reversals.is_empty());
    }

    #[test]
    fn a_recall_reaches_every_kind_of_artifact_a_receipt_names() {
        // A mis-pinned runner and a bad professional pack are the same problem
        // as a bad derivation rule, and the query has to reach all of them.
        let artifacts = [
            rule(1),
            ArtifactVersion::MethodLibraryEntry {
                id: "tdd".to_string(),
                version: 2,
            },
            ArtifactVersion::ProfessionalPack {
                id: "security".to_string(),
                version: 5,
            },
            runner("sha256:bad"),
            ArtifactVersion::ExecutionPolicy { version: 9 },
        ];
        for artifact in &artifacts {
            let receipts = [receipt("r1", Verdict::Verified, vec![artifact.clone()])];
            assert_eq!(
                blast_radius(&receipts, artifact).len(),
                1,
                "{artifact:?} must be recallable"
            );
        }
    }

    #[test]
    fn a_clean_artifact_recalls_nothing() {
        let receipts = [receipt("r1", Verdict::Verified, vec![rule(4)])];
        let r = recall(&receipts, &rule(3), &BTreeMap::new());
        assert!(r.notifications.is_empty());
        assert!(r.ledger_reversals.is_empty());
    }
}
