//! Billing at the ledger, not just the quote: billing is pass-through and does
//! not vary by `verdict_class` -- `strong` and `authored` steps of the same
//! task class are charged and refunded identically. `verdict_class` is a
//! stored label only, surfaced on the plan receipt and the verification
//! receipt; it is never a pricing input. This asserts the money actually
//! moves through `deduct_credits`/`refund_credits`, the same calls
//! `verification_driver::finish_and_bill` makes.

use cortex_api::db::Database;
use cortex_api::pricing::{self, PriceStatus, StepQuote};
use cortex_core::billing_binding::{ChargeKey, RefundKey};
use cortex_core::diff_surface::VerdictClass;
use cortex_core::task_class::TaskClass;
use uuid::Uuid;

fn db() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().expect("temp dir");
    let db = Database::open(&dir.path().join("cortex.db"));
    (dir, db)
}

/// A price list committed enough to bill, with its first class committed too
/// — the same shape `a_committed_class_in_a_committed_list_bills` in
/// `pricing.rs` uses to get a billable quote out of `quote()`.
fn billable_list(db: &Database) -> (cortex_api::pricing::PriceList, TaskClass) {
    let mut list = db.active_price_list().expect("seed list publishes");
    list.status = PriceStatus::Committed;
    let class = TaskClass::all()[0];
    let key = class.key();
    let priced = list
        .classes
        .iter_mut()
        .find(|c| c.task_class == key)
        .expect("seed list prices every class");
    priced.status = PriceStatus::Committed;
    (list, class)
}

/// Freezes a quote for the class. `verdict_class` is accepted only so callers
/// can record which label the step declared -- it plays no part in the price,
/// which `pricing::quote` computes from the class alone.
fn freeze(
    db: &Database,
    run_id: &str,
    step_id: &str,
    list: &cortex_api::pricing::PriceList,
    class: &TaskClass,
    _verdict_class: VerdictClass,
) -> i64 {
    let (credits, billable) = pricing::quote(list, class).expect("priced");
    assert!(billable, "test fixture must be billable");
    let quote = StepQuote {
        quote_id: Uuid::new_v4().to_string(),
        run_id: run_id.to_string(),
        step_id: step_id.to_string(),
        task_class: class.key().to_string(),
        quoted_credits: credits,
        price_list_id: list.id.clone(),
        price_list_version: list.version,
        billable,
        frozen_at: 0,
    };
    db.freeze_step_quote(&quote).expect("freeze quote");
    credits
}

#[test]
fn authored_and_strong_are_charged_the_same_price() {
    // Billing is pass-through: the declared class is a stored label, not a
    // pricing input. A class that costs the same to run costs the same
    // whether the step was declared `strong` or `authored`.
    let (_dir, db) = db();
    let (list, class) = billable_list(&db);

    let strong_credits = pricing::quote(&list, &class).expect("priced").0;
    let authored_credits = pricing::quote(&list, &class).expect("priced").0;
    assert_eq!(
        strong_credits, authored_credits,
        "quote() takes no verdict_class and must not vary by declared class"
    );

    let user = "user_authored_charge";
    db.init_credit_balance(user, 10_000).expect("init balance");

    let verification_id = "verify-authored-1";
    let frozen = freeze(
        &db,
        "run-1",
        "step-1",
        &list,
        &class,
        VerdictClass::Authored,
    );
    assert_eq!(frozen, authored_credits);

    let charge_key = ChargeKey::for_verification(verification_id);
    db.deduct_credits(user, frozen, "authored verdict charge", &charge_key)
        .expect("charge succeeds");

    let (sub_total, pack_total) = db.credit_ledger_totals(user);
    assert_eq!(
        sub_total + pack_total,
        -authored_credits,
        "the ledger must show the full class price, not a discounted one"
    );
}

#[test]
fn a_strong_task_is_charged_the_full_price() {
    let (_dir, db) = db();
    let (list, class) = billable_list(&db);

    let user = "user_strong_charge";
    db.init_credit_balance(user, 10_000).expect("init balance");

    let verification_id = "verify-strong-1";
    let strong_credits = freeze(&db, "run-2", "step-2", &list, &class, VerdictClass::Strong);

    let charge_key = ChargeKey::for_verification(verification_id);
    db.deduct_credits(user, strong_credits, "strong verdict charge", &charge_key)
        .expect("charge succeeds");

    let (sub_total, pack_total) = db.credit_ledger_totals(user);
    assert_eq!(sub_total + pack_total, -strong_credits);
}

// The two tests this replaces hand-called `deduct_credits` then
// `refund_credits` back to back to assert a net-zero ledger for a "failed"
// task. That is not a shape `finish_and_bill` ever produces: per
// `billing_binding::billing_effect`, `(Failed, Unbilled) => None` -- a step
// that never got charged in the first place is never billed at all, so there
// is nothing to refund and no ledger row is ever written for it. Charging
// then immediately refunding under a `"task failed"` reason describes a state
// transition production's terminal, once-per-verification verdict write can
// never reach. These replacements assert the real invariant instead: a
// `Failed` verdict from an unbilled state touches nothing, for either class.
#[test]
fn a_failed_task_from_an_unbilled_state_has_no_billing_effect_authored() {
    use cortex_core::billing_binding::{billing_effect, BillingEffect, BillingState};
    use cortex_core::verification::Verdict;

    let (_dir, db) = db();
    let (list, class) = billable_list(&db);

    let user = "user_authored_never_charged";
    db.init_credit_balance(user, 10_000).expect("init balance");

    let verification_id = "verify-authored-2";
    // Freezing the quote records what the step *would* cost; it is not a
    // charge, and a `Failed` delivery must never turn it into one.
    let _quoted = freeze(
        &db,
        "run-3",
        "step-3",
        &list,
        &class,
        VerdictClass::Authored,
    );

    assert_eq!(
        billing_effect(Verdict::Failed, BillingState::Unbilled, verification_id),
        BillingEffect::None,
        "a failed step that was never charged must not be refunded"
    );

    let (sub_total, pack_total) = db.credit_ledger_totals(user);
    assert_eq!(
        sub_total + pack_total,
        0,
        "no ledger row exists for this verification, so the balance is untouched"
    );
}

#[test]
fn a_failed_task_from_an_unbilled_state_has_no_billing_effect_strong() {
    use cortex_core::billing_binding::{billing_effect, BillingEffect, BillingState};
    use cortex_core::verification::Verdict;

    let (_dir, db) = db();
    let (list, class) = billable_list(&db);

    let user = "user_strong_never_charged";
    db.init_credit_balance(user, 10_000).expect("init balance");

    let verification_id = "verify-strong-2";
    let _quoted = freeze(&db, "run-4", "step-4", &list, &class, VerdictClass::Strong);

    assert_eq!(
        billing_effect(Verdict::Failed, BillingState::Unbilled, verification_id),
        BillingEffect::None,
        "a failed step that was never charged must not be refunded"
    );

    let (sub_total, pack_total) = db.credit_ledger_totals(user);
    assert_eq!(sub_total + pack_total, 0);
}

#[test]
fn the_receipt_reads_the_actual_ledger_charge_not_the_quote() {
    // A quote is a plan; `credit_transactions` is what happened. The receipt
    // must show the latter, so a refund is visible even though the frozen
    // quote never changes.
    let (_dir, db) = db();
    let (list, class) = billable_list(&db);

    let user = "user_actual_vs_quoted";
    db.init_credit_balance(user, 10_000).expect("init balance");

    let verification_id = "verify-actual-1";
    let quoted = freeze(
        &db,
        "run-5",
        "step-5",
        &list,
        &class,
        VerdictClass::Authored,
    );

    // Never charged yet: no ledger rows under this verification's keys.
    assert_eq!(db.ledger_net_charge_for_verification(verification_id), None);

    let charge_key = ChargeKey::for_verification(verification_id);
    db.deduct_credits(user, quoted, "authored verdict charge", &charge_key)
        .expect("charge succeeds");
    assert_eq!(
        db.ledger_net_charge_for_verification(verification_id),
        Some(quoted),
        "charged and not yet refunded: the ledger must show the full charge"
    );

    let refund_key = RefundKey::for_verification(verification_id);
    db.refund_credits(user, &charge_key, &refund_key, "task failed")
        .expect("refund succeeds");
    assert_eq!(
        db.ledger_net_charge_for_verification(verification_id),
        Some(0),
        "charged then fully refunded: the ledger must show zero, not the stale quote"
    );
}
