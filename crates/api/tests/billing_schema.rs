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
        "INSERT INTO credit_transactions (id, clerk_user_id, amount, balance_type, description, idempotency_key)
         VALUES ('tx-1', 'user-1', 10.0, 'subscription', 'monthly grant', 'test:tx-1')",
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

#[test]
fn a_balance_row_that_predates_migration_v72_gets_a_zero_carry_not_a_null() {
    // Simulate an installation that already has `credit_balances` rows from
    // before this migration: build one at v72, then roll the on-disk state
    // back to "v71 plus a pre-existing row" (drop the new column, rewind the
    // version marker) and reopen. `Database::open` must re-run migrate_v72
    // and backfill the existing row to carry_micro_usd = 0 rather than
    // leaving it unset.
    let (_dir, _db) = db();
    let conn = raw_conn(&_dir);
    conn.execute(
        "INSERT INTO credit_balances (clerk_user_id) VALUES ('pre-existing-user')",
        [],
    )
    .expect("insert a balance row as if it existed before v72");
    conn.execute(
        "ALTER TABLE credit_balances DROP COLUMN carry_micro_usd",
        [],
    )
    .expect("roll the column back to simulate the pre-migration shape");
    conn.execute(
        "ALTER TABLE credit_transactions DROP COLUMN cost_micro_usd",
        [],
    )
    .expect("roll the other v72 column back too, or migrate_v72 hits a duplicate column");
    conn.execute("UPDATE schema_version SET version = 71", [])
        .expect("rewind the version marker so migrate_v72 runs again on reopen");
    drop(conn);

    let _reopened = Database::open(&_dir.path().join("cortex.db"));
    let conn = raw_conn(&_dir);
    let carry: i64 = conn
        .query_row(
            "SELECT carry_micro_usd FROM credit_balances WHERE clerk_user_id = 'pre-existing-user'",
            [],
            |r| r.get(0),
        )
        .expect("carry_micro_usd must be backfilled on reopen, not left missing");
    assert_eq!(
        carry, 0,
        "a row that predates this migration must default to no carry, not NULL"
    );
}

#[test]
fn a_stale_installed_anthropic_rate_is_corrected_on_reopen_not_kept_forever() {
    // M-D-0023 review finding: `publish_gateway_model_revision_if_needed` only
    // overwrote a hardcoded OpenAI allowlist, so an existing installation's
    // wrong Claude rates (the 3x Opus overcharge / 20% Haiku undercharge this
    // PR fixes) were never corrected. Every `claude` row must now be
    // authoritative from the seed, so an old rate is replaced the next time
    // the database opens, in a new published version rather than an in-place
    // edit (invariant 23).
    let (_dir, db) = db();
    let conn = raw_conn(&_dir);

    // Publish a v2 list that copies v1's shape but carries the OLD, wrong
    // Anthropic rates (Opus 5 at $15/$75, Sonnet 5 at $3/$15, Haiku 4.5 at
    // $0.80/$4 per 1k tokens instead of the corrected $5/$25, $2/$10, $1/$5).
    conn.execute(
        "INSERT INTO price_lists (id, version, status, micros_per_credit, basis, published_at, published_by)
         SELECT 'old-rates-list', 2, status, micros_per_credit, basis, published_at, published_by
         FROM price_lists WHERE version = 1",
        [],
    )
    .expect("insert a v2 list shaped like v1");
    conn.execute(
        "INSERT INTO price_list_models
            (price_list_id, provider, model_id, input_micros_per_1k, output_micros_per_1k, cache_read_bp, context_window, capability_class)
         SELECT 'old-rates-list', provider, model_id,
            CASE WHEN provider = 'claude' AND model_id = 'claude-opus-5' THEN 15000
                 WHEN provider = 'claude' AND model_id = 'claude-sonnet-5' THEN 3000
                 WHEN provider = 'claude' AND model_id = 'claude-haiku-4-5' THEN 800
                 ELSE input_micros_per_1k END,
            CASE WHEN provider = 'claude' AND model_id = 'claude-opus-5' THEN 75000
                 WHEN provider = 'claude' AND model_id = 'claude-sonnet-5' THEN 15000
                 WHEN provider = 'claude' AND model_id = 'claude-haiku-4-5' THEN 4000
                 ELSE output_micros_per_1k END,
            cache_read_bp, context_window, capability_class
         FROM price_list_models WHERE price_list_id = (SELECT id FROM price_lists WHERE version = 1)",
        [],
    )
    .expect("insert v2 models carrying the stale claude rates");
    conn.execute(
        "INSERT INTO price_list_task_classes
            (price_list_id, task_class, quoted_credits, status, sample_count, measured_cost_micros, margin_bp)
         SELECT 'old-rates-list', task_class, quoted_credits, status, sample_count, measured_cost_micros, margin_bp
         FROM price_list_task_classes WHERE price_list_id = (SELECT id FROM price_lists WHERE version = 1)",
        [],
    )
    .expect("insert v2 task classes copied from v1");
    drop(conn);
    drop(db);

    // Reopening must see the stale v2 as active, notice its claude rows
    // differ from the seed, and publish a corrected v3 rather than leaving
    // the old rates in place.
    let reopened = Database::open(&_dir.path().join("cortex.db"));
    let active = reopened
        .active_price_list()
        .expect("a price list must still be active after reopening");
    assert!(
        active.version > 2,
        "a stale claude rate must trigger a new published version, not be left at v2"
    );

    for seed in cortex_api::pricing::seed_models()
        .into_iter()
        .filter(|m| m.provider == "claude")
    {
        let row = active
            .model(&seed.provider, &seed.model_id)
            .unwrap_or_else(|| {
                panic!(
                    "{}/{} missing from the corrected active list",
                    seed.provider, seed.model_id
                )
            });
        assert_eq!(
            row.input_micros_per_1k, seed.input_micros_per_1k,
            "{} input rate was not corrected on reopen",
            seed.model_id
        );
        assert_eq!(
            row.output_micros_per_1k, seed.output_micros_per_1k,
            "{} output rate was not corrected on reopen",
            seed.model_id
        );
        assert_eq!(
            row.cache_read_bp, seed.cache_read_bp,
            "{} cache-read rate was not corrected on reopen",
            seed.model_id
        );
    }
}
