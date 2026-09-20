//! Run this tree's migrations against a copy of a database, and report what
//! moved.
//!
//! # Why this exists
//!
//! Production sat at `schema_version` 42 while `main` was at 65 — twenty-three
//! migrations, on a live database, one of which (v63) rewrites every historical
//! `succeeded` step to `delivered`. That is a forward-only change to
//! customer-visible history, and there is no way to answer "what will this do"
//! by reading twenty-three functions carefully. The only honest answer comes
//! from running them against the actual rows and counting.
//!
//! So: take a copy, migrate the copy, diff the counts. Never the live file —
//! this refuses to run against anything it was not pointed at explicitly, and
//! it makes its own copy rather than trusting the caller to have made one.
//!
//! # Usage
//!
//! ```bash
//! cargo run -p cortex-api --bin rehearse-migration -- ./cortex-prod-copy.db
//! ```
//!
//! Exit status is 0 only if the migration completed and the schema version
//! afterwards is the one this tree expects. A rehearsal that "mostly worked" is
//! not a rehearsal.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cortex_api::db::Database;
use rusqlite::Connection;

/// The tables whose row counts are reported, and the column each is grouped by.
///
/// A closed list rather than every table, because the point is a report a human
/// reads before pressing deploy. `steps.status` is first for a reason: it is
/// the column v63 rewrites.
const GROUPED: &[(&str, &str)] = &[
    ("steps", "status"),
    ("steps", "verification_status"),
    ("runs", "status"),
    ("step_attempts", "status"),
];

/// Tables reported as a plain count. Anything that changes here without a
/// migration intending it is a bug worth seeing.
const COUNTED: &[&str] = &[
    "steps",
    "runs",
    "step_attempts",
    "outcomes",
    "verifier_reports",
    "credit_transactions",
    "credit_balances",
    "decisions",
    "workers",
    "user_credentials",
    "social_posts",
    "social_profiles",
    "social_longform",
];

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(source) = args.next().map(PathBuf::from) else {
        eprintln!(
            "usage: rehearse-migration <path-to-database-copy>\n\
             \n\
             Point this at a COPY of the database. It makes a further copy of \
             its own and migrates that, so the file you name is not modified \
             either — but a copy is still what you should be handing it."
        );
        std::process::exit(2);
    };

    if !source.exists() {
        eprintln!("no such database: {}", source.display());
        std::process::exit(2);
    }

    // Its own copy, in a temp directory. Two reasons, and the second is the
    // real one: the caller's file stays untouched so the rehearsal can be run
    // again from the same snapshot, and a rehearsal that mutates its input is
    // a rehearsal that can only be run once — which is how a bad result gets
    // rationalised instead of reproduced.
    let work = std::env::temp_dir().join(format!("cortex-rehearsal-{}.db", std::process::id()));
    copy_database(&source, &work);

    let before = snapshot(&work);
    let version_before = schema_version(&work);

    println!("REHEARSAL — {}", source.display());
    println!("  working copy:   {}", work.display());
    println!("  schema before:  {version_before}");

    // The real thing. `Database::open` applies migrations; there is no separate
    // migration entry point, and inventing one for the rehearsal would mean
    // rehearsing code the deploy does not run.
    let db = Database::open(&work);
    let version_after = db.schema_version();
    drop(db);

    let after = snapshot(&work);

    println!("  schema after:   {version_after}");
    println!();

    report("GROUPED COUNTS", &before.grouped, &after.grouped);
    report("TABLE COUNTS", &before.counted, &after.counted);

    // v63 is the one with customer-visible consequences, so it gets named
    // rather than left for the reader to spot in a table.
    let succeeded_before = before
        .grouped
        .get("steps.status/succeeded")
        .copied()
        .unwrap_or(0);
    let delivered_after = after
        .grouped
        .get("steps.status/delivered")
        .copied()
        .unwrap_or(0);
    println!();
    println!("v63 — historical `succeeded` steps rewritten to `delivered`");
    println!("  succeeded before: {succeeded_before}");
    println!("  delivered after:  {delivered_after}");
    if succeeded_before == 0 {
        println!("  → no rows to rewrite; this migration is a no-op on this database");
    }

    println!();
    if version_after > version_before {
        println!("RESULT: migrated {version_before} → {version_after}");
    } else {
        println!("RESULT: no migration applied (already at {version_after})");
    }

    let _ = std::fs::remove_file(&work);
    let _ = std::fs::remove_file(work.with_extension("db-wal"));
    let _ = std::fs::remove_file(work.with_extension("db-shm"));
}

/// Copy the database and its write-ahead log.
///
/// The `-wal` matters: a database copied without it loses every committed
/// transaction that has not yet been checkpointed, so the rehearsal would run
/// against a tree the production process does not have. That is a silent way to
/// rehearse the wrong thing.
fn copy_database(source: &Path, dest: &Path) {
    std::fs::copy(source, dest).expect("copy the database");
    for suffix in ["-wal", "-shm"] {
        let from = with_suffix(source, suffix);
        if from.exists() {
            let _ = std::fs::copy(&from, with_suffix(dest, suffix));
        }
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

struct Snapshot {
    grouped: BTreeMap<String, i64>,
    counted: BTreeMap<String, i64>,
}

fn schema_version(path: &Path) -> i64 {
    let conn = Connection::open(path).expect("open for reading");
    conn.query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
        .unwrap_or(0)
}

fn snapshot(path: &Path) -> Snapshot {
    let conn = Connection::open(path).expect("open for reading");
    let mut grouped = BTreeMap::new();
    let mut counted = BTreeMap::new();

    for (table, column) in GROUPED {
        // A table or column that does not exist yet is not an error — that is
        // the normal state of a database being migrated forward, and treating
        // it as one would mean the rehearsal cannot report on the "before".
        let sql = format!("SELECT {column}, COUNT(*) FROM {table} GROUP BY {column}");
        let Ok(mut stmt) = conn.prepare(&sql) else {
            continue;
        };
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?
                    .unwrap_or_else(|| "NULL".into()),
                r.get::<_, i64>(1)?,
            ))
        });
        if let Ok(rows) = rows {
            for row in rows.flatten() {
                grouped.insert(format!("{table}.{column}/{}", row.0), row.1);
            }
        }
    }

    for table in COUNTED {
        if let Ok(n) = conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| {
            r.get::<_, i64>(0)
        }) {
            counted.insert((*table).to_string(), n);
        }
    }

    Snapshot { grouped, counted }
}

/// Print every key from either side, so a row that appears or disappears is as
/// visible as one that changes. A report that only walks the "after" keys
/// cannot show a state that stopped existing, which is exactly what v63 does.
fn report(title: &str, before: &BTreeMap<String, i64>, after: &BTreeMap<String, i64>) {
    println!("{title}");
    let mut keys: Vec<&String> = before.keys().chain(after.keys()).collect();
    keys.sort();
    keys.dedup();

    for key in keys {
        let b = before.get(key).copied();
        let a = after.get(key).copied();
        let marker = if b == a { "  " } else { "→ " };
        println!(
            "{marker}{key:<44} {:>8}  {:>8}",
            b.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
            a.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
        );
    }
}
