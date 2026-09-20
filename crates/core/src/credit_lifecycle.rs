//! A refundable credit is a liability until the work resolves it.
//!
//! Phase 31.5. A credit that is refundable if the work fails is a **contract
//! liability with variable consideration**. It cannot be recognised as revenue
//! when it is sold; it is recognised as the underlying performance obligation
//! resolves. Authoritative guidance for agentic-AI outcome-based pricing was
//! published in mid-2026, so this is settled treatment rather than an open
//! question.
//!
//! # Why this is in an engineering plan rather than a finance one
//!
//! > The ledger has to carry the fields, and **retrofitting the money table is
//! > the worst migration in the system.**
//!
//! Phase 2.3 already builds invoice-grade reconciliation and Phase 1 already
//! makes every verdict durable, so the recognition state of a credit is one more
//! derived column on infrastructure that exists. Designed in now it is a schema
//! decision. Discovered at the first audit it is a migration on the one table
//! that must never be wrong, in a codebase whose ledger correctness is its
//! central claim.
//!
//! What the ledger must answer, per credit, at any instant: committed or free;
//! obligation open, satisfied, or refunded; recognised or deferred, with the
//! date and the receipt that resolved it. [`CreditState`] is those three
//! questions and nothing else.
//!
//! # The rule the transitions exist to enforce
//!
//! **Recognition follows satisfaction, and only satisfaction.** There is no
//! constructor for [`Recognition::Recognised`] outside [`CreditState::satisfy`],
//! so a credit cannot be recognised while its obligation is open, and a refunded
//! credit can never be recognised at all. Recognising early is the accounting
//! error this whole section exists to prevent, and it is the one that looks like
//! revenue while it is happening.
//!
//! # Refund form
//!
//! Phase 31.4 decides it: failed work refunds **to credits, at full value,
//! automatically, with no support ticket**. A cash refund is a support-path
//! exception, not a product mechanism, and so is refunding work that passed —
//! [`CreditState::refund`] refuses a satisfied obligation rather than quietly
//! reversing it. The customer who rejects a change Cortex verified is a real
//! conversation (Phase 28.5's acceptance gap says roughly half of test-passing
//! PRs get rejected), and it is a conversation, not a state transition.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Whether this credit is spoken for by dispatched work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Commitment {
    Free,
    Committed,
}

/// The performance obligation this credit carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Obligation {
    /// Sold, and not yet resolved either way. This is the liability.
    Open,
    /// The work was delivered and verified.
    Satisfied,
    /// The work failed and the credit went back to the balance.
    Refunded,
}

/// Revenue recognition state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Recognition {
    /// Held as a liability.
    Deferred,
    /// Recognised, with the date and the receipt that resolved it — both
    /// required, because "recognised" without either is an assertion rather
    /// than an entry.
    Recognised {
        on: DateTime<Utc>,
        receipt_id: String,
    },
}

/// What one credit is, at one instant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreditState {
    pub commitment: Commitment,
    pub obligation: Obligation,
    recognition: Recognition,
}

/// Why a transition was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum TransitionError {
    #[error("credit is already committed")]
    AlreadyCommitted,
    #[error("credit is not committed to any work")]
    NotCommitted,
    #[error("obligation is already resolved")]
    AlreadyResolved,
    #[error("a satisfied obligation is not reversed here; a cash refund is a support path")]
    SatisfiedObligationNeedsSupportPath,
}

impl CreditState {
    /// A credit at the moment it is sold: unspent, owing, and deferred.
    pub fn sold() -> Self {
        Self {
            commitment: Commitment::Free,
            obligation: Obligation::Open,
            recognition: Recognition::Deferred,
        }
    }

    /// Read-only. The only writer is [`satisfy`](Self::satisfy).
    pub fn recognition(&self) -> &Recognition {
        &self.recognition
    }

    pub fn is_recognised(&self) -> bool {
        matches!(self.recognition, Recognition::Recognised { .. })
    }

    /// Cortex's exposure: sold and dispatched, not yet resolved.
    ///
    /// Summed across credits this is Phase 31.3's **outstanding obligation**,
    /// and the input to whether a new large run may be accepted.
    pub fn is_outstanding_obligation(&self) -> bool {
        self.commitment == Commitment::Committed && self.obligation == Obligation::Open
    }

    /// Commit this credit to dispatched work.
    pub fn commit(mut self) -> Result<Self, TransitionError> {
        // Resolution is checked first: a resolved credit is terminal, and
        // "already resolved" tells the caller why far better than "already
        // committed", which is true of every resolved credit and explains none
        // of them.
        if self.obligation != Obligation::Open {
            return Err(TransitionError::AlreadyResolved);
        }
        if self.commitment == Commitment::Committed {
            return Err(TransitionError::AlreadyCommitted);
        }
        self.commitment = Commitment::Committed;
        Ok(self)
    }

    /// The work was verified. Satisfy the obligation and recognise the revenue,
    /// in one step, naming the receipt that did it.
    ///
    /// One step because they are one event. Two methods would allow the state
    /// where an obligation is satisfied and the revenue is still deferred, or —
    /// far worse — the reverse.
    pub fn satisfy(
        mut self,
        receipt_id: impl Into<String>,
        on: DateTime<Utc>,
    ) -> Result<Self, TransitionError> {
        if self.commitment != Commitment::Committed {
            return Err(TransitionError::NotCommitted);
        }
        if self.obligation != Obligation::Open {
            return Err(TransitionError::AlreadyResolved);
        }
        self.obligation = Obligation::Satisfied;
        self.recognition = Recognition::Recognised {
            on,
            receipt_id: receipt_id.into(),
        };
        Ok(self)
    }

    /// The work failed. Discharge the obligation and mint the replacement.
    ///
    /// Returns the now-terminal credit **and** a fresh one for the customer's
    /// balance, because Phase 31.4 refunds to credits at full value and a ledger
    /// does not un-write rows. Sending this same credit back to `Free` would
    /// erase the evidence that it was ever refunded — and the loss ratio is
    /// computed from exactly that evidence.
    ///
    /// The revenue on the refunded credit stays deferred forever: nothing was
    /// earned. Refuses a satisfied obligation, because reversing verified work
    /// is a support conversation rather than a product mechanism.
    pub fn refund(mut self) -> Result<(Self, Self), TransitionError> {
        match self.obligation {
            Obligation::Satisfied => {
                return Err(TransitionError::SatisfiedObligationNeedsSupportPath);
            }
            Obligation::Refunded => return Err(TransitionError::AlreadyResolved),
            Obligation::Open => {}
        }
        if self.commitment != Commitment::Committed {
            return Err(TransitionError::NotCommitted);
        }
        self.obligation = Obligation::Refunded;
        Ok((self, Self::sold()))
    }
}

/// Refunded credits over credits committed. Phase 31.3's health metric.
///
/// `None` when nothing was committed — no denominator, and a zero would read as
/// perfect health from a business that has not traded.
pub fn loss_ratio(credits: &[CreditState]) -> Option<f64> {
    let committed = credits
        .iter()
        .filter(|c| c.commitment == Commitment::Committed)
        .count();
    if committed == 0 {
        return None;
    }
    let refunded = credits
        .iter()
        .filter(|c| c.obligation == Obligation::Refunded)
        .count();
    Some(refunded as f64 / committed as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-05T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn committed() -> CreditState {
        CreditState::sold()
            .commit()
            .expect("a fresh credit commits")
    }

    fn satisfied() -> CreditState {
        committed()
            .satisfy("receipt-7", now())
            .expect("verified work")
    }

    /// The failed credit only; the replacement is checked separately.
    fn refunded() -> CreditState {
        committed().refund().expect("failed work refunds").0
    }

    #[test]
    fn a_sold_credit_is_a_liability_not_revenue() {
        // The whole of 31.5 in one assertion. Money has changed hands and
        // nothing has been earned.
        let c = CreditState::sold();
        assert_eq!(c.obligation, Obligation::Open);
        assert_eq!(c.recognition(), &Recognition::Deferred);
        assert!(!c.is_recognised());
    }

    #[test]
    fn revenue_is_recognised_exactly_when_the_obligation_is_satisfied() {
        let c = satisfied();
        assert_eq!(c.obligation, Obligation::Satisfied);
        assert!(c.is_recognised());
        assert_eq!(
            c.recognition(),
            &Recognition::Recognised {
                on: now(),
                receipt_id: "receipt-7".to_string()
            }
        );
    }

    #[test]
    fn a_refunded_credit_is_never_recognised() {
        // Nothing was earned. The revenue stays deferred forever, and there is
        // no method that could move it.
        let c = refunded();
        assert_eq!(c.obligation, Obligation::Refunded);
        assert!(!c.is_recognised());
        assert_eq!(c.recognition(), &Recognition::Deferred);
    }

    #[test]
    fn a_refund_mints_a_replacement_rather_than_resurrecting_the_credit() {
        // Phase 31.4 refunds to credits at full value, automatically. A ledger
        // does not un-write rows, so the failed credit stays terminal evidence
        // and the customer gets a fresh one -- otherwise the loss ratio is
        // computed from records that erased themselves.
        let (failed, replacement) = committed().refund().unwrap();
        assert_eq!(failed.obligation, Obligation::Refunded);
        assert_eq!(failed.commitment, Commitment::Committed);
        assert_eq!(replacement, CreditState::sold());
        assert!(replacement.commit().is_ok(), "the replacement is spendable");
    }

    #[test]
    fn a_refunded_credit_is_terminal() {
        let c = refunded();
        assert_eq!(c.clone().commit(), Err(TransitionError::AlreadyResolved));
        assert_eq!(
            c.clone().satisfy("r", now()),
            Err(TransitionError::AlreadyResolved)
        );
        assert_eq!(c.refund(), Err(TransitionError::AlreadyResolved));
    }

    #[test]
    fn verified_work_is_not_reversed_by_this_mechanism() {
        // Roughly half of test-passing PRs get rejected by maintainers, so a
        // customer disputing verified work is a real and common conversation.
        // It is a conversation, not a state transition, and the error says so.
        assert_eq!(
            satisfied().refund(),
            Err(TransitionError::SatisfiedObligationNeedsSupportPath)
        );
    }

    #[test]
    fn an_uncommitted_credit_cannot_be_satisfied_or_refunded() {
        // Nothing was dispatched against it, so there is no work to have
        // succeeded or failed.
        assert_eq!(
            CreditState::sold().satisfy("r", now()),
            Err(TransitionError::NotCommitted)
        );
        assert_eq!(
            CreditState::sold().refund(),
            Err(TransitionError::NotCommitted)
        );
    }

    #[test]
    fn an_obligation_is_satisfied_once() {
        assert_eq!(
            satisfied().satisfy("r2", now()),
            Err(TransitionError::AlreadyResolved)
        );
    }

    #[test]
    fn committing_twice_is_refused() {
        assert_eq!(committed().commit(), Err(TransitionError::AlreadyCommitted));
    }

    #[test]
    fn outstanding_obligation_counts_only_dispatched_unresolved_work() {
        // Phase 31.3's exposure number, and the input to whether a new large
        // run may be accepted. An unspent credit is a liability and not
        // exposure -- nothing is being attempted against it.
        assert!(!CreditState::sold().is_outstanding_obligation());
        assert!(committed().is_outstanding_obligation());
        assert!(!satisfied().is_outstanding_obligation());
        assert!(!refunded().is_outstanding_obligation());
    }

    #[test]
    fn the_loss_ratio_is_refunds_over_what_was_committed() {
        let credits = vec![satisfied(), satisfied(), satisfied(), refunded()];
        assert_eq!(loss_ratio(&credits), Some(0.25));
    }

    #[test]
    fn a_replacement_credit_does_not_flatter_the_loss_ratio() {
        // The replacement is uncommitted, so it is not yet part of the
        // denominator. If it were, every refund would dilute the metric that
        // measures refunds.
        let (failed, replacement) = committed().refund().unwrap();
        assert_eq!(loss_ratio(&[failed, replacement]), Some(1.0));
    }

    #[test]
    fn a_business_that_has_not_traded_has_no_loss_ratio() {
        // Not zero. A zero here reads as perfect health, which is the most
        // misleading possible summary of having sold nothing.
        assert_eq!(loss_ratio(&[]), None);
        assert_eq!(
            loss_ratio(&[CreditState::sold(), CreditState::sold()]),
            None
        );
    }
}
