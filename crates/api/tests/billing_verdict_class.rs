//! Billing at the ledger, not just the quote: an `authored` task is charged
//! `credits_for_verdict_class` — ceil(full/2) — of what a `strong` task would
//! pay for the same class, and a failed task of either class is refunded
//! exactly what it was charged. `pricing::quote` already asserts the halved
//! number in isolation; this asserts the money actually moves that way
//! through `deduct_credits`/`refund_credits`, the same calls
//! `verification_driver::finish_and_bill` makes.

use cortex_api::db::Database;
use cortex_api::pricing::{self, credits_for_verdict_class, PriceStatus, StepQuote};
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
    list.classes[0].status = PriceStatus::Committed;
    let class = TaskClass::all()[0];
    (list, class)
}

fn freeze(
    db: &Database,
    run_id: &str,
    step_id: &str,
    list: &cortex_api::pricing::PriceList,
    class: &TaskClass,
    verdict_class: VerdictClass,
) -> i64 {
    let (credits, billable) = pricing::quote(list, class, verdict_class).expect("priced");
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
fn an_authored_task_is_charged_half_of_strong_rounded_up() {
    let (_dir, db) = db();
    let (list, class) = billable_list(&db);

    let strong_credits = pricing::quote(&list, &class, VerdictClass::Strong)
        .expect("priced")
        .0;
    let authored_credits = pricing::quote(&list, &class, VerdictClass::Authored)
        .expect("priced")
        .0;

    assert_eq!(
        authored_credits,
        credits_for_verdict_class(strong_credits, VerdictClass::Authored)
    );
    // The halving actually rounds up rather than down, otherwise this test
    // would pass for a class whose price happens to already be even.
    assert_eq!(authored_credits, (strong_credits + 1) / 2);

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
        "the ledger must show exactly the halved amount, not the full price"
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

#[test]
fn a_failed_authored_task_is_refunded_exactly_what_it_was_charged() {
    let (_dir, db) = db();
    let (list, class) = billable_list(&db);

    let user = "user_authored_refund";
    db.init_credit_balance(user, 10_000).expect("init balance");

    let verification_id = "verify-authored-2";
    let charged = freeze(
        &db,
        "run-3",
        "step-3",
        &list,
        &class,
        VerdictClass::Authored,
    );

    let charge_key = ChargeKey::for_verification(verification_id);
    let refund_key = RefundKey::for_verification(verification_id);

    db.deduct_credits(user, charged, "authored verdict charge", &charge_key)
        .expect("charge succeeds");
    db.refund_credits(user, &charge_key, &refund_key, "task failed")
        .expect("refund succeeds");

    let (sub_total, pack_total) = db.credit_ledger_totals(user);
    assert_eq!(
        sub_total + pack_total,
        0,
        "a failed authored task must be refunded in full, leaving the ledger net zero"
    );
}

#[test]
fn a_failed_strong_task_is_refunded_exactly_what_it_was_charged() {
    let (_dir, db) = db();
    let (list, class) = billable_list(&db);

    let user = "user_strong_refund";
    db.init_credit_balance(user, 10_000).expect("init balance");

    let verification_id = "verify-strong-2";
    let charged = freeze(&db, "run-4", "step-4", &list, &class, VerdictClass::Strong);

    let charge_key = ChargeKey::for_verification(verification_id);
    let refund_key = RefundKey::for_verification(verification_id);

    db.deduct_credits(user, charged, "strong verdict charge", &charge_key)
        .expect("charge succeeds");
    db.refund_credits(user, &charge_key, &refund_key, "task failed")
        .expect("refund succeeds");

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
