//! Issue, list and revoke Cortex worker service credentials (`cwk_` keys).
//!
//! # Why this is a binary and not a route
//!
//! Issuing a worker credential mints a bearer token that can be handed steps to
//! execute and bills them to a user. A self-serve HTTP endpoint for that needs
//! its own authorization story — who may mint a key, for which owner, with what
//! scope, under what rate limit — and getting that wrong is a privilege
//! escalation, not a missing feature. So issuance lives here: it runs on the
//! host, next to the database, as whoever can already read that file.
//!
//! The public issuance route is deliberately out of scope for this change.
//!
//! # The plaintext is shown once
//!
//! Only the SHA-256 hash is stored. This binary prints the secret to stdout and
//! then drops it; nothing — not the database, not the log — can produce it
//! again. A lost key is replaced, not recovered.
//!
//! # Usage
//!
//! ```bash
//! # Mint a key for a user, valid until revoked:
//! cargo run -p cortex-api --bin cortex-worker-key -- issue --owner user_2abc
//!
//! # ...or one that expires in 90 days:
//! cargo run -p cortex-api --bin cortex-worker-key -- issue --owner user_2abc --days 90
//!
//! cargo run -p cortex-api --bin cortex-worker-key -- list
//! cargo run -p cortex-api --bin cortex-worker-key -- revoke <id-or-prefix>
//! ```
//!
//! The database is `CORTEX_DB_PATH` if set, otherwise `.cortex/cortex.db` under
//! the current directory — the same resolution the server uses, so this cannot
//! write keys into a file the API does not read.

use std::process::ExitCode;

use cortex_api::db::Database;
use cortex_api::state::cortex_db_path;
use cortex_api::worker_key::{generate_worker_key, DEFAULT_WORKER_SCOPE};

const USAGE: &str = "\
cortex-worker-key — Cortex worker service credentials

USAGE:
    cortex-worker-key issue --owner <USER_ID> [--scope <SCOPE>] [--days <N>]
    cortex-worker-key list [--owner <USER_ID>]
    cortex-worker-key revoke <ID_OR_PREFIX>

The issued secret is printed once and never stored. Store the hash only.
Database: $CORTEX_DB_PATH, else ./.cortex/cortex.db
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first().map(String::as_str) else {
        eprint!("{USAGE}");
        return ExitCode::FAILURE;
    };

    let db = Database::open(&cortex_db_path(
        &std::env::current_dir().unwrap_or_default(),
    ));

    match command {
        "issue" => issue(&db, &args[1..]),
        "list" => list(&db, &args[1..]),
        "revoke" => revoke(&db, &args[1..]),
        _ => {
            eprint!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

/// Read `--name value` from a flat argument list. Returns `None` if absent.
fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let at = args.iter().position(|a| a == name)?;
    args.get(at + 1).map(String::as_str)
}

fn issue(db: &Database, args: &[String]) -> ExitCode {
    let Some(owner) = flag(args, "--owner") else {
        eprintln!("issue: --owner <USER_ID> is required\n");
        eprint!("{USAGE}");
        return ExitCode::FAILURE;
    };
    let scope = flag(args, "--scope").unwrap_or(DEFAULT_WORKER_SCOPE);

    let expires_at = match flag(args, "--days") {
        Some(days) => match days.parse::<i64>() {
            Ok(days) if days > 0 => Some(chrono::Utc::now().timestamp_millis() + days * 86_400_000),
            _ => {
                eprintln!("issue: --days must be a positive whole number of days");
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };

    let key = generate_worker_key();
    let id = format!("wk_{}", &key.hash[..12]);

    if let Err(e) = db.create_worker_key(
        &id,
        &key.hash,
        &key.display_prefix,
        owner,
        scope,
        expires_at,
    ) {
        eprintln!("issue: {e}");
        return ExitCode::FAILURE;
    }

    // The one and only time this value exists outside the worker that will
    // hold it. Printed on its own line so it can be piped without the framing.
    println!("id:      {id}");
    println!("owner:   {owner}");
    println!("scope:   {scope}");
    let expiry = match expires_at {
        None => "never (revoke to end)".to_string(),
        Some(ms) => chrono::DateTime::from_timestamp_millis(ms)
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| ms.to_string()),
    };
    println!("expires: {expiry}");
    println!();
    println!("Shown once. Store it as CORTEX_TOKEN in the worker's env file:");
    println!();
    println!("{}", key.secret);
    println!();

    ExitCode::SUCCESS
}

fn list(db: &Database, args: &[String]) -> ExitCode {
    let rows = db.list_worker_keys(flag(args, "--owner"));
    if rows.is_empty() {
        println!("no worker keys");
        return ExitCode::SUCCESS;
    }
    for row in rows {
        let state = if !row["revokedAt"].is_null() {
            "revoked"
        } else if row["expiresAt"]
            .as_i64()
            .is_some_and(|e| e <= chrono::Utc::now().timestamp_millis())
        {
            "expired"
        } else {
            "active"
        };
        println!(
            "{:<8} {:<18} {:<24} {:<10} last_used={}",
            state,
            row["id"].as_str().unwrap_or(""),
            row["ownerUserId"].as_str().unwrap_or(""),
            row["scope"].as_str().unwrap_or(""),
            row["lastUsedAt"]
                .as_i64()
                .map_or_else(|| "never".to_string(), |ms| ms.to_string()),
        );
    }
    ExitCode::SUCCESS
}

fn revoke(db: &Database, args: &[String]) -> ExitCode {
    let Some(target) = args.first() else {
        eprintln!("revoke: <ID_OR_PREFIX> is required\n");
        eprint!("{USAGE}");
        return ExitCode::FAILURE;
    };
    match db.revoke_worker_key(target) {
        0 => {
            // Not an error worth a stack trace, but it must not report success:
            // an operator who thinks they revoked a key and did not is worse off
            // than one who knows the id was wrong.
            eprintln!("revoke: no active key matched {target}");
            ExitCode::FAILURE
        }
        n => {
            println!("revoked {n} key(s) matching {target}");
            ExitCode::SUCCESS
        }
    }
}
