//! The catalog against a real database.
//!
//! The unit tests in `cortex_api::pricing` assert the *rules* — a provisional
//! class does not bill, an unpriced class quotes nothing. These assert what the
//! storage layer actually does with them, which is where invariant 23 either
//! holds or does not: a trigger that was written but never fired is a trigger
//! nobody has, and "published, never edited" enforced by a doc comment is the
//! same thing as not enforced.
//!
//! This is the standing correction applied on purpose. `check_runner`'s missing
//! mount survived because reaching it needed a daemon; these tables' triggers
//! need only a temp file, so there is no excuse for the gap.

use cortex_api::db::Database;
use cortex_api::pricing::{self, PriceStatus};
use cortex_core::routing::RiskLevel;
use cortex_core::task::WorkKind;
use cortex_core::task_class::TaskClass;

fn db() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().expect("temp dir");
    let db = Database::open(&dir.path().join("cortex.db"));
    (dir, db)
}

#[test]
fn a_fresh_database_publishes_a_provisional_list_that_prices_every_class() {
    let (_dir, db) = db();
    let list = db
        .active_price_list()
        .expect("opening a database publishes the seed list");

    assert_eq!(list.version, 1);
    assert_eq!(list.status, PriceStatus::Provisional);
    assert_eq!(list.classes.len(), TaskClass::all().len());
    assert!(!list.models.is_empty(), "the model catalog is empty");
    assert!(
        list.micros_per_credit > 0,
        "a credit is worth nothing, so every price is unbounded"
    );

    for class in TaskClass::all() {
        let (credits, billable) =
            pricing::quote(&list, &class).unwrap_or_else(|| panic!("no price for {}", class.key()));
        assert!(credits > 0, "{} quoted zero", class.key());
        assert!(
            !billable,
            "{} would charge from a seeded list with no measured outcomes",
            class.key()
        );
    }
}

#[test]
fn seeding_is_idempotent_and_never_republishes() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("cortex.db");

    let first = Database::open(&path)
        .active_price_list()
        .expect("published");
    // Reopening is what a restart does, and a restart that republished would
    // mint a new version on every deploy — so every receipt would name a
    // different list than the one before it, for no change in price.
    let second = Database::open(&path)
        .active_price_list()
        .expect("published");

    assert_eq!(first.id, second.id, "reopening republished the price list");
    assert_eq!(first.version, second.version);
}

#[test]
fn a_published_price_cannot_be_edited() {
    // Invariant 23, at the boundary. This is the assertion that makes "the
    // price you were quoted" mean anything: a support script, a migration, or a
    // well-meaning correction must all fail here rather than quietly change
    // what a past receipt refers to.
    let (dir, db) = db();
    let list = db.active_price_list().expect("published");

    let conn = rusqlite::Connection::open(dir.path().join("cortex.db")).expect("open");

    let edit = conn.execute(
        "UPDATE price_list_task_classes SET quoted_credits = 999 WHERE price_list_id = ?1",
        rusqlite::params![list.id],
    );
    assert!(
        edit.is_err(),
        "a published price was edited in place; invariant 23 is not enforced"
    );

    let delete = conn.execute(
        "DELETE FROM price_lists WHERE id = ?1",
        rusqlite::params![list.id],
    );
    assert!(
        delete.is_err(),
        "a published price list was deleted; receipts naming it are now dangling"
    );

    // And the row is unchanged, not merely the statement rejected.
    let after = db.active_price_list().expect("still published");
    let before_credits = list.classes[0].quoted_credits;
    let after_credits = after
        .classes
        .iter()
        .find(|c| c.task_class == list.classes[0].task_class)
        .expect("class survives")
        .quoted_credits;
    assert_eq!(before_credits, after_credits);
}

#[test]
fn a_new_version_publishes_beside_the_old_one() {
    // The only way a price changes. The old version must still be readable,
    // because receipts name it.
    let (dir, db) = db();
    let v1 = db.active_price_list().expect("published");

    let mut v2 = pricing::seed_provisional(2, "test", 1_700_000_000, pricing::seed_models());
    v2.status = PriceStatus::Committed;
    for class in &mut v2.classes {
        class.status = PriceStatus::Committed;
        class.sample_count = 100;
    }
    db.publish_price_list(&v2).expect("v2 publishes");

    let active = db.active_price_list().expect("published");
    assert_eq!(active.version, 2, "the newest version is not active");
    assert_eq!(active.status, PriceStatus::Committed);

    // v1 is still there.
    let conn = rusqlite::Connection::open(dir.path().join("cortex.db")).expect("open");
    let still_there: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM price_lists WHERE id = ?1",
            rusqlite::params![v1.id],
            |r| r.get(0),
        )
        .expect("query");
    assert_eq!(still_there, 1, "publishing v2 removed v1");
}

#[test]
fn republishing_a_version_number_fails_rather_than_overwriting() {
    let (_dir, db) = db();
    let clash = pricing::seed_provisional(1, "test", 1_700_000_000, pricing::seed_models());
    assert!(
        db.publish_price_list(&clash).is_err(),
        "version 1 was republished, which is an edit wearing an insert's clothes"
    );
}

#[test]
fn a_frozen_quote_survives_a_retry_at_its_original_price() {
    // Cortex absorbing the cost of its own second attempt is the entire content
    // of an outcome guarantee. Re-quoting on retry would bill the customer for
    // Cortex having been wrong the first time.
    let (_dir, db) = db();
    let list = db.active_price_list().expect("published");

    let quote = pricing::StepQuote {
        quote_id: "q1".into(),
        run_id: "run-1".into(),
        step_id: "step-1".into(),
        task_class: TaskClass::new(WorkKind::Modify, RiskLevel::Low, true).key(),
        quoted_credits: 3,
        price_list_id: list.id.clone(),
        price_list_version: list.version,
        billable: false,
        frozen_at: 1,
    };
    db.freeze_step_quote(&quote).expect("freeze");

    let requote = pricing::StepQuote {
        quote_id: "q2".into(),
        quoted_credits: 99,
        ..quote.clone()
    };
    db.freeze_step_quote(&requote)
        .expect("second freeze is a no-op");

    let stored = db.get_step_quote("run-1", "step-1").expect("stored");
    assert_eq!(stored.quote_id, "q1", "a retry re-quoted the step");
    assert_eq!(stored.quoted_credits, 3);
}

#[test]
fn a_provisional_quote_offers_nothing_to_the_ledger() {
    // The join between this module and the money path. `verification_dispatcher`
    // reads `billable_credits()`, and this is what it gets for a seeded class.
    let (_dir, db) = db();
    let list = db.active_price_list().expect("published");
    let class = TaskClass::new(WorkKind::Modify, RiskLevel::High, true);
    let (credits, billable) = pricing::quote(&list, &class).expect("priced");

    db.freeze_step_quote(&pricing::StepQuote {
        quote_id: "q".into(),
        run_id: "run-2".into(),
        step_id: "step-2".into(),
        task_class: class.key(),
        quoted_credits: credits,
        price_list_id: list.id.clone(),
        price_list_version: list.version,
        billable,
        frozen_at: 1,
    })
    .expect("freeze");

    let stored = db.get_step_quote("run-2", "step-2").expect("stored");
    assert!(
        stored.quoted_credits > 0,
        "the customer was shown no price at all"
    );
    assert_eq!(
        stored.billable_credits(),
        None,
        "a provisional class reached the ledger"
    );
}

#[test]
fn an_unquoted_step_reads_as_none_rather_than_zero() {
    // A step dispatched before any list existed. `None` and `Some(0)` are
    // different facts and the driver branches on them differently — `None`
    // records the verdict and warns, `Some(0)` would be a silent no-op charge.
    let (_dir, db) = db();
    assert!(db.get_step_quote("nope", "nope").is_none());
}
