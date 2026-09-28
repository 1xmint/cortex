//! Binding an attempt's end to the ledger.
//!
//! Task **V4** of `cortex/plan/VERIFIER.md`, minus its persistence. Storing
//! verdicts needs migration v61 and is blocked behind the shared counter;
//! deciding what an attempt's end *does to money* is not, and it is the part
//! worth getting right first, because it is the part that can lose money in
//! both directions.
//!
//! **The owner's settled rule (2026-09-27, final — do not re-open):**
//! customers pay exactly what the model calls an attempt made cost, nothing
//! more. There are no credit holds and no locking of credits — those were
//! considered and explicitly dropped. A failed attempt is still charged for
//! the calls it made (the customer receives the work anyway, as a draft PR
//! labelled with the failure — see `routes.rs`'s
//! `a_failed_run_is_delivered_as_a_draft_marked_failed_checks`). A Cortex
//! outage, or Cortex's own machinery breaking before a verdict, is never the
//! customer's problem: Cortex absorbs the cost of calls already made and
//! charges nothing. Nothing here ever refunds a charge that already
//! happened — the old verdict-driven `Refund` path (a `Failed` verdict
//! reversing a charge) is gone. [`RefundKey`] and `Database::refund_credits`
//! still exist and are still used — by `voice.rs`, for a dictation-token
//! refund that has nothing to do with a task attempt's verdict — so they are
//! kept, but nothing in this module reaches them anymore.
//!
//! Two failure modes this exists to make impossible:
//!
//! - **Double-charging.** The orchestrator retries by design (`max_attempts`,
//!   lease expiry requeues). A step that completes, charges, and then has its
//!   result rejected for a stale `lease_gen` will be re-run — and must not be
//!   charged twice. [`ChargeKey::for_attempt`] is derived from the attempt id
//!   alone, so a re-grade or replay of the same attempt reuses the same key
//!   and `charge_settled_cost` turns it into a no-op.
//! - **Charging or refunding for our own outage.** An infrastructure failure
//!   is not a customer's problem, and it is equally wrong to bill them for it
//!   or to refund them work that was never done. There is no refund arm left
//!   to reach by accident.

use serde::{Deserialize, Serialize};

/// Why an attempt ended, for billing purposes.
///
/// This is deliberately one enum rather than a `(cause, Option<Verdict>)`
/// pair: several of these ends never produce a `Verdict` at all (there is no
/// "N/A" verdict), and the three `Verdict::Inconclusive`-producing call sites
/// in `verification_driver.rs` (an exam-tamper refusal, an integrity-unknown
/// refusal, and a failed tree checkout) already know their own specific
/// reason without needing to route it back through a shared `Verdict` first.
/// Folding cause into one richer enum avoids an unused parameter, which
/// would trip clippy's `-D warnings`.
///
/// `WorkerLost` is deliberately not a separate variant from `LeaseExpired`:
/// this codebase has exactly one mechanism for noticing a worker is gone —
/// `expire_stale_leases`, driven by `lease_deadline` — so "the worker
/// disappeared" and "the lease expired" are the same observable event, never
/// two things that could be told apart. Giving them separate variants would
/// invite a caller to guess which one happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptEndCause {
    /// The verifier ran the frozen checks and they passed.
    Verified,
    /// No executable ground truth existed to check against. Sold at the
    /// normal rate, without the "verified" badge.
    Unverified,
    /// The verifier ran the frozen checks and they failed. Still charged: the
    /// customer receives the work (as a draft PR marked failed) and the calls
    /// that produced it were real calls.
    Failed,
    /// The attempt declared a strong contract and then edited the protected
    /// exam surface that would have graded it. This is the agent breaking its
    /// own contract, not Cortex's problem — charged like any other outcome
    /// the agent is responsible for.
    ExamTampered,
    /// The customer cancelled the run. Calls already made were made for them.
    CustomerCancel,
    /// The check runner (or a service it depends on) was down, so no verdict
    /// could be produced after retries were exhausted. Cortex's problem.
    RunnerDown,
    /// The attempt's lease expired (equivalently: the worker never reported
    /// back) before it delivered anything to verify. Cortex's problem: the
    /// customer did not cause this and cannot be shown a verdict for it.
    LeaseExpired,
    /// Cortex's own machinery failed before a verdict could be reached (for
    /// example: the delivered tree could not be checked out for grading, or
    /// the frozen exam's integrity could not be established at all).
    CortexCrash,
}

/// What settling an attempt does to the ledger.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AttemptSettlement {
    /// Charge the customer for the attempt's settled observed cost.
    Charge,
    /// Cortex absorbs the attempt's settled observed cost. Customer balance
    /// and carry are untouched; the cost is still recorded, against Cortex.
    Absorb(AttemptEndCause),
}

/// Decide what an attempt's end does to the ledger.
///
/// Pure, and total over [`AttemptEndCause`] — every variant maps to exactly
/// one settlement, so there is no default arm to silently swallow a new
/// cause added later without updating this table.
pub fn settle_attempt(end_cause: AttemptEndCause) -> AttemptSettlement {
    use AttemptEndCause::*;
    match end_cause {
        Verified | Unverified | Failed | ExamTampered | CustomerCancel => {
            AttemptSettlement::Charge
        }
        RunnerDown | LeaseExpired | CortexCrash => AttemptSettlement::Absorb(end_cause),
    }
}

/// Ledger reason codes for an attempt settlement. Kept in one place so a
/// receipt, an invoice line and a ledger row cannot drift into describing the
/// same event differently.
pub mod reason {
    pub const TASK_ATTEMPT_CHARGED: &str = "task_attempt_charged";
    pub const TASK_ATTEMPT_ABSORBED: &str = "task_attempt_absorbed";
}

/// The exact customer-facing message when production dispatch is blocked
/// because the provider gateway is off. Stable and quoted by tests: this is
/// what a customer sees, and it must not silently reword itself.
pub const GATEWAY_DOWN_MESSAGE: &str = "Cortex can't run tasks right now. Try again later.";

/// What the scheduler should do with a step it is about to dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DispatchGate {
    /// Dispatch normally.
    Allow,
    /// Dispatch, but log a warning: not production, so the money gate does
    /// not apply, but something about the environment is worth noting.
    AllowWithWarning,
    /// Refuse to dispatch. No customer charge follows a blocked step.
    Block,
}

/// Whether a step may be dispatched, given the environment and whether the
/// provider gateway is reachable.
///
/// Production requires the gateway to be on: a step dispatched without it
/// cannot make a priced call, so the correct behaviour is to refuse before
/// any lease is spent, not to let the worker discover it has nothing to call.
/// A balance or spend-cap check does **not** belong here — that is the
/// account-level pause/top-up gate, a separate concern from "can Cortex run
/// anything at all right now."
///
/// Outside production, the gateway being off is expected (local/dev/stub
/// setups) and dispatch proceeds with a warning rather than a block.
pub fn dispatch_money_gate(is_production: bool, gateway_on: bool) -> DispatchGate {
    match (is_production, gateway_on) {
        (true, true) => DispatchGate::Allow,
        (true, false) => DispatchGate::Block,
        (false, _) => DispatchGate::AllowWithWarning,
    }
}

/// The key a charge is written under.
///
/// A type rather than a `String` because the ledger's refund takes both keys
/// and they used to be two bare `&str` in a row: nothing stopped a caller
/// passing them the wrong way round, which would look for a charge under the
/// refund's key, find none, and move no money while reporting success.
///
/// The field is private, so a `ChargeKey` can only come from one of the
/// constructors below. That is the whole enforcement — not that the key is
/// *correct*, but that it was derived somewhere that had to think about it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChargeKey(String);

/// The key a refund is written under. Still used by `voice.rs` for a
/// dictation-token refund — an unrelated feature with its own idempotency
/// need — even though nothing in this module produces one anymore.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefundKey(String);

impl ChargeKey {
    /// The charge for a verification. Derived, never generated: replaying a
    /// transition after a crash between verdict and ledger write must
    /// re-derive the same key, so the ledger's `UNIQUE` constraint turns the
    /// retry into a no-op instead of a second charge.
    pub fn for_verification(verification_id: &str) -> Self {
        Self(format!("verify:{verification_id}"))
    }

    /// The charge for one task attempt's settled observed cost. Derived from
    /// the attempt id alone (not the verification id): a re-grade or replay
    /// of the same attempt must reuse the same key and write zero new rows,
    /// and an attempt can in principle be graded more than once (a verifier
    /// retried after a crash) while remaining one attempt for billing.
    pub fn for_attempt(attempt_id: &str) -> Self {
        Self(format!("attempt:{attempt_id}"))
    }

    /// A charge for one unit of work that is not a verification.
    ///
    /// The credit ledger is shared with HeyVera Socials, which bills actions
    /// that no verdict stands behind — a Pulse draft, for one. Those are real
    /// charges and they need real keys; what they cannot have is a verification
    /// id, because there is no verification. The caller supplies a label that
    /// is unique per unit of work.
    ///
    /// This is the constructor that stops the type being a Cortex-only
    /// guarantee, and it is why the guarantee is "derived deliberately"
    /// rather than "tied to a verdict". Narrowing further waits on the
    /// product split.
    pub fn per_unit(label: impl Into<String>) -> Self {
        Self(label.into())
    }

    /// The charge for one Cortex-paid chat reply. Derived from the reply's
    /// own id (`chat-reply:<uuid>`, minted once per reply) so a retried
    /// settlement — the gateway reservation reconciling twice, a handler
    /// re-entered after a crash — re-derives the same key and the ledger's
    /// `UNIQUE` constraint turns it into a no-op rather than a second charge.
    pub fn for_chat_reply(reply_id: &str) -> Self {
        Self(format!("chat-reply:{reply_id}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl RefundKey {
    /// The refund for a verification.
    pub fn for_verification(verification_id: &str) -> Self {
        Self(format!("refund:{verification_id}"))
    }

    /// A refund for one unit of work charged with [`ChargeKey::per_unit`].
    /// The caller supplies a label distinct from the charge's own label, so
    /// the refund lands in its own idempotency namespace rather than
    /// colliding with the charge it reverses.
    pub fn per_unit(label: impl Into<String>) -> Self {
        Self(label.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AID: &str = "att_9f2c";

    #[test]
    fn verified_unverified_failed_charge() {
        for cause in [
            AttemptEndCause::Verified,
            AttemptEndCause::Unverified,
            AttemptEndCause::Failed,
        ] {
            assert_eq!(
                settle_attempt(cause),
                AttemptSettlement::Charge,
                "{cause:?} must charge"
            );
        }
    }

    #[test]
    fn customer_cancel_charges() {
        // Calls already made were made for the customer.
        assert_eq!(
            settle_attempt(AttemptEndCause::CustomerCancel),
            AttemptSettlement::Charge
        );
    }

    #[test]
    fn exam_tampering_charges() {
        // The agent broke its own contract; that is not Cortex's outage.
        assert_eq!(
            settle_attempt(AttemptEndCause::ExamTampered),
            AttemptSettlement::Charge
        );
    }

    #[test]
    fn infra_failures_absorb() {
        for cause in [
            AttemptEndCause::RunnerDown,
            AttemptEndCause::LeaseExpired,
            AttemptEndCause::CortexCrash,
        ] {
            assert_eq!(
                settle_attempt(cause),
                AttemptSettlement::Absorb(cause),
                "{cause:?} must absorb, carrying its own cause"
            );
        }
    }

    #[test]
    fn no_end_cause_ever_refunds() {
        // The old verdict-driven Refund path is gone. There is no
        // `AttemptSettlement::Refund` variant to reach — this is a
        // compile-time guarantee, not just a runtime one — but assert the
        // full enum is exhaustively Charge or Absorb so a reviewer sees it
        // stated, not just implied by the type signature.
        for cause in [
            AttemptEndCause::Verified,
            AttemptEndCause::Unverified,
            AttemptEndCause::Failed,
            AttemptEndCause::ExamTampered,
            AttemptEndCause::CustomerCancel,
            AttemptEndCause::RunnerDown,
            AttemptEndCause::LeaseExpired,
            AttemptEndCause::CortexCrash,
        ] {
            assert!(matches!(
                settle_attempt(cause),
                AttemptSettlement::Charge | AttemptSettlement::Absorb(_)
            ));
        }
    }

    #[test]
    fn charge_keys_are_derived_so_a_replay_reproduces_them() {
        assert_eq!(ChargeKey::for_attempt(AID), ChargeKey::for_attempt(AID));
        assert_eq!(
            ChargeKey::for_attempt(AID).as_str(),
            format!("attempt:{AID}")
        );
    }

    #[test]
    fn attempt_keys_never_collide_with_verification_or_refund_keys() {
        assert_ne!(
            ChargeKey::for_attempt(AID).as_str(),
            ChargeKey::for_verification(AID).as_str()
        );
        assert_ne!(
            ChargeKey::for_attempt(AID).as_str(),
            RefundKey::for_verification(AID).as_str()
        );
    }

    #[test]
    fn a_charge_key_cannot_be_conjured_from_a_string() {
        // The property this type exists for, stated where a reader will look
        // for it. Both of these are compile errors, which is why they are
        // written down rather than asserted:
        //
        //     let k: ChargeKey = "attempt:anything".into();
        //     let k = ChargeKey("attempt:anything".to_string());
        //
        // The field is private and there is no `From<&str>`, so every key in
        // the ledger came from a constructor that had to name what it was for.
        let unit = ChargeKey::per_unit("pulse-draft:abc");
        assert_eq!(unit.as_str(), "pulse-draft:abc");
        assert_ne!(unit, ChargeKey::for_attempt(AID));
    }

    #[test]
    fn dispatch_gate_truth_table() {
        assert_eq!(dispatch_money_gate(true, true), DispatchGate::Allow);
        assert_eq!(dispatch_money_gate(true, false), DispatchGate::Block);
        assert_eq!(
            dispatch_money_gate(false, true),
            DispatchGate::AllowWithWarning
        );
        assert_eq!(
            dispatch_money_gate(false, false),
            DispatchGate::AllowWithWarning
        );
    }

    #[test]
    fn production_without_gateway_is_the_only_block() {
        // Stated as its own test because it is the one row a reviewer of
        // money-moving code will look for first: is there ever a customer
        // charge with nothing behind it? No — Block dispatches nothing, so
        // nothing downstream can be charged for a call that never happened.
        for is_production in [true, false] {
            for gateway_on in [true, false] {
                let blocked = dispatch_money_gate(is_production, gateway_on) == DispatchGate::Block;
                assert_eq!(blocked, is_production && !gateway_on);
            }
        }
    }
}
