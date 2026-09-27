//! M-D-0023: the schema half of pass-through billing.
//!
//! `cost_micro_usd` and `carry_micro_usd` are the two columns the pure money
//! functions in `pricing.rs` (`charge`, `cost_micro_usd`) need somewhere to
//! land. This pins that migration v72 actually adds them, with the right
//! defaults, on top of a fresh database (which — per `db/mod.rs`'s own
//! `schema_version_const_matches_what_migrations_actually_produce` test —
//! always runs the full chain from v0).

use cortex_api::db::{Database, SCHEMA_VERSION};

fn db() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().expect("temp dir");
    let db = Database::open(&dir.path().join("cortex.db"));
    (dir, db)
}

/// `Database::conn` is crate-private, so an integration test opens the same
/// file directly — the same approach `pricing_integration.rs` uses.
fn raw_conn(dir: &tempfile::TempDir) -> rusqlite::Connection {
    rusqlite::Connection::open(dir.path().join("cortex.db")).expect("open db file")
}

fn column_names(conn: &rusqlite::Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("PRAGMA table_info");
    stmt.query_map([], |row| row.get::<_, String>(1))
        .expect("query_map")
        .collect::<Result<_, _>>()
        .expect("collect column names")
}

#[test]
fn a_fresh_database_reaches_v72_and_carries_the_new_columns() {
    let (_dir, db) = db();
    assert!(
        SCHEMA_VERSION >= 72,
        "SCHEMA_VERSION const has not been bumped to include migration v72"
    );
    assert_eq!(
        db.schema_version(),
        i64::from(SCHEMA_VERSION),
        "a fresh boot must land on the current schema version"
    );

    let conn = raw_conn(&_dir);
    let ledger_cols = column_names(&conn, "credit_transactions");
    assert!(
        ledger_cols.contains(&"cost_micro_usd".to_string()),
        "credit_transactions is missing cost_micro_usd: {ledger_cols:?}"
    );

    let balance_cols = column_names(&conn, "credit_balances");
    assert!(
        balance_cols.contains(&"carry_micro_usd".to_string()),
        "credit_balances is missing carry_micro_usd: {balance_cols:?}"
    );
}

#[test]
fn carry_micro_usd_defaults_to_zero_and_rejects_negative_values() {
    let (_dir, _db) = db();
    let conn = raw_conn(&_dir);

    conn.execute(
        "INSERT INTO credit_balances (clerk_user_id) VALUES ('user-1')",
        [],
    )
    .expect("insert a balance row relying on defaults");
    let carry: i64 = conn
        .query_row(
            "SELECT carry_micro_usd FROM credit_balances WHERE clerk_user_id = 'user-1'",
            [],
            |r| r.get(0),
        )
        .expect("read back carry_micro_usd");
    assert_eq!(carry, 0, "a new balance must start with no carry");

    let negative = conn.execute(
        "UPDATE credit_balances SET carry_micro_usd = -1 WHERE clerk_user_id = 'user-1'",
        [],
    );
    assert!(
        negative.is_err(),
        "a negative carry means a customer was charged less than their calls cost"
    );
}

#[test]
fn cost_micro_usd_is_nullable_for_rows_that_are_not_a_pass_through_call_charge() {
    // A purchase or a promo grant is not "the cost of a model call", so this
    // column must stay optional rather than force every ledger row through a
    // pass-through shape it was never charged under.
    let (_dir, _db) = db();
    let conn = raw_conn(&_dir);
    conn.execute(
        "INSERT INTO credit_transactions (id, clerk_user_id, amount, balance_type, description)
         VALUES ('tx-1', 'user-1', 10.0, 'subscription', 'monthly grant')",
        [],
    )
    .expect("insert a non-call ledger row with cost_micro_usd left NULL");
    let cost: Option<i64> = conn
        .query_row(
            "SELECT cost_micro_usd FROM credit_transactions WHERE id = 'tx-1'",
            [],
            |r| r.get(0),
        )
        .expect("read back cost_micro_usd");
    assert_eq!(cost, None);
}
