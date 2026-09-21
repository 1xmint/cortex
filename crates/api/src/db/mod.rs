use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use chrono::{Datelike, Utc};
use cortex_core::task::TaskContract;
use cortex_core::usage::{estimate_cost_by_provider, DailyUsage, ProviderUsage, UsageSummary};
use cortex_core::verification::{
    compute_verdict, CheckExecution, CheckOutcome, CheckSource, CheckSpec, Verdict, VerdictReport,
};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::lock::LockRecovering;

mod ledger;
mod provider_gateway;
mod verification_queue;

pub use provider_gateway::{ProviderReservation, SpendAuthorization};

pub struct Database {
    conn: Mutex<Connection>,
}

// --- Conversation types (existing) ---

#[derive(Debug, Serialize, Clone)]
pub struct Conversation {
    pub id: String,
    pub user_id: String,
    pub title: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub role: String,
    pub content: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct ConversationWithMessages {
    #[serde(flatten)]
    pub conversation: Conversation,
    pub messages: Vec<Message>,
}

#[derive(Debug, Serialize)]
pub struct ConversationSummary {
    pub id: String,
    pub title: Option<String>,
    pub updated_at: String,
    pub message_count: i64,
    pub last_message_preview: Option<String>,
}

pub struct ActiveRunSummary {
    pub id: String,
    pub goal: String,
    pub status: String,
    pub created_at: String,
    pub step_count: usize,
    pub steps_completed: usize,
    pub steps_failed: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct VerifierReport {
    pub id: String,
    pub step_id: String,
    pub run_id: String,
    pub lease_gen: i64,
    pub worker_id: Option<String>,
    pub verifier: String,
    pub status: String,
    pub verdict: String,
    pub evidence_json: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl VerifierReport {
    pub fn is_verified_success(&self) -> bool {
        self.status == "verified" && self.verdict == "pass"
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OperationsEvent {
    pub id: String,
    pub created_at: i64,
    pub actor_user_id: Option<String>,
    pub scope_id: Option<String>,
    pub project_id: Option<String>,
    pub task_id: Option<String>,
    pub run_id: Option<String>,
    pub step_id: Option<String>,
    pub attempt_id: Option<String>,
    pub event_type: String,
    pub entity_type: String,
    pub entity_id: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct CortexTaskStateEvent {
    pub task_id: Option<String>,
    pub event_type: String,
    pub entity_type: String,
    pub entity_id: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StepDependencyEdge {
    pub step_id: String,
    pub depends_on_id: String,
    pub edge_type: String,
}

pub struct RunStepSnapshot {
    pub id: String,
    pub status: String,
    pub kind: String,
    pub work_kind: String,
    pub tier: String,
    pub risk: String,
    pub objective: String,
    pub attempt_count: i64,
    pub max_attempts: i64,
    pub lease_gen: i64,
    pub lease_deadline: Option<i64>,
    pub assigned_worker: Option<String>,
    pub recipe_seed_json: Option<String>,
    pub output_summary: Option<String>,
    pub files_changed: Option<String>,
    pub last_error: Option<String>,
    pub predecessors: Vec<String>,
    pub verifier_report: Option<VerifierReport>,
    pub work_contract: Option<TaskContract>,
    pub latest_attempt: Option<RunStepAttemptSnapshot>,
}

pub struct RunStepAttemptSnapshot {
    pub attempt_number: i64,
    pub worker_id: Option<String>,
    pub lease_gen: i64,
    pub status: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub failure_kind: Option<String>,
    pub error_summary: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ResourceLease {
    pub id: String,
    pub user_id: String,
    pub authority_scope_id: Option<String>,
    pub group_id: Option<String>,
    pub task_id: Option<String>,
    pub run_id: String,
    pub step_id: Option<String>,
    pub holder_type: String,
    pub resource_type: String,
    pub repo_key: String,
    pub resource_key: String,
    pub mode: String,
    pub status: String,
    pub lease_gen: i64,
    pub acquired_at: i64,
    pub expires_at: i64,
    pub released_at: Option<i64>,
    pub reason: Option<String>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct RunPrAuthorityContext {
    pub run_id: String,
    pub user_id: String,
    pub repo_key: Option<String>,
    pub authority_scope_id: Option<String>,
    pub authority_context: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ResourceLeaseConflict {
    pub lease_id: String,
    pub run_id: String,
    pub step_id: Option<String>,
    pub holder_type: String,
    pub resource_type: String,
    pub repo_key: String,
    pub resource_key: String,
    pub mode: String,
    pub expires_at: i64,
}

impl ResourceLeaseConflict {
    pub fn message(&self) -> String {
        format!(
            "resource conflict: active {} lease on {}:{} held by run {} until {}",
            self.resource_type, self.repo_key, self.resource_key, self.run_id, self.expires_at
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceLeaseRequest {
    pub resource_type: String,
    pub repo_key: String,
    pub resource_key: String,
    pub mode: String,
    pub reason: Option<String>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateRunError {
    ResourceConflict(ResourceLeaseConflict),
    Database(String),
}

impl CreateRunError {
    pub fn message(&self) -> String {
        match self {
            Self::ResourceConflict(conflict) => conflict.message(),
            Self::Database(message) => message.clone(),
        }
    }
}

// --- Billing types ---

pub struct SubscriptionRecord {
    pub clerk_user_id: String,
    pub stripe_customer_id: String,
    pub stripe_subscription_id: Option<String>,
    pub plan_type: String,
    pub status: String,
    pub trial_end: Option<String>,
    pub current_period_start: Option<String>,
    pub current_period_end: Option<String>,
}

/// Whole credits. Never floating point — see cortex/plan/CREDITS.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreditBalanceRecord {
    pub subscription_remaining: i64,
    pub subscription_total: i64,
    pub pack_remaining: i64,
}

pub struct BillingHistoryRecord {
    pub id: String,
    pub amount_cents: i64,
    pub description: String,
    pub status: String,
    pub created_at: String,
}

pub struct ReferralCodeRecord {
    pub code: String,
    pub creator_user_id: String,
    pub uses_remaining: i32,
    pub total_uses: i32,
    pub weeks_earned: i32,
}

#[derive(Debug, Serialize, Clone)]
pub struct PromoCode {
    pub id: String,
    pub code: String,
    pub discount_type: String,
    pub discount_value: f64,
    pub max_uses: i32,
    pub current_uses: i32,
    pub expires_at: Option<String>,
    pub active: bool,
    pub created_by: String,
    pub created_at: String,
    pub description: Option<String>,
    pub discount_options: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct CodeRedemption {
    pub id: String,
    pub promo_code_id: String,
    pub code: String,
    pub user_id: String,
    pub redeemed_at: String,
}

const RUN_RESOURCE_LEASE_TTL_MS: i64 = 24 * 60 * 60 * 1000;

fn apply_migrations(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER NOT NULL
        );",
    )
    .expect("failed to create schema_version table");

    let current: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    if current < 1 {
        migrate_v1(conn);
    }
    if current < 2 {
        migrate_v2(conn);
    }
    if current < 3 {
        migrate_v3(conn);
    }
    if current < 4 {
        migrate_v4(conn);
    }
    if current < 5 {
        migrate_v5(conn);
    }
    if current < 6 {
        migrate_v6(conn);
    }
    if current < 7 {
        migrate_v7(conn);
    }
    if current < 8 {
        migrate_v8(conn);
    }
    if current < 9 {
        migrate_v9(conn);
    }
    if current < 10 {
        migrate_v10(conn);
    }
    if current < 11 {
        migrate_v11(conn);
    }
    if current < 12 {
        migrate_v12(conn);
    }
    if current < 13 {
        migrate_v13(conn);
    }
    if current < 14 {
        migrate_v14(conn);
    }
    if current < 15 {
        migrate_v15(conn);
    }
    if current < 16 {
        migrate_v16(conn);
    }
    if current < 17 {
        migrate_v17(conn);
    }
    if current < 18 {
        migrate_v18(conn);
    }
    if current < 19 {
        migrate_v19(conn);
    }
    if current < 20 {
        migrate_v20(conn);
    }
    if current < 21 {
        migrate_v21(conn);
    }
    if current < 22 {
        migrate_v22(conn);
    }
    if current < 23 {
        migrate_v23(conn);
    }
    if current < 24 {
        migrate_v24(conn);
    }
    if current < 25 {
        migrate_v25(conn);
    }
    if current < 26 {
        migrate_v26(conn);
    }
    if current < 27 {
        migrate_v27(conn);
    }
    // Ensure social tables exist regardless of version. Handles version collision
    // where production DB ran main-branch v26-v27 (authority metadata) and skipped
    // social table creation that the feature branch put at the same version numbers.
    ensure_social_tables(conn);
    // Best-effort FTS5 index for post search (falls back to LIKE when unavailable).
    ensure_social_posts_fts(conn);

    if current < 28 {
        migrate_v28(conn);
    }
    if current < 29 {
        migrate_v29(conn);
    }
    if current < 30 {
        migrate_v30(conn);
    }
    if current < 31 {
        migrate_v31(conn);
    }
    if current < 32 {
        migrate_v32(conn);
    }
    if current < 33 {
        migrate_v33(conn);
    }
    if current < 34 {
        migrate_v34(conn);
    }
    if current < 35 {
        migrate_v35(conn);
    }
    if current < 36 {
        migrate_v36(conn);
    }
    if current < 37 {
        migrate_v37(conn);
    }
    if current < 38 {
        migrate_v38(conn);
    }
    if current < 39 {
        migrate_v39(conn);
    }
    if current < 40 {
        migrate_v40(conn);
    }
    if current < 41 {
        migrate_v41(conn);
    }
    if current < 42 {
        migrate_v42(conn);
    }
    if current < 43 {
        migrate_v43(conn);
    }
    if current < 44 {
        migrate_v44(conn);
    }
    if current < 45 {
        migrate_v45(conn);
    }
    if current < 46 {
        migrate_v46(conn);
    }
    if current < 47 {
        migrate_v47(conn);
    }
    if current < 48 {
        migrate_v48(conn);
    }
    if current < 49 {
        migrate_v49(conn);
    }
    if current < 50 {
        migrate_v50(conn);
    }
    if current < 51 {
        migrate_v51(conn);
    }
    if current < 52 {
        migrate_v52(conn);
    }
    if current < 53 {
        migrate_v53(conn);
    }
    if current < 54 {
        migrate_v54(conn);
    }
    if current < 55 {
        migrate_v55(conn);
    }
    if current < 56 {
        migrate_v56(conn);
    }
    if current < 57 {
        migrate_v57(conn);
    }
    if current < 58 {
        migrate_v58(conn);
    }
    if current < 59 {
        migrate_v59(conn);
    }
    // v60, not v53: fix/socials-message-integrity has already claimed v53–v59
    // on its branch, and schema_version is a single counter — a collision means
    // whichever branch merges second gets its migration silently skipped. That
    // branch must merge before this one.
    if current < 60 {
        migrate_v60(conn);
    }
    if current < 61 {
        migrate_v61(conn);
    }
    if current < 62 {
        migrate_v62(conn);
    }
    if current < 63 {
        migrate_v63(conn);
    }
    if current < 64 {
        migrate_v64(conn);
    }
    if current < 65 {
        migrate_v65(conn);
    }
    if current < 66 {
        migrate_v66(conn);
    }
    if current < 67 {
        migrate_v67(conn);
    }
    if current < 68 {
        migrate_v68(conn);
    }
}

fn migrate_v68(conn: &Connection) {
    // Supplier spend is a different ledger from customer credits. A request
    // first occupies capacity here, then either settles to observed COGS,
    // releases because it provably was not sent, or remains unresolved. A
    // timeout is deliberately not a release: the provider may still charge it.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS supplier_capacities (
            provider          TEXT PRIMARY KEY,
            funded_micro_usd  INTEGER NOT NULL CHECK(funded_micro_usd >= 0),
            updated_at        INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS provider_spend_authorizations (
            id                 TEXT PRIMARY KEY,
            user_id            TEXT NOT NULL,
            run_id             TEXT NOT NULL,
            attempt_id         TEXT NOT NULL,
            provider           TEXT NOT NULL,
            model              TEXT NOT NULL,
            price_list_id      TEXT NOT NULL,
            max_micro_usd      INTEGER NOT NULL CHECK(max_micro_usd > 0),
            expires_at         INTEGER NOT NULL,
            status             TEXT NOT NULL DEFAULT 'active'
                CHECK(status IN ('active', 'revoked')),
            created_at         INTEGER NOT NULL,
            UNIQUE(user_id, run_id, attempt_id, provider, model)
        );

        CREATE TABLE IF NOT EXISTS provider_request_reservations (
            id                  TEXT PRIMARY KEY,
            request_key         TEXT NOT NULL UNIQUE,
            request_digest      TEXT NOT NULL,
            authorization_id    TEXT NOT NULL REFERENCES provider_spend_authorizations(id),
            user_id             TEXT NOT NULL,
            run_id              TEXT NOT NULL,
            attempt_id          TEXT NOT NULL,
            provider            TEXT NOT NULL,
            model               TEXT NOT NULL,
            price_list_id       TEXT NOT NULL,
            reserved_micro_usd  INTEGER NOT NULL CHECK(reserved_micro_usd > 0),
            observed_micro_usd  INTEGER CHECK(observed_micro_usd >= 0),
            status              TEXT NOT NULL
                CHECK(status IN ('reserved', 'settled', 'unresolved', 'released', 'mismatch')),
            upstream_request_id TEXT,
            terminal_reason     TEXT,
            created_at          INTEGER NOT NULL,
            reconciled_at       INTEGER
        );

        CREATE INDEX IF NOT EXISTS idx_provider_reservations_authorization
            ON provider_request_reservations(authorization_id, status);
        CREATE INDEX IF NOT EXISTS idx_provider_reservations_capacity
            ON provider_request_reservations(provider, status);
        CREATE INDEX IF NOT EXISTS idx_provider_reservations_unresolved
            ON provider_request_reservations(status, created_at);

        UPDATE schema_version SET version = 68;",
    )
    .expect("migration v68 failed creating provider gateway spend controls");

    tracing::info!(
        "applied migration v68: provider gateway authorizations and durable reservations"
    );
}

fn migrate_v67(conn: &Connection) {
    // The worker service credential (F11).
    //
    // Until this table there was no way for a worker to prove who it was. The
    // only credentials `authenticate_worker` understood were a Clerk *user*
    // JWT — which a long-lived headless daemon has no way to obtain or renew —
    // and the empty string, which was accepted whenever `CLERK_SECRET_KEY` was
    // unset. So the deployed configuration was: no worker could authenticate
    // legitimately, and any client at all could authenticate anonymously.
    //
    // A worker key is a service credential, not a user session. It belongs to a
    // user (`owner_user_id`, so the work it dispatches is still billed and
    // attributed to somebody), carries a coarse `scope`, and can be expired or
    // revoked without touching the owner's account.
    //
    // `key_hash` is the SHA-256 hex of the full `cwk_`-prefixed secret and is
    // UNIQUE: two rows can never resolve the same presented token, and an
    // issuance that would collide fails loudly at INSERT instead of silently
    // creating an ambiguous credential.
    //
    // **On the plain digest.** The secret is `cwk_` plus 32 bytes from the OS
    // CSPRNG — a high-entropy token, not a password. Nobody chooses it, nobody
    // reuses it, and there is no dictionary to run against the hash column, so
    // a password KDF (bcrypt/argon2) would add per-frame latency and buy
    // exactly nothing; inverting SHA-256 over 256 uniform bits is the attack.
    // A plain digest is the correct primitive *here* and would not be if this
    // column ever held something a human picked. See `key_material`.
    //
    // The plaintext is never stored. `key_prefix` is a truncated display form
    // so an operator can tell two keys apart in a listing; it is not a secret
    // and is not sufficient to authenticate.
    //
    // Revocation and expiry are two separate columns on purpose. `revoked_at`
    // is an operator act with a time, and keeping it (rather than deleting the
    // row) means a revoked key stays visible in an audit and can never be
    // re-issued as itself, since `key_hash` remains taken.
    //
    // Numbered v67: the maximum on main at rebase time was v66 (PR I).
    // `schema_version` is one counter shared with the HeyVera Socials product —
    // re-check the maximum before claiming a number, because whichever branch
    // merges second has its migration silently skipped.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS worker_keys (
            id             TEXT PRIMARY KEY,
            key_hash       TEXT NOT NULL UNIQUE,
            key_prefix     TEXT NOT NULL,
            owner_user_id  TEXT NOT NULL,
            scope          TEXT NOT NULL,
            created_at     INTEGER NOT NULL,
            expires_at     INTEGER,
            revoked_at     INTEGER,
            last_used_at   INTEGER
        );

        -- Authentication looks up by hash on every worker connect.
        CREATE INDEX IF NOT EXISTS idx_worker_keys_hash ON worker_keys(key_hash);
        -- Listing and revocation are per-owner.
        CREATE INDEX IF NOT EXISTS idx_worker_keys_owner ON worker_keys(owner_user_id);

        UPDATE schema_version SET version = 67;",
    )
    .expect("migration v67 failed creating worker_keys");

    tracing::info!("applied migration v67: worker_keys (worker service credential)");
}

fn migrate_v1(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS conversations (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            title TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            provider TEXT,
            model TEXT,
            created_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_conversations_user
            ON conversations(user_id, updated_at DESC);

        CREATE INDEX IF NOT EXISTS idx_messages_conversation
            ON messages(conversation_id, created_at ASC);

        INSERT OR REPLACE INTO schema_version (version) VALUES (1);",
    )
    .expect("migration v1 failed");

    tracing::info!("applied migration v1: conversations + messages");
}

fn migrate_v2(conn: &Connection) {
    conn.execute_batch(
        "-- Workers (Brain-minted identities)
        CREATE TABLE IF NOT EXISTS workers (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'connected',
            created_at INTEGER NOT NULL,
            last_seen INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS worker_sessions (
            id TEXT PRIMARY KEY,
            worker_id TEXT NOT NULL REFERENCES workers(id),
            connected_at INTEGER NOT NULL,
            disconnected_at INTEGER,
            last_heartbeat INTEGER NOT NULL,
            grace_deadline INTEGER
        );

        CREATE INDEX IF NOT EXISTS idx_worker_sessions_worker
            ON worker_sessions(worker_id);

        -- Provider capabilities
        CREATE TABLE IF NOT EXISTS provider_capabilities (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            worker_id TEXT NOT NULL REFERENCES workers(id),
            user_id TEXT NOT NULL,
            provider TEXT NOT NULL,
            cli_version TEXT,
            status TEXT NOT NULL DEFAULT 'claimed',
            last_reported INTEGER NOT NULL,
            last_verified INTEGER,
            last_success INTEGER,
            last_failure INTEGER,
            failure_streak INTEGER NOT NULL DEFAULT 0,
            UNIQUE(worker_id, provider)
        );

        CREATE INDEX IF NOT EXISTS idx_capabilities_user
            ON provider_capabilities(user_id, provider);

        -- User profiles
        CREATE TABLE IF NOT EXISTS user_profiles (
            user_id TEXT PRIMARY KEY,
            active_profile TEXT NOT NULL DEFAULT 'auto',
            auto_mode TEXT NOT NULL DEFAULT 'normal',
            auto_mode_since INTEGER,
            custom_overrides TEXT,
            updated_at INTEGER NOT NULL
        );

        -- Decision ledger
        CREATE TABLE IF NOT EXISTS decisions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            run_id TEXT,
            step_id TEXT,
            timestamp INTEGER NOT NULL,
            intent TEXT NOT NULL,
            risk TEXT NOT NULL,
            tier TEXT NOT NULL,
            provider TEXT NOT NULL,
            model TEXT NOT NULL,
            worker_id TEXT,
            rationale TEXT NOT NULL,
            profile TEXT NOT NULL DEFAULT 'auto'
        );

        CREATE INDEX IF NOT EXISTS idx_decisions_user_ts
            ON decisions(user_id, timestamp DESC);

        CREATE TABLE IF NOT EXISTS outcomes (
            id TEXT PRIMARY KEY,
            decision_id TEXT NOT NULL REFERENCES decisions(id),
            timestamp INTEGER NOT NULL,
            success INTEGER NOT NULL,
            duration_ms INTEGER,
            failure_kind TEXT,
            failure_scope TEXT,
            files_changed TEXT,
            exit_code INTEGER
        );

        CREATE INDEX IF NOT EXISTS idx_outcomes_decision
            ON outcomes(decision_id);

        CREATE INDEX IF NOT EXISTS idx_outcomes_ts
            ON outcomes(timestamp DESC);

        -- Score evidence
        CREATE TABLE IF NOT EXISTS score_evidence (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            decision_id TEXT NOT NULL REFERENCES decisions(id),
            evaluator TEXT NOT NULL,
            evidence_json TEXT NOT NULL,
            score REAL,
            timestamp INTEGER NOT NULL
        );

        -- Usage tracking (pressure calculation)
        CREATE TABLE IF NOT EXISTS usage_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            provider TEXT NOT NULL,
            tier TEXT NOT NULL,
            model TEXT NOT NULL,
            worker_id TEXT,
            tokens_in INTEGER,
            tokens_out INTEGER,
            duration_ms INTEGER
        );

        CREATE INDEX IF NOT EXISTS idx_usage_window
            ON usage_events(user_id, provider, timestamp);

        -- Runs (Ship Captain orchestration)
        CREATE TABLE IF NOT EXISTS runs (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            goal TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            profile TEXT NOT NULL DEFAULT 'auto',
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            started_at INTEGER,
            finished_at INTEGER,
            failure_reason TEXT,
            heal_attempts INTEGER NOT NULL DEFAULT 0,
            version INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS idx_runs_user_status
            ON runs(user_id, status);

        -- Steps
        CREATE TABLE IF NOT EXISTS steps (
            id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL REFERENCES runs(id),
            kind TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            work_kind TEXT NOT NULL DEFAULT 'modify',
            recipe_seed_json TEXT,
            tier TEXT NOT NULL,
            risk TEXT NOT NULL,
            objective TEXT NOT NULL,
            required_provider TEXT,
            required_repo TEXT,
            input_context TEXT,
            output_summary TEXT,
            files_changed TEXT,
            base_commit TEXT,
            head_commit TEXT,
            attempt_count INTEGER NOT NULL DEFAULT 0,
            max_attempts INTEGER NOT NULL DEFAULT 3,
            lease_gen INTEGER NOT NULL DEFAULT 0,
            lease_deadline INTEGER,
            assigned_worker TEXT REFERENCES workers(id),
            last_error TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            version INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS idx_steps_run
            ON steps(run_id, status);

        CREATE INDEX IF NOT EXISTS idx_steps_pending
            ON steps(run_id, status) WHERE status = 'pending';

        CREATE INDEX IF NOT EXISTS idx_steps_leased
            ON steps(status, lease_deadline) WHERE status = 'leased';

        -- Step dependencies (normalized, typed edges)
        CREATE TABLE IF NOT EXISTS step_dependencies (
            step_id TEXT NOT NULL REFERENCES steps(id),
            depends_on_id TEXT NOT NULL REFERENCES steps(id),
            edge_type TEXT NOT NULL DEFAULT 'success_required',
            PRIMARY KEY (step_id, depends_on_id)
        );

        -- Step attempts (minimal history for debugging)
        CREATE TABLE IF NOT EXISTS step_attempts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            step_id TEXT NOT NULL REFERENCES steps(id),
            run_id TEXT NOT NULL,
            attempt_number INTEGER NOT NULL,
            worker_id TEXT,
            lease_gen INTEGER NOT NULL,
            status TEXT NOT NULL,
            provider TEXT,
            model TEXT,
            started_at INTEGER NOT NULL,
            finished_at INTEGER,
            failure_kind TEXT,
            error_summary TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_attempts_step
            ON step_attempts(step_id, attempt_number);

        -- Artifacts
        CREATE TABLE IF NOT EXISTS artifacts (
            id TEXT PRIMARY KEY,
            step_id TEXT NOT NULL REFERENCES steps(id),
            attempt_number INTEGER NOT NULL,
            kind TEXT NOT NULL,
            uri TEXT NOT NULL,
            sha256 TEXT,
            size_bytes INTEGER,
            created_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_artifacts_step
            ON artifacts(step_id);

        -- Idempotency keys (message dedup)
        CREATE TABLE IF NOT EXISTS idempotency_keys (
            key TEXT PRIMARY KEY,
            result_json TEXT,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL
        );

        UPDATE schema_version SET version = 2;",
    )
    .expect("migration v2 failed");

    tracing::info!(
        "applied migration v2: workers, capabilities, profiles, decisions, runs, steps, dependencies, attempts, artifacts"
    );
}

fn migrate_v3(conn: &Connection) {
    // Add a branch column to runs for PR creation after step completion.
    conn.execute_batch(
        "ALTER TABLE runs ADD COLUMN branch TEXT;

        UPDATE schema_version SET version = 3;",
    )
    .expect("migration v3 failed");

    tracing::info!("applied migration v3: runs.branch column for PR creation");
}

fn migrate_v4(conn: &Connection) {
    // Add earliest_dispatch_at column for retry backoff on heal steps.
    // NULL means "dispatch immediately" (backwards-compatible default).
    conn.execute_batch(
        "ALTER TABLE steps ADD COLUMN earliest_dispatch_at INTEGER;

        UPDATE schema_version SET version = 4;",
    )
    .expect("migration v4 failed");

    tracing::info!("applied migration v4: steps.earliest_dispatch_at for retry backoff");
}

fn migrate_v5(conn: &Connection) {
    // Add file_paths column to runs for workspace context.
    // Stored as JSON text (array of strings). NULL means no specific paths.
    conn.execute_batch(
        "ALTER TABLE runs ADD COLUMN file_paths TEXT;

        UPDATE schema_version SET version = 5;",
    )
    .expect("migration v5 failed");

    tracing::info!("applied migration v5: runs.file_paths for workspace context");
}

fn migrate_v6(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS subscriptions (
            clerk_user_id TEXT PRIMARY KEY,
            stripe_customer_id TEXT NOT NULL,
            stripe_subscription_id TEXT,
            plan_type TEXT NOT NULL DEFAULT 'monthly',
            status TEXT NOT NULL DEFAULT 'trialing',
            trial_end TEXT,
            current_period_start TEXT,
            current_period_end TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS credit_balances (
            clerk_user_id TEXT PRIMARY KEY,
            subscription_remaining REAL NOT NULL DEFAULT 200.0,
            subscription_total REAL NOT NULL DEFAULT 200.0,
            pack_remaining REAL NOT NULL DEFAULT 0.0,
            last_reset_at TEXT
        );

        CREATE TABLE IF NOT EXISTS credit_transactions (
            id TEXT PRIMARY KEY,
            clerk_user_id TEXT NOT NULL,
            amount REAL NOT NULL,
            balance_type TEXT NOT NULL,
            description TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS billing_history (
            id TEXT PRIMARY KEY,
            clerk_user_id TEXT NOT NULL,
            stripe_event_id TEXT UNIQUE,
            amount_cents INTEGER NOT NULL,
            description TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS referral_codes (
            code TEXT PRIMARY KEY,
            creator_user_id TEXT NOT NULL,
            uses_remaining INTEGER NOT NULL DEFAULT 1,
            total_uses INTEGER NOT NULL DEFAULT 0,
            credits_earned REAL NOT NULL DEFAULT 0.0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        UPDATE schema_version SET version = 6;",
    )
    .expect("migration v6 failed");

    tracing::info!(
        "applied migration v6: subscriptions, credit_balances, credit_transactions, billing_history, referral_codes"
    );
}

fn migrate_v7(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS promo_codes (
            id TEXT PRIMARY KEY,
            code TEXT NOT NULL UNIQUE COLLATE NOCASE,
            discount_type TEXT NOT NULL DEFAULT 'trial_extension',
            discount_value REAL NOT NULL DEFAULT 14.0,
            max_uses INTEGER NOT NULL DEFAULT 25,
            current_uses INTEGER NOT NULL DEFAULT 0,
            expires_at TEXT,
            active INTEGER NOT NULL DEFAULT 1,
            created_by TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            description TEXT
        );

        CREATE TABLE IF NOT EXISTS code_redemptions (
            id TEXT PRIMARY KEY,
            promo_code_id TEXT NOT NULL REFERENCES promo_codes(id),
            code TEXT NOT NULL,
            user_id TEXT NOT NULL,
            redeemed_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(code, user_id)
        );

        CREATE INDEX IF NOT EXISTS idx_code_redemptions_user ON code_redemptions(user_id);
        CREATE INDEX IF NOT EXISTS idx_code_redemptions_code ON code_redemptions(code);

        UPDATE schema_version SET version = 7;",
    )
    .expect("migration v7 failed");

    tracing::info!("applied migration v7: promo_codes, code_redemptions");
}

fn migrate_v8(conn: &Connection) {
    conn.execute(
        "ALTER TABLE referral_codes ADD COLUMN weeks_earned INTEGER NOT NULL DEFAULT 0",
        [],
    )
    .ok();

    conn.execute(
        "ALTER TABLE referral_codes ADD COLUMN max_uses INTEGER NOT NULL DEFAULT 50",
        [],
    )
    .ok();

    conn.execute_batch("UPDATE schema_version SET version = 8;")
        .expect("migration v8 failed");

    tracing::info!("applied migration v8: referral_codes weeks_earned + max_uses");
}

fn migrate_v9(conn: &Connection) {
    conn.execute(
        "ALTER TABLE promo_codes ADD COLUMN discount_options TEXT",
        [],
    )
    .ok();

    conn.execute_batch("UPDATE schema_version SET version = 9;")
        .expect("migration v9 failed");

    tracing::info!("applied migration v9: promo_codes discount_options JSON column");
}

fn migrate_v10(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS cortex_groups (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            name TEXT NOT NULL,
            kind TEXT NOT NULL DEFAULT 'team',
            description TEXT NOT NULL DEFAULT 'Shared coordination',
            members INTEGER NOT NULL DEFAULT 1,
            accent TEXT NOT NULL DEFAULT '#9cc7b8',
            source TEXT NOT NULL DEFAULT 'manual',
            external_id TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, source, external_id)
        );
        CREATE INDEX IF NOT EXISTS idx_cortex_groups_user
            ON cortex_groups(user_id, updated_at DESC);

        CREATE TABLE IF NOT EXISTS group_task_state (
            group_id TEXT NOT NULL,
            user_id TEXT NOT NULL,
            state_json TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (group_id, user_id)
        );

        CREATE TABLE IF NOT EXISTS integration_connections (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            provider TEXT NOT NULL,
            external_id TEXT,
            display_name TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'connected',
            scopes TEXT NOT NULL DEFAULT '[]',
            access_token TEXT,
            refresh_token TEXT,
            token_expires_at TEXT,
            metadata_json TEXT NOT NULL DEFAULT '{}',
            last_sync_at TEXT,
            last_error TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, provider, external_id)
        );
        CREATE INDEX IF NOT EXISTS idx_integration_connections_user
            ON integration_connections(user_id, provider);

        CREATE TABLE IF NOT EXISTS integration_mappings (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            provider TEXT NOT NULL,
            group_id TEXT NOT NULL,
            external_id TEXT NOT NULL,
            external_name TEXT NOT NULL,
            mapping_type TEXT NOT NULL,
            metadata_json TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, provider, external_id, mapping_type)
        );
        CREATE INDEX IF NOT EXISTS idx_integration_mappings_group
            ON integration_mappings(user_id, group_id);

        CREATE TABLE IF NOT EXISTS integration_events (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            provider TEXT NOT NULL,
            group_id TEXT,
            event_type TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'queued',
            payload_json TEXT NOT NULL,
            retry_count INTEGER NOT NULL DEFAULT 0,
            next_attempt_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            processed_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_integration_events_status
            ON integration_events(status, next_attempt_at);

        CREATE TABLE IF NOT EXISTS integration_oauth_states (
            state TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            provider TEXT NOT NULL,
            redirect_after TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            expires_at TEXT NOT NULL
        );

        UPDATE schema_version SET version = 10;",
    )
    .expect("migration v10 failed");

    tracing::info!(
        "applied migration v10: Cortex groups, task state, Slack/Replit integration tables"
    );
}

fn migrate_v11(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS context_flow_artifacts (
            id TEXT PRIMARY KEY,
            producer_step_id TEXT NOT NULL,
            producer_run_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            content TEXT NOT NULL,
            summary TEXT NOT NULL,
            files_changed TEXT, -- JSON array
            confidence REAL NOT NULL,
            tokens INTEGER NOT NULL,
            created_at INTEGER NOT NULL, -- milliseconds since epoch
            metadata TEXT -- JSON object
        );

        CREATE INDEX IF NOT EXISTS idx_context_artifacts_run
            ON context_flow_artifacts(producer_run_id, created_at);

        CREATE INDEX IF NOT EXISTS idx_context_artifacts_step
            ON context_flow_artifacts(producer_step_id);

        UPDATE schema_version SET version = 11;",
    )
    .expect("migration v11 failed");

    tracing::info!(
        "applied migration v11: context_flow_artifacts table for AI model context pipeline"
    );
}

fn migrate_v12(conn: &Connection) {
    conn.execute(
        "ALTER TABLE steps ADD COLUMN verification_status TEXT NOT NULL DEFAULT 'unverified'",
        [],
    )
    .ok();
    conn.execute("ALTER TABLE steps ADD COLUMN verifier_report_id TEXT", [])
        .ok();
    conn.execute("ALTER TABLE steps ADD COLUMN verified_at INTEGER", [])
        .ok();

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS verifier_reports (
            id TEXT PRIMARY KEY,
            step_id TEXT NOT NULL REFERENCES steps(id),
            run_id TEXT NOT NULL REFERENCES runs(id),
            lease_gen INTEGER NOT NULL,
            worker_id TEXT,
            verifier TEXT NOT NULL,
            status TEXT NOT NULL,
            verdict TEXT NOT NULL,
            evidence_json TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_verifier_reports_step
            ON verifier_reports(step_id, created_at DESC);

        CREATE INDEX IF NOT EXISTS idx_verifier_reports_run
            ON verifier_reports(run_id, created_at DESC);

        UPDATE schema_version SET version = 12;",
    )
    .expect("migration v12 failed");

    tracing::info!("applied migration v12: verifier reports and step verification status");
}

fn migrate_v13(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS step_work_contracts (
            step_id TEXT NOT NULL REFERENCES steps(id),
            lease_gen INTEGER NOT NULL,
            run_id TEXT NOT NULL REFERENCES runs(id),
            contract_json TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (step_id, lease_gen)
        );

        CREATE INDEX IF NOT EXISTS idx_step_work_contracts_run
            ON step_work_contracts(run_id, created_at DESC);

        UPDATE schema_version SET version = 13;",
    )
    .expect("migration v13 failed");

    tracing::info!("applied migration v13: persisted step work contracts");
}

fn migrate_v14(conn: &Connection) {
    conn.execute(
        "ALTER TABLE steps ADD COLUMN work_kind TEXT NOT NULL DEFAULT 'modify'",
        [],
    )
    .ok();
    conn.execute_batch("UPDATE schema_version SET version = 14;")
        .expect("migration v14 failed");

    tracing::info!("applied migration v14: steps.work_kind planner recipe intent");
}

fn migrate_v15(conn: &Connection) {
    conn.execute("ALTER TABLE steps ADD COLUMN recipe_seed_json TEXT", [])
        .ok();
    conn.execute_batch("UPDATE schema_version SET version = 15;")
        .expect("migration v15 failed");

    tracing::info!("applied migration v15: steps.recipe_seed_json planner recipe seeds");
}

fn migrate_v16(conn: &Connection) {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_steps_run_created
            ON steps(run_id, created_at ASC, id ASC);

        CREATE INDEX IF NOT EXISTS idx_verifier_reports_run_step_latest
            ON verifier_reports(run_id, step_id, created_at DESC);

        CREATE INDEX IF NOT EXISTS idx_step_work_contracts_run_step_latest
            ON step_work_contracts(run_id, step_id, lease_gen DESC, created_at DESC);

        UPDATE schema_version SET version = 16;",
    )
    .expect("migration v16 failed");

    tracing::info!("applied migration v16: run payload snapshot indexes");
}

fn migrate_v17(conn: &Connection) {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_step_attempts_run_step_latest
            ON step_attempts(run_id, step_id, attempt_number DESC);

        UPDATE schema_version SET version = 17;",
    )
    .expect("migration v17 failed");

    tracing::info!("applied migration v17: latest attempt snapshot index");
}

fn migrate_v18(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS operations_events (
            id TEXT PRIMARY KEY,
            created_at INTEGER NOT NULL,
            actor_user_id TEXT,
            scope_id TEXT,
            project_id TEXT,
            task_id TEXT,
            run_id TEXT,
            step_id TEXT,
            attempt_id TEXT,
            event_type TEXT NOT NULL,
            entity_type TEXT NOT NULL,
            entity_id TEXT NOT NULL,
            payload_json TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_operations_events_created_at
            ON operations_events(created_at);

        CREATE INDEX IF NOT EXISTS idx_operations_events_run
            ON operations_events(run_id, created_at);

        CREATE INDEX IF NOT EXISTS idx_operations_events_step
            ON operations_events(step_id, created_at);

        CREATE INDEX IF NOT EXISTS idx_operations_events_entity
            ON operations_events(entity_type, entity_id, created_at);

        CREATE INDEX IF NOT EXISTS idx_operations_events_actor
            ON operations_events(actor_user_id, created_at);

        UPDATE schema_version SET version = 18;",
    )
    .expect("migration v18 failed");

    tracing::info!("applied migration v18: operations room event log");
}

fn migrate_v19(conn: &Connection) {
    conn.execute("ALTER TABLE runs ADD COLUMN task_id TEXT", [])
        .ok();
    conn.execute("ALTER TABLE runs ADD COLUMN group_id TEXT", [])
        .ok();
    conn.execute("ALTER TABLE runs ADD COLUMN conversation_id TEXT", [])
        .ok();
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_runs_task
            ON runs(user_id, task_id, created_at DESC);

        CREATE INDEX IF NOT EXISTS idx_runs_group
            ON runs(user_id, group_id, created_at DESC);

        UPDATE schema_version SET version = 19;",
    )
    .expect("migration v19 failed");

    tracing::info!("applied migration v19: task-aware run metadata");
}

fn migrate_v20(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS cortex_tasks (
            id TEXT NOT NULL,
            user_id TEXT NOT NULL,
            group_id TEXT NOT NULL,
            title TEXT NOT NULL,
            status TEXT NOT NULL,
            priority TEXT,
            conversation_id TEXT,
            latest_run_id TEXT,
            source_json TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            version INTEGER NOT NULL DEFAULT 1,
            PRIMARY KEY (user_id, group_id, id)
        );

        CREATE INDEX IF NOT EXISTS idx_cortex_tasks_group_status
            ON cortex_tasks(user_id, group_id, status, updated_at DESC);

        CREATE INDEX IF NOT EXISTS idx_cortex_tasks_latest_run
            ON cortex_tasks(user_id, latest_run_id);

        CREATE TABLE IF NOT EXISTS cortex_task_chats (
            user_id TEXT NOT NULL,
            group_id TEXT NOT NULL,
            task_id TEXT NOT NULL,
            conversation_id TEXT NOT NULL,
            attached_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (user_id, group_id, task_id, conversation_id)
        );",
    )
    .expect("migration v20 failed");

    let task_states = {
        let mut stmt = conn
            .prepare("SELECT user_id, group_id, state_json FROM group_task_state")
            .expect("failed to prepare task state backfill");
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .expect("failed to query task state backfill")
        .filter_map(|row| row.ok())
        .collect::<Vec<_>>()
    };

    for (user_id, group_id, raw) in task_states {
        if let Ok(state) = serde_json::from_str::<serde_json::Value>(&raw) {
            index_group_task_state(conn, &user_id, &group_id, &state);
        }
    }

    conn.execute("UPDATE schema_version SET version = 20", [])
        .expect("failed to mark migration v20");

    tracing::info!("applied migration v20: cortex task shadow index");
}

fn migrate_v21(conn: &Connection) {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_operations_events_scope_created
            ON operations_events(scope_id, created_at DESC);

        CREATE INDEX IF NOT EXISTS idx_operations_events_scope_task_created
            ON operations_events(scope_id, task_id, created_at DESC);

        CREATE INDEX IF NOT EXISTS idx_runs_group_status_created
            ON runs(user_id, group_id, status, created_at DESC);

        CREATE INDEX IF NOT EXISTS idx_cortex_task_chats_group_attached
            ON cortex_task_chats(user_id, group_id, task_id, attached_at DESC);

        UPDATE schema_version SET version = 21;",
    )
    .expect("migration v21 failed");

    tracing::info!("applied migration v21: operations summary query indexes");
}

fn migrate_v22(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS cortex_approval_requests (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            group_id TEXT NOT NULL,
            task_id TEXT,
            conversation_id TEXT,
            run_id TEXT,
            status TEXT NOT NULL DEFAULT 'pending',
            title TEXT NOT NULL,
            body TEXT NOT NULL DEFAULT '',
            priority TEXT NOT NULL DEFAULT 'normal',
            requested_by TEXT NOT NULL DEFAULT 'cortex',
            decision_json TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            resolved_at INTEGER
        );

        CREATE INDEX IF NOT EXISTS idx_cortex_approval_requests_group_status
            ON cortex_approval_requests(user_id, group_id, status, updated_at DESC);

        CREATE INDEX IF NOT EXISTS idx_cortex_approval_requests_task
            ON cortex_approval_requests(user_id, group_id, task_id, updated_at DESC);

        UPDATE schema_version SET version = 22;",
    )
    .expect("migration v22 failed");

    tracing::info!("applied migration v22: Cortex approval request ledger");
}

fn migrate_v23(conn: &Connection) {
    conn.execute(
        "ALTER TABLE cortex_approval_requests ADD COLUMN step_id TEXT",
        [],
    )
    .ok();
    conn.execute(
        "ALTER TABLE cortex_approval_requests ADD COLUMN ask_type TEXT NOT NULL DEFAULT 'approval'",
        [],
    )
    .ok();

    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_cortex_approval_requests_step_status
            ON cortex_approval_requests(user_id, step_id, status, updated_at DESC);

        CREATE INDEX IF NOT EXISTS idx_cortex_approval_requests_run_status
            ON cortex_approval_requests(user_id, run_id, status, updated_at DESC);

        UPDATE schema_version SET version = 23;",
    )
    .expect("migration v23 failed");

    tracing::info!("applied migration v23: Cortex approval step gates");
}

fn migrate_v24(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS resource_leases (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            group_id TEXT,
            task_id TEXT,
            run_id TEXT NOT NULL,
            step_id TEXT,
            holder_type TEXT NOT NULL,
            resource_type TEXT NOT NULL,
            repo_key TEXT NOT NULL DEFAULT 'default',
            resource_key TEXT NOT NULL,
            mode TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'active',
            lease_gen INTEGER NOT NULL DEFAULT 1,
            acquired_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            released_at INTEGER,
            reason TEXT,
            metadata_json TEXT NOT NULL DEFAULT '{}'
        );

        -- Numbers the scheduler hands out so no agent has to pick one.
        --
        -- A migration version is not mergeable: two branches that each choose
        -- the next number both produce a clean file, the merge succeeds, and
        -- the one that lands second is silently skipped. Allocation is
        -- serialised through this table so the value is decided once, by us.
        CREATE TABLE IF NOT EXISTS sequence_allocations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repo_key TEXT NOT NULL,
            sequence TEXT NOT NULL,
            value INTEGER NOT NULL,
            step_id TEXT,
            run_id TEXT,
            allocated_at INTEGER NOT NULL
        );

        -- The uniqueness that does the work. Two concurrent allocations of the
        -- same value cannot both commit, whatever the readers raced on.
        CREATE UNIQUE INDEX IF NOT EXISTS idx_sequence_allocations_value
            ON sequence_allocations(repo_key, sequence, value);

        CREATE INDEX IF NOT EXISTS idx_resource_leases_active
            ON resource_leases(user_id, status, resource_type, repo_key, resource_key, expires_at);

        CREATE INDEX IF NOT EXISTS idx_resource_leases_run
            ON resource_leases(run_id, status);

        CREATE INDEX IF NOT EXISTS idx_resource_leases_task
            ON resource_leases(user_id, group_id, task_id, status);

        UPDATE schema_version SET version = 24;",
    )
    .expect("migration v24 failed");

    tracing::info!("applied migration v24: Cortex resource leases");
}

fn migrate_v25(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS cortex_authority_scopes (
            id TEXT PRIMARY KEY,
            owner_user_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            name TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            source TEXT NOT NULL DEFAULT 'cortex',
            external_id TEXT,
            status TEXT NOT NULL DEFAULT 'active',
            policy_json TEXT NOT NULL DEFAULT '{}',
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_cortex_authority_scopes_owner_source_external
            ON cortex_authority_scopes(owner_user_id, source, external_id)
            WHERE external_id IS NOT NULL;

        CREATE INDEX IF NOT EXISTS idx_cortex_authority_scopes_owner_status
            ON cortex_authority_scopes(owner_user_id, status, updated_at DESC);

        CREATE TABLE IF NOT EXISTS cortex_authority_memberships (
            scope_id TEXT NOT NULL,
            user_id TEXT NOT NULL,
            role TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'active',
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (scope_id, user_id)
        );

        CREATE INDEX IF NOT EXISTS idx_cortex_authority_memberships_user_status
            ON cortex_authority_memberships(user_id, status, updated_at DESC);

        CREATE TABLE IF NOT EXISTS cortex_authority_resources (
            id TEXT PRIMARY KEY,
            scope_id TEXT NOT NULL,
            resource_type TEXT NOT NULL,
            resource_key TEXT NOT NULL,
            access TEXT NOT NULL DEFAULT 'read',
            policy_json TEXT NOT NULL DEFAULT '{}',
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_cortex_authority_resources_scope_resource
            ON cortex_authority_resources(scope_id, resource_type, resource_key);

        CREATE INDEX IF NOT EXISTS idx_cortex_authority_resources_scope
            ON cortex_authority_resources(scope_id, resource_type);

        UPDATE schema_version SET version = 25;",
    )
    .expect("migration v25 failed");

    tracing::info!("applied migration v25: Cortex authority scopes");
}

fn migrate_v26(conn: &Connection) {
    conn.execute("ALTER TABLE runs ADD COLUMN repo_key TEXT", [])
        .ok();
    conn.execute("ALTER TABLE runs ADD COLUMN authority_scope_id TEXT", [])
        .ok();
    conn.execute(
        "ALTER TABLE runs ADD COLUMN authority_context_json TEXT",
        [],
    )
    .ok();
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_runs_authority_scope
            ON runs(user_id, authority_scope_id, created_at DESC);

        CREATE INDEX IF NOT EXISTS idx_runs_repo_key
            ON runs(user_id, repo_key, created_at DESC);

        UPDATE schema_version SET version = 26;",
    )
    .expect("migration v26 failed");

    tracing::info!("applied migration v26: Cortex run authority metadata");
}

fn migrate_v27(conn: &Connection) {
    conn.execute(
        "ALTER TABLE resource_leases ADD COLUMN authority_scope_id TEXT",
        [],
    )
    .ok();
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_resource_leases_authority_active
            ON resource_leases(authority_scope_id, status, resource_type, repo_key, resource_key, expires_at);

        UPDATE resource_leases
        SET authority_scope_id = (
            SELECT runs.authority_scope_id
            FROM runs
            WHERE runs.id = resource_leases.run_id
        )
        WHERE authority_scope_id IS NULL
          AND EXISTS (
              SELECT 1
              FROM runs
              WHERE runs.id = resource_leases.run_id
                AND runs.authority_scope_id IS NOT NULL
          );

        UPDATE schema_version SET version = 27;",
    )
    .expect("migration v27 failed");

    tracing::info!("applied migration v27: authority-scoped resource leases");
}

fn ensure_social_tables(conn: &Connection) {
    let has_social_posts: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='social_posts'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;

    if has_social_posts {
        return;
    }

    tracing::info!("social tables missing — creating all social tables");

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS social_profiles (
            id TEXT PRIMARY KEY, clerk_user_id TEXT NOT NULL, handle TEXT NOT NULL,
            display_name TEXT NOT NULL, bio TEXT NOT NULL DEFAULT '', avatar_url TEXT,
            banner_url TEXT, location TEXT, website_url TEXT,
            proof_state TEXT NOT NULL DEFAULT 'pending', continuity_state TEXT NOT NULL DEFAULT 'pending',
            created_at TEXT NOT NULL DEFAULT (datetime('now')), updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_social_profiles_clerk ON social_profiles(clerk_user_id);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_social_profiles_handle ON social_profiles(handle);

        CREATE TABLE IF NOT EXISTS social_linked_agents (
            id TEXT PRIMARY KEY, profile_id TEXT NOT NULL REFERENCES social_profiles(id),
            agent_name TEXT NOT NULL, agent_slug TEXT NOT NULL, agent_key TEXT NOT NULL,
            agent_key_hash TEXT,
            agent_type TEXT NOT NULL DEFAULT 'general', link_state TEXT NOT NULL DEFAULT 'active',
            visibility TEXT NOT NULL DEFAULT 'public', proof_state TEXT NOT NULL DEFAULT 'pending',
            is_primary INTEGER NOT NULL DEFAULT 0,
            auto_reply_enabled INTEGER NOT NULL DEFAULT 0,
            auto_follow_enabled INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')), updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_social_linked_agents_profile ON social_linked_agents(profile_id);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_social_linked_agents_key_hash
            ON social_linked_agents(agent_key_hash)
            WHERE agent_key_hash IS NOT NULL AND agent_key_hash != '';

        CREATE TABLE IF NOT EXISTS social_posts (
            id TEXT PRIMARY KEY, profile_id TEXT NOT NULL REFERENCES social_profiles(id),
            linked_agent_id TEXT REFERENCES social_linked_agents(id), body TEXT NOT NULL,
            visibility TEXT NOT NULL DEFAULT 'public', proof_state TEXT NOT NULL DEFAULT 'pending',
            author_mode TEXT NOT NULL DEFAULT 'person',
            reply_to_post_id TEXT REFERENCES social_posts(id), quote_post_id TEXT REFERENCES social_posts(id),
            community_id TEXT, deleted_at TEXT DEFAULT NULL,
            view_count INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')), updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_social_posts_profile ON social_posts(profile_id);
        CREATE INDEX IF NOT EXISTS idx_social_posts_created ON social_posts(created_at);
        CREATE INDEX IF NOT EXISTS idx_social_posts_community ON social_posts(community_id);

        CREATE TABLE IF NOT EXISTS social_follows (
            id TEXT PRIMARY KEY, follower_profile_id TEXT NOT NULL REFERENCES social_profiles(id),
            following_profile_id TEXT NOT NULL REFERENCES social_profiles(id),
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_social_follows_pair ON social_follows(follower_profile_id, following_profile_id);
        CREATE INDEX IF NOT EXISTS idx_social_follows_follower ON social_follows(follower_profile_id);
        CREATE INDEX IF NOT EXISTS idx_social_follows_following ON social_follows(following_profile_id);

        CREATE TABLE IF NOT EXISTS social_communities (
            id TEXT PRIMARY KEY, creator_profile_id TEXT NOT NULL REFERENCES social_profiles(id),
            slug TEXT NOT NULL, name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
            visibility TEXT NOT NULL DEFAULT 'public',
            created_at TEXT NOT NULL DEFAULT (datetime('now')), updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_social_communities_slug ON social_communities(slug);
        CREATE INDEX IF NOT EXISTS idx_social_communities_creator ON social_communities(creator_profile_id);

        CREATE TABLE IF NOT EXISTS social_community_memberships (
            id TEXT PRIMARY KEY, community_id TEXT NOT NULL REFERENCES social_communities(id),
            profile_id TEXT NOT NULL REFERENCES social_profiles(id),
            joined_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_social_memberships_pair ON social_community_memberships(community_id, profile_id);
        CREATE INDEX IF NOT EXISTS idx_social_memberships_profile ON social_community_memberships(profile_id);

        CREATE TABLE IF NOT EXISTS social_longform (
            id TEXT PRIMARY KEY, profile_id TEXT NOT NULL REFERENCES social_profiles(id),
            linked_agent_id TEXT REFERENCES social_linked_agents(id),
            title TEXT NOT NULL, summary TEXT NOT NULL DEFAULT '', body TEXT NOT NULL,
            format_type TEXT NOT NULL DEFAULT 'essay', visibility TEXT NOT NULL DEFAULT 'public',
            proof_state TEXT NOT NULL DEFAULT 'pending', author_mode TEXT NOT NULL DEFAULT 'person',
            created_at TEXT NOT NULL DEFAULT (datetime('now')), updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_social_longform_profile ON social_longform(profile_id);
        CREATE INDEX IF NOT EXISTS idx_social_longform_created ON social_longform(created_at);

        CREATE TABLE IF NOT EXISTS social_likes (
            profile_id TEXT NOT NULL, post_id TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (profile_id, post_id)
        );
        CREATE INDEX IF NOT EXISTS idx_social_likes_post ON social_likes(post_id);

        CREATE TABLE IF NOT EXISTS social_bookmarks (
            profile_id TEXT NOT NULL, post_id TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (profile_id, post_id)
        );
        CREATE INDEX IF NOT EXISTS idx_social_bookmarks_post ON social_bookmarks(post_id);

        CREATE TABLE IF NOT EXISTS social_reposts (
            profile_id TEXT NOT NULL, post_id TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (profile_id, post_id)
        );
        CREATE INDEX IF NOT EXISTS idx_social_reposts_post ON social_reposts(post_id);

        CREATE TABLE IF NOT EXISTS pulse_drafts (
            id TEXT PRIMARY KEY, profile_id TEXT NOT NULL, body TEXT NOT NULL,
            visibility TEXT NOT NULL DEFAULT 'public', author_mode TEXT NOT NULL DEFAULT 'person',
            linked_agent_id TEXT, status TEXT NOT NULL DEFAULT 'pending',
            created_at TEXT NOT NULL DEFAULT (datetime('now')), updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_pulse_drafts_profile ON pulse_drafts(profile_id);
        CREATE INDEX IF NOT EXISTS idx_pulse_drafts_status ON pulse_drafts(status);

        CREATE TABLE IF NOT EXISTS pulse_audit_log (
            id TEXT PRIMARY KEY, draft_id TEXT NOT NULL, action TEXT NOT NULL,
            actor_profile_id TEXT NOT NULL, details_json TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_pulse_audit_log_draft ON pulse_audit_log(draft_id);

        CREATE TABLE IF NOT EXISTS pulse_schedules (
            id TEXT PRIMARY KEY,
            profile_id TEXT NOT NULL,
            draft_id TEXT NOT NULL,
            publish_at TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'scheduled',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_pulse_schedules_due
            ON pulse_schedules(status, publish_at);

        -- Deterministic goal plans (MVP; not Temporal). plan_json + steps_json are JSON text.
        CREATE TABLE IF NOT EXISTS pulse_goals (
            id TEXT PRIMARY KEY,
            profile_id TEXT NOT NULL,
            goal TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'active',
            plan_json TEXT NOT NULL DEFAULT '{}',
            steps_json TEXT NOT NULL DEFAULT '[]',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_pulse_goals_profile
            ON pulse_goals(profile_id, updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_pulse_goals_status
            ON pulse_goals(profile_id, status);

        CREATE TABLE IF NOT EXISTS social_notifications (
            id TEXT PRIMARY KEY, recipient_profile_id TEXT NOT NULL, actor_profile_id TEXT NOT NULL,
            notification_type TEXT NOT NULL, post_id TEXT, read INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_social_notifications_recipient ON social_notifications(recipient_profile_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_social_notifications_read ON social_notifications(recipient_profile_id, read);

        CREATE TABLE IF NOT EXISTS social_media_objects (
            id TEXT PRIMARY KEY, owner_profile_id TEXT NOT NULL, filename TEXT NOT NULL,
            content_type TEXT NOT NULL, size_bytes INTEGER NOT NULL, media_type TEXT NOT NULL DEFAULT 'image',
            storage_key TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'pending',
            created_at TEXT NOT NULL DEFAULT (datetime('now')), updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_social_media_objects_owner ON social_media_objects(owner_profile_id);
        CREATE INDEX IF NOT EXISTS idx_social_media_objects_status ON social_media_objects(status, created_at);

        CREATE TABLE IF NOT EXISTS social_post_media (
            post_id TEXT NOT NULL, media_id TEXT NOT NULL, position INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (post_id, media_id)
        );
        CREATE INDEX IF NOT EXISTS idx_social_post_media_post ON social_post_media(post_id);
        CREATE INDEX IF NOT EXISTS idx_social_post_media_media ON social_post_media(media_id);

        CREATE TABLE IF NOT EXISTS webhook_events (
            id TEXT PRIMARY KEY, event_type TEXT NOT NULL, timestamp INTEGER NOT NULL,
            processed_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_webhook_events_type ON webhook_events(event_type);

        CREATE TABLE IF NOT EXISTS accounts (
            clerk_user_id TEXT PRIMARY KEY, email TEXT NOT NULL DEFAULT '',
            display_name TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'active',
            created_at TEXT NOT NULL DEFAULT (datetime('now')), updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_accounts_email ON accounts(email);
        CREATE INDEX IF NOT EXISTS idx_accounts_status ON accounts(status);

        CREATE TABLE IF NOT EXISTS social_conversations (
            id TEXT PRIMARY KEY, created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE IF NOT EXISTS social_conversation_participants (
            conversation_id TEXT NOT NULL, profile_id TEXT NOT NULL,
            joined_at TEXT NOT NULL DEFAULT (datetime('now')), PRIMARY KEY (conversation_id, profile_id)
        );
        CREATE INDEX IF NOT EXISTS idx_social_conv_participants_profile ON social_conversation_participants(profile_id);
        CREATE TABLE IF NOT EXISTS social_messages (
            id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL, sender_profile_id TEXT NOT NULL,
            content TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT (datetime('now')), read INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_social_messages_conversation ON social_messages(conversation_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_social_messages_sender ON social_messages(sender_profile_id);

        CREATE TABLE IF NOT EXISTS social_blocks (
            blocker_profile_id TEXT NOT NULL, blocked_profile_id TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')), UNIQUE (blocker_profile_id, blocked_profile_id)
        );
        CREATE INDEX IF NOT EXISTS idx_social_blocks_blocker ON social_blocks(blocker_profile_id);
        CREATE INDEX IF NOT EXISTS idx_social_blocks_blocked ON social_blocks(blocked_profile_id);
        CREATE TABLE IF NOT EXISTS social_mutes (
            muter_profile_id TEXT NOT NULL, muted_profile_id TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')), UNIQUE (muter_profile_id, muted_profile_id)
        );
        CREATE INDEX IF NOT EXISTS idx_social_mutes_muter ON social_mutes(muter_profile_id);
        CREATE TABLE IF NOT EXISTS social_reports (
            id TEXT PRIMARY KEY, reporter_profile_id TEXT NOT NULL, target_type TEXT NOT NULL,
            target_id TEXT NOT NULL, reason TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'pending',
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_social_reports_reporter ON social_reports(reporter_profile_id);
        CREATE INDEX IF NOT EXISTS idx_social_reports_status ON social_reports(status);

        CREATE TABLE IF NOT EXISTS audit_log (
            id TEXT PRIMARY KEY, actor_id TEXT NOT NULL, actor_type TEXT NOT NULL DEFAULT 'user',
            action TEXT NOT NULL, target_type TEXT, target_id TEXT, details TEXT, ip_address TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_audit_log_actor ON audit_log(actor_id);
        CREATE INDEX IF NOT EXISTS idx_audit_log_created ON audit_log(created_at);

        UPDATE schema_version SET version = 33;"
    ).expect("social tables creation failed");

    tracing::info!("social tables created successfully");
}

/// Create FTS5 virtual table + triggers for post body search when SQLite has FTS5.
/// Safe no-op if FTS5 is unavailable or table already exists.
fn ensure_social_posts_fts(conn: &Connection) {
    let has_posts: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='social_posts'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
    if !has_posts {
        return;
    }

    let has_fts: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='social_posts_fts'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;

    if !has_fts {
        match conn.execute_batch(
            "CREATE VIRTUAL TABLE social_posts_fts USING fts5(
                post_id UNINDEXED,
                body,
                tokenize = 'porter unicode61'
            );",
        ) {
            Ok(()) => {
                tracing::info!("created social_posts_fts FTS5 virtual table");
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "FTS5 unavailable — social search will use LIKE fallback"
                );
                return;
            }
        }
    }

    // Triggers keep FTS in sync with social_posts (idempotent IF NOT EXISTS via drop/create).
    let _ = conn.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS social_posts_fts_ai AFTER INSERT ON social_posts BEGIN
            INSERT INTO social_posts_fts(post_id, body) VALUES (new.id, new.body);
         END;
         CREATE TRIGGER IF NOT EXISTS social_posts_fts_ad AFTER DELETE ON social_posts BEGIN
            DELETE FROM social_posts_fts WHERE post_id = old.id;
         END;
         CREATE TRIGGER IF NOT EXISTS social_posts_fts_au AFTER UPDATE OF body ON social_posts BEGIN
            DELETE FROM social_posts_fts WHERE post_id = old.id;
            INSERT INTO social_posts_fts(post_id, body) VALUES (new.id, new.body);
         END;",
    );

    // Rebuild if empty (first boot or after table create).
    let fts_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM social_posts_fts", [], |r| r.get(0))
        .unwrap_or(0);
    if fts_count == 0 {
        match conn.execute(
            "INSERT INTO social_posts_fts(post_id, body)
             SELECT id, body FROM social_posts",
            [],
        ) {
            Ok(n) => {
                if n > 0 {
                    tracing::info!(rows = n, "rebuilt social_posts_fts from social_posts");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to rebuild social_posts_fts");
            }
        }
    }
}

fn migrate_v28(_conn: &Connection) {}
fn migrate_v29(_conn: &Connection) {}
fn migrate_v30(_conn: &Connection) {}
fn migrate_v31(_conn: &Connection) {}

fn migrate_v32(conn: &Connection) {
    let has_deleted_at: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('social_posts') WHERE name='deleted_at'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;

    if !has_deleted_at {
        conn.execute_batch("ALTER TABLE social_posts ADD COLUMN deleted_at TEXT DEFAULT NULL;")
            .expect("migration v32 failed");
        tracing::info!("applied migration v32: soft-delete for social_posts");
    }
    conn.execute_batch("UPDATE schema_version SET version = 32;")
        .ok();
}

fn migrate_v33(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS audit_log (
            id TEXT PRIMARY KEY, actor_id TEXT NOT NULL, actor_type TEXT NOT NULL DEFAULT 'user',
            action TEXT NOT NULL, target_type TEXT, target_id TEXT, details TEXT, ip_address TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_audit_log_actor ON audit_log(actor_id);
        CREATE INDEX IF NOT EXISTS idx_audit_log_created ON audit_log(created_at);
        UPDATE schema_version SET version = 33;",
    )
    .expect("migration v33 failed");
    tracing::info!("applied migration v33: audit_log table");
}

fn migrate_v34(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS deployment_adapters (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            adapter_type TEXT NOT NULL,
            environment TEXT NOT NULL,
            config_json TEXT NOT NULL DEFAULT '{}',
            status TEXT NOT NULL DEFAULT 'active',
            last_inspected_at INTEGER,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        );
        CREATE INDEX IF NOT EXISTS idx_deployment_adapters_user
            ON deployment_adapters(user_id, status);
        CREATE INDEX IF NOT EXISTS idx_deployment_adapters_type
            ON deployment_adapters(user_id, adapter_type);
        UPDATE schema_version SET version = 34;",
    )
    .expect("migration v34 failed");
    tracing::info!("applied migration v34: deployment_adapters table");
}

fn migrate_v35(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS user_api_keys (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            provider TEXT NOT NULL,
            encrypted_key TEXT NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            UNIQUE(user_id, provider)
        );
        CREATE INDEX IF NOT EXISTS idx_user_api_keys_user ON user_api_keys(user_id);
        UPDATE schema_version SET version = 35;",
    )
    .expect("migration v35 failed");
    tracing::info!("applied migration v35: user_api_keys table");
}

fn migrate_v36(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS user_budgets (
            user_id TEXT PRIMARY KEY,
            daily_budget REAL NOT NULL DEFAULT 5.0,
            weekly_budget REAL NOT NULL DEFAULT 25.0,
            monthly_budget REAL NOT NULL DEFAULT 100.0,
            notifications_enabled INTEGER NOT NULL DEFAULT 1,
            warning_threshold REAL NOT NULL DEFAULT 0.8,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        );

        CREATE TABLE IF NOT EXISTS cost_sessions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            provider TEXT NOT NULL,
            cost_type TEXT NOT NULL CHECK (cost_type IN ('byok', 'byos')),
            session_start INTEGER NOT NULL,
            session_end INTEGER,
            estimated_cost REAL NOT NULL DEFAULT 0.0,
            actual_cost REAL,
            tokens_in INTEGER NOT NULL DEFAULT 0,
            tokens_out INTEGER NOT NULL DEFAULT 0,
            model TEXT,
            created_at INTEGER NOT NULL DEFAULT (unixepoch())
        );
        CREATE INDEX IF NOT EXISTS idx_cost_sessions_user_time ON cost_sessions(user_id, session_start);
        CREATE INDEX IF NOT EXISTS idx_cost_sessions_provider ON cost_sessions(provider);

        CREATE TABLE IF NOT EXISTS cost_warnings (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            warning_type TEXT NOT NULL CHECK (warning_type IN ('daily', 'weekly', 'monthly')),
            threshold_percent REAL NOT NULL,
            current_cost REAL NOT NULL,
            budget_limit REAL NOT NULL,
            triggered_at INTEGER NOT NULL DEFAULT (unixepoch()),
            acknowledged_at INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_cost_warnings_user_time ON cost_warnings(user_id, triggered_at);
        CREATE INDEX IF NOT EXISTS idx_cost_warnings_type ON cost_warnings(warning_type);

        UPDATE schema_version SET version = 36;"
    ).expect("migration v36 failed");
    tracing::info!(
        "applied migration v36: cost tracking tables (user_budgets, cost_sessions, cost_warnings)"
    );
}

fn migrate_v37(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS project_workspaces (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            project_name TEXT NOT NULL,
            workspace_id TEXT NOT NULL,
            workspace_url TEXT NOT NULL,
            chat_endpoint TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'creating',
            metadata TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_project_workspaces_user ON project_workspaces(user_id, updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_project_workspaces_workspace ON project_workspaces(workspace_id);
        CREATE INDEX IF NOT EXISTS idx_project_workspaces_status ON project_workspaces(status);

        UPDATE schema_version SET version = 37;"
    ).expect("migration v37 failed");
    tracing::info!("applied migration v37: project_workspaces table for Replit integration");
}

fn migrate_v38(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS user_credentials (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            provider TEXT NOT NULL,
            credential_type TEXT NOT NULL,
            label TEXT,
            encrypted_data TEXT NOT NULL,
            email TEXT,
            is_default INTEGER NOT NULL DEFAULT 0,
            status TEXT NOT NULL DEFAULT 'active',
            last_used_at INTEGER,
            token_expires_at INTEGER,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        );

        CREATE INDEX IF NOT EXISTS idx_user_credentials_user ON user_credentials(user_id);
        CREATE INDEX IF NOT EXISTS idx_user_credentials_user_provider ON user_credentials(user_id, provider);
        CREATE INDEX IF NOT EXISTS idx_user_credentials_status ON user_credentials(user_id, status);

        INSERT OR IGNORE INTO user_credentials (id, user_id, provider, credential_type, encrypted_data, created_at)
            SELECT id, user_id, provider, 'api_key', encrypted_key, created_at FROM user_api_keys;

        UPDATE schema_version SET version = 38;"
    ).expect("migration v38 failed");
    tracing::info!("applied migration v38: user_credentials table for multi-credential system");
}

fn migrate_v39(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS user_containers (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL UNIQUE,
            container_id TEXT NOT NULL,
            provider TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'created',
            last_activity_at INTEGER NOT NULL DEFAULT (unixepoch()),
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        );

        CREATE INDEX IF NOT EXISTS idx_user_containers_user ON user_containers(user_id);
        CREATE INDEX IF NOT EXISTS idx_user_containers_status ON user_containers(status);

        UPDATE schema_version SET version = 39;",
    )
    .expect("migration v39 failed");
    tracing::info!("applied migration v39: user_containers table for Docker BYOS");
}

fn migrate_v40(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS credential_assignments (
            id TEXT PRIMARY KEY,
            credential_id TEXT NOT NULL,
            user_id TEXT NOT NULL,
            target_type TEXT NOT NULL,
            target_id TEXT,
            permissions TEXT,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            UNIQUE(credential_id, target_type, target_id)
        );

        CREATE INDEX IF NOT EXISTS idx_cred_assign_user ON credential_assignments(user_id);
        CREATE INDEX IF NOT EXISTS idx_cred_assign_cred ON credential_assignments(credential_id);
        CREATE INDEX IF NOT EXISTS idx_cred_assign_target ON credential_assignments(target_type, target_id);

        UPDATE schema_version SET version = 40;"
    ).expect("migration v40 failed");
    tracing::info!("applied migration v40: credential_assignments table");
}

fn migrate_v41(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS audit_log (
            id TEXT PRIMARY KEY, actor_id TEXT NOT NULL, actor_type TEXT NOT NULL DEFAULT 'user',
            action TEXT NOT NULL, target_type TEXT, target_id TEXT, details TEXT, ip_address TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_audit_log_action ON audit_log(action);
        CREATE INDEX IF NOT EXISTS idx_audit_log_created_desc ON audit_log(created_at DESC);
        UPDATE schema_version SET version = 41;",
    )
    .expect("migration v41 failed");
    tracing::info!("applied migration v41: audit_log indexes");
}

fn migrate_v42(conn: &Connection) {
    // GitHub repo imports — tracks repos a user has cloned into their BYOS
    // container, the import lifecycle status, and sync state. Supports multiple
    // repos per user (UNIQUE on user_id + repo_full_name).
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS github_imports (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            repo_id INTEGER NOT NULL,
            repo_full_name TEXT NOT NULL,
            default_branch TEXT NOT NULL DEFAULT 'main',
            clone_path TEXT NOT NULL,
            private INTEGER NOT NULL DEFAULT 0,
            status TEXT NOT NULL DEFAULT 'pending',
            progress INTEGER NOT NULL DEFAULT 0,
            stage TEXT NOT NULL DEFAULT 'queued',
            error TEXT,
            last_synced_at INTEGER,
            head_commit TEXT,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
            UNIQUE(user_id, repo_full_name)
        );

        CREATE INDEX IF NOT EXISTS idx_github_imports_user ON github_imports(user_id);
        CREATE INDEX IF NOT EXISTS idx_github_imports_status ON github_imports(status);

        UPDATE schema_version SET version = 42;",
    )
    .expect("migration v42 failed");
    tracing::info!("applied migration v42: github_imports table for repo import + sync");
}

fn migrate_v43(conn: &Connection) {
    // Secure agent API keys: store SHA-256 hash of hvak_ secrets; agent_key holds display prefix only.
    let has_col: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('social_linked_agents') WHERE name='agent_key_hash'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
    if !has_col {
        // Column may already exist on some forks; ignore duplicate-column errors.
        match conn.execute(
            "ALTER TABLE social_linked_agents ADD COLUMN agent_key_hash TEXT",
            [],
        ) {
            Ok(_) => tracing::info!("migration v43: added social_linked_agents.agent_key_hash"),
            Err(e) => {
                let msg = e.to_string();
                if !msg.contains("duplicate column") {
                    panic!("migration v43 failed adding agent_key_hash: {e}");
                }
            }
        }
    }
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_social_linked_agents_key_hash
            ON social_linked_agents(agent_key_hash)
            WHERE agent_key_hash IS NOT NULL AND agent_key_hash != '';
         UPDATE schema_version SET version = 43;",
    )
    .expect("migration v43 failed creating unique index");
    tracing::info!("applied migration v43: agent_key_hash for linked-agent bearer auth");
}

fn migrate_v44(conn: &Connection) {
    // Light view counts on posts (incremented on single-post open).
    let has_col: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('social_posts') WHERE name='view_count'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
    if !has_col {
        match conn.execute(
            "ALTER TABLE social_posts ADD COLUMN view_count INTEGER NOT NULL DEFAULT 0",
            [],
        ) {
            Ok(_) => tracing::info!("migration v44: added social_posts.view_count"),
            Err(e) => {
                let msg = e.to_string();
                if !msg.contains("duplicate column") {
                    panic!("migration v44 failed adding view_count: {e}");
                }
            }
        }
    }
    conn.execute_batch("UPDATE schema_version SET version = 44;")
        .expect("migration v44 failed marking version");
    tracing::info!("applied migration v44: social_posts.view_count");
}

fn migrate_v45(conn: &Connection) {
    // Brand Pages only (person = social_profiles; agent = social_linked_agents).
    // Idempotent CREATE IF NOT EXISTS; safe on re-run / fork DBs.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS social_pages (
            id TEXT PRIMARY KEY,
            owner_profile_id TEXT NOT NULL REFERENCES social_profiles(id),
            kind TEXT NOT NULL DEFAULT 'brand',
            slug TEXT NOT NULL,
            display_name TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            avatar_url TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_social_pages_slug ON social_pages(slug);
        CREATE INDEX IF NOT EXISTS idx_social_pages_owner ON social_pages(owner_profile_id);

        CREATE TABLE IF NOT EXISTS social_page_follows (
            id TEXT PRIMARY KEY,
            page_id TEXT NOT NULL REFERENCES social_pages(id),
            follower_profile_id TEXT NOT NULL REFERENCES social_profiles(id),
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_social_page_follows_pair
            ON social_page_follows(page_id, follower_profile_id);
        CREATE INDEX IF NOT EXISTS idx_social_page_follows_follower
            ON social_page_follows(follower_profile_id);
        CREATE INDEX IF NOT EXISTS idx_social_page_follows_page
            ON social_page_follows(page_id);

        UPDATE schema_version SET version = 45;",
    )
    .expect("migration v45 failed creating social_pages tables");
    tracing::info!("applied migration v45: social_pages + social_page_follows (brand Pages)");
}

fn migrate_v46(conn: &Connection) {
    // Guild membership roles foundation (owner | member).
    let _ = conn.execute(
        "ALTER TABLE social_community_memberships ADD COLUMN role TEXT NOT NULL DEFAULT 'member'",
        [],
    );
    let _ = conn.execute(
        "UPDATE social_community_memberships
         SET role = 'owner'
         WHERE EXISTS (
             SELECT 1 FROM social_communities sc
             WHERE sc.id = social_community_memberships.community_id
               AND sc.creator_profile_id = social_community_memberships.profile_id
         )",
        [],
    );
    conn.execute_batch("UPDATE schema_version SET version = 46;")
        .expect("migration v46 failed setting schema version");
    tracing::info!("applied migration v46: social_community_memberships.role");
}

fn migrate_v47(conn: &Connection) {
    // Wave 11b — Page-owned media shelves (empty containers only; no items yet).
    // owner_profile_id = steward person Page (v1 profile_id).
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS social_media_shelves (
            id TEXT PRIMARY KEY,
            owner_profile_id TEXT NOT NULL REFERENCES social_profiles(id),
            title TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_social_media_shelves_owner
            ON social_media_shelves(owner_profile_id, created_at DESC);

        UPDATE schema_version SET version = 47;",
    )
    .expect("migration v47 failed creating social_media_shelves");
    tracing::info!("applied migration v47: social_media_shelves (empty shelf foundation)");
}

fn migrate_v48(conn: &Connection) {
    // Wave 12a — Steward-gated agent policy flags (foundation; no auto-reply/follow runner yet).
    // Defaults false. Flags persist only — workers that act on them are not shipped.
    let has_auto_reply: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('social_linked_agents') WHERE name='auto_reply_enabled'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if has_auto_reply == 0 {
        conn.execute(
            "ALTER TABLE social_linked_agents ADD COLUMN auto_reply_enabled INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .expect("migration v48 failed adding auto_reply_enabled");
    }
    let has_auto_follow: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('social_linked_agents') WHERE name='auto_follow_enabled'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if has_auto_follow == 0 {
        conn.execute(
            "ALTER TABLE social_linked_agents ADD COLUMN auto_follow_enabled INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .expect("migration v48 failed adding auto_follow_enabled");
    }
    conn.execute_batch("UPDATE schema_version SET version = 48;")
        .expect("migration v48 failed setting schema version");
    tracing::info!(
        "applied migration v48: social_linked_agents auto_reply_enabled + auto_follow_enabled (policy foundation)"
    );
}

fn migrate_v49(conn: &Connection) {
    // Batch B1 — multi-user privacy prefs (persist Settings Privacy/Account controls).
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS social_profile_prefs (
            profile_id TEXT PRIMARY KEY,
            dm_policy TEXT NOT NULL DEFAULT 'verified',
            discoverable_by_contact INTEGER NOT NULL DEFAULT 0,
            show_in_search INTEGER NOT NULL DEFAULT 1,
            protected_posts INTEGER NOT NULL DEFAULT 0,
            profile_visibility TEXT NOT NULL DEFAULT 'public',
            allow_agent_dms INTEGER NOT NULL DEFAULT 0,
            allow_agent_mentions INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        UPDATE schema_version SET version = 49;",
    )
    .expect("migration v49 failed creating social_profile_prefs");
    tracing::info!("applied migration v49: social_profile_prefs (privacy/trust preferences)");
}

fn migrate_v50(conn: &Connection) {
    // Batch C — private guild invites (token_hash only; plaintext returned once on create).
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS social_community_invites (
            id TEXT PRIMARY KEY,
            community_id TEXT NOT NULL REFERENCES social_communities(id) ON DELETE CASCADE,
            created_by_profile_id TEXT NOT NULL REFERENCES social_profiles(id),
            token_hash TEXT NOT NULL UNIQUE,
            max_uses INTEGER,
            use_count INTEGER NOT NULL DEFAULT 0,
            expires_at TEXT,
            revoked_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_social_community_invites_community
            ON social_community_invites(community_id, created_at DESC);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_social_community_invites_token_hash
            ON social_community_invites(token_hash);

        UPDATE schema_version SET version = 50;",
    )
    .expect("migration v50 failed creating social_community_invites");
    tracing::info!("applied migration v50: social_community_invites (private guild invites)");
}

fn migrate_v51(conn: &Connection) {
    // Wave 14i — LiveSession model foundation (no RTMP/WHIP provider yet).
    // phase is real DB state: preview | scheduled | live | ended.
    // ingest_url / playback_url stay null until a provider is wired (Wave 14k).
    // go-live is allowed without provider URLs (honest offline player chrome on FE).
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS social_live_sessions (
            id TEXT PRIMARY KEY,
            owner_profile_id TEXT NOT NULL REFERENCES social_profiles(id),
            title TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            phase TEXT NOT NULL DEFAULT 'preview',
            ingest_url TEXT,
            playback_url TEXT,
            provider TEXT NOT NULL DEFAULT 'none',
            started_at TEXT,
            ended_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_social_live_sessions_owner
            ON social_live_sessions(owner_profile_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_social_live_sessions_phase
            ON social_live_sessions(phase, updated_at DESC);

        UPDATE schema_version SET version = 51;",
    )
    .expect("migration v51 failed creating social_live_sessions");
    tracing::info!(
        "applied migration v51: social_live_sessions (LiveSession model foundation; no provider)"
    );
}

fn migrate_v52(conn: &Connection) {
    // Wave 14m/n — x402 verify receipts (Social only; idempotent by key).
    // status: pending | verified | failed
    // Never store private keys — raw_response is facilitator JSON / notes only.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS social_x402_receipts (
            id TEXT PRIMARY KEY,
            idempotency_key TEXT NOT NULL UNIQUE,
            payload_hash TEXT NOT NULL,
            amount TEXT,
            network TEXT NOT NULL,
            status TEXT NOT NULL,
            mode TEXT NOT NULL DEFAULT 'shape_only',
            note TEXT,
            raw_response TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_social_x402_receipts_status
            ON social_x402_receipts(status, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_social_x402_receipts_hash
            ON social_x402_receipts(payload_hash);

        UPDATE schema_version SET version = 52;",
    )
    .expect("migration v52 failed creating social_x402_receipts");
    tracing::info!(
        "applied migration v52: social_x402_receipts (x402 verify receipts; pending|verified|failed)"
    );
}

fn migrate_v53(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS social_ws_tickets (
            token_hash TEXT PRIMARY KEY,
            clerk_user_id TEXT NOT NULL,
            expires_at INTEGER NOT NULL,
            consumed_at INTEGER,
            created_at INTEGER NOT NULL DEFAULT (unixepoch())
        );
        CREATE INDEX IF NOT EXISTS idx_social_ws_tickets_expiry
            ON social_ws_tickets(expires_at);

        UPDATE schema_version SET version = 53;",
    )
    .expect("migration v53 failed creating social_ws_tickets");
    tracing::info!("applied migration v53: single-use Socials WebSocket tickets");
}

fn migrate_v54(conn: &Connection) {
    conn.execute_batch("BEGIN IMMEDIATE;")
        .expect("migration v54 failed acquiring the migration lock");

    let result = (|| -> rusqlite::Result<bool> {
        // Another process may have completed v54 while this connection waited
        // for the write lock. Re-read under the lock before applying any DDL.
        let current = conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        if current >= 54 {
            return Ok(false);
        }

        let has_audience_column = conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM pragma_table_info('social_posts')
                 WHERE name = 'audience_profile_id'
            )",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        if !has_audience_column {
            conn.execute_batch("ALTER TABLE social_posts ADD COLUMN audience_profile_id TEXT;")?;
        }

        conn.execute_batch(
        "DROP TRIGGER IF EXISTS social_posts_visibility_insert;
        DROP TRIGGER IF EXISTS social_posts_visibility_update;
        DROP TRIGGER IF EXISTS social_posts_audience_insert;
        DROP TRIGGER IF EXISTS social_posts_audience_update;
        DROP TRIGGER IF EXISTS social_posts_reply_parent_immutable;
        DROP TRIGGER IF EXISTS social_posts_audience_with_replies_immutable;
        DROP TRIGGER IF EXISTS social_longform_visibility_insert;
        DROP TRIGGER IF EXISTS social_longform_visibility_update;
        DROP TRIGGER IF EXISTS pulse_drafts_visibility_insert;
        DROP TRIGGER IF EXISTS pulse_drafts_visibility_update;

        UPDATE social_posts SET visibility = 'author-only' WHERE visibility = 'private';
        UPDATE social_posts SET visibility = 'author-only'
         WHERE visibility NOT IN ('public', 'followers', 'mutuals', 'guild', 'circle', 'author-only');
        UPDATE social_posts SET visibility = 'guild' WHERE community_id IS NOT NULL;

        -- Replies are part of the root post's audience, not independent public
        -- objects. Carry the root audience owner, visibility, and Guild down
        -- every valid legacy reply tree before authorization begins using the
        -- new column.
        WITH RECURSIVE inherited(id, audience_profile_id, visibility, community_id, depth) AS (
            SELECT id, profile_id, visibility, community_id, 0
              FROM social_posts
             WHERE reply_to_post_id IS NULL
            UNION ALL
            SELECT child.id, parent.audience_profile_id,
                   CASE
                       WHEN child.visibility = 'public'
                         OR child.visibility = parent.visibility
                       THEN parent.visibility
                       ELSE 'author-only'
                   END,
                   parent.community_id, parent.depth + 1
              FROM social_posts child
              JOIN inherited parent ON child.reply_to_post_id = parent.id
             WHERE parent.depth < 1000
        )
        UPDATE social_posts
           SET audience_profile_id = (
                   SELECT inherited.audience_profile_id
                     FROM inherited
                    WHERE inherited.id = social_posts.id
               ),
               visibility = (
                   SELECT inherited.visibility
                     FROM inherited
                    WHERE inherited.id = social_posts.id
               ),
               community_id = (
                   SELECT inherited.community_id
                     FROM inherited
                    WHERE inherited.id = social_posts.id
               )
         WHERE id IN (SELECT id FROM inherited);

        -- Orphaned, cyclic, or pathologically deep legacy replies have no
        -- trustworthy audience root. Preserve them fail-closed for their own
        -- author instead of guessing and risking disclosure.
        UPDATE social_posts
           SET audience_profile_id = profile_id,
               visibility = 'author-only',
               community_id = NULL
         WHERE audience_profile_id IS NULL OR audience_profile_id = '';

        UPDATE social_longform SET visibility = 'author-only' WHERE visibility = 'private';
        UPDATE social_longform SET visibility = 'author-only'
         WHERE visibility NOT IN ('public', 'followers', 'mutuals', 'author-only');
        UPDATE pulse_drafts SET visibility = 'author-only' WHERE visibility = 'private';
        UPDATE pulse_drafts SET visibility = 'author-only'
         WHERE visibility NOT IN ('public', 'followers', 'mutuals', 'author-only');

        CREATE TRIGGER social_posts_visibility_insert
        BEFORE INSERT ON social_posts
        WHEN NEW.visibility NOT IN ('public', 'followers', 'mutuals', 'guild', 'circle', 'author-only')
        BEGIN SELECT RAISE(ABORT, 'invalid social post visibility'); END;
        CREATE TRIGGER social_posts_visibility_update
        BEFORE UPDATE OF visibility ON social_posts
        WHEN NEW.visibility NOT IN ('public', 'followers', 'mutuals', 'guild', 'circle', 'author-only')
        BEGIN SELECT RAISE(ABORT, 'invalid social post visibility'); END;
        CREATE TRIGGER social_posts_audience_insert
        BEFORE INSERT ON social_posts
        WHEN (
            NEW.reply_to_post_id IS NULL
            AND (
                NULLIF(NEW.audience_profile_id, '') IS NULL
                OR NEW.audience_profile_id != NEW.profile_id
            )
        ) OR (
            NEW.reply_to_post_id IS NOT NULL
            AND NOT EXISTS (
                SELECT 1
                  FROM social_posts parent
                 WHERE parent.id = NEW.reply_to_post_id
                   AND parent.id != NEW.id
                   AND NEW.audience_profile_id = parent.audience_profile_id
                   AND NEW.visibility = parent.visibility
                   AND NEW.community_id IS parent.community_id
            )
        )
        BEGIN SELECT RAISE(ABORT, 'invalid social reply audience'); END;
        CREATE TRIGGER social_posts_audience_update
        BEFORE UPDATE OF profile_id, reply_to_post_id, visibility, community_id, audience_profile_id
        ON social_posts
        WHEN (
            NEW.reply_to_post_id IS NULL
            AND (
                NULLIF(NEW.audience_profile_id, '') IS NULL
                OR NEW.audience_profile_id != NEW.profile_id
            )
        ) OR (
            NEW.reply_to_post_id IS NOT NULL
            AND NOT EXISTS (
                SELECT 1
                  FROM social_posts parent
                 WHERE parent.id = NEW.reply_to_post_id
                   AND parent.id != NEW.id
                   AND NEW.audience_profile_id = parent.audience_profile_id
                   AND NEW.visibility = parent.visibility
                   AND NEW.community_id IS parent.community_id
            )
        )
        BEGIN SELECT RAISE(ABORT, 'invalid social reply audience'); END;
        CREATE TRIGGER social_posts_reply_parent_immutable
        BEFORE UPDATE OF reply_to_post_id ON social_posts
        WHEN NEW.reply_to_post_id IS NOT OLD.reply_to_post_id
        BEGIN SELECT RAISE(ABORT, 'social reply parent is immutable'); END;
        CREATE TRIGGER social_posts_audience_with_replies_immutable
        BEFORE UPDATE OF profile_id, visibility, community_id, audience_profile_id ON social_posts
        WHEN EXISTS (
            SELECT 1 FROM social_posts child WHERE child.reply_to_post_id = OLD.id
        ) AND (
            NEW.profile_id IS NOT OLD.profile_id
            OR NEW.visibility IS NOT OLD.visibility
            OR NEW.community_id IS NOT OLD.community_id
            OR NEW.audience_profile_id IS NOT OLD.audience_profile_id
        )
        BEGIN SELECT RAISE(ABORT, 'social post audience with replies is immutable'); END;
        CREATE TRIGGER social_longform_visibility_insert
        BEFORE INSERT ON social_longform
        WHEN NEW.visibility NOT IN ('public', 'followers', 'mutuals', 'author-only')
        BEGIN SELECT RAISE(ABORT, 'invalid social longform visibility'); END;
        CREATE TRIGGER social_longform_visibility_update
        BEFORE UPDATE OF visibility ON social_longform
        WHEN NEW.visibility NOT IN ('public', 'followers', 'mutuals', 'author-only')
        BEGIN SELECT RAISE(ABORT, 'invalid social longform visibility'); END;
        CREATE TRIGGER pulse_drafts_visibility_insert
        BEFORE INSERT ON pulse_drafts
        WHEN NEW.visibility NOT IN ('public', 'followers', 'mutuals', 'author-only')
        BEGIN SELECT RAISE(ABORT, 'invalid Pulse draft visibility'); END;
        CREATE TRIGGER pulse_drafts_visibility_update
        BEFORE UPDATE OF visibility ON pulse_drafts
        WHEN NEW.visibility NOT IN ('public', 'followers', 'mutuals', 'author-only')
        BEGIN SELECT RAISE(ABORT, 'invalid Pulse draft visibility'); END;

        UPDATE schema_version SET version = 54;
        ",
        )?;
        Ok(true)
    })();

    match result {
        Ok(applied) => {
            conn.execute_batch("COMMIT;")
                .expect("migration v54 failed committing canonical Socials audiences");
            if applied {
                tracing::info!(
                    "applied migration v54: canonical post audiences and reply audience ownership"
                );
            }
        }
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK;");
            panic!("migration v54 failed adding canonical Socials post audiences: {error}");
        }
    }
}

fn migrate_v55(conn: &Connection) {
    conn.execute_batch("BEGIN IMMEDIATE;")
        .expect("migration v55 failed acquiring the migration lock");
    let result = (|| -> rusqlite::Result<bool> {
        let current = conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        if current >= 55 {
            return Ok(false);
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS social_follow_requests (
                id TEXT PRIMARY KEY,
                requester_profile_id TEXT NOT NULL REFERENCES social_profiles(id),
                target_profile_id TEXT NOT NULL REFERENCES social_profiles(id),
                status TEXT NOT NULL DEFAULT 'pending',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(requester_profile_id, target_profile_id)
            );
            CREATE INDEX IF NOT EXISTS idx_social_follow_requests_target
                ON social_follow_requests(target_profile_id, status, created_at DESC);
            CREATE INDEX IF NOT EXISTS idx_social_follow_requests_requester
                ON social_follow_requests(requester_profile_id, status, created_at DESC);
            CREATE TRIGGER IF NOT EXISTS social_follow_requests_status_insert
            BEFORE INSERT ON social_follow_requests
            WHEN NEW.status NOT IN ('pending', 'accepted', 'rejected')
            BEGIN SELECT RAISE(ABORT, 'invalid follow request status'); END;
            CREATE TRIGGER IF NOT EXISTS social_follow_requests_status_update
            BEFORE UPDATE OF status ON social_follow_requests
            WHEN NEW.status NOT IN ('pending', 'accepted', 'rejected')
            BEGIN SELECT RAISE(ABORT, 'invalid follow request status'); END;
            UPDATE schema_version SET version = 55;",
        )?;
        Ok(true)
    })();

    match result {
        Ok(applied) => {
            conn.execute_batch("COMMIT;")
                .expect("migration v55 failed committing follow requests");
            if applied {
                tracing::info!("applied migration v55: approval-based protected follows");
            }
        }
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK;");
            panic!("migration v55 failed adding follow requests: {error}");
        }
    }
}

fn social_direct_conversation_key(profile_a: &str, profile_b: &str) -> String {
    let (first, second) = if profile_a <= profile_b {
        (profile_a, profile_b)
    } else {
        (profile_b, profile_a)
    };
    let mut hasher = Sha256::new();
    hasher.update(b"heyvera-social-direct-conversation:v1:");
    hasher.update((first.len() as u64).to_be_bytes());
    hasher.update(first.as_bytes());
    hasher.update((second.len() as u64).to_be_bytes());
    hasher.update(second.as_bytes());
    hex::encode(hasher.finalize())
}

fn migrate_v56(conn: &Connection) {
    conn.execute_batch("BEGIN IMMEDIATE;")
        .expect("migration v56 failed acquiring the migration lock");
    let result = (|| -> rusqlite::Result<bool> {
        let current = conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        if current >= 56 {
            return Ok(false);
        }

        let has_column = |table: &str, column: &str| -> rusqlite::Result<bool> {
            conn.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2
                )",
                params![table, column],
                |row| row.get(0),
            )
        };

        if !has_column("social_messages", "sequence")? {
            conn.execute_batch(
                "ALTER TABLE social_messages
                    ADD COLUMN sequence INTEGER NOT NULL DEFAULT 0;",
            )?;
        }
        if !has_column("social_messages", "client_message_id")? {
            conn.execute_batch("ALTER TABLE social_messages ADD COLUMN client_message_id TEXT;")?;
        }
        if !has_column(
            "social_conversation_participants",
            "joined_message_sequence",
        )? {
            conn.execute_batch(
                "ALTER TABLE social_conversation_participants
                    ADD COLUMN joined_message_sequence INTEGER NOT NULL DEFAULT 0;",
            )?;
        }
        if !has_column(
            "social_conversation_participants",
            "last_read_message_sequence",
        )? {
            conn.execute_batch(
                "ALTER TABLE social_conversation_participants
                    ADD COLUMN last_read_message_sequence INTEGER NOT NULL DEFAULT 0;",
            )?;
        }
        if !has_column("social_conversation_participants", "last_read_at")? {
            conn.execute_batch(
                "ALTER TABLE social_conversation_participants ADD COLUMN last_read_at TEXT;",
            )?;
        }
        if !has_column("social_conversations", "direct_key")? {
            conn.execute_batch("ALTER TABLE social_conversations ADD COLUMN direct_key TEXT;")?;
        }
        if !has_column("social_conversations", "creation_key")? {
            conn.execute_batch("ALTER TABLE social_conversations ADD COLUMN creation_key TEXT;")?;
        }
        if !has_column("social_conversations", "creator_profile_id")? {
            conn.execute_batch(
                "ALTER TABLE social_conversations ADD COLUMN creator_profile_id TEXT;",
            )?;
        }

        // Collapse legacy duplicate 1:1 conversations before installing the
        // durable pair uniqueness constraint. Preserve every message by moving
        // it to the oldest canonical conversation; sequences are rebuilt below.
        let direct_rows: Vec<(String, String, String, String)> = {
            let mut stmt = conn.prepare(
                "SELECT c.id, MIN(cp.profile_id), MAX(cp.profile_id), c.created_at
                   FROM social_conversations c
                   JOIN social_conversation_participants cp ON cp.conversation_id = c.id
                  GROUP BY c.id
                 HAVING COUNT(*) = 2
                  ORDER BY c.created_at ASC, c.id ASC",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        let mut canonical_by_key: HashMap<String, String> = HashMap::new();
        for (conversation_id, profile_a, profile_b, _) in direct_rows {
            let direct_key = social_direct_conversation_key(&profile_a, &profile_b);
            if let Some(canonical_id) = canonical_by_key.get(&direct_key) {
                conn.execute(
                    "UPDATE social_messages SET conversation_id = ?1
                      WHERE conversation_id = ?2",
                    params![canonical_id, conversation_id],
                )?;
                conn.execute(
                    "DELETE FROM social_conversation_participants WHERE conversation_id = ?1",
                    params![conversation_id],
                )?;
                conn.execute(
                    "DELETE FROM social_conversations WHERE id = ?1",
                    params![conversation_id],
                )?;
            } else {
                conn.execute(
                    "UPDATE social_conversations SET direct_key = ?1 WHERE id = ?2",
                    params![direct_key, conversation_id],
                )?;
                canonical_by_key.insert(direct_key, conversation_id);
            }
        }

        conn.execute_batch(
            "WITH ranked AS (
                SELECT id,
                       ROW_NUMBER() OVER (
                           PARTITION BY conversation_id ORDER BY created_at ASC, id ASC
                       ) AS new_sequence
                  FROM social_messages
            )
            UPDATE social_messages
               SET sequence = (
                   SELECT new_sequence FROM ranked WHERE ranked.id = social_messages.id
               );

            -- The legacy global read bit is attributable only in a 1:1 DM.
            -- Group reads are intentionally reset rather than fabricating which
            -- participant saw them.
            UPDATE social_conversation_participants AS participant
               SET last_read_message_sequence = COALESCE((
                   SELECT MAX(message.sequence)
                     FROM social_messages message
                    WHERE message.conversation_id = participant.conversation_id
                      AND message.sender_profile_id != participant.profile_id
                      AND message.read = 1
                      AND 2 = (
                          SELECT COUNT(*)
                            FROM social_conversation_participants count_participant
                           WHERE count_participant.conversation_id = participant.conversation_id
                      )
               ), 0);

            UPDATE social_conversations
               SET updated_at = COALESCE((
                   SELECT MAX(message.created_at)
                     FROM social_messages message
                    WHERE message.conversation_id = social_conversations.id
               ), updated_at);

            DROP INDEX IF EXISTS idx_social_messages_conversation;
            CREATE UNIQUE INDEX IF NOT EXISTS idx_social_messages_conversation_sequence
                ON social_messages(conversation_id, sequence);
            CREATE INDEX IF NOT EXISTS idx_social_messages_conversation_created
                ON social_messages(conversation_id, created_at DESC, id DESC);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_social_messages_client_id
                ON social_messages(sender_profile_id, client_message_id)
                WHERE client_message_id IS NOT NULL;
            CREATE UNIQUE INDEX IF NOT EXISTS idx_social_conversations_direct_key
                ON social_conversations(direct_key) WHERE direct_key IS NOT NULL;
            CREATE UNIQUE INDEX IF NOT EXISTS idx_social_conversations_creation_key
                ON social_conversations(creator_profile_id, creation_key)
                WHERE creation_key IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_social_participants_unread
                ON social_conversation_participants(
                    profile_id, conversation_id, last_read_message_sequence
                );

            CREATE TRIGGER IF NOT EXISTS social_messages_integrity_insert
            BEFORE INSERT ON social_messages
            WHEN NEW.sequence <= 0
              OR NEW.client_message_id IS NULL
              OR length(NEW.client_message_id) < 8
              OR length(NEW.client_message_id) > 128
              OR length(NEW.content) < 1
              OR length(NEW.content) > 4000
              OR length(CAST(NEW.content AS BLOB)) > 16384
              OR instr(NEW.content, char(0)) != 0
            BEGIN SELECT RAISE(ABORT, 'invalid social message'); END;

            UPDATE schema_version SET version = 56;",
        )?;
        Ok(true)
    })();

    match result {
        Ok(applied) => {
            conn.execute_batch("COMMIT;")
                .expect("migration v56 failed committing Socials message integrity");
            if applied {
                tracing::info!(
                    "applied migration v56: per-participant DM reads, sequences, and idempotency"
                );
            }
        }
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK;");
            panic!("migration v56 failed adding Socials message integrity: {error}");
        }
    }
}

fn migrate_v57(conn: &Connection) {
    conn.execute_batch("BEGIN IMMEDIATE;")
        .expect("migration v57 failed acquiring the migration lock");
    let result = (|| -> rusqlite::Result<bool> {
        let current = conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        if current >= 57 {
            return Ok(false);
        }

        let has_activity_sequence: bool = conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM pragma_table_info('social_conversations')
                 WHERE name = 'activity_sequence'
            )",
            [],
            |row| row.get(0),
        )?;
        if !has_activity_sequence {
            conn.execute_batch(
                "ALTER TABLE social_conversations
                    ADD COLUMN activity_sequence INTEGER NOT NULL DEFAULT 0;",
            )?;
        }

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS social_conversation_activity_clock (
                singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                next_sequence INTEGER NOT NULL CHECK(next_sequence > 0)
             ) WITHOUT ROWID;

             WITH ranked AS (
                SELECT id,
                       ROW_NUMBER() OVER (
                           ORDER BY COALESCE(
                               julianday(updated_at), julianday(created_at), 0
                           ) ASC, id ASC
                       ) AS activity_sequence
                  FROM social_conversations
             )
             UPDATE social_conversations
                SET activity_sequence = (
                    SELECT ranked.activity_sequence
                      FROM ranked
                     WHERE ranked.id = social_conversations.id
                );

             INSERT INTO social_conversation_activity_clock(singleton, next_sequence)
             VALUES (
                 1,
                 (SELECT COALESCE(MAX(activity_sequence), 0) + 1
                    FROM social_conversations)
             )
             ON CONFLICT(singleton) DO UPDATE
                 SET next_sequence = MAX(
                     excluded.next_sequence,
                     social_conversation_activity_clock.next_sequence
                 );

             CREATE UNIQUE INDEX IF NOT EXISTS idx_social_conversations_activity
                 ON social_conversations(activity_sequence DESC, id DESC);
             CREATE INDEX IF NOT EXISTS idx_social_conv_participants_profile_conversation
                 ON social_conversation_participants(profile_id, conversation_id);
             UPDATE schema_version SET version = 57;",
        )?;
        Ok(true)
    })();

    match result {
        Ok(applied) => {
            conn.execute_batch("COMMIT;")
                .expect("migration v57 failed committing conversation activity clock");
            if applied {
                tracing::info!("applied migration v57: stable Socials conversation activity clock");
            }
        }
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK;");
            panic!("migration v57 failed adding the conversation activity clock: {error}");
        }
    }
}

fn migrate_v58(conn: &Connection) {
    conn.execute_batch("BEGIN IMMEDIATE;")
        .expect("migration v58 failed acquiring the migration lock");
    let result = (|| -> rusqlite::Result<bool> {
        let current = conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        if current >= 58 {
            return Ok(false);
        }

        conn.execute_batch(
            "UPDATE social_profile_prefs
                SET dm_policy = CASE lower(trim(dm_policy))
                    WHEN 'everyone' THEN 'everyone'
                    WHEN 'verified' THEN 'verified'
                    WHEN 'following' THEN 'following'
                    WHEN 'mutuals' THEN 'mutuals'
                    WHEN 'nobody' THEN 'nobody'
                    ELSE 'nobody'
                END;

             DROP TRIGGER IF EXISTS social_profile_prefs_dm_policy_insert;
             DROP TRIGGER IF EXISTS social_profile_prefs_dm_policy_update;
             CREATE TRIGGER social_profile_prefs_dm_policy_insert
             BEFORE INSERT ON social_profile_prefs
             WHEN NEW.dm_policy NOT IN ('everyone', 'verified', 'following', 'mutuals', 'nobody')
             BEGIN SELECT RAISE(ABORT, 'invalid social DM policy'); END;
             CREATE TRIGGER social_profile_prefs_dm_policy_update
             BEFORE UPDATE OF dm_policy ON social_profile_prefs
             WHEN NEW.dm_policy NOT IN ('everyone', 'verified', 'following', 'mutuals', 'nobody')
             BEGIN SELECT RAISE(ABORT, 'invalid social DM policy'); END;

             UPDATE schema_version SET version = 58;",
        )?;
        Ok(true)
    })();

    match result {
        Ok(applied) => {
            conn.execute_batch("COMMIT;")
                .expect("migration v58 failed committing typed DM policies");
            if applied {
                tracing::info!("applied migration v58: typed Socials DM consent policies");
            }
        }
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK;");
            panic!("migration v58 failed adding typed DM consent policies: {error}");
        }
    }
}

fn migrate_v59(conn: &Connection) {
    conn.execute_batch("BEGIN IMMEDIATE;")
        .expect("migration v59 failed acquiring the migration lock");
    let result = (|| -> rusqlite::Result<bool> {
        let current = conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        if current >= 59 {
            return Ok(false);
        }

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS social_message_request_clock (
                singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                next_sequence INTEGER NOT NULL CHECK(next_sequence > 0)
             ) WITHOUT ROWID;
             INSERT INTO social_message_request_clock(singleton, next_sequence)
             VALUES (1, 1) ON CONFLICT(singleton) DO NOTHING;

             CREATE TABLE IF NOT EXISTS social_message_requests (
                id TEXT PRIMARY KEY,
                sender_profile_id TEXT NOT NULL REFERENCES social_profiles(id) ON DELETE CASCADE,
                recipient_profile_id TEXT NOT NULL REFERENCES social_profiles(id) ON DELETE CASCADE,
                client_request_id TEXT NOT NULL,
                content TEXT NOT NULL,
                content_fingerprint TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'pending'
                    CHECK(state IN ('pending', 'accepted', 'declined', 'spam',
                                    'cancelled', 'expired', 'blocked')),
                bucket TEXT NOT NULL DEFAULT 'inbox'
                    CHECK(bucket IN ('inbox', 'spam')),
                risk_score INTEGER NOT NULL DEFAULT 0
                    CHECK(risk_score BETWEEN 0 AND 100),
                risk_reasons_json TEXT NOT NULL DEFAULT '[]',
                activity_sequence INTEGER NOT NULL CHECK(activity_sequence > 0),
                conversation_id TEXT REFERENCES social_conversations(id),
                accepted_message_id TEXT REFERENCES social_messages(id),
                created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                resolved_at TEXT,
                resolver_profile_id TEXT,
                CHECK(sender_profile_id != recipient_profile_id),
                CHECK(length(content_fingerprint) = 64
                    AND content_fingerprint NOT GLOB '*[^0-9a-f]*'),
                CHECK(json_valid(risk_reasons_json) AND json_type(risk_reasons_json) = 'array'),
                CHECK(
                    (state = 'pending' AND resolved_at IS NULL
                        AND resolver_profile_id IS NULL AND conversation_id IS NULL
                        AND accepted_message_id IS NULL)
                    OR (state = 'accepted' AND resolved_at IS NOT NULL
                        AND resolver_profile_id = recipient_profile_id
                        AND conversation_id IS NOT NULL AND accepted_message_id IS NOT NULL)
                    OR (state IN ('declined', 'spam') AND resolved_at IS NOT NULL
                        AND resolver_profile_id = recipient_profile_id
                        AND conversation_id IS NULL AND accepted_message_id IS NULL)
                    OR (state = 'blocked' AND resolved_at IS NOT NULL
                        AND resolver_profile_id IN (sender_profile_id, recipient_profile_id)
                        AND conversation_id IS NULL AND accepted_message_id IS NULL)
                    OR (state = 'cancelled' AND resolved_at IS NOT NULL
                        AND resolver_profile_id = sender_profile_id
                        AND conversation_id IS NULL AND accepted_message_id IS NULL)
                    OR (state = 'expired' AND resolved_at IS NOT NULL
                        AND resolver_profile_id IS NULL
                        AND conversation_id IS NULL AND accepted_message_id IS NULL)
                )
             );
             CREATE UNIQUE INDEX IF NOT EXISTS idx_social_message_requests_sender_client
                ON social_message_requests(sender_profile_id, client_request_id);
             CREATE UNIQUE INDEX IF NOT EXISTS idx_social_message_requests_pending_pair
                ON social_message_requests(sender_profile_id, recipient_profile_id)
                WHERE state = 'pending';
             CREATE UNIQUE INDEX IF NOT EXISTS idx_social_message_requests_activity
                ON social_message_requests(activity_sequence DESC, id DESC);
             CREATE INDEX IF NOT EXISTS idx_social_message_requests_recipient_inbox
                ON social_message_requests(recipient_profile_id, bucket, state,
                                           activity_sequence DESC, id DESC);
             CREATE INDEX IF NOT EXISTS idx_social_message_requests_sender_state
                ON social_message_requests(sender_profile_id, state, created_at DESC, id DESC);
             CREATE INDEX IF NOT EXISTS idx_social_message_requests_fingerprint_recent
                ON social_message_requests(sender_profile_id, content_fingerprint, created_at DESC);

             CREATE TRIGGER IF NOT EXISTS social_message_requests_integrity_insert
             BEFORE INSERT ON social_message_requests
             WHEN length(NEW.sender_profile_id) < 1 OR length(NEW.sender_profile_id) > 128
               OR length(NEW.recipient_profile_id) < 1 OR length(NEW.recipient_profile_id) > 128
               OR length(NEW.client_request_id) < 8 OR length(NEW.client_request_id) > 128
               OR length(NEW.content) < 1 OR length(NEW.content) > 4000
               OR length(CAST(NEW.content AS BLOB)) > 16384
               OR instr(NEW.content, char(0)) != 0
             BEGIN SELECT RAISE(ABORT, 'invalid social message request'); END;

             CREATE TRIGGER IF NOT EXISTS social_message_requests_immutable_update
             BEFORE UPDATE ON social_message_requests
             WHEN OLD.state != 'pending'
               OR NEW.id != OLD.id
               OR NEW.sender_profile_id != OLD.sender_profile_id
               OR NEW.recipient_profile_id != OLD.recipient_profile_id
               OR NEW.client_request_id != OLD.client_request_id
               OR NEW.content != OLD.content
               OR NEW.content_fingerprint != OLD.content_fingerprint
               OR NEW.bucket != OLD.bucket
               OR NEW.risk_score != OLD.risk_score
               OR NEW.risk_reasons_json != OLD.risk_reasons_json
               OR NEW.activity_sequence != OLD.activity_sequence
               OR NEW.created_at != OLD.created_at
             BEGIN SELECT RAISE(ABORT, 'immutable social message request'); END;

             CREATE TRIGGER IF NOT EXISTS social_message_requests_accept_links
             BEFORE UPDATE ON social_message_requests
             WHEN NEW.state = 'accepted' AND (
                NOT EXISTS(SELECT 1 FROM social_messages message
                    WHERE message.id = NEW.accepted_message_id
                      AND message.conversation_id = NEW.conversation_id
                      AND message.sender_profile_id = NEW.sender_profile_id
                      AND message.content = NEW.content)
                OR NOT EXISTS(SELECT 1 FROM social_conversation_participants participant
                    WHERE participant.conversation_id = NEW.conversation_id
                      AND participant.profile_id = NEW.sender_profile_id)
                OR NOT EXISTS(SELECT 1 FROM social_conversation_participants participant
                    WHERE participant.conversation_id = NEW.conversation_id
                      AND participant.profile_id = NEW.recipient_profile_id)
             ) BEGIN SELECT RAISE(ABORT, 'invalid accepted message request links'); END;

             CREATE TABLE IF NOT EXISTS social_direct_message_starts (
                sender_profile_id TEXT NOT NULL REFERENCES social_profiles(id) ON DELETE CASCADE,
                client_request_id TEXT NOT NULL,
                recipient_profile_id TEXT NOT NULL REFERENCES social_profiles(id) ON DELETE CASCADE,
                content_fingerprint TEXT NOT NULL CHECK(length(content_fingerprint) = 64
                    AND content_fingerprint NOT GLOB '*[^0-9a-f]*'),
                outcome_type TEXT NOT NULL CHECK(outcome_type IN ('request', 'message')),
                outcome_id TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                PRIMARY KEY(sender_profile_id, client_request_id)
             ) WITHOUT ROWID;
             CREATE INDEX IF NOT EXISTS idx_social_direct_message_starts_quota
                ON social_direct_message_starts(sender_profile_id, created_at DESC);

             CREATE TRIGGER IF NOT EXISTS social_direct_message_starts_outcome
             BEFORE INSERT ON social_direct_message_starts
             WHEN (NEW.outcome_type = 'request'
                    AND NOT EXISTS(SELECT 1 FROM social_message_requests WHERE id = NEW.outcome_id))
                OR (NEW.outcome_type = 'message'
                    AND NOT EXISTS(SELECT 1 FROM social_messages WHERE id = NEW.outcome_id))
             BEGIN SELECT RAISE(ABORT, 'invalid direct message start outcome'); END;

             UPDATE schema_version SET version = 59;",
        )?;
        Ok(true)
    })();
    match result {
        Ok(applied) => {
            conn.execute_batch("COMMIT;")
                .expect("migration v59 failed committing message requests");
            if applied {
                tracing::info!("applied migration v59: durable Socials message requests");
            }
        }
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK;");
            panic!("migration v59 failed adding Socials message requests: {error}");
        }
    }
}

fn migrate_v60(conn: &Connection) {
    // Credits become integers, and the transaction log becomes the authority.
    // See cortex/plan/CREDITS.md.
    //
    // Numbered v60 because fix/socials-message-integrity holds v53–v59; the
    // counter in schema_version cannot express out-of-order application, so
    // that branch merges first and this block keeps the tail.
    //
    // Money was REAL. Binary floating point cannot represent decimal fractions
    // exactly, so a mutable balance column drifts from the sum of its own
    // transaction log — and because the column was authoritative, that drift
    // was both invisible and unrecoverable. Whole credits, stored as INTEGER.
    //
    // The rebuild is safe: SQLite cannot ALTER a column type, and both tables
    // are empty in production. ROUND() is there for dev databases that may
    // hold fractional values; whole values round-trip exactly.
    //
    // credit_transactions gains idempotency_key UNIQUE. That single constraint
    // is the exactly-once mechanism — the orchestrator retries by design
    // (max_attempts 3, orphaned steps requeue on lease expiry), so without it a
    // step that charges and is then rejected for a stale lease_gen is charged
    // again. Same pattern as social_x402_receipts in v52.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS credit_balances_v60 (
            clerk_user_id          TEXT PRIMARY KEY,
            subscription_remaining INTEGER NOT NULL DEFAULT 200,
            subscription_total     INTEGER NOT NULL DEFAULT 200,
            pack_remaining         INTEGER NOT NULL DEFAULT 0,
            last_reset_at          TEXT
        );
        INSERT OR IGNORE INTO credit_balances_v60
            (clerk_user_id, subscription_remaining, subscription_total, pack_remaining, last_reset_at)
        SELECT clerk_user_id,
               CAST(ROUND(subscription_remaining) AS INTEGER),
               CAST(ROUND(subscription_total)     AS INTEGER),
               CAST(ROUND(pack_remaining)         AS INTEGER),
               last_reset_at
        FROM credit_balances;
        DROP TABLE credit_balances;
        ALTER TABLE credit_balances_v60 RENAME TO credit_balances;

        CREATE TABLE IF NOT EXISTS credit_transactions_v60 (
            id              TEXT PRIMARY KEY,
            clerk_user_id   TEXT NOT NULL,
            amount          INTEGER NOT NULL,
            balance_type    TEXT NOT NULL,
            reason          TEXT NOT NULL DEFAULT 'legacy',
            description     TEXT NOT NULL,
            idempotency_key TEXT NOT NULL UNIQUE,
            run_id          TEXT,
            step_id         TEXT,
            created_at      TEXT NOT NULL DEFAULT (datetime('now'))
        );
        INSERT OR IGNORE INTO credit_transactions_v60
            (id, clerk_user_id, amount, balance_type, reason, description, idempotency_key, created_at)
        SELECT id, clerk_user_id, CAST(ROUND(amount) AS INTEGER), balance_type,
               'legacy', description, 'legacy:' || id, created_at
        FROM credit_transactions;
        DROP TABLE credit_transactions;
        ALTER TABLE credit_transactions_v60 RENAME TO credit_transactions;

        CREATE INDEX IF NOT EXISTS idx_credit_transactions_user
            ON credit_transactions(clerk_user_id, created_at DESC);

        -- COGS. Token-denominated and internal; never a customer balance.
        -- Kept separate from credit_transactions on purpose: a credit priced as
        -- a function of tokens consumed is token resale with an exchange rate,
        -- which is the reading Anthropic's commercial terms D.4 prohibits.
        CREATE TABLE IF NOT EXISTS provider_spend (
            id               TEXT PRIMARY KEY,
            user_id          TEXT NOT NULL,
            run_id           TEXT,
            step_id          TEXT,
            provider         TEXT NOT NULL,
            model            TEXT NOT NULL,
            cost_type        TEXT NOT NULL,
            tokens_in        INTEGER NOT NULL DEFAULT 0,
            tokens_out       INTEGER NOT NULL DEFAULT 0,
            tokens_cached_in INTEGER NOT NULL DEFAULT 0,
            cost_micro_usd   INTEGER NOT NULL DEFAULT 0,
            created_at       INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_provider_spend_user
            ON provider_spend(user_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_provider_spend_run
            ON provider_spend(run_id);

        UPDATE schema_version SET version = 60;",
    )
    .expect("migration v60 failed converting credits to integers");
    tracing::info!(
        "applied migration v60: integer credits, idempotent credit_transactions, provider_spend"
    );
}

fn migrate_v61(conn: &Connection) {
    // V3 — verdicts become durable. See cortex/plan/VERIFIER.md ("Evidence and
    // receipts") for the canonical shape and V3-LAUNCH-SPEC.md for the wiring.
    //
    // Numbered v61 because v53–v59 belong to fix/socials-message-integrity and
    // v60 to fix/credit-metering-idempotency; schema_version is a single
    // counter, so both must merge before this one or a migration is silently
    // skipped.
    //
    // `verification_checks` carries three columns beyond the doc's sketch —
    // `spec_id`, `outcome`, `runner_image`. They are not embellishment: a row
    // must round-trip to `cortex_core::verification::CheckExecution`, which the
    // receipt endpoint serves and the frontend already types against. Without
    // spec_id the execution cannot be joined back to the spec it ran; without
    // outcome, Failed and TimedOut collapse into an exit code that NotExecuted
    // does not have at all.
    //
    // `verification_specs` freezes the derived checks at dispatch. Derivation
    // must happen before the worker sees the task, and verification happens
    // after delivery, so the specs have to survive the gap somewhere.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS verification_runs (
            id              TEXT PRIMARY KEY,
            run_id          TEXT NOT NULL,
            step_id         TEXT NOT NULL,
            attempt         INTEGER NOT NULL,
            tree_hash       TEXT NOT NULL,
            runner_image    TEXT NOT NULL,
            verdict         TEXT NOT NULL DEFAULT 'pending',
            started_at      INTEGER NOT NULL,
            finished_at     INTEGER,
            UNIQUE(run_id, step_id, attempt)
        );
        CREATE INDEX IF NOT EXISTS idx_verification_runs_run_step
            ON verification_runs(run_id, step_id);

        CREATE TABLE IF NOT EXISTS verification_checks (
            id              TEXT PRIMARY KEY,
            verification_id TEXT NOT NULL REFERENCES verification_runs(id),
            spec_id         TEXT NOT NULL,
            source          TEXT NOT NULL,
            command         TEXT NOT NULL,
            outcome         TEXT NOT NULL,
            exit_code       INTEGER,
            duration_ms     INTEGER,
            output_digest   TEXT NOT NULL,
            output_tail     TEXT NOT NULL,
            runner_image    TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_verification_checks_verification
            ON verification_checks(verification_id);

        CREATE TABLE IF NOT EXISTS verification_specs (
            run_id     TEXT NOT NULL,
            step_id    TEXT NOT NULL,
            specs_json TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (run_id, step_id)
        );

        UPDATE schema_version SET version = 61;",
    )
    .expect("migration v61 failed creating verification_runs/verification_checks");
    tracing::info!(
        "applied migration v61: verification_runs, verification_checks, verification_specs"
    );
}

fn migrate_v62(conn: &Connection) {
    // PR C — what actually ran, recorded at dispatch.
    //
    // Invariant 4 requires a receipt to name an immutable source tree, an
    // image, a resource profile, and the argv that produced it. None of that
    // was persisted anywhere: the worker built a command, ran it, and reported
    // an exit code. The model identity in particular was reconstructable only
    // from a routing decision that nothing joined to the delivery.
    //
    // Fields that nothing populates yet are columns here anyway — quote_id,
    // plan_receipt_id, effort, budgets, context bundle. They are NULL until
    // PRs F, K, and the context work fill them, and a column added later
    // cannot describe an attempt that has already run.
    //
    // Numbered v62 because schema_version is a single counter shared with the
    // HeyVera Socials product; v61 was the maximum on main when this was
    // written. A collision means whichever branch merges second has its
    // migration silently skipped, so re-check before claiming a number.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS execution_jobs (
            job_id              TEXT PRIMARY KEY,
            job_version         INTEGER NOT NULL,
            run_id              TEXT NOT NULL,
            step_id             TEXT NOT NULL,
            attempt_id          TEXT NOT NULL,
            lease_gen           INTEGER NOT NULL,

            model_catalog_id    TEXT NOT NULL,
            model_catalog_ver   TEXT,
            backend_kind        TEXT NOT NULL,
            effort_requested    TEXT,
            effort_applied      TEXT NOT NULL,

            token_budget        INTEGER,
            wall_clock_ms       INTEGER NOT NULL,
            max_tool_calls      INTEGER,

            network_policy      TEXT NOT NULL,
            capability_grants   TEXT NOT NULL DEFAULT '[]',
            context_bundle      TEXT,
            packed_bytes        INTEGER,

            quote_id            TEXT,
            plan_receipt_id     TEXT,

            image_ref           TEXT NOT NULL,
            isolation_class     TEXT NOT NULL,
            resource_profile    TEXT NOT NULL,
            profile_version     TEXT NOT NULL,

            submitted_at        INTEGER NOT NULL,

            -- One logical execution per attempt and lease generation. A
            -- resubmission under the same key is the same job and must not
            -- record a second sandbox; this mirrors the claim key already used
            -- on the dispatch path.
            UNIQUE(attempt_id, lease_gen)
        );
        CREATE INDEX IF NOT EXISTS idx_execution_jobs_step
            ON execution_jobs(run_id, step_id);

        UPDATE schema_version SET version = 62;",
    )
    .expect("migration v62 failed creating execution_jobs");
    tracing::info!("applied migration v62: execution_jobs");
}

fn migrate_v63(conn: &Connection) {
    // PR A — the truth model. A worker's report of success stops being the
    // thing that makes a step succeed.
    //
    // The backfill is the consequential line. Every existing `succeeded` step
    // becomes `delivered`, **not** `verified`: those steps were never
    // independently verified, and labelling them verified would assert a claim
    // about work already delivered to customers that we never checked. Anyone
    // reading old runs sees fewer verified steps than yesterday. The count did
    // not change; the honesty of the label did.
    //
    // It is forward-only. Reversing it means re-asserting a claim that was
    // never true, so rollback is the additive tables dropping and the status
    // column staying where it is.
    //
    // Numbered v63, not v62 as the brief anticipated: PR C landed first and
    // took v62. `schema_version` is a single counter shared with the HeyVera
    // Socials product, so a collision means whichever branch merges second has
    // its migration silently skipped. Re-check the maximum before claiming.
    conn.execute_batch(
        "UPDATE steps SET status = 'delivered' WHERE status = 'succeeded';
        UPDATE step_attempts SET status = 'delivered' WHERE status = 'succeeded';

        -- Where a step is in the verification lifecycle, keyed by the attempt
        -- that produced it. `lease_gen` is in the key because a verdict belongs
        -- to one attempt: a result arriving for a superseded attempt writes to
        -- its own row and can never move the live one.
        CREATE TABLE IF NOT EXISTS step_verification_state (
            step_id           TEXT NOT NULL,
            attempt_id        TEXT NOT NULL,
            lease_gen         INTEGER NOT NULL,
            state             TEXT NOT NULL,
            entered_at        INTEGER NOT NULL,
            version           INTEGER NOT NULL DEFAULT 0,
            terminal_reason   TEXT,
            PRIMARY KEY (step_id, attempt_id, lease_gen)
        );
        CREATE INDEX IF NOT EXISTS idx_step_verification_state_step
            ON step_verification_state(step_id, state);

        -- A human deciding to ship unverified work is legitimate and routine.
        -- It is recorded as its own fact, with an actor to hold responsible, a
        -- reason, and an expiry so it does not silently become permanent.
        CREATE TABLE IF NOT EXISTS manual_overrides (
            step_id      TEXT NOT NULL,
            attempt_id   TEXT NOT NULL,
            actor_id     TEXT NOT NULL,
            reason       TEXT NOT NULL,
            expires_at   INTEGER,
            created_at   INTEGER NOT NULL,
            PRIMARY KEY (step_id, attempt_id)
        );

        UPDATE schema_version SET version = 63;",
    )
    .expect("migration v63 failed creating the truth-model tables");
    tracing::info!("applied migration v63: step_verification_state, manual_overrides");
}

fn migrate_v64(conn: &Connection) {
    // PR C2 — what egress was actually enforced, not just what was requested.
    //
    // `network_policy` already records what the job asked for. These record
    // what held: the `host:port` set that survived intersecting the allowlist
    // with the capability grants and expanding registry names, and the image
    // that enforced it. "It could only reach the registry" is not a checkable
    // claim unless a reader can see which hosts and what was deciding.
    //
    // Both nullable, because rows written before this migration genuinely do
    // not have the values and a plausible default would be a claim nobody
    // verified. A job that ran under `Deny` records `'[]'` rather than NULL, so
    // "nothing was reachable" stays distinguishable from "not recorded".
    //
    // Numbered v64: v62 is PR C, v63 is PR A. `schema_version` is a single
    // counter shared with the HeyVera Socials product, so re-check the maximum
    // before claiming a number — whichever branch merges second has its
    // migration silently skipped.
    conn.execute_batch(
        "ALTER TABLE execution_jobs ADD COLUMN effective_egress TEXT;
        ALTER TABLE execution_jobs ADD COLUMN egress_mediator TEXT;

        UPDATE schema_version SET version = 64;",
    )
    .expect("migration v64 failed adding the egress enforcement columns");
    tracing::info!("applied migration v64: execution_jobs.effective_egress, egress_mediator");
}

fn migrate_v65(conn: &Connection) {
    // PR B — verification stops being fire-and-forget.
    //
    // A delivery used to hand its verification to `tokio::spawn`. A restart, a
    // deploy, an unavailable container runtime, or a panic and the verification
    // simply never happened: nothing retried it, nothing reconciled it, and
    // nothing surfaced that a delivered change was permanently stranded.
    //
    // The row is inserted in the same transaction that moves the step to
    // `verifying`. If the transition commits the job exists; if it rolls back
    // neither happened. That is the whole durability argument, and it is why
    // there is no public enqueue that could be called on its own.
    //
    // `UNIQUE(run_id, step_id, attempt_id, lease_gen)` is what makes "at most
    // one receipt, charge, or refund" true under concurrent enqueue and under
    // replay.
    //
    // Numbered v65: v62 is PR C, v63 is PR A, v64 is PR C2. `schema_version` is
    // one counter shared with the HeyVera Socials product — re-check the
    // maximum before claiming a number, because whichever branch merges second
    // has its migration silently skipped.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS verification_jobs (
            job_id             TEXT PRIMARY KEY,
            run_id             TEXT NOT NULL,
            step_id            TEXT NOT NULL,
            attempt_id         TEXT NOT NULL,
            lease_gen          INTEGER NOT NULL,

            -- Frozen inputs, not references that can drift. A job that
            -- re-resolved either of these at claim time could grade a tree the
            -- worker never delivered, against an exam nobody was promised.
            delivered_commit   TEXT NOT NULL,
            spec_set_id        TEXT NOT NULL,
            quote_id           TEXT,
            runner_policy_ver  TEXT NOT NULL,

            state              TEXT NOT NULL,
            claim_token        TEXT,
            claimed_at         INTEGER,
            heartbeat_at       INTEGER,
            lease_expires_at   INTEGER,
            attempt_count      INTEGER NOT NULL DEFAULT 0,
            next_run_at        INTEGER,
            terminal_reason    TEXT,

            created_at         INTEGER NOT NULL,
            updated_at         INTEGER NOT NULL,
            version            INTEGER NOT NULL DEFAULT 0,

            UNIQUE(run_id, step_id, attempt_id, lease_gen)
        );

        CREATE INDEX IF NOT EXISTS idx_verification_jobs_claimable
            ON verification_jobs(state, next_run_at);
        CREATE INDEX IF NOT EXISTS idx_verification_jobs_reclaim
            ON verification_jobs(state, lease_expires_at);

        UPDATE schema_version SET version = 65;",
    )
    .expect("migration v65 failed creating verification_jobs");

    // Backfill: every step sitting in `verifying` with no verdict is a delivery
    // the spawned path stranded. They get a job.
    //
    // Steps in `delivered` deliberately do **not**. Those are PR A's historical
    // rows — work that predates the verifier entirely — and re-verifying them
    // against a tree that has moved since would produce a verdict about
    // something nobody delivered.
    let backfilled = conn
        .execute(
            "INSERT INTO verification_jobs (
                job_id, run_id, step_id, attempt_id, lease_gen,
                delivered_commit, spec_set_id, quote_id, runner_policy_ver,
                state, attempt_count, next_run_at, created_at, updated_at, version
             )
             SELECT
                'backfill-' || s.id || '-' || svs.lease_gen,
                s.run_id, s.id, svs.attempt_id, svs.lease_gen,
                s.head_commit,
                -- Unknown: these were enqueued before a digest was recorded.
                -- Named rather than faked, so the dispatcher can see it never
                -- had a frozen exam to compare against.
                'unknown',
                NULL, 'backfill',
                'queued', 0, NULL,
                svs.entered_at, svs.entered_at, 0
             FROM steps s
             JOIN step_verification_state svs
               ON svs.step_id = s.id AND svs.state = 'verifying'
             WHERE s.status = 'verifying' AND s.head_commit IS NOT NULL",
            [],
        )
        .unwrap_or(0);

    tracing::info!(
        backfilled,
        "applied migration v65: verification_jobs (stranded verifying steps enqueued)"
    );
}

fn migrate_v66(conn: &Connection) {
    // PR I — the catalog, and the reason `quoted_credits` has been `None`.
    //
    // Nothing has ever persisted a per-step price, so `verification_driver`
    // records a verdict and then declines to touch the ledger with a warning.
    // That refusal is correct: inventing a price is never right. What was
    // missing is a price that is not invented.
    //
    // Two invariants shape every table below, and neither is decoration.
    //
    // **Invariant 11 — one fact, one table.** Model identity, price, capability
    // class and context window live in exactly one versioned catalog, and no
    // pricing code carries a hardcoded model name. Today `usage::model_rates`
    // is a `match` on substrings of model ids ("contains haiku"), which is that
    // invariant's exact prohibition. These tables are where those facts move to.
    //
    // **Invariant 23 — a shared artifact is immutable and versioned.** Price
    // lists are published, never edited; consumers pin a version; every receipt
    // names the version that ran. That is enforced here with triggers rather
    // than asserted in a doc comment, because the whole value of "this receipt
    // shows the price you were quoted" evaporates the first time somebody
    // corrects a typo in a published row.
    //
    // Numbered v66: the maximum on main was v65 (PR B). `schema_version` is one
    // counter shared with the HeyVera Socials product — re-check the maximum
    // before claiming a number, because whichever branch merges second has its
    // migration silently skipped.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS price_lists (
            id              TEXT PRIMARY KEY,
            -- Monotonic and unique. A consumer pins this integer, so it is the
            -- thing a receipt names.
            version         INTEGER NOT NULL UNIQUE,

            -- 'provisional' or 'committed'. Phase 31.3's graduation gate: a
            -- class is committed only once its measured pass rate and cost
            -- distribution clear a threshold. Until then it is quoted and
            -- labelled, never charged.
            status          TEXT NOT NULL,

            -- What one credit is worth, in micros. A published fact rather
            -- than a constant: it is the resolution of the whole price space,
            -- and a value too coarse for the spread collapses every class to
            -- the same number while every row still looks plausible.
            micros_per_credit INTEGER NOT NULL,

            -- How the numbers were arrived at, in prose, on the row. A price
            -- whose derivation lives in a commit message is a price nobody can
            -- audit two years later.
            basis           TEXT NOT NULL,

            published_at    INTEGER NOT NULL,
            published_by    TEXT NOT NULL
        );

        -- Per-model facts. This is invariant 11's 'one table': routing,
        -- estimation and reporting read model identity, price, context window
        -- and capability class from here and from nowhere else.
        CREATE TABLE IF NOT EXISTS price_list_models (
            price_list_id   TEXT NOT NULL REFERENCES price_lists(id),
            provider        TEXT NOT NULL,
            model_id        TEXT NOT NULL,

            -- Micros per 1k tokens: 1_000_000 micros = 1 USD. Integers, not
            -- floats. Money that round-trips through an f64 is money that
            -- disagrees with itself at the third decimal, and this number is
            -- multiplied by token counts in the millions.
            input_micros_per_1k   INTEGER NOT NULL,
            output_micros_per_1k  INTEGER NOT NULL,
            -- Basis points of the input rate charged for a cache read, so a
            -- 90% discount is 1000. Also an integer, same reason.
            cache_read_bp         INTEGER NOT NULL,

            context_window        INTEGER NOT NULL,
            capability_class      TEXT NOT NULL,

            PRIMARY KEY (price_list_id, provider, model_id)
        );

        -- Per-class prices. The customer-facing half: a credit price for a
        -- verified outcome of a given class, denominated in whole credits
        -- because a credit is a verified task and half a task is not a thing.
        CREATE TABLE IF NOT EXISTS price_list_task_classes (
            price_list_id   TEXT NOT NULL REFERENCES price_lists(id),
            -- `cortex_core::task_class::TaskClass::key()`.
            task_class      TEXT NOT NULL,

            quoted_credits  INTEGER NOT NULL,

            -- Per-class status, not just per-list. A list graduates one class
            -- at a time as evidence arrives, which is exactly what Phase 31.3
            -- describes and what a single list-level flag cannot express.
            status          TEXT NOT NULL,

            -- The evidence behind this number, so a graduation decision can be
            -- reviewed rather than trusted. Zero samples is the honest state
            -- for a seeded list and is visible as such.
            sample_count            INTEGER NOT NULL DEFAULT 0,
            measured_cost_micros    INTEGER,
            margin_bp               INTEGER NOT NULL,

            PRIMARY KEY (price_list_id, task_class)
        );

        -- Invariant 23, enforced rather than asserted.
        --
        -- Published, never edited. An UPDATE or DELETE on any published row is
        -- an error at the storage layer, so 'the price you were quoted' cannot
        -- be quietly changed after the fact by a migration, a support script,
        -- or a well-meaning correction. Republishing means a new version.
        CREATE TRIGGER IF NOT EXISTS price_lists_are_immutable
            BEFORE UPDATE ON price_lists
            BEGIN SELECT RAISE(ABORT,
                'price lists are published, never edited (invariant 23) — publish a new version');
            END;
        CREATE TRIGGER IF NOT EXISTS price_lists_are_not_deleted
            BEFORE DELETE ON price_lists
            BEGIN SELECT RAISE(ABORT,
                'a published price list cannot be deleted — receipts name it');
            END;
        CREATE TRIGGER IF NOT EXISTS price_list_models_are_immutable
            BEFORE UPDATE ON price_list_models
            BEGIN SELECT RAISE(ABORT,
                'price list models are published, never edited (invariant 23)');
            END;
        CREATE TRIGGER IF NOT EXISTS price_list_models_are_not_deleted
            BEFORE DELETE ON price_list_models
            BEGIN SELECT RAISE(ABORT,
                'a published price list model cannot be deleted — receipts name it');
            END;
        CREATE TRIGGER IF NOT EXISTS price_list_task_classes_are_immutable
            BEFORE UPDATE ON price_list_task_classes
            BEGIN SELECT RAISE(ABORT,
                'price list classes are published, never edited (invariant 23)');
            END;
        CREATE TRIGGER IF NOT EXISTS price_list_task_classes_are_not_deleted
            BEFORE DELETE ON price_list_task_classes
            BEGIN SELECT RAISE(ABORT,
                'a published price list class cannot be deleted — receipts name it');
            END;

        -- The quote frozen for one step, at dispatch.
        --
        -- Frozen for the same reason the check specs are: a price resolved at
        -- verdict time is a price the work could have influenced, and a
        -- customer who was quoted before execution must be charged what they
        -- were quoted. `price_list_version` is stored rather than joined so
        -- the receipt survives even if the list is somehow unreachable.
        --
        -- UNIQUE on (run_id, step_id) rather than per attempt: a retry does not
        -- get a new price. Cortex absorbing the cost of its own second attempt
        -- is the whole content of an outcome guarantee.
        CREATE TABLE IF NOT EXISTS step_quotes (
            quote_id            TEXT PRIMARY KEY,
            run_id              TEXT NOT NULL,
            step_id             TEXT NOT NULL,

            task_class          TEXT NOT NULL,
            quoted_credits      INTEGER NOT NULL,
            price_list_id       TEXT NOT NULL REFERENCES price_lists(id),
            price_list_version  INTEGER NOT NULL,

            -- Whether this quote may move the ledger.
            --
            -- Derived from the class's status at freeze time and stored, not
            -- recomputed. A class that graduates between dispatch and verdict
            -- must not retroactively make a quoted-but-not-billable step
            -- billable — the customer was told it was free.
            billable            INTEGER NOT NULL,

            frozen_at           INTEGER NOT NULL,

            UNIQUE(run_id, step_id)
        );

        CREATE INDEX IF NOT EXISTS idx_step_quotes_lookup
            ON step_quotes(run_id, step_id);

        UPDATE schema_version SET version = 66;",
    )
    .expect("migration v66 failed creating the price list catalog");

    tracing::info!("applied migration v66: versioned price list catalog + frozen step quotes");
}

/// The cost of every attempt at one step, successful or not.
///
/// `Default` is zero attempts and zero time, which is what a step with no
/// recorded attempts should report — the caller treats that as "no evidence"
/// rather than as "free".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptChain {
    pub attempts: u64,
    pub total_duration_ms: u64,
}

/// What a customer is shown when they ask why they were charged, and what a
/// dispute reads first.
///
/// Field names and shape are load-bearing: they are typed on the frontend at
/// `cortex/src/components/mission/Receipt.tsx` and must serialize to match.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Receipt {
    pub verification_id: String,
    pub run_id: String,
    pub step_id: String,
    pub attempt: i64,
    pub tree_hash: String,
    pub gate: VerdictReport,
    pub executions: Vec<CheckExecution>,
    /// What the sandbox could reach while this step ran.
    ///
    /// `None` for a step executed before scoped egress was recorded. `Some`
    /// with an empty `endpoints` means the sandbox reached nothing, which is a
    /// different fact and the one a reader should be able to rely on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub egress: Option<EgressReceipt>,
}

/// The egress half of a receipt: what was asked for, and what was opened.
///
/// Both, not one. `granted_registries` is the planner's decision and
/// `endpoints` is the intersection the sandbox actually enforced — if they ever
/// disagree, a reader can see it rather than having to trust that they cannot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EgressReceipt {
    /// Registry aliases the plan granted, e.g. `["crates"]`.
    pub granted_registries: Vec<String>,
    /// The provider the routing decision granted, e.g. `"claude"`, or `None`
    /// for a step that was given no route to a model API.
    ///
    /// Reported beside `granted_registries` rather than mixed into it, because
    /// the two were decided from different inputs and a reader needs to be able
    /// to tell which grant opened which host. A repository's manifests can add
    /// a registry here; nothing in a repository can add a provider.
    ///
    /// `None` on a step that ran before provider grants existed is
    /// indistinguishable from `None` on a step that was genuinely denied one —
    /// the same limit `endpoints` has, and the reason `endpoints` is `Vec` on
    /// an `Option<EgressReceipt>` rather than an `Option<Vec>` of its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub granted_provider: Option<String>,
    /// `host:port` entries the sandbox was actually opened to. Empty means no
    /// network at all.
    ///
    /// The union of both grants: this is the honest answer to "what could this
    /// task reach", so it must not be the ecosystem half alone.
    pub endpoints: Vec<String>,
    /// The mediator image that enforced it, or `None` when nothing was opened
    /// and no mediator was stood up.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mediator_image: Option<String>,
}

// Enum <-> TEXT mapping for the verification tables. These are spelled out
// rather than routed through serde so the stored strings are a deliberate
// schema decision: renaming a Rust variant must not silently rewrite what is
// already on disk.

fn check_source_str(source: CheckSource) -> &'static str {
    match source {
        CheckSource::Ecosystem => "ecosystem",
        CheckSource::Contract => "contract",
        CheckSource::Risk => "risk",
    }
}

fn check_outcome_str(outcome: CheckOutcome) -> &'static str {
    match outcome {
        CheckOutcome::Passed => "passed",
        CheckOutcome::Failed => "failed",
        CheckOutcome::TimedOut => "timed_out",
        CheckOutcome::NotExecuted => "not_executed",
    }
}

/// Unknown text reads back as `NotExecuted`, which is the only outcome that
/// cannot bill. A corrupted row must not be able to manufacture a charge.
fn check_outcome_from_str(s: &str) -> CheckOutcome {
    match s {
        "passed" => CheckOutcome::Passed,
        "failed" => CheckOutcome::Failed,
        "timed_out" => CheckOutcome::TimedOut,
        _ => CheckOutcome::NotExecuted,
    }
}

pub(crate) fn verdict_str(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Verified => "verified",
        Verdict::Failed => "failed",
        Verdict::Inconclusive => "inconclusive",
        Verdict::Unverified => "unverified",
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct AuditEntry {
    pub id: String,
    pub user_id: String,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub metadata: Option<String>,
    pub ip_address: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UserContainer {
    pub id: String,
    pub user_id: String,
    pub container_id: String,
    pub provider: String,
    pub status: String,
    pub last_activity_at: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GithubImport {
    pub id: String,
    pub user_id: String,
    pub repo_id: i64,
    pub repo_full_name: String,
    pub default_branch: String,
    pub clone_path: String,
    pub private: bool,
    /// Lifecycle: pending | importing | ready | failed
    pub status: String,
    /// 0..100 progress for UI feedback.
    pub progress: i64,
    /// Human-readable stage label (e.g. "cloning", "ready").
    pub stage: String,
    pub error: Option<String>,
    pub last_synced_at: Option<i64>,
    pub head_commit: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CortexGroup {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub description: String,
    pub members: i64,
    pub accent: String,
    pub source: String,
    pub external_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct CortexAuthorityScope {
    pub id: String,
    pub owner_user_id: String,
    pub kind: String,
    pub name: String,
    pub description: String,
    pub source: String,
    pub external_id: Option<String>,
    pub status: String,
    pub policy: serde_json::Value,
    pub role: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct CortexAuthorityResource {
    pub id: String,
    pub scope_id: String,
    pub resource_type: String,
    pub resource_key: String,
    pub access: String,
    pub policy: serde_json::Value,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CortexApprovalRequest {
    pub id: String,
    pub group_id: String,
    pub task_id: Option<String>,
    pub step_id: Option<String>,
    pub conversation_id: Option<String>,
    pub run_id: Option<String>,
    pub ask_type: String,
    pub status: String,
    pub title: String,
    pub body: String,
    pub priority: String,
    pub requested_by: String,
    pub decision: Option<serde_json::Value>,
    pub created_at: i64,
    pub updated_at: i64,
    pub resolved_at: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct IntegrationConnection {
    pub id: String,
    pub provider: String,
    pub external_id: Option<String>,
    pub display_name: String,
    pub status: String,
    pub scopes: Vec<String>,
    pub metadata: serde_json::Value,
    pub last_sync_at: Option<String>,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct IntegrationMapping {
    pub id: String,
    pub provider: String,
    pub group_id: String,
    pub external_id: String,
    pub external_name: String,
    pub mapping_type: String,
    pub metadata: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
}

// --- Database implementation ---

fn json_text<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a str> {
    object.get(key).and_then(|value| value.as_str())
}

fn index_group_task_state(
    conn: &Connection,
    user_id: &str,
    group_id: &str,
    state: &serde_json::Value,
) {
    if let Err(err) = index_group_task_state_checked(conn, user_id, group_id, state) {
        tracing::warn!(
            user_id = user_id,
            group_id = group_id,
            error = %err,
            "failed to index group task state"
        );
    }
}

fn index_group_task_state_checked(
    conn: &Connection,
    user_id: &str,
    group_id: &str,
    state: &serde_json::Value,
) -> rusqlite::Result<()> {
    let Some(tasks) = state.get("tasks").and_then(|value| value.as_array()) else {
        conn.execute(
            "DELETE FROM cortex_tasks WHERE user_id = ?1 AND group_id = ?2",
            params![user_id, group_id],
        )?;
        return Ok(());
    };

    let mut seen_ids = HashSet::new();
    for task in tasks {
        let Some(object) = task.as_object() else {
            continue;
        };
        let Some(id) = json_text(object, "id") else {
            continue;
        };
        let Some(title) = json_text(object, "title") else {
            continue;
        };
        let Some(status) = json_text(object, "status") else {
            continue;
        };
        if json_text(object, "groupId") != Some(group_id) {
            continue;
        }

        seen_ids.insert(id.to_string());
        let priority = json_text(object, "priority");
        let conversation_id = json_text(object, "projectChatConversationId");
        let latest_run_id = json_text(object, "latestRunId");
        let source_json = serde_json::to_string(task).unwrap_or_else(|_| "{}".to_string());

        conn.execute(
            "INSERT INTO cortex_tasks (
                id, user_id, group_id, title, status, priority, conversation_id,
                latest_run_id, source_json, created_at, updated_at, version
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, datetime('now'), datetime('now'), 1)
             ON CONFLICT(user_id, group_id, id) DO UPDATE SET
                title = excluded.title,
                status = excluded.status,
                priority = excluded.priority,
                conversation_id = excluded.conversation_id,
                latest_run_id = excluded.latest_run_id,
                source_json = excluded.source_json,
                updated_at = datetime('now'),
                version = cortex_tasks.version + 1",
            params![
                id,
                user_id,
                group_id,
                title,
                status,
                priority,
                conversation_id,
                latest_run_id,
                source_json
            ],
        )?;

        if let Some(conversation_id) = conversation_id {
            conn.execute(
                "INSERT OR IGNORE INTO cortex_task_chats (
                    user_id, group_id, task_id, conversation_id, attached_at
                 )
                 VALUES (?1, ?2, ?3, ?4, datetime('now'))",
                params![user_id, group_id, id, conversation_id],
            )?;
        }
    }

    let existing_ids = {
        let mut stmt =
            conn.prepare("SELECT id FROM cortex_tasks WHERE user_id = ?1 AND group_id = ?2")?;
        let results: Vec<_> = stmt
            .query_map(params![user_id, group_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        results
    };

    for existing_id in existing_ids {
        if !seen_ids.contains(&existing_id) {
            conn.execute(
                "DELETE FROM cortex_tasks WHERE user_id = ?1 AND group_id = ?2 AND id = ?3",
                params![user_id, group_id, existing_id],
            )?;
        }
    }

    Ok(())
}

fn attach_cortex_task_chat_tx(
    conn: &Connection,
    user_id: &str,
    group_id: &str,
    task_id: &str,
    conversation_id: &str,
    run_id: Option<&str>,
) -> bool {
    if let Err(err) = attach_cortex_task_chat_tx_checked(
        conn,
        user_id,
        group_id,
        task_id,
        conversation_id,
        run_id,
    ) {
        tracing::warn!(
            user_id = user_id,
            group_id = group_id,
            task_id = task_id,
            conversation_id = conversation_id,
            error = %err,
            "failed to attach cortex task chat"
        );
        return false;
    }
    true
}

fn attach_cortex_task_chat_tx_checked(
    conn: &Connection,
    user_id: &str,
    group_id: &str,
    task_id: &str,
    conversation_id: &str,
    run_id: Option<&str>,
) -> rusqlite::Result<()> {
    let now = Utc::now().timestamp_millis();
    let rows = if let Some(run_id) = run_id {
        conn.execute(
            "UPDATE cortex_tasks
             SET latest_run_id = ?1,
                 conversation_id = ?2,
                 updated_at = datetime('now'),
                 version = version + 1
             WHERE user_id = ?3 AND group_id = ?4 AND id = ?5",
            params![run_id, conversation_id, user_id, group_id, task_id],
        )?
    } else {
        conn.execute(
            "UPDATE cortex_tasks
             SET conversation_id = ?1,
                 updated_at = datetime('now'),
                 version = version + 1
             WHERE user_id = ?2 AND group_id = ?3 AND id = ?4",
            params![conversation_id, user_id, group_id, task_id],
        )?
    };
    if rows == 0 {
        return Err(rusqlite::Error::QueryReturnedNoRows);
    }

    conn.execute(
        "INSERT INTO cortex_task_chats (
            user_id, group_id, task_id, conversation_id, attached_at
         )
         VALUES (?1, ?2, ?3, ?4, datetime('now'))
         ON CONFLICT(user_id, group_id, task_id, conversation_id) DO UPDATE SET
            attached_at = datetime('now')",
        params![user_id, group_id, task_id, conversation_id],
    )?;

    try_insert_operations_event(
        conn,
        Some(user_id),
        Some(group_id),
        None,
        Some(task_id),
        run_id,
        None,
        None,
        "chat.attached",
        "chat",
        conversation_id,
        &serde_json::json!({
            "conversation_id": conversation_id,
            "group_id": group_id,
            "task_id": task_id,
            "run_id": run_id,
            "attached_at": now,
        }),
    )?;
    Ok(())
}

fn attach_run_to_cortex_task(
    conn: &Connection,
    user_id: &str,
    group_id: Option<&str>,
    task_id: Option<&str>,
    conversation_id: Option<&str>,
    run_id: &str,
) {
    let (Some(group_id), Some(task_id)) = (group_id, task_id) else {
        return;
    };

    if let Some(conversation_id) = conversation_id {
        attach_cortex_task_chat_tx(
            conn,
            user_id,
            group_id,
            task_id,
            conversation_id,
            Some(run_id),
        );
    } else {
        conn.execute(
            "UPDATE cortex_tasks
             SET latest_run_id = ?1,
                 updated_at = datetime('now'),
                 version = version + 1
             WHERE user_id = ?2 AND group_id = ?3 AND id = ?4",
            params![run_id, user_id, group_id, task_id],
        )
        .ok();
    }
}

fn attach_run_to_cortex_task_tx_checked(
    conn: &Connection,
    user_id: &str,
    group_id: Option<&str>,
    task_id: Option<&str>,
    conversation_id: Option<&str>,
    run_id: &str,
) -> rusqlite::Result<bool> {
    let (Some(group_id), Some(task_id)) = (group_id, task_id) else {
        return Ok(false);
    };

    if let Some(conversation_id) = conversation_id {
        match attach_cortex_task_chat_tx_checked(
            conn,
            user_id,
            group_id,
            task_id,
            conversation_id,
            Some(run_id),
        ) {
            Ok(()) => Ok(true),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
            Err(err) => Err(err),
        }
    } else {
        let rows = conn.execute(
            "UPDATE cortex_tasks
             SET latest_run_id = ?1,
                 updated_at = datetime('now'),
                 version = version + 1
             WHERE user_id = ?2 AND group_id = ?3 AND id = ?4",
            params![run_id, user_id, group_id, task_id],
        )?;
        Ok(rows > 0)
    }
}

fn insert_operations_event(
    conn: &Connection,
    actor_user_id: Option<&str>,
    scope_id: Option<&str>,
    project_id: Option<&str>,
    task_id: Option<&str>,
    run_id: Option<&str>,
    step_id: Option<&str>,
    attempt_id: Option<&str>,
    event_type: &str,
    entity_type: &str,
    entity_id: &str,
    payload: &serde_json::Value,
) {
    if let Err(err) = try_insert_operations_event(
        conn,
        actor_user_id,
        scope_id,
        project_id,
        task_id,
        run_id,
        step_id,
        attempt_id,
        event_type,
        entity_type,
        entity_id,
        payload,
    ) {
        tracing::warn!(
            event_type = event_type,
            entity_type = entity_type,
            entity_id = entity_id,
            error = %err,
            "failed to record operations event"
        );
    }
}

fn try_insert_operations_event(
    conn: &Connection,
    actor_user_id: Option<&str>,
    scope_id: Option<&str>,
    project_id: Option<&str>,
    task_id: Option<&str>,
    run_id: Option<&str>,
    step_id: Option<&str>,
    attempt_id: Option<&str>,
    event_type: &str,
    entity_type: &str,
    entity_id: &str,
    payload: &serde_json::Value,
) -> rusqlite::Result<()> {
    let payload_json = serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string());
    conn.execute(
        "INSERT INTO operations_events (
            id, created_at, actor_user_id, scope_id, project_id, task_id, run_id, step_id,
            attempt_id, event_type, entity_type, entity_id, payload_json
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            Uuid::new_v4().to_string(),
            Utc::now().timestamp_millis(),
            actor_user_id,
            scope_id,
            project_id,
            task_id,
            run_id,
            step_id,
            attempt_id,
            event_type,
            entity_type,
            entity_id,
            payload_json,
        ],
    )?;
    Ok(())
}

struct CortexCompletionGate {
    gated_done: bool,
    reason: &'static str,
    run_id: Option<String>,
    run_status: Option<String>,
    total_steps: i64,
    verified_pass_steps: i64,
    failed_steps: i64,
    unverified_steps: i64,
}

impl CortexCompletionGate {
    fn to_json(&self, raw_done: bool) -> serde_json::Value {
        serde_json::json!({
            "gated_done": self.gated_done,
            "raw_done": raw_done,
            "reason": self.reason,
            "run_id": self.run_id,
            "run_status": self.run_status,
            "steps": {
                "total": self.total_steps,
                "verified_pass": self.verified_pass_steps,
                "failed": self.failed_steps,
                "unverified": self.unverified_steps,
            },
        })
    }
}

fn cortex_completion_gate(
    conn: &Connection,
    user_id: &str,
    group_id: &str,
    task_id: &str,
    latest_run_id: Option<&str>,
) -> CortexCompletionGate {
    let run: Option<(String, String)> = if let Some(run_id) = latest_run_id {
        conn.query_row(
            "SELECT id, status FROM runs
             WHERE id = ?1 AND user_id = ?2 AND group_id = ?3 AND task_id = ?4",
            params![run_id, user_id, group_id, task_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok()
    } else {
        conn.query_row(
            "SELECT id, status FROM runs
             WHERE user_id = ?1 AND group_id = ?2 AND task_id = ?3
             ORDER BY created_at DESC, id DESC
             LIMIT 1",
            params![user_id, group_id, task_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok()
    };

    let Some((run_id, run_status)) = run else {
        return CortexCompletionGate {
            gated_done: false,
            reason: "no_run",
            run_id: None,
            run_status: None,
            total_steps: 0,
            verified_pass_steps: 0,
            failed_steps: 0,
            unverified_steps: 0,
        };
    };

    let (total_steps, verified_pass_steps, failed_steps, unverified_steps): (i64, i64, i64, i64) =
        conn.query_row(
            "WITH latest_reports AS (
                SELECT vr.*
                FROM verifier_reports vr
                JOIN (
                    SELECT step_id, MAX(created_at) AS created_at
                    FROM verifier_reports
                    WHERE run_id = ?1
                    GROUP BY step_id
                ) latest
                    ON latest.step_id = vr.step_id
                   AND latest.created_at = vr.created_at
                WHERE vr.run_id = ?1
            )
            SELECT
                COUNT(*),
                COALESCE(SUM(CASE
                    WHEN s.status = 'verified'
                     AND s.verifier_report_id = lr.id
                     AND s.lease_gen = lr.lease_gen
                     AND lr.status = 'verified'
                     AND lr.verdict = 'pass'
                    THEN 1 ELSE 0
                END), 0),
                COALESCE(SUM(CASE WHEN s.status IN ('failed', 'orphaned') THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE
                    WHEN lr.id IS NULL OR NOT (
                        s.status = 'verified'
                        AND s.verifier_report_id = lr.id
                        AND s.lease_gen = lr.lease_gen
                        AND lr.status = 'verified'
                        AND lr.verdict = 'pass'
                    )
                    THEN 1 ELSE 0
                END), 0)
             FROM steps s
             LEFT JOIN latest_reports lr ON lr.step_id = s.id
             WHERE s.run_id = ?1",
            params![run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap_or((0, 0, 0, 0));

    let reason = if !matches!(run_status.as_str(), "succeeded" | "recovered") {
        "run_not_successful"
    } else if total_steps == 0 {
        "no_steps"
    } else if failed_steps > 0 {
        "failed_steps"
    } else if unverified_steps > 0 {
        "unverified_steps"
    } else {
        "passed"
    };

    CortexCompletionGate {
        gated_done: reason == "passed",
        reason,
        run_id: Some(run_id),
        run_status: Some(run_status),
        total_steps,
        verified_pass_steps,
        failed_steps,
        unverified_steps,
    }
}

fn resource_lease_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ResourceLease> {
    let metadata_raw: String = row.get(18)?;
    Ok(ResourceLease {
        id: row.get(0)?,
        user_id: row.get(1)?,
        authority_scope_id: row.get(2)?,
        group_id: row.get(3)?,
        task_id: row.get(4)?,
        run_id: row.get(5)?,
        step_id: row.get(6)?,
        holder_type: row.get(7)?,
        resource_type: row.get(8)?,
        repo_key: row.get(9)?,
        resource_key: row.get(10)?,
        mode: row.get(11)?,
        status: row.get(12)?,
        lease_gen: row.get(13)?,
        acquired_at: row.get(14)?,
        expires_at: row.get(15)?,
        released_at: row.get(16)?,
        reason: row.get(17)?,
        metadata: serde_json::from_str(&metadata_raw).unwrap_or_else(|_| serde_json::json!({})),
    })
}

fn resource_conflict_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ResourceLeaseConflict> {
    Ok(ResourceLeaseConflict {
        lease_id: row.get(0)?,
        run_id: row.get(1)?,
        step_id: row.get(2)?,
        holder_type: row.get(3)?,
        resource_type: row.get(4)?,
        repo_key: row.get(5)?,
        resource_key: row.get(6)?,
        mode: row.get(7)?,
        expires_at: row.get(8)?,
    })
}

fn json_i64(value: &serde_json::Value, path: &[&str]) -> i64 {
    let mut current = value;
    for key in path {
        let Some(next) = current.get(*key) else {
            return 0;
        };
        current = next;
    }
    current
        .as_i64()
        .unwrap_or_else(|| current.as_u64().map(|value| value as i64).unwrap_or(0))
}

fn append_group_scoped_items(
    target: &mut Vec<serde_json::Value>,
    summary: &serde_json::Value,
    field: &str,
    group_id: &str,
) {
    let Some(items) = summary.get(field).and_then(|value| value.as_array()) else {
        return;
    };
    for item in items {
        let mut item = item.clone();
        if let Some(object) = item.as_object_mut() {
            object
                .entry("group_id".to_string())
                .or_insert_with(|| serde_json::Value::String(group_id.to_string()));
        }
        target.push(item);
    }
}

fn json_sort_timestamp(value: &serde_json::Value, preferred_key: &str) -> i64 {
    [preferred_key, "updated_at", "created_at", "expires_at"]
        .into_iter()
        .filter_map(|key| value.get(key))
        .find_map(|value| {
            value.as_i64().or_else(|| {
                value.as_str().and_then(|raw| {
                    chrono::DateTime::parse_from_rfc3339(raw)
                        .ok()
                        .map(|parsed| parsed.timestamp_millis())
                })
            })
        })
        .unwrap_or(0)
}

fn sort_json_array_desc(items: &mut [serde_json::Value], preferred_key: &str) {
    items.sort_by(|left, right| {
        json_sort_timestamp(right, preferred_key).cmp(&json_sort_timestamp(left, preferred_key))
    });
}

fn authority_access_allows(actual: &str, required: &str) -> bool {
    matches!(
        (actual, required),
        ("admin", _) | ("write", "write") | ("write", "read") | ("read", "read") | ("owner", _)
    )
}

fn authority_role_allows(role: &str, required: &str) -> bool {
    matches!(
        (role, required),
        ("owner", _) | ("admin", _) | ("member", "read") | ("member", "write") | ("viewer", "read")
    )
}

fn path_keys_overlap(a: &str, b: &str) -> bool {
    a == "."
        || b == "."
        || a == b
        || a.strip_prefix(b)
            .is_some_and(|suffix| suffix.starts_with('/'))
        || b.strip_prefix(a)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn find_resource_lease_conflict_tx(
    conn: &Connection,
    user_id: &str,
    authority_scope_id: Option<&str>,
    request: &ResourceLeaseRequest,
    now: i64,
) -> Result<Option<ResourceLeaseConflict>, rusqlite::Error> {
    let candidates: Vec<ResourceLeaseConflict> = if let Some(authority_scope_id) =
        authority_scope_id
    {
        let mut stmt = conn.prepare(
            "SELECT id, run_id, step_id, holder_type, resource_type, repo_key, resource_key, mode, expires_at
             FROM resource_leases
             WHERE authority_scope_id = ?1
               AND status = 'active'
               AND expires_at > ?2
               AND resource_type = ?3
               AND repo_key = ?4
               AND mode IN ('write', 'exclusive')
             ORDER BY acquired_at ASC",
        )?;
        let rows = stmt.query_map(
            params![
                authority_scope_id,
                now,
                request.resource_type,
                request.repo_key
            ],
            resource_conflict_from_row,
        )?;
        rows.filter_map(|row| row.ok()).collect()
    } else {
        let mut stmt = conn.prepare(
            "SELECT id, run_id, step_id, holder_type, resource_type, repo_key, resource_key, mode, expires_at
             FROM resource_leases
             WHERE user_id = ?1
               AND authority_scope_id IS NULL
               AND status = 'active'
               AND expires_at > ?2
               AND resource_type = ?3
               AND repo_key = ?4
               AND mode IN ('write', 'exclusive')
             ORDER BY acquired_at ASC",
        )?;
        let rows = stmt.query_map(
            params![user_id, now, request.resource_type, request.repo_key],
            resource_conflict_from_row,
        )?;
        rows.filter_map(|row| row.ok()).collect()
    };

    for candidate in candidates {
        let conflicts = if request.resource_type == "path" {
            path_keys_overlap(&candidate.resource_key, &request.resource_key)
        } else {
            candidate.resource_key == request.resource_key
        };
        if conflicts {
            return Ok(Some(candidate));
        }
    }

    Ok(None)
}

fn acquire_run_resource_leases_tx(
    conn: &Connection,
    user_id: &str,
    authority_scope_id: Option<&str>,
    group_id: Option<&str>,
    task_id: Option<&str>,
    run_id: &str,
    requests: &[ResourceLeaseRequest],
    now: i64,
) -> Result<(), CreateRunError> {
    let expires_at = now + RUN_RESOURCE_LEASE_TTL_MS;

    conn.execute(
        "UPDATE resource_leases
         SET status = 'expired', released_at = ?1
         WHERE status = 'active' AND expires_at <= ?1",
        params![now],
    )
    .ok();

    for request in requests {
        if let Some(conflict) =
            find_resource_lease_conflict_tx(conn, user_id, authority_scope_id, request, now)
                .map_err(|err| CreateRunError::Database(err.to_string()))?
        {
            return Err(CreateRunError::ResourceConflict(conflict));
        }
    }

    for request in requests {
        let id = Uuid::new_v4().to_string();
        let metadata_json =
            serde_json::to_string(&request.metadata).unwrap_or_else(|_| "{}".to_string());
        conn.execute(
            "INSERT INTO resource_leases (
                id, user_id, authority_scope_id, group_id, task_id, run_id, step_id,
                holder_type, resource_type, repo_key, resource_key, mode, status,
                lease_gen, acquired_at, expires_at, reason, metadata_json
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, 'run', ?7, ?8, ?9, ?10, 'active', 1, ?11, ?12, ?13, ?14)",
            params![
                id,
                user_id,
                authority_scope_id,
                group_id,
                task_id,
                run_id,
                request.resource_type,
                request.repo_key,
                request.resource_key,
                request.mode,
                now,
                expires_at,
                request.reason,
                metadata_json,
            ],
        )
        .map_err(|err| CreateRunError::Database(err.to_string()))?;

        insert_operations_event(
            conn,
            Some(user_id),
            group_id,
            None,
            task_id,
            Some(run_id),
            None,
            None,
            "resource_lease.acquired",
            "resource_lease",
            &id,
            &serde_json::json!({
                "holder_type": "run",
                "authority_scope_id": authority_scope_id,
                "resource_type": request.resource_type,
                "repo_key": request.repo_key,
                "resource_key": request.resource_key,
                "mode": request.mode,
                "expires_at": expires_at,
                "reason": request.reason,
            }),
        );
    }

    Ok(())
}

fn run_repo_key_from_resource_leases(requests: &[ResourceLeaseRequest]) -> Option<&str> {
    requests
        .iter()
        .find(|request| request.repo_key != "default")
        .or_else(|| requests.first())
        .map(|request| request.repo_key.as_str())
}

fn authority_scope_id_from_context(authority_context: Option<&serde_json::Value>) -> Option<&str> {
    authority_context
        .and_then(|value| value.get("scope_id"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
}

fn release_resource_leases_for_run_tx_checked(
    conn: &Connection,
    run_id: &str,
    now: i64,
) -> rusqlite::Result<Vec<ResourceLease>> {
    let leases: Vec<ResourceLease> = conn
        .prepare(
            "SELECT id, user_id, authority_scope_id, group_id, task_id, run_id, step_id, holder_type, resource_type,
                    repo_key, resource_key, mode, status, lease_gen, acquired_at, expires_at,
                    released_at, reason, metadata_json
             FROM resource_leases
             WHERE run_id = ?1 AND status = 'active'",
        )?
        .query_map(params![run_id], resource_lease_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    if leases.is_empty() {
        return Ok(leases);
    }

    conn.execute(
        "UPDATE resource_leases
         SET status = 'released', released_at = ?1
         WHERE run_id = ?2 AND status = 'active'",
        params![now, run_id],
    )?;

    for lease in &leases {
        try_insert_operations_event(
            conn,
            Some(&lease.user_id),
            lease.group_id.as_deref(),
            None,
            lease.task_id.as_deref(),
            Some(run_id),
            lease.step_id.as_deref(),
            None,
            "resource_lease.released",
            "resource_lease",
            &lease.id,
            &serde_json::json!({
                "holder_type": lease.holder_type,
                "authority_scope_id": lease.authority_scope_id,
                "resource_type": lease.resource_type,
                "repo_key": lease.repo_key,
                "resource_key": lease.resource_key,
                "mode": lease.mode,
                "released_at": now,
            }),
        )?;
    }

    Ok(leases)
}

struct OperationEventContext {
    run_id: String,
    user_id: String,
    group_id: Option<String>,
    task_id: Option<String>,
}

fn run_event_context(conn: &Connection, run_id: &str) -> Option<OperationEventContext> {
    conn.query_row(
        "SELECT id, user_id, group_id, task_id
         FROM runs
         WHERE id = ?1",
        params![run_id],
        |row| {
            Ok(OperationEventContext {
                run_id: row.get(0)?,
                user_id: row.get(1)?,
                group_id: row.get(2)?,
                task_id: row.get(3)?,
            })
        },
    )
    .ok()
}

fn step_event_context(conn: &Connection, step_id: &str) -> Option<OperationEventContext> {
    conn.query_row(
        "SELECT s.run_id, r.user_id, r.group_id, r.task_id
         FROM steps s
         JOIN runs r ON r.id = s.run_id
         WHERE s.id = ?1",
        params![step_id],
        |row| {
            Ok(OperationEventContext {
                run_id: row.get(0)?,
                user_id: row.get(1)?,
                group_id: row.get(2)?,
                task_id: row.get(3)?,
            })
        },
    )
    .ok()
}

/// Write the verification-lifecycle row for one attempt, with the version CAS.
///
/// The row is keyed by `(step_id, attempt_id, lease_gen)` — a verdict belongs to
/// exactly one attempt, so a result for a superseded attempt writes to its own
/// row and can never reach the live one.
///
/// `from` is the set of states this transition may leave. When it is non-empty
/// the update is guarded by it and `version` advances, which is the CAS: two
/// concurrent transitions read the same state, the first advances it, and the
/// second matches no rows and fails. `from` is empty only for the first state
/// of an attempt, where there is nothing to advance from.
///
/// Takes `&Connection` so a `&Transaction` coerces in — every caller is inside
/// one, because a lifecycle row written without its projection describes a
/// state that never existed.
fn upsert_verification_state(
    conn: &Connection,
    step_id: &str,
    attempt_id: &str,
    lease_gen: i64,
    state: &str,
    reason: Option<&str>,
    entered_at: i64,
    from: &[&str],
) -> Result<(), String> {
    if from.is_empty() {
        conn.execute(
            "INSERT INTO step_verification_state
                 (step_id, attempt_id, lease_gen, state, entered_at, version, terminal_reason)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)
             ON CONFLICT(step_id, attempt_id, lease_gen) DO UPDATE SET
                 state = excluded.state,
                 entered_at = excluded.entered_at,
                 terminal_reason = excluded.terminal_reason,
                 version = step_verification_state.version + 1",
            params![step_id, attempt_id, lease_gen, state, entered_at, reason],
        )
        .map_err(|e| e.to_string())?;
        return Ok(());
    }

    let placeholders = std::iter::repeat_n("?", from.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "INSERT INTO step_verification_state
             (step_id, attempt_id, lease_gen, state, entered_at, version, terminal_reason)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)
         ON CONFLICT(step_id, attempt_id, lease_gen) DO UPDATE SET
             state = excluded.state,
             entered_at = excluded.entered_at,
             terminal_reason = excluded.terminal_reason,
             version = step_verification_state.version + 1
         WHERE step_verification_state.state IN ({placeholders})"
    );
    let mut args: Vec<&dyn rusqlite::ToSql> = vec![
        &step_id,
        &attempt_id,
        &lease_gen,
        &state,
        &entered_at,
        &reason,
    ];
    for source in from {
        args.push(source);
    }
    let rows = conn
        .execute(&sql, args.as_slice())
        .map_err(|e| e.to_string())?;
    if rows == 0 {
        return Err(format!(
            "lifecycle row for step {step_id} attempt {attempt_id} gen {lease_gen} \
             is not in {from:?}; refusing to move it to {state}"
        ));
    }
    Ok(())
}

fn insert_run_operations_event(
    conn: &Connection,
    run_id: &str,
    event_type: &str,
    payload: &serde_json::Value,
) {
    let context = run_event_context(conn, run_id);
    insert_operations_event(
        conn,
        context.as_ref().map(|context| context.user_id.as_str()),
        context
            .as_ref()
            .and_then(|context| context.group_id.as_deref()),
        None,
        context
            .as_ref()
            .and_then(|context| context.task_id.as_deref()),
        Some(run_id),
        None,
        None,
        event_type,
        "run",
        run_id,
        payload,
    );
}

fn insert_step_operations_event(
    conn: &Connection,
    step_id: &str,
    event_type: &str,
    payload: &serde_json::Value,
) {
    let context = step_event_context(conn, step_id);
    insert_operations_event(
        conn,
        context.as_ref().map(|context| context.user_id.as_str()),
        context
            .as_ref()
            .and_then(|context| context.group_id.as_deref()),
        None,
        context
            .as_ref()
            .and_then(|context| context.task_id.as_deref()),
        context.as_ref().map(|context| context.run_id.as_str()),
        Some(step_id),
        None,
        event_type,
        "step",
        step_id,
        payload,
    );
}

impl Database {
    /// Take the database lock, recovering the guard if a previous holder panicked.
    ///
    /// `std::sync::Mutex` poisons on panic, so taking this lock as
    /// `lock().unwrap()` turned any single panic anywhere in the crate into a
    /// permanent, process-wide database outage: every subsequent call would
    /// unwrap a `PoisonError` and panic in turn. The connection itself survives —
    /// SQLite statements are atomic, and `rusqlite` rolls a `Transaction` back
    /// when it is dropped during unwinding — so poisoning bought nothing here
    /// except the outage.
    ///
    /// The field is private precisely so this is the only way to reach it.
    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock_recovering()
    }

    pub fn open(path: &Path) -> Self {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let conn = Connection::open(path).expect("failed to open database");

        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             PRAGMA temp_store = MEMORY;",
        )
        .expect("failed to set pragmas");

        apply_migrations(&conn);

        let db = Self {
            conn: Mutex::new(conn),
        };
        db.seed_price_list_if_absent();
        db.publish_gateway_model_revision_if_needed();
        db
    }

    /// Publish version 1 if nothing is published yet.
    ///
    /// Seeding here rather than in the migration is deliberate. A migration is
    /// SQL and the seed is a computation over `TaskClass::all()` and a modelled
    /// spend function — expressing it as literal `INSERT`s would put 88 prices
    /// into a migration where they cannot be unit tested and cannot be
    /// regenerated when the class space changes.
    ///
    /// Idempotent, and it never touches an existing list. Invariant 23 means a
    /// published list cannot be edited anyway — the triggers would refuse — so
    /// the only two outcomes are "published version 1" and "left alone".
    fn seed_price_list_if_absent(&self) {
        if self.active_price_list().is_some() {
            return;
        }
        let list = crate::pricing::seed_provisional(
            1,
            "cortex:seed",
            Utc::now().timestamp(),
            crate::pricing::seed_models(),
        );
        match self.publish_price_list(&list) {
            Ok(()) => tracing::info!(
                version = list.version,
                classes = list.classes.len(),
                "published the provisional price list — every class quotes and none charges"
            ),
            Err(e) => tracing::error!(
                error = %e,
                "could not publish the seed price list; steps will dispatch without a quote"
            ),
        }
    }

    /// Existing installations already have immutable price-list v1. Publish a
    /// new version beside it when that historical seed predates the model ids
    /// the router actually emits; never append to or rewrite the old list.
    fn publish_gateway_model_revision_if_needed(&self) {
        const ROUTED_CLAUDE_MODELS: &[&str] =
            &["claude-haiku-4-5", "claude-sonnet-4-6", "claude-opus-4-6"];
        let Some(mut list) = self.active_price_list() else {
            return;
        };
        let missing = ROUTED_CLAUDE_MODELS
            .iter()
            .filter(|model| list.model("claude", model).is_none())
            .copied()
            .collect::<Vec<_>>();
        if missing.is_empty() {
            return;
        }
        let seed = crate::pricing::seed_models();
        for model_id in missing {
            let Some(model) = seed
                .iter()
                .find(|model| model.provider == "claude" && model.model_id == model_id)
            else {
                tracing::error!(model_id, "routed model has no provisional seed rate");
                return;
            };
            list.models.push(model.clone());
        }
        list.id = Uuid::new_v4().to_string();
        list.version += 1;
        list.published_at = Utc::now().timestamp();
        list.published_by = "cortex:gateway-model-revision".into();
        list.basis = format!(
            "{}; adds provisional rows for routed Claude model ids without editing prior versions",
            list.basis
        );
        if let Err(error) = self.publish_price_list(&list) {
            tracing::error!(%error, "could not publish gateway-compatible model revision");
        }
    }

    // --- Schema info ---

    pub fn schema_version(&self) -> i64 {
        let conn = self.conn();
        conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0)
    }

    pub fn list_run_operations_events(&self, run_id: &str, limit: usize) -> Vec<OperationsEvent> {
        let conn = self.conn();
        let limit = limit.clamp(1, 500) as i64;
        let mut stmt = conn
            .prepare(
                "SELECT id, created_at, actor_user_id, scope_id, project_id, task_id, run_id,
                    step_id, attempt_id, event_type, entity_type, entity_id, payload_json
             FROM operations_events
             WHERE run_id = ?1
             ORDER BY created_at ASC, id ASC
             LIMIT ?2",
            )
            .unwrap();

        stmt.query_map(params![run_id, limit], |row| {
            let payload_json: String = row.get(12)?;
            Ok(OperationsEvent {
                id: row.get(0)?,
                created_at: row.get(1)?,
                actor_user_id: row.get(2)?,
                scope_id: row.get(3)?,
                project_id: row.get(4)?,
                task_id: row.get(5)?,
                run_id: row.get(6)?,
                step_id: row.get(7)?,
                attempt_id: row.get(8)?,
                event_type: row.get(9)?,
                entity_type: row.get(10)?,
                entity_id: row.get(11)?,
                payload: serde_json::from_str(&payload_json)
                    .unwrap_or_else(|_| serde_json::json!({})),
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn list_deployment_operations_events(&self, limit: usize) -> Vec<OperationsEvent> {
        let conn = self.conn();
        let limit = limit.clamp(1, 100) as i64;
        let mut stmt = conn
            .prepare(
                "SELECT id, created_at, actor_user_id, scope_id, project_id, task_id, run_id,
                    step_id, attempt_id, event_type, entity_type, entity_id, payload_json
             FROM operations_events
             WHERE scope_id = ?1
                AND entity_type = ?2
             ORDER BY created_at DESC, id DESC
             LIMIT ?3",
            )
            .unwrap();

        stmt.query_map(params!["cortex", "deployment", limit], |row| {
            let payload_json: String = row.get(12)?;
            Ok(OperationsEvent {
                id: row.get(0)?,
                created_at: row.get(1)?,
                actor_user_id: row.get(2)?,
                scope_id: row.get(3)?,
                project_id: row.get(4)?,
                task_id: row.get(5)?,
                run_id: row.get(6)?,
                step_id: row.get(7)?,
                attempt_id: row.get(8)?,
                event_type: row.get(9)?,
                entity_type: row.get(10)?,
                entity_id: row.get(11)?,
                payload: serde_json::from_str(&payload_json)
                    .unwrap_or_else(|_| serde_json::json!({})),
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn get_run_binding(
        &self,
        run_id: &str,
    ) -> Option<(Option<String>, Option<String>, Option<String>)> {
        let conn = self.conn();
        conn.query_row(
            "SELECT task_id, group_id, conversation_id FROM runs WHERE id = ?1",
            params![run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .ok()
    }

    // --- Conversations ---

    pub fn create_conversation(&self, user_id: &str, title: Option<&str>) -> Conversation {
        let conn = self.conn();
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        conn.execute(
            "INSERT INTO conversations (id, user_id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, user_id, title, now, now],
        ).expect("failed to insert conversation");

        Conversation {
            id,
            user_id: user_id.to_string(),
            title: title.map(String::from),
            created_at: now.clone(),
            updated_at: now,
        }
    }

    pub fn list_conversations(
        &self,
        user_id: &str,
        limit: i64,
        offset: i64,
    ) -> (Vec<ConversationSummary>, i64) {
        let conn = self.conn();

        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM conversations WHERE user_id = ?1",
                params![user_id],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let mut stmt = conn.prepare(
            "SELECT c.id, c.title, c.updated_at,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as msg_count,
                    (SELECT m.content FROM messages m WHERE m.conversation_id = c.id ORDER BY m.created_at DESC LIMIT 1) as last_msg
             FROM conversations c
             WHERE c.user_id = ?1
             ORDER BY c.updated_at DESC
             LIMIT ?2 OFFSET ?3"
        ).unwrap();

        let conversations = stmt
            .query_map(params![user_id, limit, offset], |row| {
                let preview: Option<String> = row.get(4)?;
                Ok(ConversationSummary {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    updated_at: row.get(2)?,
                    message_count: row.get(3)?,
                    last_message_preview: preview.map(|s| {
                        if s.len() > 100 {
                            format!("{}...", &s[..97])
                        } else {
                            s
                        }
                    }),
                })
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        (conversations, total)
    }

    pub fn get_conversation(
        &self,
        conversation_id: &str,
        user_id: &str,
    ) -> Option<ConversationWithMessages> {
        let conn = self.conn();

        let conversation = conn.query_row(
            "SELECT id, user_id, title, created_at, updated_at FROM conversations WHERE id = ?1 AND user_id = ?2",
            params![conversation_id, user_id],
            |row| Ok(Conversation {
                id: row.get(0)?,
                user_id: row.get(1)?,
                title: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            }),
        ).ok()?;

        let mut stmt = conn
            .prepare(
                "SELECT id, conversation_id, role, content, provider, model, created_at
             FROM messages WHERE conversation_id = ?1 ORDER BY created_at ASC",
            )
            .unwrap();

        let messages = stmt
            .query_map(params![conversation_id], |row| {
                Ok(Message {
                    id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    role: row.get(2)?,
                    content: row.get(3)?,
                    provider: row.get(4)?,
                    model: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        Some(ConversationWithMessages {
            conversation,
            messages,
        })
    }

    pub fn delete_conversation(&self, conversation_id: &str, user_id: &str) -> bool {
        let conn = self.conn();
        let rows = conn
            .execute(
                "DELETE FROM conversations WHERE id = ?1 AND user_id = ?2",
                params![conversation_id, user_id],
            )
            .unwrap_or(0);
        rows > 0
    }

    pub fn update_conversation_title(
        &self,
        conversation_id: &str,
        user_id: &str,
        title: &str,
    ) -> bool {
        let conn = self.conn();
        let now = Utc::now().to_rfc3339();
        let rows = conn.execute(
            "UPDATE conversations SET title = ?1, updated_at = ?2 WHERE id = ?3 AND user_id = ?4",
            params![title, now, conversation_id, user_id],
        ).unwrap_or(0);
        rows > 0
    }

    pub fn add_message(
        &self,
        conversation_id: &str,
        role: &str,
        content: &str,
        provider: Option<&str>,
        model: Option<&str>,
    ) -> Message {
        let conn = self.conn();
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        conn.execute(
            "INSERT INTO messages (id, conversation_id, role, content, provider, model, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, conversation_id, role, content, provider, model, now],
        ).expect("failed to insert message");

        conn.execute(
            "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
            params![now, conversation_id],
        )
        .ok();

        Message {
            id,
            conversation_id: conversation_id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            provider: provider.map(String::from),
            model: model.map(String::from),
            created_at: now,
        }
    }

    // --- Cortex groups + task state ---

    pub fn upsert_group(
        &self,
        user_id: &str,
        id: &str,
        name: &str,
        kind: &str,
        description: &str,
        members: i64,
        accent: &str,
        source: &str,
        external_id: Option<&str>,
    ) -> CortexGroup {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO cortex_groups
                (id, user_id, name, kind, description, members, accent, source, external_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, datetime('now'))
             ON CONFLICT(user_id, source, external_id) DO UPDATE SET
                name = excluded.name,
                description = excluded.description,
                members = excluded.members,
                accent = excluded.accent,
                updated_at = datetime('now')",
            params![id, user_id, name, kind, description, members, accent, source, external_id],
        ).expect("failed to upsert cortex group");

        drop(conn);
        self.get_group(user_id, id).unwrap_or_else(|| CortexGroup {
            id: id.to_string(),
            name: name.to_string(),
            kind: kind.to_string(),
            description: description.to_string(),
            members,
            accent: accent.to_string(),
            source: source.to_string(),
            external_id: external_id.map(String::from),
        })
    }

    pub fn list_groups(&self, user_id: &str) -> Vec<CortexGroup> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, name, kind, description, members, accent, source, external_id
             FROM cortex_groups WHERE user_id = ?1 ORDER BY updated_at DESC",
            )
            .unwrap();

        stmt.query_map(params![user_id], |row| {
            Ok(CortexGroup {
                id: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                description: row.get(3)?,
                members: row.get(4)?,
                accent: row.get(5)?,
                source: row.get(6)?,
                external_id: row.get(7)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn get_group(&self, user_id: &str, group_id: &str) -> Option<CortexGroup> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, name, kind, description, members, accent, source, external_id
             FROM cortex_groups WHERE user_id = ?1 AND id = ?2",
            params![user_id, group_id],
            |row| {
                Ok(CortexGroup {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    kind: row.get(2)?,
                    description: row.get(3)?,
                    members: row.get(4)?,
                    accent: row.get(5)?,
                    source: row.get(6)?,
                    external_id: row.get(7)?,
                })
            },
        )
        .ok()
    }

    pub fn ensure_personal_authority_scope(&self, user_id: &str) -> CortexAuthorityScope {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let scope_id = format!("personal:{user_id}");
        let policy = serde_json::json!({
            "mutation_context": "personal",
            "requires_org_handoff": false,
            "allowed_actions": ["read", "plan", "queue", "run_personal"],
            "approval_policy": "user"
        });
        let policy_json = serde_json::to_string(&policy).unwrap_or_else(|_| "{}".to_string());

        conn.execute(
            "INSERT INTO cortex_authority_scopes
                (id, owner_user_id, kind, name, description, source, external_id, status, policy_json, created_at, updated_at)
             VALUES (?1, ?2, 'personal', 'Personal Operations', 'Personal Cortex operations authority', 'cortex', ?3, 'active', ?4, ?5, ?5)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                description = excluded.description,
                status = 'active',
                policy_json = excluded.policy_json,
                updated_at = excluded.updated_at",
            params![scope_id, user_id, user_id, policy_json, now],
        )
        .expect("failed to upsert personal authority scope");
        conn.execute(
            "INSERT INTO cortex_authority_memberships
                (scope_id, user_id, role, status, created_at, updated_at)
             VALUES (?1, ?2, 'owner', 'active', ?3, ?3)
             ON CONFLICT(scope_id, user_id) DO UPDATE SET
                role = excluded.role,
                status = 'active',
                updated_at = excluded.updated_at",
            params![scope_id, user_id, now],
        )
        .expect("failed to upsert personal authority membership");

        CortexAuthorityScope {
            id: scope_id,
            owner_user_id: user_id.to_string(),
            kind: "personal".to_string(),
            name: "Personal Operations".to_string(),
            description: "Personal Cortex operations authority".to_string(),
            source: "cortex".to_string(),
            external_id: Some(user_id.to_string()),
            status: "active".to_string(),
            policy,
            role: "owner".to_string(),
            created_at: now,
            updated_at: now,
        }
    }

    pub fn upsert_authority_scope(
        &self,
        owner_user_id: &str,
        id: &str,
        kind: &str,
        name: &str,
        description: &str,
        source: &str,
        external_id: Option<&str>,
        policy: serde_json::Value,
    ) -> CortexAuthorityScope {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let policy_json = serde_json::to_string(&policy).unwrap_or_else(|_| "{}".to_string());
        conn.execute(
            "INSERT INTO cortex_authority_scopes
                (id, owner_user_id, kind, name, description, source, external_id, status, policy_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'active', ?8, ?9, ?9)
             ON CONFLICT(id) DO UPDATE SET
                kind = excluded.kind,
                name = excluded.name,
                description = excluded.description,
                source = excluded.source,
                external_id = excluded.external_id,
                status = 'active',
                policy_json = excluded.policy_json,
                updated_at = excluded.updated_at",
            params![
                id,
                owner_user_id,
                kind,
                name,
                description,
                source,
                external_id,
                policy_json,
                now
            ],
        )
        .expect("failed to upsert authority scope");
        conn.execute(
            "INSERT INTO cortex_authority_memberships
                (scope_id, user_id, role, status, created_at, updated_at)
             VALUES (?1, ?2, 'owner', 'active', ?3, ?3)
             ON CONFLICT(scope_id, user_id) DO UPDATE SET
                role = excluded.role,
                status = 'active',
                updated_at = excluded.updated_at",
            params![id, owner_user_id, now],
        )
        .expect("failed to upsert authority owner membership");

        CortexAuthorityScope {
            id: id.to_string(),
            owner_user_id: owner_user_id.to_string(),
            kind: kind.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            source: source.to_string(),
            external_id: external_id.map(String::from),
            status: "active".to_string(),
            policy,
            role: "owner".to_string(),
            created_at: now,
            updated_at: now,
        }
    }

    pub fn add_authority_membership(&self, scope_id: &str, user_id: &str, role: &str) {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO cortex_authority_memberships
                (scope_id, user_id, role, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'active', ?4, ?4)
             ON CONFLICT(scope_id, user_id) DO UPDATE SET
                role = excluded.role,
                status = 'active',
                updated_at = excluded.updated_at",
            params![scope_id, user_id, role, now],
        )
        .expect("failed to upsert authority membership");
    }

    pub fn upsert_authority_resource(
        &self,
        scope_id: &str,
        resource_type: &str,
        resource_key: &str,
        access: &str,
        policy: serde_json::Value,
    ) -> CortexAuthorityResource {
        let conn = self.conn();
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().timestamp_millis();
        let policy_json = serde_json::to_string(&policy).unwrap_or_else(|_| "{}".to_string());
        conn.execute(
            "INSERT INTO cortex_authority_resources
                (id, scope_id, resource_type, resource_key, access, policy_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(scope_id, resource_type, resource_key) DO UPDATE SET
                access = excluded.access,
                policy_json = excluded.policy_json,
                updated_at = excluded.updated_at",
            params![id, scope_id, resource_type, resource_key, access, policy_json, now],
        )
        .expect("failed to upsert authority resource");

        CortexAuthorityResource {
            id,
            scope_id: scope_id.to_string(),
            resource_type: resource_type.to_string(),
            resource_key: resource_key.to_string(),
            access: access.to_string(),
            policy,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn list_authority_scopes_for_user(&self, user_id: &str) -> Vec<CortexAuthorityScope> {
        self.ensure_personal_authority_scope(user_id);
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT s.id, s.owner_user_id, s.kind, s.name, s.description, s.source,
                        s.external_id, s.status, s.policy_json, m.role, s.created_at, s.updated_at
                 FROM cortex_authority_scopes s
                 JOIN cortex_authority_memberships m ON m.scope_id = s.id
                 WHERE m.user_id = ?1 AND m.status = 'active' AND s.status = 'active'
                 ORDER BY
                    CASE s.kind WHEN 'personal' THEN 0 WHEN 'team' THEN 1 WHEN 'org' THEN 2 ELSE 3 END,
                    s.updated_at DESC",
            )
            .unwrap();

        stmt.query_map(params![user_id], |row| {
            let policy_json: String = row.get(8)?;
            Ok(CortexAuthorityScope {
                id: row.get(0)?,
                owner_user_id: row.get(1)?,
                kind: row.get(2)?,
                name: row.get(3)?,
                description: row.get(4)?,
                source: row.get(5)?,
                external_id: row.get(6)?,
                status: row.get(7)?,
                policy: serde_json::from_str(&policy_json)
                    .unwrap_or_else(|_| serde_json::json!({})),
                role: row.get(9)?,
                created_at: row.get(10)?,
                updated_at: row.get(11)?,
            })
        })
        .unwrap()
        .filter_map(|row| row.ok())
        .collect()
    }

    pub fn get_authority_scope_for_user(
        &self,
        user_id: &str,
        scope_id: &str,
    ) -> Option<CortexAuthorityScope> {
        if scope_id == format!("personal:{user_id}") {
            self.ensure_personal_authority_scope(user_id);
        }
        let conn = self.conn();
        conn.query_row(
            "SELECT s.id, s.owner_user_id, s.kind, s.name, s.description, s.source,
                    s.external_id, s.status, s.policy_json, m.role, s.created_at, s.updated_at
             FROM cortex_authority_scopes s
             JOIN cortex_authority_memberships m ON m.scope_id = s.id
             WHERE m.user_id = ?1
                AND s.id = ?2
                AND m.status = 'active'
                AND s.status = 'active'",
            params![user_id, scope_id],
            |row| {
                let policy_json: String = row.get(8)?;
                Ok(CortexAuthorityScope {
                    id: row.get(0)?,
                    owner_user_id: row.get(1)?,
                    kind: row.get(2)?,
                    name: row.get(3)?,
                    description: row.get(4)?,
                    source: row.get(5)?,
                    external_id: row.get(6)?,
                    status: row.get(7)?,
                    policy: serde_json::from_str(&policy_json)
                        .unwrap_or_else(|_| serde_json::json!({})),
                    role: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                })
            },
        )
        .ok()
    }

    pub fn authority_resource_allows(
        &self,
        user_id: &str,
        scope_id: &str,
        resource_type: &str,
        resource_key: &str,
        required_access: &str,
    ) -> bool {
        let conn = self.conn();
        let mut stmt = match conn.prepare(
            "SELECT m.role, r.access
             FROM cortex_authority_scopes s
             JOIN cortex_authority_memberships m ON m.scope_id = s.id
             JOIN cortex_authority_resources r ON r.scope_id = s.id
             WHERE m.user_id = ?1
                AND s.id = ?2
                AND m.status = 'active'
                AND s.status = 'active'
                AND r.resource_type = ?3
                AND r.resource_key = ?4",
        ) {
            Ok(stmt) => stmt,
            Err(_) => return false,
        };
        stmt.query_map(
            params![user_id, scope_id, resource_type, resource_key],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map(|rows| {
            rows.filter_map(|row| row.ok()).any(|(role, access)| {
                authority_role_allows(&role, required_access)
                    && authority_access_allows(&access, required_access)
            })
        })
        .unwrap_or(false)
    }

    pub fn find_non_personal_authority_resource_scope(
        &self,
        user_id: &str,
        resource_type: &str,
        resource_key: &str,
    ) -> Option<CortexAuthorityScope> {
        let conn = self.conn();
        conn.query_row(
            "SELECT s.id, s.owner_user_id, s.kind, s.name, s.description, s.source,
                    s.external_id, s.status, s.policy_json, m.role, s.created_at, s.updated_at
             FROM cortex_authority_scopes s
             JOIN cortex_authority_memberships m ON m.scope_id = s.id
             JOIN cortex_authority_resources r ON r.scope_id = s.id
             WHERE m.user_id = ?1
                AND m.status = 'active'
                AND s.status = 'active'
                AND s.kind != 'personal'
                AND r.resource_type = ?2
                AND r.resource_key = ?3
             ORDER BY
                CASE s.kind WHEN 'team' THEN 0 WHEN 'org' THEN 1 ELSE 2 END,
                s.updated_at DESC
             LIMIT 1",
            params![user_id, resource_type, resource_key],
            |row| {
                let policy_json: String = row.get(8)?;
                Ok(CortexAuthorityScope {
                    id: row.get(0)?,
                    owner_user_id: row.get(1)?,
                    kind: row.get(2)?,
                    name: row.get(3)?,
                    description: row.get(4)?,
                    source: row.get(5)?,
                    external_id: row.get(6)?,
                    status: row.get(7)?,
                    policy: serde_json::from_str(&policy_json)
                        .unwrap_or_else(|_| serde_json::json!({})),
                    role: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                })
            },
        )
        .ok()
    }

    pub fn list_authority_resources_for_user(
        &self,
        user_id: &str,
        scope_id: &str,
    ) -> Vec<CortexAuthorityResource> {
        let conn = self.conn();
        let allowed = conn
            .query_row(
                "SELECT 1
                 FROM cortex_authority_memberships m
                 JOIN cortex_authority_scopes s ON s.id = m.scope_id
                 WHERE m.user_id = ?1
                    AND m.scope_id = ?2
                    AND m.status = 'active'
                    AND s.status = 'active'",
                params![user_id, scope_id],
                |_| Ok(()),
            )
            .is_ok();
        if !allowed {
            return Vec::new();
        }

        let mut stmt = conn
            .prepare(
                "SELECT id, scope_id, resource_type, resource_key, access, policy_json, created_at, updated_at
                 FROM cortex_authority_resources
                 WHERE scope_id = ?1
                 ORDER BY resource_type ASC, resource_key ASC",
            )
            .unwrap();
        stmt.query_map(params![scope_id], |row| {
            let policy_json: String = row.get(5)?;
            Ok(CortexAuthorityResource {
                id: row.get(0)?,
                scope_id: row.get(1)?,
                resource_type: row.get(2)?,
                resource_key: row.get(3)?,
                access: row.get(4)?,
                policy: serde_json::from_str(&policy_json)
                    .unwrap_or_else(|_| serde_json::json!({})),
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })
        .unwrap()
        .filter_map(|row| row.ok())
        .collect()
    }

    pub fn add_authority_resource(
        &self,
        scope_id: &str,
        resource_type: &str,
        resource_key: &str,
        access: &str,
        policy: serde_json::Value,
    ) -> CortexAuthorityResource {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let resource_id = Uuid::new_v4().to_string();
        let policy_json = serde_json::to_string(&policy).unwrap_or_else(|_| "{}".to_string());

        conn.execute(
            "INSERT INTO cortex_authority_resources
                (id, scope_id, resource_type, resource_key, access, policy_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                resource_id,
                scope_id,
                resource_type,
                resource_key,
                access,
                policy_json,
                now
            ],
        )
        .expect("failed to insert authority resource");

        CortexAuthorityResource {
            id: resource_id,
            scope_id: scope_id.to_string(),
            resource_type: resource_type.to_string(),
            resource_key: resource_key.to_string(),
            access: access.to_string(),
            policy,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn list_user_operation_group_ids(&self, user_id: &str) -> Vec<String> {
        let conn = self.conn();
        let mut ids = HashSet::new();

        let queries = [
            "SELECT id FROM cortex_groups WHERE user_id = ?1",
            "SELECT group_id FROM group_task_state WHERE user_id = ?1",
            "SELECT group_id FROM cortex_tasks WHERE user_id = ?1",
            "SELECT group_id FROM runs WHERE user_id = ?1 AND group_id IS NOT NULL",
            "SELECT group_id FROM cortex_approval_requests WHERE user_id = ?1",
            "SELECT group_id FROM resource_leases WHERE user_id = ?1 AND group_id IS NOT NULL",
        ];

        for query in queries {
            let Ok(mut stmt) = conn.prepare(query) else {
                continue;
            };
            let Ok(rows) = stmt.query_map(params![user_id], |row| row.get::<_, String>(0)) else {
                continue;
            };
            for id in rows.filter_map(|row| row.ok()) {
                if !id.trim().is_empty() {
                    ids.insert(id);
                }
            }
        }

        let mut ids = ids.into_iter().collect::<Vec<_>>();
        ids.sort();
        ids
    }

    pub fn get_group_task_state(&self, user_id: &str, group_id: &str) -> Option<serde_json::Value> {
        let conn = self.conn();
        let raw: String = conn
            .query_row(
                "SELECT state_json FROM group_task_state WHERE user_id = ?1 AND group_id = ?2",
                params![user_id, group_id],
                |row| row.get(0),
            )
            .ok()?;
        serde_json::from_str(&raw).ok()
    }

    pub fn upsert_group_task_state(
        &self,
        user_id: &str,
        group_id: &str,
        state: &serde_json::Value,
    ) -> serde_json::Value {
        let conn = self.conn();
        let raw = serde_json::to_string(state).unwrap_or_else(|_| "{}".to_string());
        conn.execute(
            "INSERT INTO group_task_state (group_id, user_id, state_json, updated_at)
             VALUES (?1, ?2, ?3, datetime('now'))
             ON CONFLICT(group_id, user_id) DO UPDATE SET
                state_json = excluded.state_json,
                updated_at = datetime('now')",
            params![group_id, user_id, raw],
        )
        .expect("failed to upsert group task state");
        index_group_task_state(&conn, user_id, group_id, state);
        state.clone()
    }

    pub fn upsert_group_task_state_with_events(
        &self,
        user_id: &str,
        group_id: &str,
        state: &serde_json::Value,
        events: &[CortexTaskStateEvent],
        chat_attachment: Option<(&str, &str)>,
    ) -> Result<serde_json::Value, String> {
        let mut conn = self.conn();
        let tx = conn
            .transaction()
            .map_err(|err| format!("failed to begin task state transaction: {err}"))?;
        let raw = serde_json::to_string(state).unwrap_or_else(|_| "{}".to_string());
        tx.execute(
            "INSERT INTO group_task_state (group_id, user_id, state_json, updated_at)
             VALUES (?1, ?2, ?3, datetime('now'))
             ON CONFLICT(group_id, user_id) DO UPDATE SET
                state_json = excluded.state_json,
                updated_at = datetime('now')",
            params![group_id, user_id, raw],
        )
        .map_err(|err| format!("failed to upsert group task state: {err}"))?;

        index_group_task_state_checked(&tx, user_id, group_id, state)
            .map_err(|err| format!("failed to index group task state: {err}"))?;

        for event in events {
            try_insert_operations_event(
                &tx,
                Some(user_id),
                Some(group_id),
                None,
                event.task_id.as_deref(),
                None,
                None,
                None,
                &event.event_type,
                &event.entity_type,
                &event.entity_id,
                &event.payload,
            )
            .map_err(|err| format!("failed to record task state event: {err}"))?;
        }

        if let Some((task_id, conversation_id)) = chat_attachment {
            attach_cortex_task_chat_tx_checked(
                &tx,
                user_id,
                group_id,
                task_id,
                conversation_id,
                None,
            )
            .map_err(|err| format!("failed to attach task chat: {err}"))?;
        }

        tx.commit()
            .map_err(|err| format!("failed to commit task state transaction: {err}"))?;
        Ok(state.clone())
    }

    pub fn cortex_task_exists(&self, user_id: &str, group_id: &str, task_id: &str) -> bool {
        let conn = self.conn();
        conn.query_row(
            "SELECT 1 FROM cortex_tasks WHERE user_id = ?1 AND group_id = ?2 AND id = ?3",
            params![user_id, group_id, task_id],
            |_| Ok(()),
        )
        .is_ok()
    }

    pub fn cortex_task_has_evidence_backed_completion(
        &self,
        user_id: &str,
        group_id: &str,
        task_id: &str,
    ) -> bool {
        let conn = self.conn();
        let Some(latest_run_id) = conn
            .query_row(
                "SELECT latest_run_id FROM cortex_tasks
                 WHERE user_id = ?1 AND group_id = ?2 AND id = ?3",
                params![user_id, group_id, task_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .ok()
        else {
            return false;
        };
        cortex_completion_gate(&conn, user_id, group_id, task_id, latest_run_id.as_deref())
            .gated_done
    }

    pub fn record_cortex_task_event(
        &self,
        user_id: &str,
        group_id: &str,
        task_id: &str,
        event_type: &str,
        payload: &serde_json::Value,
    ) {
        let conn = self.conn();
        insert_operations_event(
            &conn,
            Some(user_id),
            Some(group_id),
            None,
            Some(task_id),
            None,
            None,
            None,
            event_type,
            "task",
            task_id,
            payload,
        );
    }

    pub fn record_cortex_task_manager_event(
        &self,
        user_id: &str,
        group_id: &str,
        event_type: &str,
        payload: &serde_json::Value,
    ) {
        let conn = self.conn();
        insert_operations_event(
            &conn,
            Some(user_id),
            Some(group_id),
            None,
            None,
            None,
            None,
            None,
            event_type,
            "task_manager",
            group_id,
            payload,
        );
    }

    pub fn attach_cortex_task_chat(
        &self,
        user_id: &str,
        group_id: &str,
        task_id: &str,
        conversation_id: &str,
    ) -> bool {
        let conn = self.conn();
        attach_cortex_task_chat_tx(&conn, user_id, group_id, task_id, conversation_id, None)
    }

    pub fn record_deployment_event(
        &self,
        event_type: &str,
        entity_id: &str,
        payload: &serde_json::Value,
    ) {
        let conn = self.conn();
        insert_operations_event(
            &conn,
            None,
            Some("cortex"),
            None,
            None,
            None,
            None,
            None,
            event_type,
            "deployment",
            entity_id,
            payload,
        );
    }

    pub fn create_cortex_approval_request(
        &self,
        user_id: &str,
        group_id: &str,
        task_id: Option<&str>,
        conversation_id: Option<&str>,
        run_id: Option<&str>,
        title: &str,
        body: &str,
        priority: &str,
        requested_by: &str,
    ) -> CortexApprovalRequest {
        self.create_cortex_approval_request_with_gate(
            user_id,
            group_id,
            task_id,
            None,
            conversation_id,
            run_id,
            "approval",
            title,
            body,
            priority,
            requested_by,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_cortex_approval_request_with_gate(
        &self,
        user_id: &str,
        group_id: &str,
        task_id: Option<&str>,
        step_id: Option<&str>,
        conversation_id: Option<&str>,
        run_id: Option<&str>,
        ask_type: &str,
        title: &str,
        body: &str,
        priority: &str,
        requested_by: &str,
    ) -> CortexApprovalRequest {
        let conn = self.conn();
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().timestamp_millis();
        let normalized_priority = match priority {
            "high" | "urgent" => priority,
            _ => "normal",
        };
        let ask_type = ask_type.trim();
        let ask_type = if ask_type.is_empty() {
            "approval"
        } else {
            ask_type
        };
        conn.execute(
            "INSERT INTO cortex_approval_requests (
                id, user_id, group_id, task_id, step_id, conversation_id, run_id, ask_type, status,
                title, body, priority, requested_by, created_at, updated_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', ?9, ?10, ?11, ?12, ?13, ?13)",
            params![
                id,
                user_id,
                group_id,
                task_id,
                step_id,
                conversation_id,
                run_id,
                ask_type,
                title.trim(),
                body.trim(),
                normalized_priority,
                requested_by.trim(),
                now,
            ],
        )
        .expect("failed to create cortex approval request");

        insert_operations_event(
            &conn,
            Some(user_id),
            Some(group_id),
            None,
            task_id,
            run_id,
            step_id,
            None,
            "approval.requested",
            "approval_request",
            &id,
            &serde_json::json!({
                "id": id,
                "ask_type": ask_type,
                "title": title.trim(),
                "priority": normalized_priority,
                "requested_by": requested_by.trim(),
            }),
        );

        self.approval_request_from_row(&conn, user_id, group_id, &id)
            .expect("approval request should exist after insert")
    }

    pub fn ensure_cortex_step_approval_request(
        &self,
        user_id: &str,
        step_id: &str,
        ask_type: &str,
        title: &str,
        body: &str,
        priority: &str,
        requested_by: &str,
    ) -> Option<CortexApprovalRequest> {
        let conn = self.conn();
        if let Ok((id, group_id)) = conn.query_row(
            "SELECT id, group_id FROM cortex_approval_requests
             WHERE user_id = ?1 AND step_id = ?2 AND ask_type = ?3 AND status = 'pending'
             ORDER BY updated_at DESC, id DESC
             LIMIT 1",
            params![user_id, step_id, ask_type],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        ) {
            return self.approval_request_from_row(&conn, user_id, &group_id, &id);
        }

        let context = conn
            .query_row(
                "SELECT r.group_id, r.task_id, r.conversation_id, r.id
             FROM steps s
             JOIN runs r ON r.id = s.run_id
             WHERE s.id = ?1 AND r.user_id = ?2",
                params![step_id, user_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .ok()?;
        let (Some(group_id), task_id, conversation_id, run_id) = context else {
            return None;
        };
        drop(conn);

        Some(self.create_cortex_approval_request_with_gate(
            user_id,
            &group_id,
            task_id.as_deref(),
            Some(step_id),
            conversation_id.as_deref(),
            Some(&run_id),
            ask_type,
            title,
            body,
            priority,
            requested_by,
        ))
    }

    pub fn has_pending_cortex_step_approval(&self, user_id: &str, step_id: &str) -> bool {
        let conn = self.conn();
        conn.query_row(
            "SELECT 1 FROM cortex_approval_requests
             WHERE user_id = ?1 AND step_id = ?2 AND status = 'pending'
             LIMIT 1",
            params![user_id, step_id],
            |_| Ok(()),
        )
        .is_ok()
    }

    pub fn has_approved_cortex_step_approval(
        &self,
        user_id: &str,
        step_id: &str,
        ask_type: &str,
    ) -> bool {
        let conn = self.conn();
        conn.query_row(
            "SELECT 1 FROM cortex_approval_requests
             WHERE user_id = ?1 AND step_id = ?2 AND ask_type = ?3 AND status = 'approved'
             LIMIT 1",
            params![user_id, step_id, ask_type],
            |_| Ok(()),
        )
        .is_ok()
    }

    pub fn latest_cortex_step_approval_status(
        &self,
        user_id: &str,
        step_id: &str,
        ask_type: &str,
    ) -> Option<(String, String)> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, status FROM cortex_approval_requests
             WHERE user_id = ?1 AND step_id = ?2 AND ask_type = ?3
             ORDER BY updated_at DESC, id DESC
             LIMIT 1",
            params![user_id, step_id, ask_type],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok()
    }

    pub fn list_cortex_approval_requests(
        &self,
        user_id: &str,
        group_id: &str,
        status: Option<&str>,
        limit: usize,
    ) -> Vec<CortexApprovalRequest> {
        let conn = self.conn();
        let limit = limit.clamp(1, 100) as i64;
        let mut requests = Vec::new();
        if let Some(status) = status {
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM cortex_approval_requests
                 WHERE user_id = ?1 AND group_id = ?2 AND status = ?3
                 ORDER BY updated_at DESC, id DESC
                 LIMIT ?4",
                )
                .unwrap();
            let rows = stmt
                .query_map(params![user_id, group_id, status, limit], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap();
            for id in rows.filter_map(|row| row.ok()) {
                if let Some(request) = self.approval_request_from_row(&conn, user_id, group_id, &id)
                {
                    requests.push(request);
                }
            }
        } else {
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM cortex_approval_requests
                 WHERE user_id = ?1 AND group_id = ?2
                 ORDER BY updated_at DESC, id DESC
                 LIMIT ?3",
                )
                .unwrap();
            let rows = stmt
                .query_map(params![user_id, group_id, limit], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap();
            for id in rows.filter_map(|row| row.ok()) {
                if let Some(request) = self.approval_request_from_row(&conn, user_id, group_id, &id)
                {
                    requests.push(request);
                }
            }
        }
        requests
    }

    pub fn resolve_cortex_approval_request(
        &self,
        user_id: &str,
        group_id: &str,
        request_id: &str,
        status: &str,
        decision: &serde_json::Value,
    ) -> Option<CortexApprovalRequest> {
        let status = match status {
            "approved" | "rejected" | "cancelled" => status,
            _ => return None,
        };
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let decision_json = serde_json::to_string(decision).unwrap_or_else(|_| "{}".to_string());
        let updated = conn
            .execute(
                "UPDATE cortex_approval_requests
             SET status = ?1, decision_json = ?2, updated_at = ?3, resolved_at = ?3
             WHERE user_id = ?4 AND group_id = ?5 AND id = ?6 AND status = 'pending'",
                params![status, decision_json, now, user_id, group_id, request_id],
            )
            .ok()?
            > 0;
        if !updated {
            return None;
        }

        let request = self.approval_request_from_row(&conn, user_id, group_id, request_id)?;
        insert_operations_event(
            &conn,
            Some(user_id),
            Some(group_id),
            None,
            request.task_id.as_deref(),
            request.run_id.as_deref(),
            request.step_id.as_deref(),
            None,
            "approval.resolved",
            "approval_request",
            request_id,
            &serde_json::json!({
                "id": request_id,
                "status": status,
                "decision": decision,
            }),
        );
        Some(request)
    }

    fn approval_request_from_row(
        &self,
        conn: &Connection,
        user_id: &str,
        group_id: &str,
        request_id: &str,
    ) -> Option<CortexApprovalRequest> {
        conn.query_row(
            "SELECT id, group_id, task_id, step_id, conversation_id, run_id, ask_type, status,
                    title, body, priority, requested_by, decision_json, created_at, updated_at,
                    resolved_at
             FROM cortex_approval_requests
             WHERE user_id = ?1 AND group_id = ?2 AND id = ?3",
            params![user_id, group_id, request_id],
            |row| {
                let decision_json: Option<String> = row.get(12)?;
                Ok(CortexApprovalRequest {
                    id: row.get(0)?,
                    group_id: row.get(1)?,
                    task_id: row.get(2)?,
                    step_id: row.get(3)?,
                    conversation_id: row.get(4)?,
                    run_id: row.get(5)?,
                    ask_type: row.get(6)?,
                    status: row.get(7)?,
                    title: row.get(8)?,
                    body: row.get(9)?,
                    priority: row.get(10)?,
                    requested_by: row.get(11)?,
                    decision: decision_json.and_then(|raw| serde_json::from_str(&raw).ok()),
                    created_at: row.get(13)?,
                    updated_at: row.get(14)?,
                    resolved_at: row.get(15)?,
                })
            },
        )
        .ok()
    }

    pub fn get_cortex_task_projection(
        &self,
        user_id: &str,
        group_id: &str,
        task_id: &str,
        event_limit: usize,
    ) -> Option<serde_json::Value> {
        let conn = self.conn();
        let (
            id,
            title,
            status,
            priority,
            conversation_id,
            latest_run_id,
            source_json,
            created_at,
            updated_at,
            version,
        ): (
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            String,
            String,
            i64,
        ) = conn
            .query_row(
                "SELECT id, title, status, priority, conversation_id, latest_run_id,
                        source_json, created_at, updated_at, version
                 FROM cortex_tasks
                 WHERE user_id = ?1 AND group_id = ?2 AND id = ?3",
                params![user_id, group_id, task_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                    ))
                },
            )
            .ok()?;
        let source = serde_json::from_str(&source_json).unwrap_or_else(|_| serde_json::json!({}));
        let completion_gate =
            cortex_completion_gate(&conn, user_id, group_id, task_id, latest_run_id.as_deref());
        let completion = completion_gate.to_json(status == "done");

        let mut runs_stmt = conn
            .prepare(
                "SELECT id, goal, status, profile, created_at, updated_at, started_at, finished_at,
                        heal_attempts, task_id, group_id, conversation_id
                 FROM runs
                 WHERE user_id = ?1 AND group_id = ?2 AND task_id = ?3
                 ORDER BY created_at DESC, id DESC
                 LIMIT 25",
            )
            .unwrap();
        let runs: Vec<serde_json::Value> = runs_stmt
            .query_map(params![user_id, group_id, task_id], |row| {
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "goal": row.get::<_, String>(1)?,
                    "status": row.get::<_, String>(2)?,
                    "profile": row.get::<_, String>(3)?,
                    "created_at": row.get::<_, i64>(4)?,
                    "updated_at": row.get::<_, i64>(5)?,
                    "started_at": row.get::<_, Option<i64>>(6)?,
                    "finished_at": row.get::<_, Option<i64>>(7)?,
                    "heal_attempts": row.get::<_, i32>(8)?,
                    "task_id": row.get::<_, Option<String>>(9)?,
                    "group_id": row.get::<_, Option<String>>(10)?,
                    "conversation_id": row.get::<_, Option<String>>(11)?,
                }))
            })
            .unwrap()
            .filter_map(|row| row.ok())
            .collect();

        let mut chats_stmt = conn
            .prepare(
                "SELECT c.id, c.title, c.created_at, c.updated_at, tc.attached_at
                 FROM cortex_task_chats tc
                 JOIN conversations c ON c.id = tc.conversation_id AND c.user_id = tc.user_id
                 WHERE tc.user_id = ?1 AND tc.group_id = ?2 AND tc.task_id = ?3
                 ORDER BY tc.attached_at DESC, c.updated_at DESC
                 LIMIT 25",
            )
            .unwrap();
        let chats: Vec<serde_json::Value> = chats_stmt
            .query_map(params![user_id, group_id, task_id], |row| {
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "title": row.get::<_, Option<String>>(1)?,
                    "created_at": row.get::<_, String>(2)?,
                    "updated_at": row.get::<_, String>(3)?,
                    "attached_at": row.get::<_, String>(4)?,
                }))
            })
            .unwrap()
            .filter_map(|row| row.ok())
            .collect();

        let event_limit = event_limit.clamp(1, 500) as i64;
        let mut events_stmt = conn
            .prepare(
                "SELECT id, created_at, actor_user_id, scope_id, project_id, task_id, run_id,
                        step_id, attempt_id, event_type, entity_type, entity_id, payload_json
                 FROM operations_events
                 WHERE task_id = ?1
                    AND (actor_user_id = ?2 OR actor_user_id IS NULL)
                    AND (scope_id = ?3 OR scope_id IS NULL)
                 ORDER BY created_at DESC, id DESC
                 LIMIT ?4",
            )
            .unwrap();
        let events: Vec<serde_json::Value> = events_stmt
            .query_map(params![task_id, user_id, group_id, event_limit], |row| {
                let payload_json: String = row.get(12)?;
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "created_at": row.get::<_, i64>(1)?,
                    "actor_user_id": row.get::<_, Option<String>>(2)?,
                    "scope_id": row.get::<_, Option<String>>(3)?,
                    "project_id": row.get::<_, Option<String>>(4)?,
                    "task_id": row.get::<_, Option<String>>(5)?,
                    "run_id": row.get::<_, Option<String>>(6)?,
                    "step_id": row.get::<_, Option<String>>(7)?,
                    "attempt_id": row.get::<_, Option<String>>(8)?,
                    "event_type": row.get::<_, String>(9)?,
                    "entity_type": row.get::<_, String>(10)?,
                    "entity_id": row.get::<_, String>(11)?,
                    "payload": serde_json::from_str(&payload_json).unwrap_or_else(|_| serde_json::json!({})),
                }))
            })
            .unwrap()
            .filter_map(|row| row.ok())
            .collect();

        let mut approvals_stmt = conn
            .prepare(
                "SELECT id FROM cortex_approval_requests
                 WHERE user_id = ?1 AND group_id = ?2 AND task_id = ?3
                 ORDER BY updated_at DESC, id DESC
                 LIMIT 100",
            )
            .unwrap();
        let approval_ids = approvals_stmt
            .query_map(params![user_id, group_id, task_id], |row| {
                row.get::<_, String>(0)
            })
            .unwrap()
            .filter_map(|row| row.ok())
            .collect::<Vec<_>>();
        let approvals = approval_ids
            .into_iter()
            .filter_map(|id| self.approval_request_from_row(&conn, user_id, group_id, &id))
            .collect::<Vec<_>>();

        Some(serde_json::json!({
            "task": {
                "id": id,
                "group_id": group_id,
                "title": title,
                "status": status,
                "priority": priority,
                "conversation_id": conversation_id,
                "latest_run_id": latest_run_id,
                "source": source,
                "created_at": created_at,
                "updated_at": updated_at,
                "version": version,
                "completion": completion,
            },
            "runs": runs,
            "chats": chats,
            "approvals": approvals,
            "events": events,
        }))
    }

    pub fn get_group_operations_summary(
        &self,
        user_id: &str,
        group_id: &str,
        event_limit: usize,
    ) -> serde_json::Value {
        let conn = self.conn();
        let generated_at = Utc::now().timestamp_millis();

        let mut tasks_stmt = conn
            .prepare(
                "SELECT id, title, status, priority, latest_run_id, source_json, updated_at
                 FROM cortex_tasks
                 WHERE user_id = ?1 AND group_id = ?2",
            )
            .unwrap();
        let task_rows = tasks_stmt
            .query_map(params![user_id, group_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })
            .unwrap()
            .filter_map(|row| row.ok())
            .collect::<Vec<_>>();

        let mut task_created = 0;
        let mut task_assigned = 0;
        let mut task_active = 0;
        let mut task_done = 0;
        let mut task_urgent = 0;
        let mut task_unassigned = 0;
        let mut task_without_run = 0;
        let mut task_gated_done = 0;
        let mut task_done_without_evidence = 0;
        let mut attention: Vec<serde_json::Value> = Vec::new();

        for (task_id, title, status, priority, latest_run_id, source_json, updated_at) in &task_rows
        {
            match status.as_str() {
                "created" => task_created += 1,
                "assigned" => task_assigned += 1,
                "in-progress" => task_active += 1,
                "done" => task_done += 1,
                _ => {}
            }
            if status == "done" {
                let gate = cortex_completion_gate(
                    &conn,
                    user_id,
                    group_id,
                    task_id,
                    latest_run_id.as_deref(),
                );
                if gate.gated_done {
                    task_gated_done += 1;
                } else {
                    task_done_without_evidence += 1;
                    if attention.len() < 10 {
                        attention.push(serde_json::json!({
                            "kind": "done_without_evidence",
                            "task_id": task_id,
                            "title": title,
                            "status": status,
                            "run_id": gate.run_id,
                            "reason": gate.reason,
                            "updated_at": updated_at,
                        }));
                    }
                }
            }
            if priority == "urgent" {
                task_urgent += 1;
                if status != "in-progress" && status != "done" && attention.len() < 10 {
                    attention.push(serde_json::json!({
                        "kind": "urgent_not_active",
                        "task_id": task_id,
                        "title": title,
                        "priority": priority,
                        "status": status,
                        "updated_at": updated_at,
                    }));
                }
            }
            if latest_run_id.is_none() {
                task_without_run += 1;
                if status == "in-progress" && attention.len() < 10 {
                    attention.push(serde_json::json!({
                        "kind": "active_without_run",
                        "task_id": task_id,
                        "title": title,
                        "status": status,
                        "updated_at": updated_at,
                    }));
                }
            }
            let source = serde_json::from_str::<serde_json::Value>(source_json)
                .unwrap_or_else(|_| serde_json::json!({}));
            let assignee = source.get("assigneeId").and_then(|value| value.as_str());
            if assignee
                .map(|value| value.trim().is_empty())
                .unwrap_or(true)
            {
                task_unassigned += 1;
            }
        }

        let mut runs_stmt = conn
            .prepare(
                "SELECT id, status, task_id, created_at
                 FROM runs
                 WHERE user_id = ?1 AND group_id = ?2
                 ORDER BY created_at DESC, id DESC",
            )
            .unwrap();
        let run_rows = runs_stmt
            .query_map(params![user_id, group_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .unwrap()
            .filter_map(|row| row.ok())
            .collect::<Vec<_>>();

        let mut runs_active = 0;
        let mut runs_failed = 0;
        let mut runs_succeeded = 0;
        for (run_id, status, task_id, created_at) in &run_rows {
            match status.as_str() {
                "pending" | "planning" | "ready" | "leased" | "running" => runs_active += 1,
                "failed" | "orphaned" => {
                    runs_failed += 1;
                    if attention.len() < 10 {
                        attention.push(serde_json::json!({
                            "kind": "failed_run",
                            "run_id": run_id,
                            "task_id": task_id,
                            "status": status,
                            "created_at": created_at,
                        }));
                    }
                }
                "succeeded" | "recovered" => runs_succeeded += 1,
                _ => {}
            }
        }
        let latest_run_id = run_rows.first().map(|(run_id, _, _, _)| run_id.clone());

        let mut steps_stmt = conn
            .prepare(
                "SELECT s.id, s.run_id, s.status, s.verification_status, r.task_id
                 FROM steps s
                 JOIN runs r ON r.id = s.run_id
                 WHERE r.user_id = ?1 AND r.group_id = ?2",
            )
            .unwrap();
        let step_rows = steps_stmt
            .query_map(params![user_id, group_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })
            .unwrap()
            .filter_map(|row| row.ok())
            .collect::<Vec<_>>();

        let mut steps_active = 0;
        let mut steps_failed = 0;
        let mut steps_orphaned = 0;
        let mut steps_verified_pass = 0;
        let mut steps_verified_fail = 0;
        for (step_id, run_id, status, verification_status, task_id) in &step_rows {
            match status.as_str() {
                "leased" | "running" => steps_active += 1,
                "failed" => {
                    steps_failed += 1;
                    if attention.len() < 10 {
                        attention.push(serde_json::json!({
                            "kind": "failed_step",
                            "step_id": step_id,
                            "run_id": run_id,
                            "task_id": task_id,
                            "status": status,
                        }));
                    }
                }
                "orphaned" => {
                    steps_orphaned += 1;
                    if attention.len() < 10 {
                        attention.push(serde_json::json!({
                            "kind": "orphaned_step",
                            "step_id": step_id,
                            "run_id": run_id,
                            "task_id": task_id,
                            "status": status,
                        }));
                    }
                }
                _ => {}
            }
            match verification_status.as_deref() {
                Some("verified_pass") => steps_verified_pass += 1,
                Some("verified_fail") => steps_verified_fail += 1,
                _ => {}
            }
        }

        let mut approvals_stmt = conn
            .prepare(
                "SELECT id, task_id, run_id, status, title, priority, updated_at
                 FROM cortex_approval_requests
                 WHERE user_id = ?1 AND group_id = ?2
                 ORDER BY updated_at DESC, id DESC",
            )
            .unwrap();
        let approval_rows = approvals_stmt
            .query_map(params![user_id, group_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            })
            .unwrap()
            .filter_map(|row| row.ok())
            .collect::<Vec<_>>();
        let mut approvals_pending = 0;
        let mut approvals_approved = 0;
        let mut approvals_rejected = 0;
        let mut approvals_cancelled = 0;
        for (approval_id, task_id, run_id, status, title, priority, updated_at) in &approval_rows {
            match status.as_str() {
                "pending" => {
                    approvals_pending += 1;
                    if attention.len() < 10 {
                        attention.push(serde_json::json!({
                            "kind": "approval_pending",
                            "approval_id": approval_id,
                            "task_id": task_id,
                            "run_id": run_id,
                            "title": title,
                            "priority": priority,
                            "status": status,
                            "updated_at": updated_at,
                        }));
                    }
                }
                "approved" => approvals_approved += 1,
                "rejected" => approvals_rejected += 1,
                "cancelled" => approvals_cancelled += 1,
                _ => {}
            }
        }

        let mut leases_stmt = conn
            .prepare(
                "SELECT id, user_id, authority_scope_id, group_id, task_id, run_id, step_id, holder_type,
                        resource_type, repo_key, resource_key, mode, status, lease_gen,
                        acquired_at, expires_at, released_at, reason, metadata_json
                 FROM resource_leases
                 WHERE user_id = ?1 AND group_id = ?2 AND status = 'active'
                 ORDER BY acquired_at DESC, id DESC
                 LIMIT 50",
            )
            .unwrap();
        let active_resource_leases = leases_stmt
            .query_map(params![user_id, group_id], resource_lease_from_row)
            .unwrap()
            .filter_map(|row| row.ok())
            .collect::<Vec<_>>();
        let mut resource_lease_path = 0;
        let mut resource_lease_task = 0;
        let mut resource_lease_repo = 0;
        let mut resource_lease_read = 0;
        let mut resource_lease_write = 0;
        let mut resource_lease_exclusive = 0;
        let active_resource_lease_summaries = active_resource_leases
            .iter()
            .map(|lease| {
                match lease.resource_type.as_str() {
                    "path" => resource_lease_path += 1,
                    "task" => resource_lease_task += 1,
                    "repo" => resource_lease_repo += 1,
                    _ => {}
                }
                match lease.mode.as_str() {
                    "read" => resource_lease_read += 1,
                    "write" => resource_lease_write += 1,
                    "exclusive" => resource_lease_exclusive += 1,
                    _ => {}
                }
                if lease.expires_at <= generated_at && attention.len() < 10 {
                    attention.push(serde_json::json!({
                        "kind": "stale_resource_lease",
                        "lease_id": lease.id,
                        "authority_scope_id": lease.authority_scope_id,
                        "task_id": lease.task_id,
                        "run_id": lease.run_id,
                        "step_id": lease.step_id,
                        "resource_type": lease.resource_type,
                        "repo_key": lease.repo_key,
                        "resource_key": lease.resource_key,
                        "mode": lease.mode,
                        "expires_at": lease.expires_at,
                    }));
                }
                serde_json::json!({
                    "id": lease.id,
                    "authority_scope_id": lease.authority_scope_id,
                    "group_id": lease.group_id,
                    "task_id": lease.task_id,
                    "run_id": lease.run_id,
                    "step_id": lease.step_id,
                    "holder_type": lease.holder_type,
                    "resource_type": lease.resource_type,
                    "repo_key": lease.repo_key,
                    "resource_key": lease.resource_key,
                    "mode": lease.mode,
                    "lease_gen": lease.lease_gen,
                    "acquired_at": lease.acquired_at,
                    "expires_at": lease.expires_at,
                    "seconds_until_expiry": ((lease.expires_at - generated_at).max(0)) / 1000,
                    "reason": lease.reason,
                    "metadata": lease.metadata,
                })
            })
            .collect::<Vec<_>>();

        let event_limit = event_limit.clamp(1, 100) as i64;
        let mut events_stmt = conn
            .prepare(
                "SELECT id, created_at, actor_user_id, scope_id, project_id, task_id, run_id,
                        step_id, attempt_id, event_type, entity_type, entity_id, payload_json
                 FROM operations_events
                 WHERE scope_id = ?1
                    AND (actor_user_id = ?2 OR actor_user_id IS NULL)
                 ORDER BY created_at DESC, id DESC
                 LIMIT ?3",
            )
            .unwrap();
        let recent_events: Vec<serde_json::Value> = events_stmt
            .query_map(params![group_id, user_id, event_limit], |row| {
                let payload_json: String = row.get(12)?;
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "created_at": row.get::<_, i64>(1)?,
                    "actor_user_id": row.get::<_, Option<String>>(2)?,
                    "scope_id": row.get::<_, Option<String>>(3)?,
                    "project_id": row.get::<_, Option<String>>(4)?,
                    "task_id": row.get::<_, Option<String>>(5)?,
                    "run_id": row.get::<_, Option<String>>(6)?,
                    "step_id": row.get::<_, Option<String>>(7)?,
                    "attempt_id": row.get::<_, Option<String>>(8)?,
                    "event_type": row.get::<_, String>(9)?,
                    "entity_type": row.get::<_, String>(10)?,
                    "entity_id": row.get::<_, String>(11)?,
                    "payload": serde_json::from_str(&payload_json).unwrap_or_else(|_| serde_json::json!({})),
                }))
            })
            .unwrap()
            .filter_map(|row| row.ok())
            .collect();

        let open_tasks = task_created + task_assigned + task_active;
        serde_json::json!({
            "group_id": group_id,
            "scope": "group",
            "generated_at": generated_at,
            "tasks": {
                "total": task_rows.len(),
                "open": open_tasks,
                "active": task_active,
                "done_raw": task_done,
                "urgent": task_urgent,
                "unassigned": task_unassigned,
                "without_run": task_without_run,
                "by_status": {
                    "created": task_created,
                    "assigned": task_assigned,
                    "in_progress": task_active,
                    "done": task_done,
                },
                "completion": {
                    "gated_done_available": true,
                    "raw_done": task_done,
                    "gated_done": task_gated_done,
                    "done_without_evidence": task_done_without_evidence,
                },
            },
            "runs": {
                "total": run_rows.len(),
                "active": runs_active,
                "failed": runs_failed,
                "succeeded": runs_succeeded,
                "latest_run_id": latest_run_id,
            },
            "steps": {
                "total": step_rows.len(),
                "active": steps_active,
                "failed": steps_failed,
                "orphaned": steps_orphaned,
                "verified_pass": steps_verified_pass,
                "verified_fail": steps_verified_fail,
            },
            "approvals": {
                "total": approval_rows.len(),
                "pending": approvals_pending,
                "approved": approvals_approved,
                "rejected": approvals_rejected,
                "cancelled": approvals_cancelled,
            },
            "resource_leases": {
                "active": active_resource_leases.len(),
                "by_type": {
                    "path": resource_lease_path,
                    "task": resource_lease_task,
                    "repo": resource_lease_repo,
                },
                "by_mode": {
                    "read": resource_lease_read,
                    "write": resource_lease_write,
                    "exclusive": resource_lease_exclusive,
                },
                "leases": active_resource_lease_summaries,
            },
            "attention": attention,
            "recent_events": recent_events,
        })
    }

    pub fn get_group_operations_graph(
        &self,
        user_id: &str,
        group_id: &str,
        event_limit: usize,
    ) -> serde_json::Value {
        let conn = self.conn();
        let generated_at = Utc::now().timestamp_millis();
        let mut nodes: Vec<serde_json::Value> = Vec::new();
        let mut edges: Vec<serde_json::Value> = Vec::new();

        let mut tasks_stmt = conn
            .prepare(
                "SELECT id, title, status, priority, conversation_id, latest_run_id,
                        source_json, created_at, updated_at, version
                 FROM cortex_tasks
                 WHERE user_id = ?1 AND group_id = ?2
                 ORDER BY updated_at DESC, id ASC",
            )
            .unwrap();
        let task_rows = tasks_stmt
            .query_map(params![user_id, group_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            })
            .unwrap()
            .filter_map(|row| row.ok())
            .collect::<Vec<_>>();

        for (
            task_id,
            title,
            status,
            priority,
            conversation_id,
            latest_run_id,
            source_json,
            created_at,
            updated_at,
            version,
        ) in &task_rows
        {
            let source =
                serde_json::from_str(source_json).unwrap_or_else(|_| serde_json::json!({}));
            let completion =
                cortex_completion_gate(&conn, user_id, group_id, task_id, latest_run_id.as_deref())
                    .to_json(status == "done");
            nodes.push(serde_json::json!({
                "id": format!("task:{task_id}"),
                "type": "task",
                "entity_id": task_id,
                "group_id": group_id,
                "label": title,
                "status": status,
                "priority": priority,
                "conversation_id": conversation_id,
                "latest_run_id": latest_run_id,
                "source": source,
                "created_at": created_at,
                "updated_at": updated_at,
                "version": version,
                "completion": completion,
            }));

            if let Some(conversation_id) = conversation_id {
                edges.push(serde_json::json!({
                    "id": format!("task:{task_id}->chat:{conversation_id}"),
                    "from": format!("task:{task_id}"),
                    "to": format!("chat:{conversation_id}"),
                    "type": "task_chat",
                }));
            }
        }

        let mut runs_stmt = conn
            .prepare(
                "SELECT id, goal, status, profile, created_at, updated_at, started_at, finished_at,
                        heal_attempts, task_id, conversation_id, repo_key, authority_scope_id
                 FROM runs
                 WHERE user_id = ?1 AND group_id = ?2
                 ORDER BY created_at DESC, id DESC
                 LIMIT 100",
            )
            .unwrap();
        let run_rows = runs_stmt
            .query_map(params![user_id, group_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, i32>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                ))
            })
            .unwrap()
            .filter_map(|row| row.ok())
            .collect::<Vec<_>>();
        let run_ids = run_rows
            .iter()
            .map(|(run_id, ..)| run_id.clone())
            .collect::<HashSet<_>>();

        for (
            run_id,
            goal,
            status,
            profile,
            created_at,
            updated_at,
            started_at,
            finished_at,
            heal_attempts,
            task_id,
            conversation_id,
            repo_key,
            authority_scope_id,
        ) in &run_rows
        {
            nodes.push(serde_json::json!({
                "id": format!("run:{run_id}"),
                "type": "run",
                "entity_id": run_id,
                "group_id": group_id,
                "task_id": task_id,
                "conversation_id": conversation_id,
                "label": goal,
                "status": status,
                "profile": profile,
                "repo_key": repo_key,
                "authority_scope_id": authority_scope_id,
                "heal_attempts": heal_attempts,
                "created_at": created_at,
                "updated_at": updated_at,
                "started_at": started_at,
                "finished_at": finished_at,
            }));
            if let Some(task_id) = task_id {
                edges.push(serde_json::json!({
                    "id": format!("task:{task_id}->run:{run_id}"),
                    "from": format!("task:{task_id}"),
                    "to": format!("run:{run_id}"),
                    "type": "task_run",
                }));
            }
            if let Some(conversation_id) = conversation_id {
                edges.push(serde_json::json!({
                    "id": format!("chat:{conversation_id}->run:{run_id}"),
                    "from": format!("chat:{conversation_id}"),
                    "to": format!("run:{run_id}"),
                    "type": "chat_run",
                }));
            }
        }

        let mut chats_stmt = conn
            .prepare(
                "SELECT DISTINCT c.id, c.title, c.created_at, c.updated_at
                 FROM cortex_task_chats tc
                 JOIN conversations c ON c.id = tc.conversation_id AND c.user_id = tc.user_id
                 WHERE tc.user_id = ?1 AND tc.group_id = ?2
                 ORDER BY c.updated_at DESC, c.id ASC
                 LIMIT 100",
            )
            .unwrap();
        let chat_rows = chats_stmt
            .query_map(params![user_id, group_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .unwrap()
            .filter_map(|row| row.ok())
            .collect::<Vec<_>>();
        for (conversation_id, title, created_at, updated_at) in chat_rows {
            nodes.push(serde_json::json!({
                "id": format!("chat:{conversation_id}"),
                "type": "chat",
                "entity_id": conversation_id,
                "group_id": group_id,
                "label": title.as_deref().unwrap_or("Project Chat"),
                "title": title,
                "created_at": created_at,
                "updated_at": updated_at,
            }));
        }

        let mut steps_stmt = conn
            .prepare(
                "SELECT s.id, s.run_id, r.task_id, s.status, s.kind, s.work_kind, s.tier, s.risk,
                        s.objective, s.attempt_count, s.max_attempts, s.lease_gen,
                        s.lease_deadline, s.assigned_worker, s.verification_status,
                        s.verifier_report_id, s.updated_at
                 FROM steps s
                 JOIN runs r ON r.id = s.run_id
                 WHERE r.user_id = ?1 AND r.group_id = ?2
                 ORDER BY r.created_at DESC, s.created_at ASC, s.id ASC
                 LIMIT 500",
            )
            .unwrap();
        let step_rows = steps_stmt
            .query_map(params![user_id, group_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, i64>(11)?,
                    row.get::<_, Option<i64>>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, String>(14)?,
                    row.get::<_, Option<String>>(15)?,
                    row.get::<_, i64>(16)?,
                ))
            })
            .unwrap()
            .filter_map(|row| row.ok())
            .collect::<Vec<_>>();
        let step_ids = step_rows
            .iter()
            .map(|(step_id, ..)| step_id.clone())
            .collect::<HashSet<_>>();

        for (
            step_id,
            run_id,
            task_id,
            status,
            kind,
            work_kind,
            tier,
            risk,
            objective,
            attempt_count,
            max_attempts,
            lease_gen,
            lease_deadline,
            assigned_worker,
            verification_status,
            verifier_report_id,
            updated_at,
        ) in &step_rows
        {
            let lease_stale = lease_deadline
                .map(|deadline| {
                    matches!(status.as_str(), "leased" | "running") && deadline < generated_at
                })
                .unwrap_or(false);
            nodes.push(serde_json::json!({
                "id": format!("step:{step_id}"),
                "type": "step",
                "entity_id": step_id,
                "group_id": group_id,
                "task_id": task_id,
                "run_id": run_id,
                "label": objective,
                "status": status,
                "kind": kind,
                "work_kind": work_kind,
                "tier": tier,
                "risk": risk,
                "attempt_count": attempt_count,
                "max_attempts": max_attempts,
                "lease_gen": lease_gen,
                "lease_deadline": lease_deadline,
                "assigned_worker": assigned_worker,
                "lease_stale": lease_stale,
                "verification_status": verification_status,
                "verifier_report_id": verifier_report_id,
                "updated_at": updated_at,
            }));
            edges.push(serde_json::json!({
                "id": format!("run:{run_id}->step:{step_id}"),
                "from": format!("run:{run_id}"),
                "to": format!("step:{step_id}"),
                "type": "run_step",
            }));
        }

        let mut dependency_stmt = conn
            .prepare(
                "SELECT sd.depends_on_id, sd.step_id, sd.edge_type
                 FROM step_dependencies sd
                 JOIN steps s ON s.id = sd.step_id
                 JOIN runs r ON r.id = s.run_id
                 WHERE r.user_id = ?1 AND r.group_id = ?2
                 ORDER BY sd.depends_on_id ASC, sd.step_id ASC",
            )
            .unwrap();
        for (depends_on_id, step_id, edge_type) in dependency_stmt
            .query_map(params![user_id, group_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .unwrap()
            .filter_map(|row| row.ok())
        {
            edges.push(serde_json::json!({
                "id": format!("step:{depends_on_id}->step:{step_id}:{edge_type}"),
                "from": format!("step:{depends_on_id}"),
                "to": format!("step:{step_id}"),
                "type": "step_dependency",
                "edge_type": edge_type,
            }));
        }

        let mut seen_evidence_steps = HashSet::new();
        let mut verifier_stmt = conn
            .prepare(
                "SELECT vr.id, vr.step_id, vr.run_id, vr.lease_gen, vr.worker_id, vr.verifier,
                        vr.status, vr.verdict, vr.created_at, vr.updated_at
                 FROM verifier_reports vr
                 JOIN runs r ON r.id = vr.run_id
                 WHERE r.user_id = ?1 AND r.group_id = ?2
                 ORDER BY vr.step_id ASC, vr.created_at DESC, vr.id DESC",
            )
            .unwrap();
        for (
            report_id,
            step_id,
            run_id,
            lease_gen,
            worker_id,
            verifier,
            status,
            verdict,
            created_at,
            updated_at,
        ) in verifier_stmt
            .query_map(params![user_id, group_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            })
            .unwrap()
            .filter_map(|row| row.ok())
        {
            if !seen_evidence_steps.insert(step_id.clone()) {
                continue;
            }
            nodes.push(serde_json::json!({
                "id": format!("evidence:{report_id}"),
                "type": "evidence",
                "entity_id": report_id,
                "group_id": group_id,
                "step_id": step_id,
                "run_id": run_id,
                "label": format!("{verifier} {verdict}"),
                "status": status,
                "verdict": verdict,
                "verifier": verifier,
                "worker_id": worker_id,
                "lease_gen": lease_gen,
                "created_at": created_at,
                "updated_at": updated_at,
            }));
            edges.push(serde_json::json!({
                "id": format!("step:{step_id}->evidence:{report_id}"),
                "from": format!("step:{step_id}"),
                "to": format!("evidence:{report_id}"),
                "type": "step_evidence",
            }));
        }

        let mut approvals_stmt = conn
            .prepare(
                "SELECT id, task_id, step_id, conversation_id, run_id, ask_type, status, title,
                        priority, requested_by, created_at, updated_at, resolved_at
                 FROM cortex_approval_requests
                 WHERE user_id = ?1 AND group_id = ?2
                 ORDER BY updated_at DESC, id DESC
                 LIMIT 100",
            )
            .unwrap();
        for (
            approval_id,
            task_id,
            step_id,
            conversation_id,
            run_id,
            ask_type,
            status,
            title,
            priority,
            requested_by,
            created_at,
            updated_at,
            resolved_at,
        ) in approvals_stmt
            .query_map(params![user_id, group_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, i64>(11)?,
                    row.get::<_, Option<i64>>(12)?,
                ))
            })
            .unwrap()
            .filter_map(|row| row.ok())
        {
            nodes.push(serde_json::json!({
                "id": format!("approval:{approval_id}"),
                "type": "approval",
                "entity_id": approval_id,
                "group_id": group_id,
                "task_id": task_id,
                "step_id": step_id,
                "conversation_id": conversation_id,
                "run_id": run_id,
                "label": title,
                "ask_type": ask_type,
                "status": status,
                "priority": priority,
                "requested_by": requested_by,
                "created_at": created_at,
                "updated_at": updated_at,
                "resolved_at": resolved_at,
            }));
            if let Some(task_id) = task_id.as_deref() {
                edges.push(serde_json::json!({
                    "id": format!("task:{task_id}->approval:{approval_id}"),
                    "from": format!("task:{task_id}"),
                    "to": format!("approval:{approval_id}"),
                    "type": "task_approval",
                }));
            }
            if let Some(run_id) = run_id.as_deref() {
                edges.push(serde_json::json!({
                    "id": format!("run:{run_id}->approval:{approval_id}"),
                    "from": format!("run:{run_id}"),
                    "to": format!("approval:{approval_id}"),
                    "type": "run_approval",
                }));
            }
            if let Some(step_id) = step_id.as_deref() {
                edges.push(serde_json::json!({
                    "id": format!("step:{step_id}->approval:{approval_id}"),
                    "from": format!("step:{step_id}"),
                    "to": format!("approval:{approval_id}"),
                    "type": "step_approval",
                }));
            }
        }

        let mut leases_stmt = conn
            .prepare(
                "SELECT id, user_id, authority_scope_id, group_id, task_id, run_id, step_id, holder_type,
                        resource_type, repo_key, resource_key, mode, status, lease_gen,
                        acquired_at, expires_at, released_at, reason, metadata_json
                 FROM resource_leases
                 WHERE user_id = ?1 AND group_id = ?2 AND status = 'active'
                 ORDER BY acquired_at DESC, id DESC
                 LIMIT 100",
            )
            .unwrap();
        for lease in leases_stmt
            .query_map(params![user_id, group_id], resource_lease_from_row)
            .unwrap()
            .filter_map(|row| row.ok())
        {
            nodes.push(serde_json::json!({
                "id": format!("resource_lease:{}", lease.id),
                "type": "resource_lease",
                "entity_id": lease.id,
                "authority_scope_id": lease.authority_scope_id,
                "group_id": lease.group_id,
                "task_id": lease.task_id,
                "run_id": lease.run_id,
                "step_id": lease.step_id,
                "label": format!("{}:{} {}", lease.resource_type, lease.resource_key, lease.mode),
                "holder_type": lease.holder_type,
                "resource_type": lease.resource_type,
                "repo_key": lease.repo_key,
                "resource_key": lease.resource_key,
                "mode": lease.mode,
                "status": lease.status,
                "lease_gen": lease.lease_gen,
                "acquired_at": lease.acquired_at,
                "expires_at": lease.expires_at,
                "seconds_until_expiry": ((lease.expires_at - generated_at).max(0)) / 1000,
                "reason": lease.reason,
                "metadata": lease.metadata,
            }));
            let from = lease
                .step_id
                .as_ref()
                .map(|step_id| format!("step:{step_id}"))
                .unwrap_or_else(|| format!("run:{}", lease.run_id));
            edges.push(serde_json::json!({
                "id": format!("{}->resource_lease:{}", from, lease.id),
                "from": from,
                "to": format!("resource_lease:{}", lease.id),
                "type": "resource_lease",
            }));
        }

        let event_limit = event_limit.clamp(1, 250) as i64;
        let mut events_stmt = conn
            .prepare(
                "SELECT id, created_at, actor_user_id, scope_id, project_id, task_id, run_id,
                        step_id, attempt_id, event_type, entity_type, entity_id, payload_json
                 FROM operations_events
                 WHERE scope_id = ?1
                    AND (actor_user_id = ?2 OR actor_user_id IS NULL)
                 ORDER BY created_at DESC, id DESC
                 LIMIT ?3",
            )
            .unwrap();
        let recent_events = events_stmt
            .query_map(params![group_id, user_id, event_limit], |row| {
                let payload_json: String = row.get(12)?;
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "created_at": row.get::<_, i64>(1)?,
                    "actor_user_id": row.get::<_, Option<String>>(2)?,
                    "scope_id": row.get::<_, Option<String>>(3)?,
                    "project_id": row.get::<_, Option<String>>(4)?,
                    "task_id": row.get::<_, Option<String>>(5)?,
                    "run_id": row.get::<_, Option<String>>(6)?,
                    "step_id": row.get::<_, Option<String>>(7)?,
                    "attempt_id": row.get::<_, Option<String>>(8)?,
                    "event_type": row.get::<_, String>(9)?,
                    "entity_type": row.get::<_, String>(10)?,
                    "entity_id": row.get::<_, String>(11)?,
                    "payload": serde_json::from_str(&payload_json).unwrap_or_else(|_| serde_json::json!({})),
                }))
            })
            .unwrap()
            .filter_map(|row| row.ok())
            .collect::<Vec<_>>();

        nodes.retain(|node| {
            let node_type = node.get("type").and_then(|value| value.as_str());
            let run_id = node.get("run_id").and_then(|value| value.as_str());
            let step_id = node.get("step_id").and_then(|value| value.as_str());
            match node_type {
                Some("resource_lease") => run_id.is_some_and(|run_id| run_ids.contains(run_id)),
                Some("evidence") | Some("approval") => {
                    run_id
                        .map(|run_id| run_ids.contains(run_id))
                        .unwrap_or(true)
                        && step_id
                            .map(|step_id| step_ids.contains(step_id))
                            .unwrap_or(true)
                }
                _ => true,
            }
        });
        let node_ids = nodes
            .iter()
            .filter_map(|node| node.get("id").and_then(|value| value.as_str()))
            .map(str::to_string)
            .collect::<HashSet<_>>();
        edges.retain(|edge| {
            let from = edge.get("from").and_then(|value| value.as_str());
            let to = edge.get("to").and_then(|value| value.as_str());
            from.is_some_and(|from| node_ids.contains(from))
                && to.is_some_and(|to| node_ids.contains(to))
        });

        serde_json::json!({
            "group_id": group_id,
            "scope": "group",
            "generated_at": generated_at,
            "nodes": nodes,
            "edges": edges,
            "recent_events": recent_events,
        })
    }

    pub fn get_personal_operations_summary(
        &self,
        user_id: &str,
        event_limit: usize,
    ) -> serde_json::Value {
        let generated_at = Utc::now().timestamp_millis();
        let group_ids = self.list_user_operation_group_ids(user_id);
        let groups_by_id = self
            .list_groups(user_id)
            .into_iter()
            .map(|group| (group.id.clone(), group))
            .collect::<HashMap<_, _>>();

        let mut task_total = 0;
        let mut task_open = 0;
        let mut task_active = 0;
        let mut task_done_raw = 0;
        let mut task_urgent = 0;
        let mut task_unassigned = 0;
        let mut task_without_run = 0;
        let mut task_created = 0;
        let mut task_assigned = 0;
        let mut task_in_progress = 0;
        let mut task_done = 0;
        let mut task_gated_done = 0;
        let mut task_done_without_evidence = 0;
        let mut runs_total = 0;
        let mut runs_active = 0;
        let mut runs_failed = 0;
        let mut runs_succeeded = 0;
        let mut steps_total = 0;
        let mut steps_active = 0;
        let mut steps_failed = 0;
        let mut steps_orphaned = 0;
        let mut steps_verified_pass = 0;
        let mut steps_verified_fail = 0;
        let mut approvals_total = 0;
        let mut approvals_pending = 0;
        let mut approvals_approved = 0;
        let mut approvals_rejected = 0;
        let mut approvals_cancelled = 0;
        let mut resource_leases_active = 0;
        let mut resource_leases_path = 0;
        let mut resource_leases_task = 0;
        let mut resource_leases_repo = 0;
        let mut resource_leases_read = 0;
        let mut resource_leases_write = 0;
        let mut resource_leases_exclusive = 0;
        let mut active_groups = 0;
        let mut group_summaries = Vec::new();
        let mut attention = Vec::new();
        let mut recent_events = Vec::new();
        let mut active_resource_leases = Vec::new();

        for group_id in &group_ids {
            let summary = self.get_group_operations_summary(user_id, group_id, event_limit);
            let group_active = json_i64(&summary, &["tasks", "open"])
                + json_i64(&summary, &["runs", "active"])
                + json_i64(&summary, &["approvals", "pending"])
                + json_i64(&summary, &["resource_leases", "active"]);
            if group_active > 0 {
                active_groups += 1;
            }

            task_total += json_i64(&summary, &["tasks", "total"]);
            task_open += json_i64(&summary, &["tasks", "open"]);
            task_active += json_i64(&summary, &["tasks", "active"]);
            task_done_raw += json_i64(&summary, &["tasks", "done_raw"]);
            task_urgent += json_i64(&summary, &["tasks", "urgent"]);
            task_unassigned += json_i64(&summary, &["tasks", "unassigned"]);
            task_without_run += json_i64(&summary, &["tasks", "without_run"]);
            task_created += json_i64(&summary, &["tasks", "by_status", "created"]);
            task_assigned += json_i64(&summary, &["tasks", "by_status", "assigned"]);
            task_in_progress += json_i64(&summary, &["tasks", "by_status", "in_progress"]);
            task_done += json_i64(&summary, &["tasks", "by_status", "done"]);
            task_gated_done += json_i64(&summary, &["tasks", "completion", "gated_done"]);
            task_done_without_evidence +=
                json_i64(&summary, &["tasks", "completion", "done_without_evidence"]);
            runs_total += json_i64(&summary, &["runs", "total"]);
            runs_active += json_i64(&summary, &["runs", "active"]);
            runs_failed += json_i64(&summary, &["runs", "failed"]);
            runs_succeeded += json_i64(&summary, &["runs", "succeeded"]);
            steps_total += json_i64(&summary, &["steps", "total"]);
            steps_active += json_i64(&summary, &["steps", "active"]);
            steps_failed += json_i64(&summary, &["steps", "failed"]);
            steps_orphaned += json_i64(&summary, &["steps", "orphaned"]);
            steps_verified_pass += json_i64(&summary, &["steps", "verified_pass"]);
            steps_verified_fail += json_i64(&summary, &["steps", "verified_fail"]);
            approvals_total += json_i64(&summary, &["approvals", "total"]);
            approvals_pending += json_i64(&summary, &["approvals", "pending"]);
            approvals_approved += json_i64(&summary, &["approvals", "approved"]);
            approvals_rejected += json_i64(&summary, &["approvals", "rejected"]);
            approvals_cancelled += json_i64(&summary, &["approvals", "cancelled"]);
            resource_leases_active += json_i64(&summary, &["resource_leases", "active"]);
            resource_leases_path += json_i64(&summary, &["resource_leases", "by_type", "path"]);
            resource_leases_task += json_i64(&summary, &["resource_leases", "by_type", "task"]);
            resource_leases_repo += json_i64(&summary, &["resource_leases", "by_type", "repo"]);
            resource_leases_read += json_i64(&summary, &["resource_leases", "by_mode", "read"]);
            resource_leases_write += json_i64(&summary, &["resource_leases", "by_mode", "write"]);
            resource_leases_exclusive +=
                json_i64(&summary, &["resource_leases", "by_mode", "exclusive"]);

            append_group_scoped_items(&mut attention, &summary, "attention", group_id);
            append_group_scoped_items(&mut recent_events, &summary, "recent_events", group_id);
            if let Some(leases) = summary
                .get("resource_leases")
                .and_then(|value| value.get("leases"))
                .and_then(|value| value.as_array())
            {
                active_resource_leases.extend(leases.iter().cloned());
            }

            let group = groups_by_id.get(group_id);
            group_summaries.push(serde_json::json!({
                "group_id": group_id,
                "name": group.map(|group| group.name.as_str()).unwrap_or(group_id),
                "kind": group.map(|group| group.kind.as_str()).unwrap_or("project"),
                "source": group.map(|group| group.source.as_str()).unwrap_or("derived"),
                "accent": group.map(|group| group.accent.as_str()).unwrap_or("#9cc7b8"),
                "active": group_active,
                "tasks": summary["tasks"].clone(),
                "runs": summary["runs"].clone(),
                "steps": summary["steps"].clone(),
                "approvals": summary["approvals"].clone(),
                "resource_leases": summary["resource_leases"].clone(),
                "attention": summary["attention"].clone(),
            }));
        }

        sort_json_array_desc(&mut attention, "updated_at");
        sort_json_array_desc(&mut recent_events, "created_at");
        attention.truncate(25);
        recent_events.truncate(event_limit.clamp(1, 100));
        active_resource_leases.truncate(100);

        serde_json::json!({
            "scope": "personal",
            "generated_at": generated_at,
            "groups_total": group_ids.len(),
            "active_groups": active_groups,
            "tasks": {
                "total": task_total,
                "open": task_open,
                "active": task_active,
                "done_raw": task_done_raw,
                "urgent": task_urgent,
                "unassigned": task_unassigned,
                "without_run": task_without_run,
                "by_status": {
                    "created": task_created,
                    "assigned": task_assigned,
                    "in_progress": task_in_progress,
                    "done": task_done,
                },
                "completion": {
                    "gated_done_available": true,
                    "raw_done": task_done_raw,
                    "gated_done": task_gated_done,
                    "done_without_evidence": task_done_without_evidence,
                },
            },
            "runs": {
                "total": runs_total,
                "active": runs_active,
                "failed": runs_failed,
                "succeeded": runs_succeeded,
            },
            "steps": {
                "total": steps_total,
                "active": steps_active,
                "failed": steps_failed,
                "orphaned": steps_orphaned,
                "verified_pass": steps_verified_pass,
                "verified_fail": steps_verified_fail,
            },
            "approvals": {
                "total": approvals_total,
                "pending": approvals_pending,
                "approved": approvals_approved,
                "rejected": approvals_rejected,
                "cancelled": approvals_cancelled,
            },
            "resource_leases": {
                "active": resource_leases_active,
                "by_type": {
                    "path": resource_leases_path,
                    "task": resource_leases_task,
                    "repo": resource_leases_repo,
                },
                "by_mode": {
                    "read": resource_leases_read,
                    "write": resource_leases_write,
                    "exclusive": resource_leases_exclusive,
                },
                "leases": active_resource_leases,
            },
            "attention": attention,
            "recent_events": recent_events,
            "groups": group_summaries,
        })
    }

    pub fn conversation_exists(&self, user_id: &str, conversation_id: &str) -> bool {
        let conn = self.conn();
        conn.query_row(
            "SELECT 1 FROM conversations WHERE user_id = ?1 AND id = ?2",
            params![user_id, conversation_id],
            |_| Ok(()),
        )
        .is_ok()
    }

    // --- Integrations ---

    pub fn upsert_integration_connection(
        &self,
        user_id: &str,
        provider: &str,
        external_id: Option<&str>,
        display_name: &str,
        status: &str,
        scopes: &[String],
        access_token: Option<&str>,
        refresh_token: Option<&str>,
        metadata: &serde_json::Value,
    ) -> IntegrationConnection {
        let conn = self.conn();
        let id = Uuid::new_v4().to_string();
        let scopes_json = serde_json::to_string(scopes).unwrap_or_else(|_| "[]".to_string());
        let metadata_json = serde_json::to_string(metadata).unwrap_or_else(|_| "{}".to_string());
        conn.execute(
            "INSERT INTO integration_connections
                (id, user_id, provider, external_id, display_name, status, scopes, access_token, refresh_token, metadata_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, datetime('now'))
             ON CONFLICT(user_id, provider, external_id) DO UPDATE SET
                display_name = excluded.display_name,
                status = excluded.status,
                scopes = excluded.scopes,
                access_token = COALESCE(excluded.access_token, access_token),
                refresh_token = COALESCE(excluded.refresh_token, refresh_token),
                metadata_json = excluded.metadata_json,
                last_error = NULL,
                updated_at = datetime('now')",
            params![id, user_id, provider, external_id, display_name, status, scopes_json, access_token, refresh_token, metadata_json],
        ).expect("failed to upsert integration connection");

        self.get_integration_connection(user_id, provider, external_id)
            .expect("integration connection should exist after upsert")
    }

    pub fn get_integration_connection(
        &self,
        user_id: &str,
        provider: &str,
        external_id: Option<&str>,
    ) -> Option<IntegrationConnection> {
        let conn = self.conn();
        let sql = if external_id.is_some() {
            "SELECT id, provider, external_id, display_name, status, scopes, metadata_json, last_sync_at, last_error, created_at, updated_at
             FROM integration_connections WHERE user_id = ?1 AND provider = ?2 AND external_id = ?3"
        } else {
            "SELECT id, provider, external_id, display_name, status, scopes, metadata_json, last_sync_at, last_error, created_at, updated_at
             FROM integration_connections WHERE user_id = ?1 AND provider = ?2 ORDER BY updated_at DESC LIMIT 1"
        };

        let mut stmt = conn.prepare(sql).ok()?;
        let mut rows = if let Some(external_id) = external_id {
            stmt.query(params![user_id, provider, external_id]).ok()?
        } else {
            stmt.query(params![user_id, provider]).ok()?
        };
        let row = rows.next().ok()??;
        let scopes_raw: String = row.get(5).ok()?;
        let metadata_raw: String = row.get(6).ok()?;
        Some(IntegrationConnection {
            id: row.get(0).ok()?,
            provider: row.get(1).ok()?,
            external_id: row.get(2).ok()?,
            display_name: row.get(3).ok()?,
            status: row.get(4).ok()?,
            scopes: serde_json::from_str(&scopes_raw).unwrap_or_default(),
            metadata: serde_json::from_str(&metadata_raw).unwrap_or_else(|_| serde_json::json!({})),
            last_sync_at: row.get(7).ok()?,
            last_error: row.get(8).ok()?,
            created_at: row.get(9).ok()?,
            updated_at: row.get(10).ok()?,
        })
    }

    pub fn get_integration_token(&self, user_id: &str, provider: &str) -> Option<String> {
        let conn = self.conn();
        conn.query_row(
            "SELECT access_token FROM integration_connections
             WHERE user_id = ?1 AND provider = ?2 AND status = 'connected'
             ORDER BY updated_at DESC LIMIT 1",
            params![user_id, provider],
            |row| row.get(0),
        )
        .ok()
    }

    pub fn list_integration_connections(&self, user_id: &str) -> Vec<IntegrationConnection> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, provider, external_id, display_name, status, scopes, metadata_json, last_sync_at, last_error, created_at, updated_at
             FROM integration_connections WHERE user_id = ?1 ORDER BY provider ASC, updated_at DESC"
        ).unwrap();

        stmt.query_map(params![user_id], |row| {
            let scopes_raw: String = row.get(5)?;
            let metadata_raw: String = row.get(6)?;
            Ok(IntegrationConnection {
                id: row.get(0)?,
                provider: row.get(1)?,
                external_id: row.get(2)?,
                display_name: row.get(3)?,
                status: row.get(4)?,
                scopes: serde_json::from_str(&scopes_raw).unwrap_or_default(),
                metadata: serde_json::from_str(&metadata_raw)
                    .unwrap_or_else(|_| serde_json::json!({})),
                last_sync_at: row.get(7)?,
                last_error: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn upsert_integration_mapping(
        &self,
        user_id: &str,
        provider: &str,
        group_id: &str,
        external_id: &str,
        external_name: &str,
        mapping_type: &str,
        metadata: &serde_json::Value,
    ) -> IntegrationMapping {
        let conn = self.conn();
        let id = Uuid::new_v4().to_string();
        let metadata_json = serde_json::to_string(metadata).unwrap_or_else(|_| "{}".to_string());
        conn.execute(
            "INSERT INTO integration_mappings
                (id, user_id, provider, group_id, external_id, external_name, mapping_type, metadata_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, datetime('now'))
             ON CONFLICT(user_id, provider, external_id, mapping_type) DO UPDATE SET
                group_id = excluded.group_id,
                external_name = excluded.external_name,
                metadata_json = excluded.metadata_json,
                updated_at = datetime('now')",
            params![id, user_id, provider, group_id, external_id, external_name, mapping_type, metadata_json],
        ).expect("failed to upsert integration mapping");

        IntegrationMapping {
            id,
            provider: provider.to_string(),
            group_id: group_id.to_string(),
            external_id: external_id.to_string(),
            external_name: external_name.to_string(),
            mapping_type: mapping_type.to_string(),
            metadata: metadata.clone(),
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        }
    }

    pub fn list_integration_mappings(&self, user_id: &str) -> Vec<IntegrationMapping> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, provider, group_id, external_id, external_name, mapping_type, metadata_json, created_at, updated_at
             FROM integration_mappings WHERE user_id = ?1 ORDER BY updated_at DESC"
        ).unwrap();

        stmt.query_map(params![user_id], |row| {
            let metadata_raw: String = row.get(6)?;
            Ok(IntegrationMapping {
                id: row.get(0)?,
                provider: row.get(1)?,
                group_id: row.get(2)?,
                external_id: row.get(3)?,
                external_name: row.get(4)?,
                mapping_type: row.get(5)?,
                metadata: serde_json::from_str(&metadata_raw)
                    .unwrap_or_else(|_| serde_json::json!({})),
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn record_integration_event(
        &self,
        user_id: &str,
        provider: &str,
        group_id: Option<&str>,
        event_type: &str,
        status: &str,
        payload: &serde_json::Value,
    ) {
        let conn = self.conn();
        let payload_json = serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string());
        let _ = conn.execute(
            "INSERT INTO integration_events (id, user_id, provider, group_id, event_type, status, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![Uuid::new_v4().to_string(), user_id, provider, group_id, event_type, status, payload_json],
        );
    }

    pub fn store_oauth_state(
        &self,
        user_id: &str,
        provider: &str,
        state: &str,
        redirect_after: Option<&str>,
    ) {
        let conn = self.conn();
        let expires_at = (Utc::now() + chrono::Duration::minutes(10)).to_rfc3339();
        let _ = conn.execute(
            "INSERT OR REPLACE INTO integration_oauth_states (state, user_id, provider, redirect_after, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![state, user_id, provider, redirect_after, expires_at],
        );
    }

    pub fn consume_oauth_state(
        &self,
        provider: &str,
        state: &str,
    ) -> Option<(String, Option<String>)> {
        let conn = self.conn();
        let row = conn
            .query_row(
                "SELECT user_id, redirect_after FROM integration_oauth_states
             WHERE provider = ?1 AND state = ?2 AND expires_at > datetime('now')",
                params![provider, state],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .ok();
        let _ = conn.execute(
            "DELETE FROM integration_oauth_states WHERE state = ?1",
            params![state],
        );
        row
    }

    // --- Workers ---

    pub fn register_worker(&self, worker_id: &str, user_id: &str) {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "INSERT OR REPLACE INTO workers (id, user_id, status, created_at, last_seen) VALUES (?1, ?2, 'connected', ?3, ?3)",
            params![worker_id, user_id, now],
        )
        .map_err(|e| {
            // Same reasoning as `lease_step`: a worker that fails to persist
            // still registers in memory, so dispatch finds it and then cannot
            // lease to it. Silence here produces a failure two layers away.
            tracing::error!(worker_id, user_id, error = %e, "register_worker failed");
            e
        })
        .ok();
    }

    pub fn update_worker_seen(&self, worker_id: &str) {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE workers SET last_seen = ?1 WHERE id = ?2",
            params![now, worker_id],
        )
        .ok();
    }

    pub fn set_worker_status(&self, worker_id: &str, status: &str) {
        let conn = self.conn();
        conn.execute(
            "UPDATE workers SET status = ?1 WHERE id = ?2",
            params![status, worker_id],
        )
        .ok();
    }

    pub fn upsert_provider_capability(
        &self,
        worker_id: &str,
        user_id: &str,
        provider: &str,
        cli_version: Option<&str>,
    ) {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO provider_capabilities (worker_id, user_id, provider, cli_version, status, last_reported)
             VALUES (?1, ?2, ?3, ?4, 'claimed', ?5)
             ON CONFLICT(worker_id, provider) DO UPDATE SET
                cli_version = excluded.cli_version,
                last_reported = excluded.last_reported",
            params![worker_id, user_id, provider, cli_version, now],
        ).ok();
    }

    // --- Runs ---

    pub fn create_run(
        &self,
        user_id: &str,
        goal: &str,
        profile: &str,
        file_paths: &[String],
    ) -> String {
        self.create_run_with_metadata(user_id, goal, profile, file_paths, None, None, None)
    }

    pub fn create_run_with_metadata(
        &self,
        user_id: &str,
        goal: &str,
        profile: &str,
        file_paths: &[String],
        task_id: Option<&str>,
        group_id: Option<&str>,
        conversation_id: Option<&str>,
    ) -> String {
        let mut conn = self.conn();
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().timestamp_millis();
        let file_paths_json = if file_paths.is_empty() {
            None
        } else {
            serde_json::to_string(file_paths).ok()
        };
        let tx = conn
            .transaction()
            .expect("failed to begin create run transaction");
        tx.execute(
            "INSERT INTO runs (id, user_id, goal, status, profile, file_paths, task_id, group_id, conversation_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
            params![id, user_id, goal, profile, file_paths_json, task_id, group_id, conversation_id, now],
        ).expect("failed to create run");
        attach_run_to_cortex_task_tx_checked(&tx, user_id, group_id, task_id, conversation_id, &id)
            .expect("failed to attach run to cortex task");
        try_insert_operations_event(
            &tx,
            Some(user_id),
            group_id,
            None,
            task_id,
            Some(&id),
            None,
            None,
            "run.created",
            "run",
            &id,
            &serde_json::json!({
                "status": "pending",
                "profile": profile,
                "file_paths": file_paths,
                "task_id": task_id,
                "group_id": group_id,
                "conversation_id": conversation_id,
            }),
        )
        .expect("failed to record run created event");
        tx.commit()
            .expect("failed to commit create run transaction");
        id
    }

    /// Create a run with all its steps and dependency edges in a single transaction.
    /// This avoids N+1 lock acquisitions that occur when calling create_run + N*create_step_with_id
    /// + N*add_step_dependency individually.
    pub fn create_run_with_steps(
        &self,
        user_id: &str,
        goal: &str,
        profile: &str,
        file_paths: &[String],
        task_id: Option<&str>,
        group_id: Option<&str>,
        conversation_id: Option<&str>,
        steps: &[(
            String,
            String,
            String,
            Option<String>,
            String,
            String,
            String,
            i64,
        )], // (id, kind, work_kind, recipe_seed_json, tier, risk, objective, created_at)
        edges: &[(String, String, String)], // (step_id, depends_on_id, edge_type)
    ) -> String {
        self.create_run_with_steps_and_resource_leases(
            user_id,
            goal,
            profile,
            file_paths,
            task_id,
            group_id,
            conversation_id,
            &[],
            steps,
            edges,
        )
        .expect("failed to create run with steps")
    }

    pub fn create_run_with_steps_and_resource_leases(
        &self,
        user_id: &str,
        goal: &str,
        profile: &str,
        file_paths: &[String],
        task_id: Option<&str>,
        group_id: Option<&str>,
        conversation_id: Option<&str>,
        resource_leases: &[ResourceLeaseRequest],
        steps: &[(
            String,
            String,
            String,
            Option<String>,
            String,
            String,
            String,
            i64,
        )],
        edges: &[(String, String, String)],
    ) -> Result<String, CreateRunError> {
        self.create_run_with_steps_and_resource_leases_with_authority(
            user_id,
            goal,
            profile,
            file_paths,
            task_id,
            group_id,
            conversation_id,
            resource_leases,
            steps,
            edges,
            None,
        )
    }

    pub fn create_run_with_steps_and_resource_leases_with_authority(
        &self,
        user_id: &str,
        goal: &str,
        profile: &str,
        file_paths: &[String],
        task_id: Option<&str>,
        group_id: Option<&str>,
        conversation_id: Option<&str>,
        resource_leases: &[ResourceLeaseRequest],
        steps: &[(
            String,
            String,
            String,
            Option<String>,
            String,
            String,
            String,
            i64,
        )],
        edges: &[(String, String, String)],
        authority_context: Option<&serde_json::Value>,
    ) -> Result<String, CreateRunError> {
        let conn = self.conn();
        let run_id = Uuid::new_v4().to_string();
        let now = Utc::now().timestamp_millis();
        let file_paths_json = if file_paths.is_empty() {
            None
        } else {
            serde_json::to_string(file_paths).ok()
        };

        conn.execute("BEGIN IMMEDIATE", [])
            .map_err(|err| CreateRunError::Database(err.to_string()))?;

        let repo_key = run_repo_key_from_resource_leases(resource_leases);
        let authority_scope_id = authority_scope_id_from_context(authority_context);
        let authority_context_json = authority_context
            .map(|value| serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string()));

        if let Err(err) = acquire_run_resource_leases_tx(
            &conn,
            user_id,
            authority_scope_id,
            group_id,
            task_id,
            &run_id,
            resource_leases,
            now,
        ) {
            conn.execute("ROLLBACK", []).ok();
            return Err(err);
        }

        conn.execute(
            "INSERT INTO runs (
                id, user_id, goal, status, profile, file_paths, task_id, group_id,
                conversation_id, repo_key, authority_scope_id, authority_context_json,
                created_at, updated_at
             )
             VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
            params![
                run_id,
                user_id,
                goal,
                profile,
                file_paths_json,
                task_id,
                group_id,
                conversation_id,
                repo_key,
                authority_scope_id,
                authority_context_json,
                now
            ],
        )
        .expect("failed to create run in batch");
        attach_run_to_cortex_task(&conn, user_id, group_id, task_id, conversation_id, &run_id);
        insert_operations_event(
            &conn,
            Some(user_id),
            group_id,
            None,
            task_id,
            Some(&run_id),
            None,
            None,
            "run.created",
            "run",
            &run_id,
            &serde_json::json!({
                "status": "pending",
                "profile": profile,
                "file_paths": file_paths,
                "step_count": steps.len(),
                "edge_count": edges.len(),
                "task_id": task_id,
                "group_id": group_id,
                "conversation_id": conversation_id,
                "authority": authority_context,
            }),
        );

        for (id, kind, work_kind, recipe_seed_json, tier, risk, objective, created_at) in steps {
            conn.execute(
                "INSERT INTO steps (id, run_id, kind, work_kind, recipe_seed_json, status, tier, risk, objective, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7, ?8, ?9, ?9)",
                params![id, run_id, kind, work_kind, recipe_seed_json, tier, risk, objective, created_at],
            ).expect("failed to create step in batch");
            insert_operations_event(
                &conn,
                Some(user_id),
                group_id,
                None,
                task_id,
                Some(&run_id),
                Some(id.as_str()),
                None,
                "step.planned",
                "step",
                id,
                &serde_json::json!({
                    "status": "pending",
                    "kind": kind,
                    "work_kind": work_kind,
                    "tier": tier,
                    "risk": risk,
                    "objective": objective,
                }),
            );
        }

        for (step_id, depends_on_id, edge_type) in edges {
            conn.execute(
                "INSERT OR IGNORE INTO step_dependencies (step_id, depends_on_id, edge_type) VALUES (?1, ?2, ?3)",
                params![step_id, depends_on_id, edge_type],
            ).ok();
        }

        conn.execute("COMMIT", [])
            .map_err(|err| CreateRunError::Database(err.to_string()))?;
        Ok(run_id)
    }

    /// Record (or update) the branch name associated with a run.
    ///
    /// Called when a step completes with changes — the branch is preserved so
    /// a PR can be created after the run finishes.  If multiple steps produce
    /// branches, the last one recorded wins.
    pub fn record_run_branch(&self, run_id: &str, branch_name: &str) {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let rows = conn
            .execute(
                "UPDATE runs SET branch = ?1, updated_at = ?2 WHERE id = ?3",
                params![branch_name, now, run_id],
            )
            .unwrap_or(0);
        if rows > 0 {
            insert_run_operations_event(
                &conn,
                run_id,
                "run.branch_recorded",
                &serde_json::json!({
                    "branch_name": branch_name,
                }),
            );
        }
    }

    /// Retrieve the branch name recorded for a run, if any.
    pub fn get_run_branch(&self, run_id: &str) -> Option<String> {
        let conn = self.conn();
        conn.query_row(
            "SELECT branch FROM runs WHERE id = ?1",
            params![run_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    }

    pub fn get_run_pr_authority_context(
        &self,
        run_id: &str,
        user_id: &str,
    ) -> Option<RunPrAuthorityContext> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, user_id, repo_key, authority_scope_id, authority_context_json
             FROM runs
             WHERE id = ?1 AND user_id = ?2",
            params![run_id, user_id],
            |row| {
                let authority_context_json: Option<String> = row.get(4)?;
                Ok(RunPrAuthorityContext {
                    run_id: row.get(0)?,
                    user_id: row.get(1)?,
                    repo_key: row.get(2)?,
                    authority_scope_id: row.get(3)?,
                    authority_context: authority_context_json
                        .as_deref()
                        .and_then(|raw| serde_json::from_str(raw).ok())
                        .unwrap_or_else(|| serde_json::json!({})),
                })
            },
        )
        .ok()
    }

    pub fn run_has_pr_write_lease(&self, run_id: &str, repo_key: Option<&str>) -> bool {
        let conn = self.conn();
        let mut query = String::from(
            "SELECT 1 FROM resource_leases
             WHERE run_id = ?1
                AND status IN ('active', 'released')
                AND mode IN ('write', 'exclusive')
                AND resource_type IN ('repo', 'path')",
        );
        if repo_key.is_some() {
            query.push_str(" AND repo_key = ?2");
        }
        query.push_str(" LIMIT 1");

        if let Some(repo_key) = repo_key {
            conn.query_row(&query, params![run_id, repo_key], |_| Ok(()))
                .is_ok()
        } else {
            conn.query_row(&query, params![run_id], |_| Ok(())).is_ok()
        }
    }

    /// Get the latest head_commit from any completed step in the given run.
    /// Used to pass as base_commit to subsequent steps for workspace continuity.
    pub fn get_run_latest_commit(&self, run_id: &str) -> Option<String> {
        let conn = self.conn();
        conn.query_row(
            "SELECT head_commit FROM steps
             WHERE run_id = ?1 AND status IN ('verified', 'manual_override')
               AND head_commit IS NOT NULL
             ORDER BY updated_at DESC LIMIT 1",
            params![run_id],
            |row| row.get::<_, String>(0),
        )
        .ok()
    }

    /// Get the file_paths stored for a run (from the original goal decomposition).
    pub fn get_run_file_paths(&self, run_id: &str) -> Vec<String> {
        let conn = self.conn();
        let json: Option<String> = conn
            .query_row(
                "SELECT file_paths FROM runs WHERE id = ?1",
                params![run_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten();

        json.and_then(|j| serde_json::from_str::<Vec<String>>(&j).ok())
            .unwrap_or_default()
    }

    pub fn update_run_status(
        &self,
        run_id: &str,
        status: &str,
        failure_reason: Option<&str>,
    ) -> bool {
        let mut conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let finished = if matches!(status, "succeeded" | "failed" | "cancelled") {
            Some(now)
        } else {
            None
        };
        let tx = match conn.transaction() {
            Ok(tx) => tx,
            Err(err) => {
                tracing::warn!(
                    run_id = run_id,
                    status = status,
                    error = %err,
                    "failed to begin run status transaction"
                );
                return false;
            }
        };
        let rows = match tx.execute(
            "UPDATE runs SET status = ?1, failure_reason = ?2, finished_at = ?3, updated_at = ?4, version = version + 1
             WHERE id = ?5",
            params![status, failure_reason, finished, now, run_id],
        ) {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!(
                    run_id = run_id,
                    status = status,
                    error = %err,
                    "failed to update run status"
                );
                return false;
            }
        };
        if rows == 0 {
            return false;
        }

        let context = run_event_context(&tx, run_id);
        let result = try_insert_operations_event(
            &tx,
            context.as_ref().map(|context| context.user_id.as_str()),
            context
                .as_ref()
                .and_then(|context| context.group_id.as_deref()),
            None,
            context
                .as_ref()
                .and_then(|context| context.task_id.as_deref()),
            Some(run_id),
            None,
            None,
            "run.status_changed",
            "run",
            run_id,
            &serde_json::json!({
                "status": status,
                "failure_reason": failure_reason,
                "finished_at": finished,
            }),
        )
        .and_then(|_| {
            if finished.is_some() {
                release_resource_leases_for_run_tx_checked(&tx, run_id, now).map(|_| ())
            } else {
                Ok(())
            }
        })
        .and_then(|_| tx.commit());

        if let Err(err) = result {
            tracing::warn!(
                run_id = run_id,
                status = status,
                error = %err,
                "failed to commit run status transaction"
            );
            return false;
        }

        true
    }

    // --- Steps ---

    pub fn create_step(
        &self,
        run_id: &str,
        kind: &str,
        tier: &str,
        risk: &str,
        objective: &str,
    ) -> String {
        let conn = self.conn();
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO steps (id, run_id, kind, work_kind, status, tier, risk, objective, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'modify', 'pending', ?4, ?5, ?6, ?7, ?7)",
            params![id, run_id, kind, tier, risk, objective, now],
        ).expect("failed to create step");
        let context = run_event_context(&conn, run_id);
        insert_operations_event(
            &conn,
            context.as_ref().map(|context| context.user_id.as_str()),
            context
                .as_ref()
                .and_then(|context| context.group_id.as_deref()),
            None,
            context
                .as_ref()
                .and_then(|context| context.task_id.as_deref()),
            Some(run_id),
            Some(&id),
            None,
            "step.planned",
            "step",
            &id,
            &serde_json::json!({
                "status": "pending",
                "kind": kind,
                "work_kind": "modify",
                "tier": tier,
                "risk": risk,
                "objective": objective,
            }),
        );
        id
    }

    pub fn add_step_dependency(&self, step_id: &str, depends_on_id: &str, edge_type: &str) {
        let conn = self.conn();
        conn.execute(
            "INSERT OR IGNORE INTO step_dependencies (step_id, depends_on_id, edge_type) VALUES (?1, ?2, ?3)",
            params![step_id, depends_on_id, edge_type],
        ).ok();
    }

    pub fn find_ready_steps(&self, run_id: &str) -> Vec<String> {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let mut stmt = conn.prepare(
            "SELECT s.id FROM steps s
             WHERE s.run_id = ?1 AND s.status IN ('pending', 'orphaned')
             AND (s.earliest_dispatch_at IS NULL OR s.earliest_dispatch_at <= ?2)
             AND NOT EXISTS (
                 SELECT 1 FROM cortex_approval_requests ar
                 WHERE ar.step_id = s.id AND ar.status = 'pending'
             )
             AND NOT EXISTS (
                 SELECT 1 FROM step_dependencies sd
                 JOIN steps dep ON dep.id = sd.depends_on_id
                 WHERE sd.step_id = s.id
                 AND (
                     -- `verified` or `manual_override` only. A delivered
                     -- tree nobody checked does not unblock a dependent, and
                     -- the scheduler cannot show a dependent does not write,
                     -- so unknown scope serialises rather than unblocking.
                     (sd.edge_type = 'success_required'
                        AND dep.status NOT IN ('verified', 'manual_override'))
                     OR (sd.edge_type = 'completion_required'
                        AND dep.status NOT IN ('verified', 'manual_override', 'failed',
                                               'execution_failed', 'recovered', 'cancelled', 'skipped'))
                 )
             )"
        ).unwrap();

        stmt.query_map(params![run_id, now], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    /// Find all ready steps across all active runs in a single query.
    /// Returns (step_id, run_id, user_id, kind, work_kind, tier, risk, objective) tuples.
    /// This replaces the N+1 pattern of get_active_run_ids() + find_ready_steps() per run.
    pub fn find_all_ready_steps(
        &self,
    ) -> Vec<(
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
    )> {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let mut stmt = conn.prepare(
            "SELECT s.id, s.run_id, r.user_id, s.kind, s.work_kind, s.tier, s.risk, s.objective
             FROM steps s
             JOIN runs r ON s.run_id = r.id
             WHERE r.status IN ('planning', 'running')
             AND s.status IN ('pending', 'orphaned')
             AND (s.earliest_dispatch_at IS NULL OR s.earliest_dispatch_at <= ?1)
             AND NOT EXISTS (
                 SELECT 1 FROM cortex_approval_requests ar
                 WHERE ar.step_id = s.id AND ar.status = 'pending'
             )
             AND NOT EXISTS (
                 SELECT 1 FROM step_dependencies sd
                 JOIN steps dep ON dep.id = sd.depends_on_id
                 WHERE sd.step_id = s.id
                 AND (
                     -- `verified` or `manual_override` only. A delivered
                     -- tree nobody checked does not unblock a dependent, and
                     -- the scheduler cannot show a dependent does not write,
                     -- so unknown scope serialises rather than unblocking.
                     (sd.edge_type = 'success_required'
                        AND dep.status NOT IN ('verified', 'manual_override'))
                     OR (sd.edge_type = 'completion_required'
                        AND dep.status NOT IN ('verified', 'manual_override', 'failed',
                                               'execution_failed', 'recovered', 'cancelled', 'skipped'))
                 )
             )"
        ).unwrap();

        stmt.query_map(params![now], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
            ))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn lease_step(&self, step_id: &str, worker_id: &str, deadline_ms: i64) -> Option<i64> {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let rows = conn
            .execute(
                "UPDATE steps SET status = 'leased', assigned_worker = ?1, lease_deadline = ?2,
                 lease_gen = lease_gen + 1, attempt_count = attempt_count + 1,
                 updated_at = ?3, version = version + 1
             WHERE id = ?4 AND status IN ('pending', 'ready', 'orphaned')",
                params![worker_id, deadline_ms, now, step_id],
            )
            // A failed statement is **not** a lost race, and conflating them is
            // how this stayed hidden. `assigned_worker` is a foreign key onto
            // `workers(id)`, so leasing with a worker id that does not exist
            // raises `FOREIGN KEY constraint failed` — and with the error
            // swallowed, the caller saw exactly what it sees when another
            // dispatcher won the CAS: zero rows. The scheduler then logged
            // "CAS lease failed — skipping" and waited for the next tick,
            // forever, and nothing in the system said the word "constraint".
            //
            // Logged at error, because a step that cannot be leased is a step
            // that never runs.
            .map_err(|e| {
                tracing::error!(
                    step_id,
                    worker_id,
                    error = %e,
                    "lease_step failed at the database, not at the CAS — the step \
                     cannot be dispatched"
                );
                e
            })
            .unwrap_or(0);
        if rows == 0 {
            return None;
        }
        let lease_gen = conn
            .query_row(
                "SELECT lease_gen FROM steps WHERE id = ?1",
                params![step_id],
                |row| row.get::<_, i64>(0),
            )
            .ok();
        if let Some(lease_gen) = lease_gen {
            let context = step_event_context(&conn, step_id);
            insert_operations_event(
                &conn,
                context.as_ref().map(|context| context.user_id.as_str()),
                context
                    .as_ref()
                    .and_then(|context| context.group_id.as_deref()),
                None,
                context
                    .as_ref()
                    .and_then(|context| context.task_id.as_deref()),
                context.as_ref().map(|context| context.run_id.as_str()),
                Some(step_id),
                None,
                "step.leased",
                "step",
                step_id,
                &serde_json::json!({
                    "status": "leased",
                    "worker_id": worker_id,
                    "lease_gen": lease_gen,
                    "lease_deadline": deadline_ms,
                }),
            );
        }
        lease_gen
    }

    /// Record what actually ran, at dispatch.
    ///
    /// Idempotent on `(attempt_id, lease_gen)`: a resubmission under the same
    /// attempt is the same logical execution and must not produce a second row.
    /// Returns whether a row was inserted, so a duplicate is visible rather
    /// than silent.
    ///
    /// This is a provenance record, not a lifecycle state. Nothing reads it to
    /// decide whether a step succeeded.
    pub fn record_execution_job(
        &self,
        run_id: &str,
        job: &cortex_core::execution_job::ExecutionJob,
    ) -> bool {
        let conn = self.conn();
        let network_policy = serde_json::to_string(&job.network_policy)
            .unwrap_or_else(|_| "\"unserializable\"".to_string());
        let capability_grants =
            serde_json::to_string(&job.capability_grants).unwrap_or_else(|_| "[]".to_string());
        let resource_profile = serde_json::to_string(&job.resource_profile)
            .unwrap_or_else(|_| "\"unserializable\"".to_string());
        let effort_applied = serde_json::to_string(&job.effort_applied)
            .unwrap_or_else(|_| "\"unserializable\"".to_string());
        // NULL when the worker did not record it — a job from before scoped
        // egress. `'[]'` when it recorded that nothing was reachable. The two
        // are different facts and the column keeps them apart.
        let effective_egress = job
            .effective_egress
            .as_ref()
            .map(|hosts| serde_json::to_string(hosts).unwrap_or_else(|_| "[]".to_string()));

        conn.execute(
            "INSERT OR IGNORE INTO execution_jobs (
                job_id, job_version, run_id, step_id, attempt_id, lease_gen,
                model_catalog_id, model_catalog_ver, backend_kind,
                effort_requested, effort_applied,
                token_budget, wall_clock_ms, max_tool_calls,
                network_policy, capability_grants, context_bundle, packed_bytes,
                quote_id, plan_receipt_id,
                image_ref, isolation_class, resource_profile, profile_version,
                submitted_at,
                effective_egress, egress_mediator
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6,
                ?7, ?8, ?9,
                ?10, ?11,
                ?12, ?13, ?14,
                ?15, ?16, ?17, ?18,
                ?19, ?20,
                ?21, ?22, ?23, ?24,
                ?25,
                ?26, ?27
            )",
            params![
                job.job_id,
                job.job_version,
                run_id,
                job.step_id,
                job.attempt_id,
                job.lease_gen,
                job.model_ref.catalog_id,
                job.model_ref.catalog_version,
                job.backend_kind.as_str(),
                job.effort.map(|effort| effort.as_str()),
                effort_applied,
                job.budgets.token_budget.map(|budget| budget as i64),
                job.budgets.wall_clock.as_millis().min(u128::from(u64::MAX)) as i64,
                job.budgets.max_tool_calls.map(|calls| calls as i64),
                network_policy,
                capability_grants,
                job.context_bundle
                    .as_ref()
                    .map(|bundle| bundle.bundle_id.clone()),
                job.context_bundle
                    .as_ref()
                    .and_then(|bundle| bundle.packed_bytes)
                    .map(|bytes| bytes as i64),
                job.quote_id,
                job.plan_receipt_id,
                job.image_ref,
                job.isolation_class.as_str(),
                resource_profile,
                job.resource_profile.profile_version,
                Utc::now().timestamp_millis(),
                effective_egress,
                job.egress_mediator,
            ],
        )
        .unwrap_or(0)
            > 0
    }

    pub fn start_step(&self, step_id: &str, lease_gen: i64) -> bool {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let rows = conn
            .execute(
                "UPDATE steps SET status = 'running', updated_at = ?1, version = version + 1
             WHERE id = ?2 AND lease_gen = ?3 AND status IN ('leased', 'running')",
                params![now, step_id, lease_gen],
            )
            .unwrap_or(0);
        if rows > 0 {
            let context = step_event_context(&conn, step_id);
            insert_operations_event(
                &conn,
                context.as_ref().map(|context| context.user_id.as_str()),
                context
                    .as_ref()
                    .and_then(|context| context.group_id.as_deref()),
                None,
                context
                    .as_ref()
                    .and_then(|context| context.task_id.as_deref()),
                context.as_ref().map(|context| context.run_id.as_str()),
                Some(step_id),
                None,
                "step.started",
                "step",
                step_id,
                &serde_json::json!({
                    "status": "running",
                    "lease_gen": lease_gen,
                }),
            );
        }
        rows > 0
    }

    /// Record that a worker handed back a commit.
    ///
    /// This is **not** success. The step becomes `delivered`: a tree exists and
    /// nothing has been checked. Only [`Self::record_verification_outcome`] can
    /// produce `verified`, and only after our own runner has executed the
    /// checks frozen at dispatch. See `docs/adr/ADR-0001-step-truth-model.md`.
    ///
    /// One transaction. The projection, the lifecycle row, and the operations
    /// event all land together or none of them do — a status that moved without
    /// an event is invisible to the audit trail, and an event without a
    /// projection describes a state that never existed.
    ///
    /// The `lease_gen` CAS is preserved verbatim from the old `complete_step`:
    /// it is the reason a stale delivery cannot bill, and it is extended here
    /// rather than replaced.
    pub fn deliver_step(
        &self,
        step_id: &str,
        attempt_id: &str,
        lease_gen: i64,
        output_summary: Option<&str>,
        files_changed: Option<&str>,
        base_commit: Option<&str>,
        head_commit: Option<&str>,
    ) -> bool {
        let mut conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let tx = match conn.transaction() {
            Ok(tx) => tx,
            Err(err) => {
                tracing::error!(step_id, error = %err, "could not begin the delivery transaction");
                return false;
            }
        };

        let rows = tx
            .execute(
                "UPDATE steps SET status = 'delivered', output_summary = ?1, files_changed = ?2,
                 base_commit = ?3, head_commit = ?4, updated_at = ?5, version = version + 1
             WHERE id = ?6 AND lease_gen = ?7 AND status IN ('leased', 'running')",
                params![
                    output_summary,
                    files_changed,
                    base_commit,
                    head_commit,
                    now,
                    step_id,
                    lease_gen
                ],
            )
            .unwrap_or(0);
        if rows == 0 {
            return false;
        }

        if let Err(err) = upsert_verification_state(
            &tx,
            step_id,
            attempt_id,
            lease_gen,
            "delivered",
            None,
            now,
            // A delivery is the first state for this attempt, so there is no
            // prior state to advance from.
            &[],
        ) {
            tracing::error!(step_id, error = %err, "could not record the delivered state");
            return false;
        }

        let context = step_event_context(&tx, step_id);
        insert_operations_event(
            &tx,
            context.as_ref().map(|context| context.user_id.as_str()),
            context
                .as_ref()
                .and_then(|context| context.group_id.as_deref()),
            None,
            context
                .as_ref()
                .and_then(|context| context.task_id.as_deref()),
            context.as_ref().map(|context| context.run_id.as_str()),
            Some(step_id),
            None,
            "step.delivered",
            "step",
            step_id,
            &serde_json::json!({
                "status": "delivered",
                "attempt_id": attempt_id,
                "lease_gen": lease_gen,
                "output_summary": output_summary,
                "files_changed": files_changed,
                "base_commit": base_commit,
                "head_commit": head_commit,
            }),
        );

        match tx.commit() {
            Ok(()) => true,
            Err(err) => {
                tracing::error!(step_id, error = %err, "delivery transaction failed to commit");
                false
            }
        }
    }

    /// Hand a delivered step to our own verifier.
    ///
    /// Separate from [`Self::deliver_step`] because they are different facts:
    /// one says a tree exists, the other says we have started grading it. A
    /// step that is `delivered` but not yet `verifying` is a real and visible
    /// condition — it is what a customer sees while the runner is starting.
    /// Hand a delivered step to our own verifier, and enqueue the durable job
    /// that will do it — in one transaction.
    ///
    /// Design decision 4 of PR B: if the transition commits the job exists; if
    /// it rolls back neither happened. Nothing else may enqueue, because an
    /// enqueue that can happen on its own is a way to have a job without a
    /// state or a state without a job.
    pub fn begin_verifying_step(
        &self,
        step_id: &str,
        attempt_id: &str,
        lease_gen: i64,
        job: Option<VerificationEnqueue<'_>>,
    ) -> bool {
        self.transition_verification(
            step_id,
            attempt_id,
            lease_gen,
            "verifying",
            None,
            &["delivered"],
            "step.verifying",
            job,
        )
    }

    /// Seal a verdict produced by our own runner.
    ///
    /// `state` is one of `verified`, `failed`, or `inconclusive`. Nothing else
    /// may reach this function: a worker's opinion is a diagnostic and never
    /// arrives here.
    ///
    /// A result for a superseded attempt is a no-op. The `lease_gen` CAS is on
    /// both the projection and the lifecycle row, so a verifier that finishes
    /// after its step was re-leased cannot move the live attempt.
    pub fn record_verification_outcome(
        &self,
        step_id: &str,
        attempt_id: &str,
        lease_gen: i64,
        state: &str,
        reason: Option<&str>,
    ) -> bool {
        debug_assert!(
            matches!(state, "verified" | "failed" | "inconclusive"),
            "a verdict is verified, failed, or inconclusive — never anything else"
        );
        self.transition_verification(
            step_id,
            attempt_id,
            lease_gen,
            state,
            reason,
            &["verifying"],
            "step.verdict",
            None,
        )
    }

    /// Record that the delivery never happened.
    ///
    /// The sandbox could not be created, the image was missing, egress was
    /// unenforceable — our infrastructure failed before the step could produce
    /// a tree. Distinct from `failed`, which is a judgement about work that
    /// exists. Attributing an operator's infrastructure problem to a customer's
    /// step is exactly the confusion this state removes.
    pub fn record_execution_failure(
        &self,
        step_id: &str,
        attempt_id: &str,
        lease_gen: i64,
        reason: &str,
    ) -> bool {
        self.transition_verification(
            step_id,
            attempt_id,
            lease_gen,
            "execution_failed",
            Some(reason),
            // Reachable from a live attempt at any point before a verdict: the
            // sandbox can fail at submit, mid-run, or while checks are running.
            &["leased", "running", "delivered", "verifying"],
            "step.execution_failed",
            None,
        )
    }

    /// Record a human's decision to accept a step without a passing verdict.
    ///
    /// The override is a first-class fact with an actor, a reason, and an
    /// optional expiry. It is never `verified`: a verification badge that also
    /// means "somebody decided" means nothing.
    pub fn record_manual_override(
        &self,
        step_id: &str,
        attempt_id: &str,
        lease_gen: i64,
        actor_id: &str,
        reason: &str,
        expires_at: Option<i64>,
    ) -> bool {
        {
            let conn = self.conn();
            let now = Utc::now().timestamp_millis();
            if let Err(err) = conn.execute(
                "INSERT INTO manual_overrides
                     (step_id, attempt_id, actor_id, reason, expires_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(step_id, attempt_id) DO UPDATE SET
                     actor_id = excluded.actor_id,
                     reason = excluded.reason,
                     expires_at = excluded.expires_at,
                     created_at = excluded.created_at",
                params![step_id, attempt_id, actor_id, reason, expires_at, now],
            ) {
                tracing::error!(step_id, error = %err, "could not record the manual override");
                return false;
            }
        }
        self.transition_verification(
            step_id,
            attempt_id,
            lease_gen,
            "manual_override",
            Some(reason),
            // A human can accept work from any state where work exists, and
            // from an unanswered verdict. Not from a step that never ran.
            &["delivered", "verifying", "failed", "inconclusive"],
            "step.manual_override",
            None,
        )
    }

    /// The one place a verification-lifecycle transition happens.
    ///
    /// Every transition is one transaction carrying three writes — the step
    /// projection, the lifecycle row, and the operations event — plus two
    /// guards. `lease_gen` says *which attempt* this is about, and `from`
    /// restricts *which state* it may leave, so a duplicate or out-of-order
    /// message is a no-op rather than a state machine running backwards.
    #[allow(clippy::too_many_arguments)]
    fn transition_verification(
        &self,
        step_id: &str,
        attempt_id: &str,
        lease_gen: i64,
        to_state: &str,
        reason: Option<&str>,
        from: &[&str],
        event_kind: &str,
        enqueue: Option<VerificationEnqueue<'_>>,
    ) -> bool {
        let mut conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let tx = match conn.transaction() {
            Ok(tx) => tx,
            Err(err) => {
                tracing::error!(step_id, to_state, error = %err, "could not begin the transition");
                return false;
            }
        };

        let placeholders = std::iter::repeat_n("?", from.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE steps SET status = ?1, updated_at = ?2, version = version + 1
             WHERE id = ?3 AND lease_gen = ?4 AND status IN ({placeholders})"
        );
        let mut args: Vec<&dyn rusqlite::ToSql> = vec![&to_state, &now, &step_id, &lease_gen];
        for state in from {
            args.push(state);
        }
        let rows = tx.execute(&sql, args.as_slice()).unwrap_or(0);
        // A step that has reached a terminal state is no longer writing, so it
        // must not keep holding paths. Released inside the same transaction as
        // the transition: a release that committed separately could leave a
        // path held by a finished step if the process died between the two,
        // and nothing would ever free it except the TTL.
        if rows > 0 && matches!(to_state, "verified" | "failed" | "inconclusive") {
            let _ = tx.execute(
                "UPDATE resource_leases
                 SET status = 'released', released_at = ?1
                 WHERE step_id = ?2 AND holder_type = 'step' AND status = 'active'",
                params![now, step_id],
            );
        }
        if rows == 0 {
            // Either the attempt moved on or the step is not in a state this
            // transition may leave. Both mean: do nothing, quietly.
            tracing::debug!(
                step_id,
                lease_gen,
                to_state,
                "transition did not apply — stale attempt or unexpected source state"
            );
            return false;
        }

        if let Err(err) = upsert_verification_state(
            &tx, step_id, attempt_id, lease_gen, to_state, reason, now, from,
        ) {
            tracing::error!(step_id, to_state, error = %err, "could not record the lifecycle row");
            return false;
        }

        // The durable job, in the same transaction as the transition that
        // justifies it. This is the whole point of threading it through here
        // rather than exposing an enqueue anyone could call.
        if let Some(enqueue) = enqueue {
            if let Err(err) = Self::enqueue_verification_job_in(
                &tx,
                enqueue.job_id,
                enqueue.run_id,
                step_id,
                attempt_id,
                lease_gen,
                enqueue.delivered_commit,
                enqueue.spec_set_digest,
                enqueue.runner_policy_ver,
                now,
            ) {
                tracing::error!(step_id, error = %err, "could not enqueue the verification job");
                return false;
            }
        }

        let context = step_event_context(&tx, step_id);
        insert_operations_event(
            &tx,
            context.as_ref().map(|context| context.user_id.as_str()),
            context
                .as_ref()
                .and_then(|context| context.group_id.as_deref()),
            None,
            context
                .as_ref()
                .and_then(|context| context.task_id.as_deref()),
            context.as_ref().map(|context| context.run_id.as_str()),
            Some(step_id),
            None,
            event_kind,
            "step",
            step_id,
            &serde_json::json!({
                "status": to_state,
                "attempt_id": attempt_id,
                "lease_gen": lease_gen,
                "reason": reason,
            }),
        );

        match tx.commit() {
            Ok(()) => true,
            Err(err) => {
                tracing::error!(step_id, to_state, error = %err, "transition failed to commit");
                false
            }
        }
    }

    /// The verification lifecycle state for an attempt, if one was recorded.
    pub fn get_verification_state(
        &self,
        step_id: &str,
        attempt_id: &str,
        lease_gen: i64,
    ) -> Option<(String, i64)> {
        let conn = self.conn();
        conn.query_row(
            "SELECT state, version FROM step_verification_state
             WHERE step_id = ?1 AND attempt_id = ?2 AND lease_gen = ?3",
            params![step_id, attempt_id, lease_gen],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .ok()
    }

    /// The manual override recorded for an attempt, if any.
    pub fn get_manual_override(
        &self,
        step_id: &str,
        attempt_id: &str,
    ) -> Option<(String, String, Option<i64>)> {
        let conn = self.conn();
        conn.query_row(
            "SELECT actor_id, reason, expires_at FROM manual_overrides
             WHERE step_id = ?1 AND attempt_id = ?2",
            params![step_id, attempt_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            },
        )
        .ok()
    }

    pub fn fail_step(
        &self,
        step_id: &str,
        lease_gen: i64,
        error: &str,
        _failure_kind: Option<&str>,
    ) -> bool {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let rows = conn.execute(
            "UPDATE steps SET status = 'failed', last_error = ?1, updated_at = ?2, version = version + 1
             WHERE id = ?3 AND lease_gen = ?4 AND status IN ('leased', 'running')",
            params![error, now, step_id, lease_gen],
        ).unwrap_or(0);
        if rows > 0 {
            let context = step_event_context(&conn, step_id);
            insert_operations_event(
                &conn,
                context.as_ref().map(|context| context.user_id.as_str()),
                context
                    .as_ref()
                    .and_then(|context| context.group_id.as_deref()),
                None,
                context
                    .as_ref()
                    .and_then(|context| context.task_id.as_deref()),
                context.as_ref().map(|context| context.run_id.as_str()),
                Some(step_id),
                None,
                "step.failed",
                "step",
                step_id,
                &serde_json::json!({
                    "status": "failed",
                    "lease_gen": lease_gen,
                    "error": error,
                }),
            );
        }
        rows > 0
    }

    pub fn fail_unleased_step(
        &self,
        step_id: &str,
        error: &str,
        _failure_kind: Option<&str>,
    ) -> bool {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let rows = conn.execute(
            "UPDATE steps SET status = 'failed', last_error = ?1, updated_at = ?2, version = version + 1
             WHERE id = ?3 AND status IN ('pending', 'ready', 'orphaned')",
            params![error, now, step_id],
        ).unwrap_or(0);
        if rows > 0 {
            let context = step_event_context(&conn, step_id);
            insert_operations_event(
                &conn,
                context.as_ref().map(|context| context.user_id.as_str()),
                context
                    .as_ref()
                    .and_then(|context| context.group_id.as_deref()),
                None,
                context
                    .as_ref()
                    .and_then(|context| context.task_id.as_deref()),
                context.as_ref().map(|context| context.run_id.as_str()),
                Some(step_id),
                None,
                "step.failed",
                "step",
                step_id,
                &serde_json::json!({
                    "status": "failed",
                    "error": error,
                }),
            );
        }
        rows > 0
    }

    pub fn cancel_step(&self, step_id: &str, lease_gen: i64, reason: &str) -> bool {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let rows = conn
            .execute(
                "UPDATE steps SET status = 'cancelled', last_error = ?1, assigned_worker = NULL,
                 lease_deadline = NULL, updated_at = ?2, version = version + 1
             WHERE id = ?3 AND lease_gen = ?4 AND status IN ('leased', 'running')",
                params![reason, now, step_id, lease_gen],
            )
            .unwrap_or(0);
        if rows > 0 {
            let context = step_event_context(&conn, step_id);
            insert_operations_event(
                &conn,
                context.as_ref().map(|context| context.user_id.as_str()),
                context
                    .as_ref()
                    .and_then(|context| context.group_id.as_deref()),
                None,
                context
                    .as_ref()
                    .and_then(|context| context.task_id.as_deref()),
                context.as_ref().map(|context| context.run_id.as_str()),
                Some(step_id),
                None,
                "step.cancelled",
                "step",
                step_id,
                &serde_json::json!({
                    "status": "cancelled",
                    "lease_gen": lease_gen,
                    "reason": reason,
                }),
            );
        }
        rows > 0
    }

    pub fn cancel_assigned_step(&self, step_id: &str, reason: &str) -> bool {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let rows = conn
            .execute(
                "UPDATE steps SET status = 'cancelled', last_error = ?1, assigned_worker = NULL,
                 lease_deadline = NULL, updated_at = ?2, version = version + 1
             WHERE id = ?3 AND status IN ('leased', 'running')",
                params![reason, now, step_id],
            )
            .unwrap_or(0);
        if rows > 0 {
            insert_step_operations_event(
                &conn,
                step_id,
                "step.cancelled",
                &serde_json::json!({
                    "status": "cancelled",
                    "reason": reason,
                    "source": "assigned_step",
                }),
            );
        }
        rows > 0
    }

    pub fn mark_step_recovered(&self, step_id: &str) -> bool {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let rows = conn
            .execute(
                "UPDATE steps SET status = 'recovered', updated_at = ?1, version = version + 1
             WHERE id = ?2 AND status = 'failed'",
                params![now, step_id],
            )
            .unwrap_or(0);
        if rows > 0 {
            insert_step_operations_event(
                &conn,
                step_id,
                "step.recovered",
                &serde_json::json!({
                    "status": "recovered",
                }),
            );
        }
        rows > 0
    }

    pub fn record_failed_step_output(
        &self,
        step_id: &str,
        lease_gen: i64,
        output_summary: Option<&str>,
        files_changed: Option<&str>,
        base_commit: Option<&str>,
        head_commit: Option<&str>,
    ) -> bool {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let rows = conn
            .execute(
                "UPDATE steps
             SET output_summary = ?1,
                 files_changed = ?2,
                 base_commit = ?3,
                 head_commit = ?4,
                 updated_at = ?5,
                 version = version + 1
             WHERE id = ?6
               AND lease_gen = ?7
               AND status IN ('leased', 'running', 'failed')",
                params![
                    output_summary,
                    files_changed,
                    base_commit,
                    head_commit,
                    now,
                    step_id,
                    lease_gen
                ],
            )
            .unwrap_or(0);
        rows > 0
    }

    pub fn expire_stale_leases(&self) -> Vec<String> {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let mut stmt = conn.prepare(
            "UPDATE steps SET status = 'orphaned', assigned_worker = NULL, lease_deadline = NULL,
                 updated_at = ?1, version = version + 1
             WHERE status IN ('leased', 'running') AND lease_deadline < ?1
             RETURNING id"
        ).unwrap();

        stmt.query_map(params![now], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    pub fn expire_stale_resource_leases(&self) -> Vec<String> {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let expired: Vec<ResourceLease> = conn
            .prepare(
                "SELECT id, user_id, authority_scope_id, group_id, task_id, run_id, step_id, holder_type, resource_type,
                        repo_key, resource_key, mode, status, lease_gen, acquired_at, expires_at,
                        released_at, reason, metadata_json
                 FROM resource_leases
                 WHERE status = 'active' AND expires_at <= ?1",
            )
            .ok()
            .map(|mut stmt| {
                stmt.query_map(params![now], resource_lease_from_row)
                    .unwrap()
                    .filter_map(|row| row.ok())
                    .collect()
            })
            .unwrap_or_default();

        if expired.is_empty() {
            return Vec::new();
        }

        conn.execute(
            "UPDATE resource_leases
             SET status = 'expired', released_at = ?1
             WHERE status = 'active' AND expires_at <= ?1",
            params![now],
        )
        .ok();

        for lease in &expired {
            insert_operations_event(
                &conn,
                Some(&lease.user_id),
                lease.group_id.as_deref(),
                None,
                lease.task_id.as_deref(),
                Some(&lease.run_id),
                lease.step_id.as_deref(),
                None,
                "resource_lease.expired",
                "resource_lease",
                &lease.id,
                &serde_json::json!({
                    "holder_type": lease.holder_type,
                    "authority_scope_id": lease.authority_scope_id,
                    "resource_type": lease.resource_type,
                    "repo_key": lease.repo_key,
                    "resource_key": lease.resource_key,
                    "mode": lease.mode,
                    "expired_at": now,
                }),
            );
        }

        expired.into_iter().map(|lease| lease.id).collect()
    }

    pub fn list_active_resource_leases_for_run(&self, run_id: &str) -> Vec<ResourceLease> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, authority_scope_id, group_id, task_id, run_id, step_id, holder_type, resource_type,
                        repo_key, resource_key, mode, status, lease_gen, acquired_at, expires_at,
                        released_at, reason, metadata_json
                 FROM resource_leases
                 WHERE run_id = ?1 AND status = 'active'
                 ORDER BY resource_type ASC, resource_key ASC",
            )
            .unwrap();
        stmt.query_map(params![run_id], resource_lease_from_row)
            .unwrap()
            .filter_map(|row| row.ok())
            .collect()
    }

    // --- Step Attempts ---

    pub fn record_attempt(
        &self,
        step_id: &str,
        run_id: &str,
        attempt_number: i32,
        worker_id: &str,
        lease_gen: i64,
        provider: Option<&str>,
        model: Option<&str>,
    ) {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO step_attempts (step_id, run_id, attempt_number, worker_id, lease_gen, status, provider, model, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'started', ?6, ?7, ?8)",
            params![step_id, run_id, attempt_number, worker_id, lease_gen, provider, model, now],
        ).ok();
    }

    /// Close out the attempt row for a delivery.
    ///
    /// This is the worker's own record of its own attempt — the legacy report
    /// kept as a diagnostic. It says `delivered`, not `succeeded`, because it
    /// reaches the frontend in `RunStepAttemptSnapshot` and a worker's opinion
    /// rendered as success next to an unverified step is the exact confusion
    /// this model removes.
    pub fn deliver_attempt(&self, step_id: &str, lease_gen: i64) {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE step_attempts SET status = 'delivered', finished_at = ?1
             WHERE step_id = ?2 AND lease_gen = ?3 AND status = 'started'",
            params![now, step_id, lease_gen],
        )
        .ok();
    }

    pub fn fail_attempt(
        &self,
        step_id: &str,
        lease_gen: i64,
        failure_kind: Option<&str>,
        error: Option<&str>,
    ) {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE step_attempts SET status = 'failed', finished_at = ?1, failure_kind = ?2, error_summary = ?3
             WHERE step_id = ?4 AND lease_gen = ?5 AND status = 'started'",
            params![now, failure_kind, error, step_id, lease_gen],
        ).ok();
    }

    // --- Decisions & Outcomes ---

    pub fn record_decision(
        &self,
        id: &str,
        user_id: &str,
        run_id: Option<&str>,
        step_id: Option<&str>,
        intent: &str,
        risk: &str,
        tier: &str,
        provider: &str,
        model: &str,
        worker_id: Option<&str>,
        rationale: &str,
        profile: &str,
    ) {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO decisions (id, user_id, run_id, step_id, timestamp, intent, risk, tier, provider, model, worker_id, rationale, profile)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![id, user_id, run_id, step_id, now, intent, risk, tier, provider, model, worker_id, rationale, profile],
        ).ok();
    }

    pub fn record_outcome(
        &self,
        decision_id: &str,
        success: bool,
        duration_ms: Option<i64>,
        failure_kind: Option<&str>,
        failure_scope: Option<&str>,
        files_changed: Option<&str>,
        exit_code: Option<i32>,
    ) {
        let conn = self.conn();
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO outcomes (id, decision_id, timestamp, success, duration_ms, failure_kind, failure_scope, files_changed, exit_code)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![id, decision_id, now, success as i32, duration_ms, failure_kind, failure_scope, files_changed, exit_code],
        ).ok();
    }

    // --- Usage Events ---

    pub fn record_usage(
        &self,
        user_id: &str,
        provider: &str,
        tier: &str,
        model: &str,
        worker_id: Option<&str>,
        tokens_in: Option<i64>,
        tokens_out: Option<i64>,
        duration_ms: Option<i64>,
    ) {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO usage_events (user_id, timestamp, provider, tier, model, worker_id, tokens_in, tokens_out, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![user_id, now, provider, tier, model, worker_id, tokens_in, tokens_out, duration_ms],
        ).ok();
    }

    /// Compute pressure per (provider, tier) using exponential time-decay.
    ///
    /// Each usage event's tokens are weighted by `exp(-lambda * age_seconds)` where
    /// `lambda = ln(2) / HALF_LIFE_SECS`. This means usage from 1 hour ago counts 50%,
    /// 2 hours ago 25%, etc. Events older than `window_ms` are still excluded entirely.
    pub fn pressure_for_user(&self, user_id: &str, window_ms: i64) -> Vec<(String, String, i64)> {
        const HALF_LIFE_SECS: f64 = 3600.0; // 1 hour
        let lambda = (2.0_f64).ln() / HALF_LIFE_SECS;

        let conn = self.conn();
        let now_ms = Utc::now().timestamp_millis();
        let cutoff = now_ms - window_ms;

        // Fetch individual events so we can apply per-event decay weights
        let mut stmt = conn
            .prepare(
                "SELECT provider, tier, timestamp, COALESCE(tokens_in, 0) + COALESCE(tokens_out, 0)
             FROM usage_events
             WHERE user_id = ?1 AND timestamp > ?2
             ORDER BY provider, tier",
            )
            .unwrap();

        let rows: Vec<(String, String, i64, i64)> = stmt
            .query_map(params![user_id, cutoff], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        // Aggregate with exponential decay
        let mut accum: std::collections::HashMap<(String, String), f64> =
            std::collections::HashMap::new();
        for (provider, tier, ts, tokens) in rows {
            let age_secs = ((now_ms - ts) as f64 / 1000.0).max(0.0);
            let weight = (-lambda * age_secs).exp();
            *accum.entry((provider, tier)).or_insert(0.0) += tokens as f64 * weight;
        }

        accum
            .into_iter()
            .map(|((provider, tier), weighted)| (provider, tier, weighted.round() as i64))
            .collect()
    }

    // --- Provider Reliability ---

    pub fn provider_reliability(&self, user_id: &str, hours: i64) -> Vec<(String, i64, i64)> {
        let conn = self.conn();
        let cutoff = Utc::now().timestamp_millis() - (hours * 3600 * 1000);
        let mut stmt = conn
            .prepare(
                "SELECT d.provider, COUNT(*) as total, SUM(o.success) as successes
             FROM outcomes o
             JOIN decisions d ON o.decision_id = d.id
             WHERE o.timestamp > ?1 AND d.user_id = ?2
             GROUP BY d.provider",
            )
            .unwrap();

        stmt.query_map(params![cutoff, user_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    // --- Idempotency ---

    pub fn check_idempotency(&self, key: &str) -> Option<String> {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        conn.query_row(
            "SELECT result_json FROM idempotency_keys WHERE key = ?1 AND expires_at > ?2",
            params![key, now],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    }

    pub fn set_idempotency(&self, key: &str, result: Option<&str>, ttl_ms: i64) {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "INSERT OR REPLACE INTO idempotency_keys (key, result_json, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
            params![key, result, now, now + ttl_ms],
        ).ok();
    }

    pub fn cleanup_expired_idempotency(&self) -> usize {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "DELETE FROM idempotency_keys WHERE expires_at < ?1",
            params![now],
        )
        .unwrap_or(0)
    }

    // --- Scheduler helpers ---

    pub fn create_step_with_id(
        &self,
        id: &str,
        run_id: &str,
        kind: &str,
        work_kind: &str,
        tier: &str,
        risk: &str,
        objective: &str,
        created_at: i64,
    ) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO steps (id, run_id, kind, work_kind, status, tier, risk, objective, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6, ?7, ?8, ?8)",
            params![id, run_id, kind, work_kind, tier, risk, objective, created_at],
        ).expect("failed to create step");
        insert_step_operations_event(
            &conn,
            id,
            "step.planned",
            &serde_json::json!({
                "status": "pending",
                "kind": kind,
                "work_kind": work_kind,
                "tier": tier,
                "risk": risk,
                "objective": objective,
            }),
        );
    }

    /// Returns (provider, kind, risk) for bandit outcome tracking.
    pub fn get_step_info(&self, step_id: &str) -> Option<(String, String, String)> {
        let conn = self.conn();
        conn.query_row(
            "SELECT COALESCE(a.provider, 'Claude'), s.kind, s.risk
             FROM steps s
             LEFT JOIN step_attempts a ON a.step_id = s.id
             WHERE s.id = ?1
             ORDER BY a.attempt_number DESC
             LIMIT 1",
            params![step_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .ok()
    }

    pub fn get_step_details(
        &self,
        step_id: &str,
    ) -> Option<(String, String, String, String, String)> {
        let conn = self.conn();
        conn.query_row(
            "SELECT kind, work_kind, tier, risk, objective FROM steps WHERE id = ?1",
            params![step_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .ok()
    }

    pub fn get_step_recipe_seed_json(&self, step_id: &str) -> Option<String> {
        let conn = self.conn();
        conn.query_row(
            "SELECT recipe_seed_json FROM steps WHERE id = ?1",
            params![step_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    }

    /// The intent the router classified this step as, if one was recorded.
    ///
    /// Read from `decisions` rather than from `steps`, because intent is a
    /// routing classification and `steps.kind` is a plan-shape one — they are
    /// not the same vocabulary and mapping between them would invent a fact.
    /// The most recent decision wins: a re-leased step was routed again, and
    /// the last routing is the one the verdict is about.
    ///
    /// `None` when nothing was recorded, which the caller must treat as
    /// "unknown" rather than substituting a default — the intent selects which
    /// capability the trust signal lands under, so guessing it puts the
    /// interaction in the wrong domain.
    ///
    /// The parse is a hand-written match, not serde, and that is not a
    /// preference. `record_decision` is called with `format!("{:?}", intent)`
    /// (`scheduler.rs`), so the column holds `"Fix"` — while `Intent` carries
    /// `#[serde(rename_all = "snake_case")]` and would only accept `"fix"`.
    /// Round-tripping through serde here compiles, type-checks, and returns
    /// `None` for every row ever written.
    pub fn get_step_intent(&self, step_id: &str) -> Option<cortex_core::routing::Intent> {
        use cortex_core::routing::Intent;
        let conn = self.conn();
        let raw: String = conn
            .query_row(
                "SELECT intent FROM decisions WHERE step_id = ?1
                 ORDER BY timestamp DESC LIMIT 1",
                params![step_id],
                |row| row.get(0),
            )
            .ok()?;
        // Accepts either spelling, so a future writer that switches to the
        // serde form does not silently blind this.
        match raw.trim().to_ascii_lowercase().as_str() {
            "fix" => Some(Intent::Fix),
            "add" => Some(Intent::Add),
            "explore" => Some(Intent::Explore),
            "review" => Some(Intent::Review),
            "think" => Some(Intent::Think),
            "test" => Some(Intent::Test),
            "refactor" => Some(Intent::Refactor),
            "ship" => Some(Intent::Ship),
            other => {
                tracing::debug!(intent = other, "unrecognised intent on a decision row");
                None
            }
        }
    }

    /// What the whole attempt chain for a step cost.
    ///
    /// A step that was verified on its fourth try cost four dispatches, not
    /// one. A routing signal that only ever sees the attempt that happened to
    /// succeed cannot tell a model that gets it right first time from one that
    /// needs coaxing — and the second is the more expensive model, which is
    /// exactly what the signal exists to notice.
    ///
    /// `attempts` is the count, which is integer-denominated by construction —
    /// the unit the ledger uses and the one CREDITS.md insists on. Wall time
    /// comes back alongside it as evidence, not as the unit.
    pub fn attempt_chain_spend(&self, step_id: &str) -> AttemptChain {
        let conn = self.conn();
        conn.query_row(
            // An attempt still in flight has no `finished_at`; it contributes
            // to the count and nothing to the duration, rather than being
            // dropped or counted as zero-length by coalescing to `started_at`.
            "SELECT COUNT(*),
                    COALESCE(SUM(CASE WHEN finished_at IS NOT NULL
                                      THEN finished_at - started_at END), 0)
             FROM step_attempts WHERE step_id = ?1",
            params![step_id],
            |row| {
                Ok(AttemptChain {
                    attempts: row.get::<_, i64>(0)?.max(0) as u64,
                    total_duration_ms: row.get::<_, i64>(1)?.max(0) as u64,
                })
            },
        )
        .unwrap_or_default()
    }

    /// Take path leases for one step, or report the first conflict.
    ///
    /// **Step scope, not run scope.** A run-scoped lease is held from run creation
    /// until the run finishes, so two runs touching the same directory serialise
    /// end to end even when only one step in each actually writes there. Holding at
    /// step scope shortens that to the step, which is the whole point of PR R.
    ///
    /// Conflict is reported, not raised: the caller leaves the step pending and
    /// tries again next tick. That is the queueing behaviour — a step waits for a
    /// path instead of a run failing to be created.
    ///
    /// One transaction: every key is checked before any is inserted, so two
    /// dispatchers cannot each acquire half of an overlapping pair. Keys are taken
    /// in canonical order (the scheduler sorts them) so concurrent acquirers agree.
    pub fn acquire_step_path_leases(
        &self,
        user_id: &str,
        run_id: &str,
        step_id: &str,
        repo_key: &str,
        keys: &[String],
    ) -> Result<(), ResourceLeaseConflict> {
        if keys.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let expires_at = now + RUN_RESOURCE_LEASE_TTL_MS;

        let tx = match conn.transaction() {
            Ok(tx) => tx,
            Err(err) => {
                tracing::error!(step_id, error = %err, "could not begin lease acquisition");
                // Treat an unopenable transaction as a conflict so the step waits
                // rather than dispatching unleased. Failing open here would put two
                // steps in the same directory, which is the thing being prevented.
                return Err(ResourceLeaseConflict {
                    lease_id: String::new(),
                    run_id: run_id.to_string(),
                    step_id: Some(step_id.to_string()),
                    holder_type: "step".to_string(),
                    resource_type: "path".to_string(),
                    repo_key: repo_key.to_string(),
                    resource_key: keys[0].clone(),
                    mode: "write".to_string(),
                    expires_at: now,
                });
            }
        };

        tx.execute(
            "UPDATE resource_leases
             SET status = 'expired', released_at = ?1
             WHERE status = 'active' AND expires_at <= ?1",
            params![now],
        )
        .ok();

        for key in keys {
            let request = ResourceLeaseRequest {
                resource_type: "path".to_string(),
                repo_key: repo_key.to_string(),
                resource_key: key.clone(),
                mode: "write".to_string(),
                reason: Some("step write set".to_string()),
                metadata: serde_json::json!({ "repo_key": repo_key, "path": key }),
            };
            match find_resource_lease_conflict_tx(&tx, user_id, None, &request, now) {
                Ok(Some(mut conflict)) => {
                    // A lease this same step already holds is not a conflict — a
                    // redispatch after a retry must not deadlock against itself.
                    if conflict.step_id.as_deref() == Some(step_id) {
                        continue;
                    }
                    conflict.resource_key = key.clone();
                    return Err(conflict);
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::error!(step_id, error = %err, "lease conflict check failed");
                    return Err(ResourceLeaseConflict {
                        lease_id: String::new(),
                        run_id: run_id.to_string(),
                        step_id: Some(step_id.to_string()),
                        holder_type: "step".to_string(),
                        resource_type: "path".to_string(),
                        repo_key: repo_key.to_string(),
                        resource_key: key.clone(),
                        mode: "write".to_string(),
                        expires_at: now,
                    });
                }
            }
        }

        for key in keys {
            let id = Uuid::new_v4().to_string();
            let metadata = serde_json::json!({ "repo_key": repo_key, "path": key }).to_string();
            if let Err(err) = tx.execute(
                "INSERT INTO resource_leases (
                    id, user_id, authority_scope_id, group_id, task_id, run_id, step_id,
                    holder_type, resource_type, repo_key, resource_key, mode, status,
                    lease_gen, acquired_at, expires_at, reason, metadata_json
                 )
                 VALUES (?1, ?2, NULL, NULL, NULL, ?3, ?4, 'step', 'path', ?5, ?6, 'write',
                         'active', 1, ?7, ?8, 'step write set', ?9)",
                params![id, user_id, run_id, step_id, repo_key, key, now, expires_at, metadata],
            ) {
                tracing::error!(step_id, error = %err, "could not insert a step lease");
            }
        }

        if let Err(err) = tx.commit() {
            tracing::error!(step_id, error = %err, "could not commit step leases");
            return Err(ResourceLeaseConflict {
                lease_id: String::new(),
                run_id: run_id.to_string(),
                step_id: Some(step_id.to_string()),
                holder_type: "step".to_string(),
                resource_type: "path".to_string(),
                repo_key: repo_key.to_string(),
                resource_key: keys[0].clone(),
                mode: "write".to_string(),
                expires_at: now,
            });
        }
        Ok(())
    }

    /// Release every lease this step holds.
    ///
    /// Called when a step reaches a terminal state. Idempotent — releasing twice is
    /// a no-op, which matters because the transition that calls it carries a CAS
    /// and may legitimately run for an attempt that has already been superseded.
    pub fn release_step_resource_leases(&self, step_id: &str) -> usize {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE resource_leases
             SET status = 'released', released_at = ?1
             WHERE step_id = ?2 AND holder_type = 'step' AND status = 'active'",
            params![now, step_id],
        )
        .unwrap_or(0)
    }

    /// The paths a step declared it would write, from its planner seed.
    ///
    /// `None` when the step has no seed or the seed declares nothing — which the
    /// caller must treat as *unknown*, not as *nothing*. A step that declared no
    /// paths is repo-wide, exactly as today.
    pub fn get_step_target_paths(&self, step_id: &str) -> Option<Vec<String>> {
        let conn = self.conn();
        let raw: Option<String> = conn
            .query_row(
                "SELECT recipe_seed_json FROM steps WHERE id = ?1",
                params![step_id],
                |row| row.get(0),
            )
            .ok()?;
        let seed: serde_json::Value = serde_json::from_str(&raw?).ok()?;
        let paths: Vec<String> = seed
            .get("target_paths")?
            .as_array()?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        if paths.is_empty() {
            None
        } else {
            Some(paths)
        }
    }

    /// The repo this run is scoped to, for keying leases.
    ///
    /// `None` when unset, which the caller normalises to `"default"` — the same
    /// value `build_resource_lease_requests` uses, so a step lease and a run
    /// lease on the same repo land on the same key rather than silently
    /// failing to contend.
    /// Hand out the next value on a sequence.
    ///
    /// The scheduler decides the number; the agent is told it. That removes an
    /// entire class of conflict rather than scheduling around it — a migration
    /// version chosen by two agents is a conflict git cannot see, because both
    /// files are syntactically clean and the merge succeeds while the meaning
    /// is wrong.
    ///
    /// Correctness rests on the unique index over `(repo_key, sequence, value)`,
    /// not on the read: two callers can read the same max, and only one insert
    /// can commit. The loser retries and gets the next value. A `SELECT max`
    /// followed by an unguarded insert would be the same race the mechanism is
    /// supposed to remove, moved one layer down.
    ///
    /// `start_at` seeds an empty sequence — for a repository already on v65,
    /// the first allocation must be 66, not 1.
    pub fn allocate_sequence_value(
        &self,
        repo_key: &str,
        sequence: &str,
        start_at: i64,
        run_id: Option<&str>,
        step_id: Option<&str>,
    ) -> Option<i64> {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();

        // Bounded: each attempt loses only to a genuine concurrent winner, so
        // the loop terminates in the number of concurrent allocators. The cap
        // stops an unexpected constraint failure spinning forever.
        for _ in 0..16 {
            let next: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(value), ?3 - 1) + 1
                     FROM sequence_allocations WHERE repo_key = ?1 AND sequence = ?2",
                    params![repo_key, sequence, start_at],
                    |row| row.get(0),
                )
                .unwrap_or(start_at);

            match conn.execute(
                "INSERT INTO sequence_allocations
                    (repo_key, sequence, value, step_id, run_id, allocated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![repo_key, sequence, next, step_id, run_id, now],
            ) {
                Ok(_) => return Some(next),
                // Someone else took this value. Re-read and try the next.
                Err(rusqlite::Error::SqliteFailure(err, _))
                    if err.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    continue;
                }
                Err(err) => {
                    tracing::error!(repo_key, sequence, error = %err, "sequence allocation failed");
                    return None;
                }
            }
        }
        tracing::error!(
            repo_key,
            sequence,
            "sequence allocation gave up after 16 attempts"
        );
        None
    }

    /// The highest value handed out on a sequence, if any.
    pub fn latest_sequence_value(&self, repo_key: &str, sequence: &str) -> Option<i64> {
        let conn = self.conn();
        conn.query_row(
            "SELECT MAX(value) FROM sequence_allocations WHERE repo_key = ?1 AND sequence = ?2",
            params![repo_key, sequence],
            |row| row.get::<_, Option<i64>>(0),
        )
        .ok()
        .flatten()
    }

    pub fn get_run_repo_key(&self, run_id: &str) -> Option<String> {
        let conn = self.conn();
        conn.query_row(
            "SELECT repo_key FROM runs WHERE id = ?1",
            params![run_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    }

    pub fn get_run_user_id(&self, run_id: &str) -> Option<String> {
        let conn = self.conn();
        conn.query_row(
            "SELECT user_id FROM runs WHERE id = ?1",
            params![run_id],
            |row| row.get(0),
        )
        .ok()
    }

    /// Check if a run belongs to the given user. Returns true if the run exists and is owned by user_id.
    pub fn verify_run_owner(&self, run_id: &str, user_id: &str) -> bool {
        let conn = self.conn();
        conn.query_row(
            "SELECT 1 FROM runs WHERE id = ?1 AND user_id = ?2",
            params![run_id, user_id],
            |_| Ok(()),
        )
        .is_ok()
    }

    /// List runs for a specific user, ordered by creation time descending.
    pub fn list_user_runs(
        &self,
        user_id: &str,
        limit: usize,
        offset: usize,
    ) -> Vec<serde_json::Value> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, goal, status, profile, created_at, updated_at, started_at, finished_at, heal_attempts,
                    task_id, group_id, conversation_id
             FROM runs WHERE user_id = ?1
             ORDER BY created_at DESC LIMIT ?2 OFFSET ?3"
        ).unwrap();
        stmt.query_map(params![user_id, limit as i64, offset as i64], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "goal": row.get::<_, String>(1)?,
                "status": row.get::<_, String>(2)?,
                "profile": row.get::<_, String>(3)?,
                "created_at": row.get::<_, i64>(4)?,
                "updated_at": row.get::<_, i64>(5)?,
                "started_at": row.get::<_, Option<i64>>(6)?,
                "finished_at": row.get::<_, Option<i64>>(7)?,
                "heal_attempts": row.get::<_, i32>(8)?,
                "task_id": row.get::<_, Option<String>>(9)?,
                "group_id": row.get::<_, Option<String>>(10)?,
                "conversation_id": row.get::<_, Option<String>>(11)?,
            }))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn get_run_goal(&self, run_id: &str) -> Option<String> {
        let conn = self.conn();
        conn.query_row(
            "SELECT goal FROM runs WHERE id = ?1",
            params![run_id],
            |row| row.get(0),
        )
        .ok()
    }

    /// Return timing metadata for a single run (used for PR body generation).
    pub fn list_user_runs_by_id(&self, run_id: &str) -> Option<serde_json::Value> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, goal, status, profile, created_at, updated_at, started_at, finished_at,
                    task_id, group_id, conversation_id
             FROM runs WHERE id = ?1",
            params![run_id],
            |row| {
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "goal": row.get::<_, String>(1)?,
                    "status": row.get::<_, String>(2)?,
                    "profile": row.get::<_, String>(3)?,
                    "created_at": row.get::<_, i64>(4)?,
                    "updated_at": row.get::<_, i64>(5)?,
                    "started_at": row.get::<_, Option<i64>>(6)?,
                    "finished_at": row.get::<_, Option<i64>>(7)?,
                    "task_id": row.get::<_, Option<String>>(8)?,
                    "group_id": row.get::<_, Option<String>>(9)?,
                    "conversation_id": row.get::<_, Option<String>>(10)?,
                }))
            },
        )
        .ok()
    }

    pub fn get_run_heal_count(&self, run_id: &str) -> i32 {
        let conn = self.conn();
        conn.query_row(
            "SELECT heal_attempts FROM runs WHERE id = ?1",
            params![run_id],
            |row| row.get::<_, i32>(0),
        )
        .unwrap_or(0)
    }

    pub fn increment_heal_count(&self, run_id: &str) {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let rows = conn
            .execute(
                "UPDATE runs SET heal_attempts = heal_attempts + 1, updated_at = ?1 WHERE id = ?2",
                params![now, run_id],
            )
            .unwrap_or(0);
        if rows > 0 {
            let heal_attempts = conn
                .query_row(
                    "SELECT heal_attempts FROM runs WHERE id = ?1",
                    params![run_id],
                    |row| row.get::<_, i32>(0),
                )
                .unwrap_or(0);
            insert_run_operations_event(
                &conn,
                run_id,
                "run.heal_incremented",
                &serde_json::json!({
                    "heal_attempts": heal_attempts,
                }),
            );
        }
    }

    pub fn get_step_last_error(&self, step_id: &str) -> Option<String> {
        let conn = self.conn();
        conn.query_row(
            "SELECT last_error FROM steps WHERE id = ?1",
            params![step_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    }

    /// The current lifecycle status of one step.
    ///
    /// Callers that need to know whether work may be built on, charged for, or
    /// rewarded must read this rather than infer it from a worker's message.
    pub fn get_step_status(&self, step_id: &str) -> Option<String> {
        let conn = self.conn();
        conn.query_row(
            "SELECT status FROM steps WHERE id = ?1",
            params![step_id],
            |row| row.get::<_, String>(0),
        )
        .ok()
    }

    pub fn get_all_step_statuses(&self, run_id: &str) -> Vec<(String, String)> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, status FROM steps WHERE run_id = ?1 ORDER BY created_at ASC, id ASC",
            )
            .unwrap();
        stmt.query_map(params![run_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn get_run_step_snapshots(&self, run_id: &str) -> Vec<RunStepSnapshot> {
        let conn = self.conn();

        let mut predecessors_by_step: HashMap<String, Vec<String>> = HashMap::new();
        let mut predecessor_stmt = conn
            .prepare(
                "SELECT sd.step_id, sd.depends_on_id
             FROM step_dependencies sd
             JOIN steps s ON s.id = sd.step_id
             WHERE s.run_id = ?1
             ORDER BY sd.step_id ASC, sd.depends_on_id ASC",
            )
            .unwrap();
        for row in predecessor_stmt
            .query_map(params![run_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .filter_map(|r| r.ok())
        {
            predecessors_by_step.entry(row.0).or_default().push(row.1);
        }

        let mut verifier_by_step: HashMap<String, VerifierReport> = HashMap::new();
        let mut verifier_stmt = conn
            .prepare(
                "SELECT id, step_id, run_id, lease_gen, worker_id, verifier, status, verdict,
                    evidence_json, created_at, updated_at
             FROM verifier_reports
             WHERE run_id = ?1
             ORDER BY step_id ASC, created_at DESC",
            )
            .unwrap();
        for report in verifier_stmt
            .query_map(params![run_id], |row| {
                Ok(VerifierReport {
                    id: row.get(0)?,
                    step_id: row.get(1)?,
                    run_id: row.get(2)?,
                    lease_gen: row.get(3)?,
                    worker_id: row.get(4)?,
                    verifier: row.get(5)?,
                    status: row.get(6)?,
                    verdict: row.get(7)?,
                    evidence_json: row.get(8)?,
                    created_at: row.get(9)?,
                    updated_at: row.get(10)?,
                })
            })
            .unwrap()
            .filter_map(|r| r.ok())
        {
            verifier_by_step
                .entry(report.step_id.clone())
                .or_insert(report);
        }

        let mut contract_by_step: HashMap<String, TaskContract> = HashMap::new();
        let mut contract_stmt = conn
            .prepare(
                "SELECT step_id, contract_json
             FROM step_work_contracts
             WHERE run_id = ?1
             ORDER BY step_id ASC, lease_gen DESC, created_at DESC",
            )
            .unwrap();
        for (step_id, contract_json) in contract_stmt
            .query_map(params![run_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .filter_map(|r| r.ok())
        {
            if contract_by_step.contains_key(&step_id) {
                continue;
            }
            match serde_json::from_str::<TaskContract>(&contract_json) {
                Ok(contract) => {
                    contract_by_step.insert(step_id, contract);
                }
                Err(err) => {
                    tracing::error!(
                        step_id = %step_id,
                        error = %err,
                        "failed to deserialize latest step work contract"
                    );
                }
            }
        }

        let mut attempt_by_step: HashMap<String, RunStepAttemptSnapshot> = HashMap::new();
        let mut attempt_stmt = conn
            .prepare(
                "SELECT step_id, attempt_number, worker_id, lease_gen, status, provider, model,
                    started_at, finished_at, failure_kind, error_summary
             FROM step_attempts
             WHERE run_id = ?1
             ORDER BY step_id ASC, attempt_number DESC",
            )
            .unwrap();
        for attempt in attempt_stmt
            .query_map(params![run_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    RunStepAttemptSnapshot {
                        attempt_number: row.get(1)?,
                        worker_id: row.get(2)?,
                        lease_gen: row.get(3)?,
                        status: row.get(4)?,
                        provider: row.get(5)?,
                        model: row.get(6)?,
                        started_at: row.get(7)?,
                        finished_at: row.get(8)?,
                        failure_kind: row.get(9)?,
                        error_summary: row.get(10)?,
                    },
                ))
            })
            .unwrap()
            .filter_map(|r| r.ok())
        {
            attempt_by_step.entry(attempt.0).or_insert(attempt.1);
        }

        let mut stmt = conn.prepare(
            "SELECT id, status, kind, work_kind, tier, risk, objective, attempt_count, max_attempts,
                    lease_gen, lease_deadline, assigned_worker, recipe_seed_json, output_summary,
                    files_changed, last_error
             FROM steps
             WHERE run_id = ?1
             ORDER BY created_at ASC, id ASC"
        ).unwrap();
        stmt.query_map(params![run_id], |row| {
            let id = row.get::<_, String>(0)?;
            Ok(RunStepSnapshot {
                predecessors: predecessors_by_step.remove(&id).unwrap_or_default(),
                verifier_report: verifier_by_step.remove(&id),
                work_contract: contract_by_step.remove(&id),
                latest_attempt: attempt_by_step.remove(&id),
                id,
                status: row.get(1)?,
                kind: row.get(2)?,
                work_kind: row.get(3)?,
                tier: row.get(4)?,
                risk: row.get(5)?,
                objective: row.get(6)?,
                attempt_count: row.get(7)?,
                max_attempts: row.get(8)?,
                lease_gen: row.get(9)?,
                lease_deadline: row.get(10)?,
                assigned_worker: row.get(11)?,
                recipe_seed_json: row.get(12)?,
                output_summary: row.get(13)?,
                files_changed: row.get(14)?,
                last_error: row.get(15)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn get_step_run_id(&self, step_id: &str) -> Option<String> {
        let conn = self.conn();
        conn.query_row(
            "SELECT run_id FROM steps WHERE id = ?1",
            params![step_id],
            |row| row.get(0),
        )
        .ok()
    }

    pub fn get_active_run_ids(&self) -> Vec<String> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT id FROM runs WHERE status IN ('planning', 'running')")
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    pub fn get_provider_status(&self, user_id: &str, provider: &str) -> Option<String> {
        let conn = self.conn();
        conn.query_row(
            "SELECT status FROM provider_capabilities WHERE user_id = ?1 AND provider = ?2
             ORDER BY last_reported DESC LIMIT 1",
            params![user_id, provider],
            |row| row.get(0),
        )
        .ok()
    }

    pub fn get_user_profile(&self, user_id: &str) -> Option<String> {
        let conn = self.conn();
        conn.query_row(
            "SELECT active_profile FROM user_profiles WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )
        .ok()
    }

    pub fn get_user_auto_mode(&self, user_id: &str) -> Option<String> {
        let conn = self.conn();
        conn.query_row(
            "SELECT auto_mode FROM user_profiles WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )
        .ok()
    }

    pub fn record_score_evidence(
        &self,
        decision_id: &str,
        evaluator: &str,
        evidence_json: &str,
        score: Option<f64>,
        timestamp: i64,
    ) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO score_evidence (decision_id, evaluator, evidence_json, score, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![decision_id, evaluator, evidence_json, score, timestamp],
        )
        .ok();
    }

    pub fn get_step_predecessors(&self, step_id: &str) -> Vec<String> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT depends_on_id FROM step_dependencies WHERE step_id = ?1 ORDER BY depends_on_id ASC"
        ).unwrap();
        stmt.query_map(params![step_id], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    pub fn get_run_step_dependency_edges(&self, run_id: &str) -> Vec<StepDependencyEdge> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT sd.step_id, sd.depends_on_id, sd.edge_type
             FROM step_dependencies sd
             JOIN steps s ON s.id = sd.step_id
             WHERE s.run_id = ?1
             ORDER BY sd.step_id ASC, sd.depends_on_id ASC",
            )
            .unwrap();
        stmt.query_map(params![run_id], |row| {
            Ok(StepDependencyEdge {
                step_id: row.get::<_, String>(0)?,
                depends_on_id: row.get::<_, String>(1)?,
                edge_type: row.get::<_, String>(2)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn get_step_output_summary(&self, step_id: &str) -> Option<String> {
        let conn = self.conn();
        conn.query_row(
            "SELECT output_summary FROM steps WHERE id = ?1",
            params![step_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    }

    pub fn get_step_files_changed(&self, step_id: &str) -> Option<String> {
        let conn = self.conn();
        conn.query_row(
            "SELECT files_changed FROM steps WHERE id = ?1",
            params![step_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    }

    pub fn record_verifier_report(
        &self,
        step_id: &str,
        run_id: &str,
        lease_gen: i64,
        worker_id: Option<&str>,
        verifier: &str,
        status: &str,
        verdict: &str,
        evidence_json: &str,
    ) -> Option<String> {
        let mut conn = self.conn();
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().timestamp_millis();
        let tx = conn.transaction().ok()?;
        if tx
            .execute(
                "INSERT INTO verifier_reports (
                id, step_id, run_id, lease_gen, worker_id, verifier, status, verdict,
                evidence_json, created_at, updated_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                params![
                    id,
                    step_id,
                    run_id,
                    lease_gen,
                    worker_id,
                    verifier,
                    status,
                    verdict,
                    evidence_json,
                    now
                ],
            )
            .is_err()
        {
            return None;
        }

        let step_verification_status = match (status, verdict) {
            ("verified", "pass") => "verified_pass",
            ("verified", "fail") => "verified_fail",
            ("verified", "blocked") => "verified_blocked",
            ("needs_evidence", _) => "needs_evidence",
            ("error", _) => "verification_error",
            _ => "unverified",
        };
        let verified_at = if status == "verified" {
            Some(now)
        } else {
            None
        };

        let updated = tx
            .execute(
                "UPDATE steps
             SET verification_status = ?1,
                 verifier_report_id = ?2,
                 verified_at = ?3,
                 updated_at = ?4
             WHERE id = ?5 AND run_id = ?6",
                params![
                    step_verification_status,
                    id,
                    verified_at,
                    now,
                    step_id,
                    run_id
                ],
            )
            .ok()?;
        if updated == 0 {
            return None;
        }

        let context = step_event_context(&tx, step_id);
        if try_insert_operations_event(
            &tx,
            context.as_ref().map(|context| context.user_id.as_str()),
            context
                .as_ref()
                .and_then(|context| context.group_id.as_deref()),
            None,
            context
                .as_ref()
                .and_then(|context| context.task_id.as_deref()),
            Some(run_id),
            Some(step_id),
            None,
            "verifier.reported",
            "verifier_report",
            &id,
            &serde_json::json!({
                "step_id": step_id,
                "lease_gen": lease_gen,
                "worker_id": worker_id,
                "verifier": verifier,
                "status": status,
                "verdict": verdict,
                "step_verification_status": step_verification_status,
            }),
        )
        .is_err()
        {
            return None;
        }

        tx.commit().ok()?;

        Some(id)
    }

    pub fn get_latest_verifier_report(&self, step_id: &str) -> Option<VerifierReport> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, step_id, run_id, lease_gen, worker_id, verifier, status, verdict,
                    evidence_json, created_at, updated_at
             FROM verifier_reports
             WHERE step_id = ?1
             ORDER BY created_at DESC
             LIMIT 1",
            params![step_id],
            |row| {
                Ok(VerifierReport {
                    id: row.get(0)?,
                    step_id: row.get(1)?,
                    run_id: row.get(2)?,
                    lease_gen: row.get(3)?,
                    worker_id: row.get(4)?,
                    verifier: row.get(5)?,
                    status: row.get(6)?,
                    verdict: row.get(7)?,
                    evidence_json: row.get(8)?,
                    created_at: row.get(9)?,
                    updated_at: row.get(10)?,
                })
            },
        )
        .ok()
    }

    pub fn get_verifier_report_for_run_step(
        &self,
        user_id: &str,
        run_id: &str,
        step_id: &str,
        report_id: &str,
    ) -> Option<serde_json::Value> {
        let conn = self.conn();
        let report = conn
            .query_row(
                "SELECT vr.id, vr.step_id, vr.run_id, vr.lease_gen, vr.worker_id, vr.verifier,
                        vr.status, vr.verdict, vr.evidence_json, vr.created_at, vr.updated_at
                 FROM verifier_reports vr
                 JOIN runs r ON r.id = vr.run_id
                 WHERE r.user_id = ?1
                    AND vr.run_id = ?2
                    AND vr.step_id = ?3
                    AND vr.id = ?4",
                params![user_id, run_id, step_id, report_id],
                |row| {
                    Ok(VerifierReport {
                        id: row.get(0)?,
                        step_id: row.get(1)?,
                        run_id: row.get(2)?,
                        lease_gen: row.get(3)?,
                        worker_id: row.get(4)?,
                        verifier: row.get(5)?,
                        status: row.get(6)?,
                        verdict: row.get(7)?,
                        evidence_json: row.get(8)?,
                        created_at: row.get(9)?,
                        updated_at: row.get(10)?,
                    })
                },
            )
            .ok()?;
        let evidence = serde_json::from_str::<serde_json::Value>(&report.evidence_json)
            .unwrap_or_else(|_| serde_json::json!({}));

        Some(serde_json::json!({
            "report": {
                "id": report.id,
                "step_id": report.step_id,
                "run_id": report.run_id,
                "lease_gen": report.lease_gen,
                "worker_id": report.worker_id,
                "verifier": report.verifier,
                "status": report.status,
                "verdict": report.verdict,
                "created_at": report.created_at,
                "updated_at": report.updated_at,
            },
            "evidence": evidence,
        }))
    }

    pub fn record_step_work_contract(
        &self,
        step_id: &str,
        run_id: &str,
        lease_gen: i64,
        contract: &TaskContract,
    ) -> bool {
        let contract_json = match serde_json::to_string(contract) {
            Ok(json) => json,
            Err(err) => {
                tracing::error!(
                    step_id = %step_id,
                    lease_gen,
                    error = %err,
                    "failed to serialize step work contract"
                );
                return false;
            }
        };

        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let rows = conn.execute(
            "INSERT INTO step_work_contracts (step_id, lease_gen, run_id, contract_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(step_id, lease_gen) DO UPDATE SET
                run_id = excluded.run_id,
                contract_json = excluded.contract_json,
                created_at = excluded.created_at",
            params![step_id, lease_gen, run_id, contract_json, now],
        ).unwrap_or_else(|err| {
            tracing::error!(
                step_id = %step_id,
                run_id = %run_id,
                lease_gen,
                error = %err,
                "failed to persist step work contract"
            );
            0
        });

        rows > 0
    }

    pub fn get_step_work_contract(&self, step_id: &str, lease_gen: i64) -> Option<TaskContract> {
        self.read_step_work_contract(step_id, lease_gen)
            .map_err(|err| {
                tracing::error!(
                    step_id = %step_id,
                    lease_gen,
                    error = %err,
                    "failed to read step work contract"
                );
                err
            })
            .ok()
            .flatten()
    }

    /// Read one attempt's contract without collapsing a missing row and an
    /// unreadable row into the same result.
    ///
    /// Verification uses this form because either condition makes exam
    /// integrity unknown, but the persisted diagnostic must say which one
    /// occurred. Other callers retain the compatibility `Option` above.
    pub(crate) fn read_step_work_contract(
        &self,
        step_id: &str,
        lease_gen: i64,
    ) -> Result<Option<TaskContract>, String> {
        let conn = self.conn();
        let contract_json = match conn.query_row(
            "SELECT contract_json FROM step_work_contracts
             WHERE step_id = ?1 AND lease_gen = ?2",
            params![step_id, lease_gen],
            |row| row.get::<_, String>(0),
        ) {
            Ok(json) => json,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(err) => return Err(format!("contract lookup failed: {err}")),
        };

        serde_json::from_str(&contract_json)
            .map(Some)
            .map_err(|err| format!("contract JSON is unreadable: {err}"))
    }

    pub fn get_latest_step_work_contract(&self, step_id: &str) -> Option<TaskContract> {
        let conn = self.conn();
        let contract_json: String = conn
            .query_row(
                "SELECT contract_json FROM step_work_contracts
             WHERE step_id = ?1
             ORDER BY lease_gen DESC, created_at DESC
             LIMIT 1",
                params![step_id],
                |row| row.get(0),
            )
            .ok()?;

        serde_json::from_str(&contract_json)
            .map_err(|err| {
                tracing::error!(
                    step_id = %step_id,
                    error = %err,
                    "failed to deserialize latest step work contract"
                );
                err
            })
            .ok()
    }

    pub fn get_run_profile(&self, run_id: &str) -> Option<String> {
        let conn = self.conn();
        conn.query_row(
            "SELECT profile FROM runs WHERE id = ?1",
            params![run_id],
            |row| row.get(0),
        )
        .ok()
    }

    // --- Attempt details (for usage recording) ---

    pub fn get_attempt_provider_model(
        &self,
        step_id: &str,
        lease_gen: i64,
    ) -> Option<(String, String, i64)> {
        let conn = self.conn();
        conn.query_row(
            "SELECT COALESCE(provider, ''), COALESCE(model, ''), started_at
             FROM step_attempts WHERE step_id = ?1 AND lease_gen = ?2
             ORDER BY attempt_number DESC LIMIT 1",
            params![step_id, lease_gen],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .ok()
    }

    // --- Worker sessions ---

    pub fn create_worker_session(&self, session_id: &str, worker_id: &str) {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO worker_sessions (id, worker_id, connected_at, last_heartbeat)
             VALUES (?1, ?2, ?3, ?3)",
            params![session_id, worker_id, now],
        )
        .ok();
    }

    pub fn disconnect_worker_session(&self, session_id: &str) {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE worker_sessions SET disconnected_at = ?1 WHERE id = ?2",
            params![now, session_id],
        )
        .ok();
    }

    pub fn set_worker_grace_deadline(&self, worker_id: &str, deadline_ms: i64) {
        let conn = self.conn();
        conn.execute(
            "UPDATE worker_sessions SET grace_deadline = ?1
             WHERE worker_id = ?2 AND disconnected_at IS NOT NULL AND grace_deadline IS NULL",
            params![deadline_ms, worker_id],
        )
        .ok();
    }

    pub fn update_heartbeat(&self, worker_id: &str) {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE worker_sessions SET last_heartbeat = ?1
             WHERE worker_id = ?2 AND disconnected_at IS NULL",
            params![now, worker_id],
        )
        .ok();
    }

    pub fn workers_past_grace(&self) -> Vec<(String, String)> {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let mut stmt = conn
            .prepare(
                "SELECT ws.worker_id, s.id FROM worker_sessions ws
             JOIN steps s ON s.assigned_worker = ws.worker_id AND s.status IN ('leased', 'running')
             WHERE ws.disconnected_at IS NOT NULL
             AND ws.grace_deadline IS NOT NULL
             AND ws.grace_deadline < ?1",
            )
            .unwrap();
        stmt.query_map(params![now], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    // --- User profile mutations ---

    pub fn upsert_user_profile(&self, user_id: &str, profile: &str) {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO user_profiles (user_id, active_profile, auto_mode, updated_at)
             VALUES (?1, ?2, 'normal', ?3)
             ON CONFLICT(user_id) DO UPDATE SET active_profile = ?2, updated_at = ?3",
            params![user_id, profile, now],
        )
        .ok();
    }

    pub fn get_full_user_profile(&self, user_id: &str) -> Option<(String, String)> {
        let conn = self.conn();
        conn.query_row(
            "SELECT active_profile, auto_mode FROM user_profiles WHERE user_id = ?1",
            params![user_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok()
    }

    /// List runs with active (non-terminal) status for the MC snapshot.
    pub fn list_active_runs(&self) -> Vec<ActiveRunSummary> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT r.id, r.goal, r.status, r.created_at,
                    COUNT(s.id) as step_count,
                    SUM(CASE WHEN s.status = 'verified' THEN 1 ELSE 0 END) as steps_completed,
                    SUM(CASE WHEN s.status = 'failed' THEN 1 ELSE 0 END) as steps_failed
             FROM runs r
             LEFT JOIN steps s ON s.run_id = r.id
             WHERE r.status IN ('pending', 'running', 'leased')
             GROUP BY r.id
             ORDER BY r.created_at DESC
             LIMIT 50",
            )
            .unwrap();
        stmt.query_map([], |row| {
            Ok(ActiveRunSummary {
                id: row.get(0)?,
                goal: row.get(1)?,
                status: row.get(2)?,
                created_at: row.get::<_, i64>(3)?.to_string(),
                step_count: row.get::<_, i64>(4)? as usize,
                steps_completed: row.get::<_, i64>(5)? as usize,
                steps_failed: row.get::<_, i64>(6)? as usize,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    // --- Admin queries ---

    pub fn list_all_runs(
        &self,
        limit: usize,
        offset: usize,
    ) -> Vec<(String, String, String, String, String)> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, goal, status, created_at FROM runs
             ORDER BY created_at DESC LIMIT ?1 OFFSET ?2",
            )
            .unwrap();
        stmt.query_map(params![limit as i64, offset as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?.to_string(),
            ))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn list_decisions(&self, limit: usize, user_id: Option<&str>) -> Vec<serde_json::Value> {
        let conn = self.conn();
        let (sql, params_vec): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(uid) =
            user_id
        {
            (
                "SELECT id, user_id, run_id, step_id, timestamp, intent, risk, tier, provider, model, rationale, profile
                 FROM decisions WHERE user_id = ?1 ORDER BY timestamp DESC LIMIT ?2",
                vec![Box::new(uid.to_string()), Box::new(limit as i64)],
            )
        } else {
            (
                "SELECT id, user_id, run_id, step_id, timestamp, intent, risk, tier, provider, model, rationale, profile
                 FROM decisions ORDER BY timestamp DESC LIMIT ?1",
                vec![Box::new(limit as i64)],
            )
        };

        let mut stmt = conn.prepare(sql).unwrap();
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        stmt.query_map(params_refs.as_slice(), |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "user_id": row.get::<_, String>(1)?,
                "run_id": row.get::<_, Option<String>>(2)?,
                "step_id": row.get::<_, Option<String>>(3)?,
                "timestamp": row.get::<_, i64>(4)?,
                "intent": row.get::<_, String>(5)?,
                "risk": row.get::<_, String>(6)?,
                "tier": row.get::<_, String>(7)?,
                "provider": row.get::<_, String>(8)?,
                "model": row.get::<_, String>(9)?,
                "rationale": row.get::<_, String>(10)?,
                "profile": row.get::<_, String>(11)?,
            }))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn system_stats(&self) -> serde_json::Value {
        let conn = self.conn();

        let total_runs: i64 = conn
            .query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))
            .unwrap_or(0);

        let active_runs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM runs WHERE status IN ('planning', 'running')",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        let total_steps: i64 = conn
            .query_row("SELECT COUNT(*) FROM steps", [], |r| r.get(0))
            .unwrap_or(0);

        let succeeded_steps: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM steps WHERE status = 'verified'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        let failed_steps: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM steps WHERE status = 'failed'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        let total_decisions: i64 = conn
            .query_row("SELECT COUNT(*) FROM decisions", [], |r| r.get(0))
            .unwrap_or(0);

        let connected_workers: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM workers WHERE status = 'connected'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        let total_users: i64 = conn
            .query_row("SELECT COUNT(DISTINCT user_id) FROM runs", [], |r| r.get(0))
            .unwrap_or(0);

        serde_json::json!({
            "runs": { "total": total_runs, "active": active_runs },
            "steps": { "total": total_steps, "succeeded": succeeded_steps, "failed": failed_steps },
            "decisions": total_decisions,
            "workers": { "connected": connected_workers },
            "users": total_users,
        })
    }

    pub fn get_worker_list(&self) -> Vec<serde_json::Value> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT w.id, w.user_id, w.status, w.last_seen,
                    GROUP_CONCAT(pc.provider, ',') as providers
             FROM workers w
             LEFT JOIN provider_capabilities pc ON pc.worker_id = w.id
             GROUP BY w.id
             ORDER BY w.last_seen DESC",
            )
            .unwrap();
        stmt.query_map([], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "user_id": row.get::<_, String>(1)?,
                "status": row.get::<_, String>(2)?,
                "last_seen": row.get::<_, i64>(3)?,
                "providers": row.get::<_, Option<String>>(4)?
                    .map(|s| s.split(',').map(String::from).collect::<Vec<_>>())
                    .unwrap_or_default(),
            }))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    // --- Steps assigned to a worker (for orphaning on disconnect) ---

    pub fn get_worker_active_steps(&self, worker_id: &str) -> Vec<String> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id FROM steps WHERE assigned_worker = ?1 AND status IN ('leased', 'running')"
        ).unwrap();
        stmt.query_map(params![worker_id], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    /// Verify that a step is currently assigned to the given worker.
    /// Used to prevent workers from spoofing step completion for steps they don't own.
    pub fn verify_step_worker(&self, step_id: &str, worker_id: &str) -> bool {
        let conn = self.conn();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM steps
                 WHERE id = ?1 AND assigned_worker = ?2
                 AND status IN ('leased', 'running')
                 AND lease_deadline IS NOT NULL AND lease_deadline >= ?3",
                params![step_id, worker_id, Utc::now().timestamp_millis()],
                |row| row.get(0),
            )
            .unwrap_or(0);
        count > 0
    }

    pub fn renew_lease(&self, step_id: &str, lease_gen: i64, new_deadline: i64) -> bool {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let rows = conn
            .execute(
                "UPDATE steps SET lease_deadline = ?1, updated_at = ?2
             WHERE id = ?3 AND lease_gen = ?4 AND status IN ('leased', 'running')",
                params![new_deadline, now, step_id, lease_gen],
            )
            .unwrap_or(0);
        rows > 0
    }

    pub fn set_step_earliest_dispatch(&self, step_id: &str, earliest_ms: i64) {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let rows = conn
            .execute(
                "UPDATE steps SET earliest_dispatch_at = ?1, updated_at = ?2
             WHERE id = ?3",
                params![earliest_ms, now, step_id],
            )
            .unwrap_or(0);
        if rows > 0 {
            insert_step_operations_event(
                &conn,
                step_id,
                "step.dispatch_deferred",
                &serde_json::json!({
                    "earliest_dispatch_at": earliest_ms,
                }),
            );
        }
    }

    pub fn unlease_step(&self, step_id: &str, lease_gen: i64) -> bool {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let rows = conn.execute(
            "UPDATE steps SET status = 'pending', assigned_worker = NULL, lease_deadline = NULL,
                 updated_at = ?1, version = version + 1
             WHERE id = ?2 AND lease_gen = ?3 AND status = 'leased'",
            params![now, step_id, lease_gen],
        ).unwrap_or(0);
        if rows > 0 {
            insert_step_operations_event(
                &conn,
                step_id,
                "step.unleased",
                &serde_json::json!({
                    "status": "pending",
                    "lease_gen": lease_gen,
                }),
            );
        }
        rows > 0
    }

    pub fn orphan_step(&self, step_id: &str) {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let rows = conn.execute(
            "UPDATE steps SET status = 'orphaned', assigned_worker = NULL, lease_deadline = NULL,
                 updated_at = ?1, version = version + 1
             WHERE id = ?2 AND status IN ('leased', 'running')",
            params![now, step_id],
        )
        .unwrap_or(0);
        if rows > 0 {
            insert_step_operations_event(
                &conn,
                step_id,
                "step.orphaned",
                &serde_json::json!({
                    "status": "orphaned",
                }),
            );
        }
    }

    /// Cascade failure from a failed step to all downstream steps that depend on it
    /// via `success_required` edges. Transitively marks them as 'skipped'.
    /// Returns the list of all skipped step IDs.
    pub fn cascade_failure(&self, failed_step_id: &str) -> Vec<String> {
        let conn = self.conn();
        let now = Utc::now().timestamp_millis();
        let mut skipped = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();

        queue.push_back(failed_step_id.to_string());
        visited.insert(failed_step_id.to_string());

        while let Some(current_id) = queue.pop_front() {
            // Find all steps that depend on current_id with success_required edge
            let mut stmt = conn
                .prepare(
                    "SELECT sd.step_id FROM step_dependencies sd
                 JOIN steps s ON s.id = sd.step_id
                 WHERE sd.depends_on_id = ?1 AND sd.edge_type = 'success_required'
                 AND s.status NOT IN ('verified', 'manual_override', 'failed',
                                      'execution_failed', 'recovered', 'cancelled', 'skipped')",
                )
                .unwrap();

            let dependents: Vec<String> = stmt
                .query_map(params![current_id], |row| row.get::<_, String>(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect();

            for dep_id in dependents {
                if visited.contains(&dep_id) {
                    continue;
                }
                visited.insert(dep_id.clone());

                let rows = conn.execute(
                    "UPDATE steps SET status = 'skipped', updated_at = ?1, version = version + 1
                     WHERE id = ?2 AND status NOT IN ('verified', 'manual_override', 'failed',
                                                      'execution_failed', 'recovered', 'cancelled', 'skipped')",
                    params![now, dep_id],
                ).unwrap_or(0);

                if rows > 0 {
                    insert_step_operations_event(
                        &conn,
                        &dep_id,
                        "step.skipped",
                        &serde_json::json!({
                            "status": "skipped",
                            "failed_step_id": failed_step_id,
                            "blocked_by_step_id": current_id,
                            "edge_type": "success_required",
                        }),
                    );
                    skipped.push(dep_id.clone());
                    queue.push_back(dep_id);
                }
            }
        }

        skipped
    }

    // --- Usage aggregation ---

    /// Get aggregated usage summary for a user since a given timestamp.
    pub fn get_user_usage_summary(&self, user_id: &str, since_ms: i64) -> UsageSummary {
        let conn = self.conn();

        // Per-provider breakdown
        let mut stmt = conn
            .prepare(
                "SELECT provider,
                    COALESCE(SUM(COALESCE(tokens_in, 0)), 0),
                    COALESCE(SUM(COALESCE(tokens_out, 0)), 0),
                    COUNT(*)
             FROM usage_events
             WHERE user_id = ?1 AND timestamp > ?2
             GROUP BY provider",
            )
            .unwrap();

        let by_provider: Vec<ProviderUsage> = stmt
            .query_map(params![user_id, since_ms], |row| {
                let provider: String = row.get(0)?;
                let tokens_in: i64 = row.get(1)?;
                let tokens_out: i64 = row.get(2)?;
                let step_count: i64 = row.get(3)?;
                let cost = estimate_cost_by_provider(&provider, tokens_in, tokens_out);
                Ok(ProviderUsage {
                    provider,
                    tokens_in,
                    tokens_out,
                    cost_estimate: cost,
                    step_count,
                })
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        let total_tokens_in = by_provider.iter().map(|p| p.tokens_in).sum();
        let total_tokens_out = by_provider.iter().map(|p| p.tokens_out).sum();
        let total_cost_estimate = by_provider.iter().map(|p| p.cost_estimate).sum();
        let step_count = by_provider.iter().map(|p| p.step_count).sum();

        UsageSummary {
            total_tokens_in,
            total_tokens_out,
            total_cost_estimate,
            step_count,
            by_provider,
        }
    }

    /// Get daily usage breakdown for a user over the last N days.
    pub fn get_user_daily_usage(&self, user_id: &str, days: u32) -> Vec<DailyUsage> {
        let conn = self.conn();
        let cutoff_ms = Utc::now().timestamp_millis() - (days as i64 * 86_400_000);

        let mut stmt = conn
            .prepare(
                "SELECT DATE(timestamp / 1000, 'unixepoch') as day,
                    COALESCE(SUM(COALESCE(tokens_in, 0)), 0),
                    COALESCE(SUM(COALESCE(tokens_out, 0)), 0),
                    COUNT(*)
             FROM usage_events
             WHERE user_id = ?1 AND timestamp > ?2
             GROUP BY day
             ORDER BY day ASC",
            )
            .unwrap();

        stmt.query_map(params![user_id, cutoff_ms], |row| {
            let date: String = row.get(0)?;
            let tokens_in: i64 = row.get(1)?;
            let tokens_out: i64 = row.get(2)?;
            let step_count: i64 = row.get(3)?;
            // Use a blended rate for daily aggregation
            let cost_estimate = estimate_cost_by_provider("claude", tokens_in, tokens_out);
            Ok(DailyUsage {
                date,
                tokens_in,
                tokens_out,
                cost_estimate,
                step_count,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    /// Get system-wide usage summary since a given timestamp (admin).
    pub fn get_system_usage_summary(&self, since_ms: i64) -> UsageSummary {
        let conn = self.conn();

        let mut stmt = conn
            .prepare(
                "SELECT provider,
                    COALESCE(SUM(COALESCE(tokens_in, 0)), 0),
                    COALESCE(SUM(COALESCE(tokens_out, 0)), 0),
                    COUNT(*)
             FROM usage_events
             WHERE timestamp > ?1
             GROUP BY provider",
            )
            .unwrap();

        let by_provider: Vec<ProviderUsage> = stmt
            .query_map(params![since_ms], |row| {
                let provider: String = row.get(0)?;
                let tokens_in: i64 = row.get(1)?;
                let tokens_out: i64 = row.get(2)?;
                let step_count: i64 = row.get(3)?;
                let cost = estimate_cost_by_provider(&provider, tokens_in, tokens_out);
                Ok(ProviderUsage {
                    provider,
                    tokens_in,
                    tokens_out,
                    cost_estimate: cost,
                    step_count,
                })
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        let total_tokens_in = by_provider.iter().map(|p| p.tokens_in).sum();
        let total_tokens_out = by_provider.iter().map(|p| p.tokens_out).sum();
        let total_cost_estimate = by_provider.iter().map(|p| p.cost_estimate).sum();
        let step_count = by_provider.iter().map(|p| p.step_count).sum();

        UsageSummary {
            total_tokens_in,
            total_tokens_out,
            total_cost_estimate,
            step_count,
            by_provider,
        }
    }

    /// Get per-user usage breakdown (admin). Returns (user_id, UsageSummary) pairs.
    pub fn get_per_user_usage(&self, since_ms: i64) -> Vec<(String, UsageSummary)> {
        let conn = self.conn();

        // Get distinct users with usage in the window
        let mut user_stmt = conn
            .prepare("SELECT DISTINCT user_id FROM usage_events WHERE timestamp > ?1")
            .unwrap();

        let user_ids: Vec<String> = user_stmt
            .query_map(params![since_ms], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        drop(user_stmt);
        drop(conn);

        // Aggregate per user (reuses get_user_usage_summary)
        user_ids
            .into_iter()
            .map(|uid| {
                let summary = self.get_user_usage_summary(&uid, since_ms);
                (uid, summary)
            })
            .collect()
    }

    /// Get historical average step costs for a (user, tier, provider) combination
    /// over the last 7 days. Returns (avg_tokens_in, avg_tokens_out, avg_duration_ms, sample_count).
    pub fn get_historical_step_costs(
        &self,
        user_id: &str,
        tier: &str,
        provider: &str,
    ) -> Option<(i64, i64, i64, i64)> {
        let conn = self.conn();
        let cutoff_ms = Utc::now().timestamp_millis() - (7 * 86_400_000);
        conn.query_row(
            "SELECT
                COALESCE(AVG(COALESCE(tokens_in, 0)), 0),
                COALESCE(AVG(COALESCE(tokens_out, 0)), 0),
                COALESCE(AVG(COALESCE(duration_ms, 0)), 0),
                COUNT(*)
             FROM usage_events
             WHERE user_id = ?1 AND tier = ?2 AND provider = ?3 AND timestamp > ?4",
            params![user_id, tier, provider, cutoff_ms],
            |row| {
                let avg_in: f64 = row.get(0)?;
                let avg_out: f64 = row.get(1)?;
                let avg_dur: f64 = row.get(2)?;
                let count: i64 = row.get(3)?;
                Ok((avg_in as i64, avg_out as i64, avg_dur as i64, count))
            },
        )
        .ok()
        .filter(|(_, _, _, count)| *count > 0)
    }

    /// Get a user's usage cost for the current day (since midnight UTC).
    pub fn get_user_daily_cost(&self, user_id: &str) -> (f64, i64) {
        let now = Utc::now();
        let midnight = now.date_naive().and_hms_opt(0, 0, 0).unwrap();
        let midnight_ms =
            chrono::DateTime::<Utc>::from_naive_utc_and_offset(midnight, Utc).timestamp_millis();
        let summary = self.get_user_usage_summary(user_id, midnight_ms);
        (summary.total_cost_estimate, summary.step_count)
    }

    /// Get a user's usage cost for the current month (since 1st of month UTC).
    pub fn get_user_monthly_cost(&self, user_id: &str) -> f64 {
        let now = Utc::now();
        let first_of_month = now
            .date_naive()
            .with_day(1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let first_ms = chrono::DateTime::<Utc>::from_naive_utc_and_offset(first_of_month, Utc)
            .timestamp_millis();
        let summary = self.get_user_usage_summary(user_id, first_ms);
        summary.total_cost_estimate
    }

    // --- Billing & Credits ---

    pub fn get_subscription(&self, clerk_user_id: &str) -> Option<SubscriptionRecord> {
        let conn = self.conn();
        conn.query_row(
            "SELECT clerk_user_id, stripe_customer_id, stripe_subscription_id, plan_type, status,
                    trial_end, current_period_start, current_period_end
             FROM subscriptions WHERE clerk_user_id = ?1",
            params![clerk_user_id],
            |row| {
                Ok(SubscriptionRecord {
                    clerk_user_id: row.get(0)?,
                    stripe_customer_id: row.get(1)?,
                    stripe_subscription_id: row.get(2)?,
                    plan_type: row.get(3)?,
                    status: row.get(4)?,
                    trial_end: row.get(5)?,
                    current_period_start: row.get(6)?,
                    current_period_end: row.get(7)?,
                })
            },
        )
        .ok()
    }

    pub fn upsert_subscription(&self, sub: &SubscriptionRecord) {
        let conn = self.conn();
        conn.execute(
            "INSERT OR REPLACE INTO subscriptions
                (clerk_user_id, stripe_customer_id, stripe_subscription_id, plan_type, status,
                 trial_end, current_period_start, current_period_end, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, datetime('now'))",
            params![
                sub.clerk_user_id,
                sub.stripe_customer_id,
                sub.stripe_subscription_id,
                sub.plan_type,
                sub.status,
                sub.trial_end,
                sub.current_period_start,
                sub.current_period_end,
            ],
        )
        .expect("failed to upsert subscription");
    }

    pub fn get_subscription_by_customer(
        &self,
        stripe_customer_id: &str,
    ) -> Option<SubscriptionRecord> {
        let conn = self.conn();
        conn.query_row(
            "SELECT clerk_user_id, stripe_customer_id, stripe_subscription_id, plan_type, status,
                    trial_end, current_period_start, current_period_end
             FROM subscriptions WHERE stripe_customer_id = ?1",
            params![stripe_customer_id],
            |row| {
                Ok(SubscriptionRecord {
                    clerk_user_id: row.get(0)?,
                    stripe_customer_id: row.get(1)?,
                    stripe_subscription_id: row.get(2)?,
                    plan_type: row.get(3)?,
                    status: row.get(4)?,
                    trial_end: row.get(5)?,
                    current_period_start: row.get(6)?,
                    current_period_end: row.get(7)?,
                })
            },
        )
        .ok()
    }
    pub fn get_referral_code(&self, code: &str) -> Option<ReferralCodeRecord> {
        let conn = self.conn();
        conn.query_row(
            "SELECT code, creator_user_id, uses_remaining, total_uses, weeks_earned
             FROM referral_codes WHERE code = ?1",
            params![code],
            |row| {
                Ok(ReferralCodeRecord {
                    code: row.get(0)?,
                    creator_user_id: row.get(1)?,
                    uses_remaining: row.get(2)?,
                    total_uses: row.get(3)?,
                    weeks_earned: row.get(4)?,
                })
            },
        )
        .ok()
    }

    pub fn get_user_referral_code(&self, user_id: &str) -> Option<ReferralCodeRecord> {
        let conn = self.conn();
        conn.query_row(
            "SELECT code, creator_user_id, uses_remaining, total_uses, weeks_earned
             FROM referral_codes WHERE creator_user_id = ?1",
            params![user_id],
            |row| {
                Ok(ReferralCodeRecord {
                    code: row.get(0)?,
                    creator_user_id: row.get(1)?,
                    uses_remaining: row.get(2)?,
                    total_uses: row.get(3)?,
                    weeks_earned: row.get(4)?,
                })
            },
        )
        .ok()
    }

    pub fn create_user_referral_code(&self, user_id: &str) -> ReferralCodeRecord {
        if let Some(existing) = self.get_user_referral_code(user_id) {
            return existing;
        }
        let short_id = &user_id[user_id.len().saturating_sub(5)..];
        let code = format!("REF-{}", short_id.to_uppercase());
        let conn = self.conn();
        conn.execute(
            "INSERT OR IGNORE INTO referral_codes (code, creator_user_id, uses_remaining, max_uses, total_uses, weeks_earned)
             VALUES (?1, ?2, 50, 50, 0, 0)",
            params![code, user_id],
        ).ok();
        drop(conn);
        self.get_user_referral_code(user_id)
            .unwrap_or(ReferralCodeRecord {
                code,
                creator_user_id: user_id.to_string(),
                uses_remaining: 50,
                total_uses: 0,
                weeks_earned: 0,
            })
    }

    pub fn consume_referral(&self, code: &str) -> bool {
        let conn = self.conn();
        let rows = conn.execute(
            "UPDATE referral_codes SET uses_remaining = uses_remaining - 1, total_uses = total_uses + 1
             WHERE code = ?1 AND uses_remaining > 0",
            params![code],
        ).unwrap_or(0);
        rows > 0
    }

    pub fn reward_referrer(&self, code: &str) {
        let conn = self.conn();
        conn.execute(
            "UPDATE referral_codes SET weeks_earned = weeks_earned + 1 WHERE code = ?1",
            params![code],
        )
        .ok();
    }

    // --- Promo Codes ---

    pub fn create_promo_code(
        &self,
        code: &str,
        discount_type: &str,
        discount_value: f64,
        max_uses: i32,
        expires_at: Option<&str>,
        created_by: &str,
        description: Option<&str>,
        discount_options: Option<&str>,
    ) -> Result<PromoCode, String> {
        let conn = self.conn();
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO promo_codes (id, code, discount_type, discount_value, max_uses, expires_at, created_by, description, discount_options)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![id, code.to_uppercase(), discount_type, discount_value, max_uses, expires_at, created_by, description, discount_options],
        ).map_err(|e| {
            if e.to_string().contains("UNIQUE") {
                "a promo code with that name already exists".to_string()
            } else {
                format!("failed to create promo code: {e}")
            }
        })?;
        Ok(PromoCode {
            id,
            code: code.to_uppercase(),
            discount_type: discount_type.to_string(),
            discount_value,
            max_uses,
            current_uses: 0,
            expires_at: expires_at.map(String::from),
            active: true,
            created_by: created_by.to_string(),
            created_at: Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            description: description.map(String::from),
            discount_options: discount_options.map(String::from),
        })
    }

    pub fn list_promo_codes(&self) -> Vec<PromoCode> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, code, discount_type, discount_value, max_uses, current_uses, expires_at, active, created_by, created_at, description, discount_options
             FROM promo_codes ORDER BY created_at DESC"
        ).unwrap();
        stmt.query_map([], |row| {
            Ok(PromoCode {
                id: row.get(0)?,
                code: row.get(1)?,
                discount_type: row.get(2)?,
                discount_value: row.get(3)?,
                max_uses: row.get(4)?,
                current_uses: row.get(5)?,
                expires_at: row.get(6)?,
                active: row.get::<_, i32>(7)? != 0,
                created_by: row.get(8)?,
                created_at: row.get(9)?,
                description: row.get(10)?,
                discount_options: row.get(11)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn get_promo_code(&self, code: &str) -> Option<PromoCode> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, code, discount_type, discount_value, max_uses, current_uses, expires_at, active, created_by, created_at, description, discount_options
             FROM promo_codes WHERE code = ?1 COLLATE NOCASE",
            params![code],
            |row| Ok(PromoCode {
                id: row.get(0)?,
                code: row.get(1)?,
                discount_type: row.get(2)?,
                discount_value: row.get(3)?,
                max_uses: row.get(4)?,
                current_uses: row.get(5)?,
                expires_at: row.get(6)?,
                active: row.get::<_, i32>(7)? != 0,
                created_by: row.get(8)?,
                created_at: row.get(9)?,
                description: row.get(10)?,
                discount_options: row.get(11)?,
            }),
        ).ok()
    }

    pub fn update_promo_code(
        &self,
        id: &str,
        active: Option<bool>,
        max_uses: Option<i32>,
        expires_at: Option<Option<&str>>,
        description: Option<Option<&str>>,
    ) -> bool {
        let conn = self.conn();
        let mut sets = Vec::new();
        let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(a) = active {
            sets.push("active = ?");
            values.push(Box::new(a as i32));
        }
        if let Some(m) = max_uses {
            sets.push("max_uses = ?");
            values.push(Box::new(m));
        }
        if let Some(e) = expires_at {
            sets.push("expires_at = ?");
            values.push(Box::new(e.map(String::from)));
        }
        if let Some(d) = description {
            sets.push("description = ?");
            values.push(Box::new(d.map(String::from)));
        }
        if sets.is_empty() {
            return false;
        }
        values.push(Box::new(id.to_string()));
        let sql = format!("UPDATE promo_codes SET {} WHERE id = ?", sets.join(", "));
        let params: Vec<&dyn rusqlite::types::ToSql> = values.iter().map(|v| v.as_ref()).collect();
        conn.execute(&sql, params.as_slice()).unwrap_or(0) > 0
    }

    pub fn delete_promo_code(&self, id: &str) -> bool {
        let conn = self.conn();
        conn.execute("DELETE FROM promo_codes WHERE id = ?1", params![id])
            .unwrap_or(0)
            > 0
    }

    pub fn validate_promo_code(&self, code: &str, user_id: &str) -> Result<PromoCode, String> {
        let promo = self.get_promo_code(code).ok_or("invalid promo code")?;
        if !promo.active {
            return Err("this promo code is no longer active".into());
        }
        if promo.current_uses >= promo.max_uses {
            return Err("this promo code has reached its usage limit".into());
        }
        if let Some(ref exp) = promo.expires_at {
            if let Ok(expiry) = chrono::NaiveDateTime::parse_from_str(exp, "%Y-%m-%d %H:%M:%S") {
                if expiry < Utc::now().naive_utc() {
                    return Err("this promo code has expired".into());
                }
            }
        }
        let conn = self.conn();
        let already_used: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM code_redemptions WHERE code = ?1 COLLATE NOCASE AND user_id = ?2",
            params![code, user_id],
            |row| row.get(0),
        ).unwrap_or(false);
        if already_used {
            return Err("you have already used this promo code".into());
        }
        Ok(promo)
    }

    pub fn redeem_promo_code(&self, code: &str, user_id: &str) -> Result<PromoCode, String> {
        let promo = self.validate_promo_code(code, user_id)?;
        let conn = self.conn();
        let redemption_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO code_redemptions (id, promo_code_id, code, user_id) VALUES (?1, ?2, ?3, ?4)",
            params![redemption_id, promo.id, promo.code, user_id],
        ).map_err(|e| format!("redemption failed: {e}"))?;
        conn.execute(
            "UPDATE promo_codes SET current_uses = current_uses + 1 WHERE id = ?1",
            params![promo.id],
        )
        .map_err(|e| format!("usage update failed: {e}"))?;
        Ok(promo)
    }

    pub fn list_redemptions(&self, code: Option<&str>) -> Vec<CodeRedemption> {
        let conn = self.conn();
        let (sql, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match code {
            Some(c) => (
                "SELECT id, promo_code_id, code, user_id, redeemed_at FROM code_redemptions WHERE code = ?1 COLLATE NOCASE ORDER BY redeemed_at DESC",
                vec![Box::new(c.to_string())],
            ),
            None => (
                "SELECT id, promo_code_id, code, user_id, redeemed_at FROM code_redemptions ORDER BY redeemed_at DESC",
                vec![],
            ),
        };
        let mut stmt = conn.prepare(sql).unwrap();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|v| v.as_ref()).collect();
        stmt.query_map(param_refs.as_slice(), |row| {
            Ok(CodeRedemption {
                id: row.get(0)?,
                promo_code_id: row.get(1)?,
                code: row.get(2)?,
                user_id: row.get(3)?,
                redeemed_at: row.get(4)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    // --- Context Flow Artifacts ---

    pub fn store_context_artifact(
        &self,
        id: &str,
        producer_step_id: &str,
        producer_run_id: &str,
        kind: &str,
        content: &str,
        summary: &str,
        files_changed: &[String],
        confidence: f32,
        tokens: u32,
        created_at: i64,
        metadata: &serde_json::Value,
    ) {
        let conn = self.conn();
        let files_json = serde_json::to_string(files_changed).unwrap();
        let metadata_json = serde_json::to_string(metadata).unwrap();

        conn.execute(
            "INSERT INTO context_flow_artifacts
            (id, producer_step_id, producer_run_id, kind, content, summary, files_changed, confidence, tokens, created_at, metadata)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![id, producer_step_id, producer_run_id, kind, content, summary, files_json, confidence, tokens, created_at, metadata_json],
        ).expect("failed to store context artifact");
    }

    pub fn get_context_artifacts_for_run(
        &self,
        run_id: &str,
    ) -> Vec<(
        String,
        String,
        String,
        String,
        String,
        Vec<String>,
        f32,
        u32,
        i64,
    )> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, producer_step_id, kind, content, summary, files_changed, confidence, tokens, created_at
            FROM context_flow_artifacts
            WHERE producer_run_id = ?1
            ORDER BY created_at ASC"
        ).unwrap();

        stmt.query_map([run_id], |row| {
            let files_json: String = row.get(5)?;
            let files_changed: Vec<String> = serde_json::from_str(&files_json).unwrap_or_default();
            Ok((
                row.get(0)?, // id
                row.get(1)?, // producer_step_id
                row.get(2)?, // kind
                row.get(3)?, // content
                row.get(4)?, // summary
                files_changed,
                row.get(6)?, // confidence
                row.get(7)?, // tokens
                row.get(8)?, // created_at
            ))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn cleanup_context_artifacts_for_run(&self, run_id: &str) {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM context_flow_artifacts WHERE producer_run_id = ?1",
            params![run_id],
        )
        .expect("failed to cleanup context artifacts");
    }

    pub fn get_context_artifact_stats(
        &self,
    ) -> (usize, std::collections::HashMap<String, usize>, f32) {
        let conn = self.conn();

        // Total count
        let total: usize = conn
            .query_row("SELECT COUNT(*) FROM context_flow_artifacts", [], |row| {
                Ok(row.get::<_, i64>(0)? as usize)
            })
            .unwrap_or(0);

        // Count by kind
        let mut stmt = conn
            .prepare("SELECT kind, COUNT(*) FROM context_flow_artifacts GROUP BY kind")
            .unwrap();
        let kind_counts: std::collections::HashMap<String, usize> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        // Average tokens
        let avg_tokens: f32 = conn
            .query_row(
                "SELECT AVG(CAST(tokens AS REAL)) FROM context_flow_artifacts",
                [],
                |row| Ok(row.get::<_, f64>(0)? as f32),
            )
            .unwrap_or(0.0);

        (total, kind_counts, avg_tokens)
    }

    pub fn get_recent_runs_with_artifacts(&self, limit: usize) -> Vec<String> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT producer_run_id FROM context_flow_artifacts
             ORDER BY created_at DESC LIMIT ?1",
            )
            .unwrap();

        stmt.query_map([limit], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    // --- Accounts (Clerk user mapping) ---

    /// Return account status for a Clerk user id (`active`, `suspended`, `deleted`), if known.
    /// Missing row means the user has never been webhooked/upserted — treat as allowed until suspended.
    pub fn get_account_status(&self, clerk_user_id: &str) -> Option<String> {
        let conn = self.conn();
        conn.query_row(
            "SELECT status FROM accounts WHERE clerk_user_id = ?1",
            params![clerk_user_id],
            |r| r.get(0),
        )
        .ok()
    }

    // --- Health check ---

    /// Run a simple SELECT 1 to verify the database is accessible.
    pub fn health_check(&self) -> bool {
        let conn = self.conn();
        conn.query_row("SELECT 1", [], |_| Ok(())).is_ok()
    }

    // ─── Audit Log ────────────────────────────────────────────────────────────

    /// Record an audit log entry for a moderation or admin action.
    #[allow(clippy::too_many_arguments)]
    pub fn audit_log(
        &self,
        actor_id: &str,
        actor_type: &str,
        action: &str,
        target_type: Option<&str>,
        target_id: Option<&str>,
        details: Option<&str>,
        ip_address: Option<&str>,
    ) {
        let conn = self.conn();
        let id = format!("audit_{}", Uuid::new_v4());
        conn.execute(
            "INSERT INTO audit_log (id, actor_id, actor_type, action, target_type, target_id, details, ip_address)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, actor_id, actor_type, action, target_type, target_id, details, ip_address],
        ).ok();
    }

    /// Return recent audit log entries, paginated (50 per page).
    pub fn audit_log_list(&self, page: i64) -> Vec<serde_json::Value> {
        let conn = self.conn();
        let offset = page.saturating_sub(1) * 50;
        let mut stmt = conn.prepare(
            "SELECT id, actor_id, actor_type, action, target_type, target_id, details, ip_address, created_at
             FROM audit_log ORDER BY created_at DESC LIMIT 50 OFFSET ?1"
        ).unwrap();
        stmt.query_map(params![offset], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "actorId": row.get::<_, String>(1)?,
                "actorType": row.get::<_, String>(2)?,
                "action": row.get::<_, String>(3)?,
                "targetType": row.get::<_, Option<String>>(4)?,
                "targetId": row.get::<_, Option<String>>(5)?,
                "details": row.get::<_, Option<String>>(6)?,
                "ipAddress": row.get::<_, Option<String>>(7)?,
                "createdAt": row.get::<_, String>(8)?,
            }))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    /// Write a structured audit entry (user_id-centric API).
    #[allow(clippy::too_many_arguments)]
    pub fn log_audit(
        &self,
        id: &str,
        user_id: &str,
        action: &str,
        target_type: Option<&str>,
        target_id: Option<&str>,
        metadata: Option<&str>,
        ip_address: Option<&str>,
    ) {
        // Delegate to the existing audit_log writer, using actor_type = "user".
        self.audit_log(
            user_id,
            "user",
            action,
            target_type,
            target_id,
            metadata,
            ip_address,
        );
        let _ = id; // id is generated internally by audit_log
    }

    /// Return recent audit log entries with explicit limit/offset.
    pub fn get_audit_log(&self, limit: i64, offset: i64) -> Vec<AuditEntry> {
        let conn = self.conn();
        let mut stmt = match conn.prepare(
            "SELECT id, actor_id, action, target_type, target_id, details, ip_address, created_at
             FROM audit_log ORDER BY created_at DESC LIMIT ?1 OFFSET ?2",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map(params![limit, offset], |row| {
            let created_raw: rusqlite::types::Value = row.get(7)?;
            let created_at: i64 = match created_raw {
                rusqlite::types::Value::Integer(n) => n,
                rusqlite::types::Value::Text(s) => s.parse().unwrap_or(0),
                _ => 0,
            };
            Ok(AuditEntry {
                id: row.get(0)?,
                user_id: row.get(1)?,
                action: row.get(2)?,
                target_type: row.get(3)?,
                target_id: row.get(4)?,
                metadata: row.get(5)?,
                ip_address: row.get(6)?,
                created_at,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    /// Return audit log entries for a specific user.
    pub fn get_user_audit_log(&self, user_id: &str, limit: i64) -> Vec<AuditEntry> {
        let conn = self.conn();
        let mut stmt = match conn.prepare(
            "SELECT id, actor_id, action, target_type, target_id, details, ip_address, created_at
             FROM audit_log WHERE actor_id = ?1 ORDER BY created_at DESC LIMIT ?2",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map(params![user_id, limit], |row| {
            let created_raw: rusqlite::types::Value = row.get(7)?;
            let created_at: i64 = match created_raw {
                rusqlite::types::Value::Integer(n) => n,
                rusqlite::types::Value::Text(s) => s.parse().unwrap_or(0),
                _ => 0,
            };
            Ok(AuditEntry {
                id: row.get(0)?,
                user_id: row.get(1)?,
                action: row.get(2)?,
                target_type: row.get(3)?,
                target_id: row.get(4)?,
                metadata: row.get(5)?,
                ip_address: row.get(6)?,
                created_at,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    // --- Deployment adapters ---

    pub fn list_deployment_adapters(&self, user_id: &str) -> Vec<crate::routes::DeploymentAdapter> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, adapter_type, environment, config_json, status,
                        last_inspected_at, created_at, updated_at
                 FROM deployment_adapters
                 WHERE user_id = ?1 AND status = 'active'
                 ORDER BY created_at ASC",
            )
            .unwrap();
        stmt.query_map(params![user_id], |row| {
            let config_raw: String = row.get(4)?;
            let config_json: serde_json::Value =
                serde_json::from_str(&config_raw).unwrap_or_else(|_| serde_json::json!({}));
            Ok(crate::routes::DeploymentAdapter {
                id: row.get(0)?,
                user_id: row.get(1)?,
                adapter_type: row.get(2)?,
                environment: row.get(3)?,
                config_json,
                status: row.get(5)?,
                last_inspected_at: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    // --- Container methods ---

    pub fn upsert_user_container(
        &self,
        id: &str,
        user_id: &str,
        container_id: &str,
        provider: &str,
        status: &str,
    ) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO user_containers (id, user_id, container_id, provider, status)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(user_id) DO UPDATE SET container_id = ?3, provider = ?4, status = ?5, updated_at = unixepoch()",
            params![id, user_id, container_id, provider, status],
        ).expect("upsert_user_container failed");
    }

    pub fn get_user_container(&self, user_id: &str) -> Option<UserContainer> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, user_id, container_id, provider, status, last_activity_at, created_at, updated_at
             FROM user_containers WHERE user_id = ?1",
            params![user_id],
            |row| Ok(UserContainer {
                id: row.get(0)?,
                user_id: row.get(1)?,
                container_id: row.get(2)?,
                provider: row.get(3)?,
                status: row.get(4)?,
                last_activity_at: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            }),
        ).ok()
    }

    pub fn update_container_status(&self, user_id: &str, status: &str) {
        let conn = self.conn();
        conn.execute(
            "UPDATE user_containers SET status = ?2, updated_at = unixepoch() WHERE user_id = ?1",
            params![user_id, status],
        )
        .expect("update_container_status failed");
    }

    pub fn touch_container_activity(&self, user_id: &str) {
        let conn = self.conn();
        conn.execute(
            "UPDATE user_containers SET last_activity_at = unixepoch(), updated_at = unixepoch() WHERE user_id = ?1",
            params![user_id],
        ).expect("touch_container_activity failed");
    }

    pub fn list_idle_containers(&self, threshold_epoch: i64) -> Vec<UserContainer> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, user_id, container_id, provider, status, last_activity_at, created_at, updated_at
             FROM user_containers WHERE status = 'running' AND last_activity_at < ?1"
        ).expect("list_idle_containers prepare failed");
        stmt.query_map(params![threshold_epoch], |row| {
            Ok(UserContainer {
                id: row.get(0)?,
                user_id: row.get(1)?,
                container_id: row.get(2)?,
                provider: row.get(3)?,
                status: row.get(4)?,
                last_activity_at: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn list_all_containers(&self) -> Vec<UserContainer> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, user_id, container_id, provider, status, last_activity_at, created_at, updated_at
             FROM user_containers ORDER BY last_activity_at DESC"
        ).expect("list_all_containers prepare failed");
        stmt.query_map([], |row| {
            Ok(UserContainer {
                id: row.get(0)?,
                user_id: row.get(1)?,
                container_id: row.get(2)?,
                provider: row.get(3)?,
                status: row.get(4)?,
                last_activity_at: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn delete_user_container(&self, user_id: &str) {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM user_containers WHERE user_id = ?1",
            params![user_id],
        )
        .expect("delete_user_container failed");
    }

    // --- GitHub repo imports ---

    /// Create or reset an import record for (user, repo). Returns the import id.
    /// If a record already exists for this repo it is reset to a fresh `pending`
    /// state so the import can be safely retried.
    pub fn upsert_github_import(
        &self,
        id: &str,
        user_id: &str,
        repo_id: i64,
        repo_full_name: &str,
        default_branch: &str,
        clone_path: &str,
        private: bool,
    ) -> String {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO github_imports
                (id, user_id, repo_id, repo_full_name, default_branch, clone_path, private,
                 status, progress, stage, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', 0, 'queued', NULL)
             ON CONFLICT(user_id, repo_full_name) DO UPDATE SET
                repo_id = ?3,
                default_branch = ?5,
                clone_path = ?6,
                private = ?7,
                status = 'pending',
                progress = 0,
                stage = 'queued',
                error = NULL,
                updated_at = unixepoch()",
            params![
                id,
                user_id,
                repo_id,
                repo_full_name,
                default_branch,
                clone_path,
                private as i64
            ],
        )
        .expect("upsert_github_import failed");

        // Return the canonical id for this (user, repo), which may differ from
        // `id` when an existing record was updated.
        conn.query_row(
            "SELECT id FROM github_imports WHERE user_id = ?1 AND repo_full_name = ?2",
            params![user_id, repo_full_name],
            |row| row.get::<_, String>(0),
        )
        .unwrap_or_else(|_| id.to_string())
    }

    pub fn update_github_import_progress(
        &self,
        import_id: &str,
        status: &str,
        progress: i64,
        stage: &str,
        error: Option<&str>,
    ) {
        let conn = self.conn();
        conn.execute(
            "UPDATE github_imports
                SET status = ?2, progress = ?3, stage = ?4, error = ?5, updated_at = unixepoch()
             WHERE id = ?1",
            params![import_id, status, progress, stage, error],
        )
        .expect("update_github_import_progress failed");
    }

    pub fn mark_github_import_synced(&self, import_id: &str, head_commit: Option<&str>) {
        let conn = self.conn();
        conn.execute(
            "UPDATE github_imports
                SET last_synced_at = unixepoch(), head_commit = ?2, updated_at = unixepoch()
             WHERE id = ?1",
            params![import_id, head_commit],
        )
        .expect("mark_github_import_synced failed");
    }

    fn map_github_import(row: &rusqlite::Row) -> rusqlite::Result<GithubImport> {
        Ok(GithubImport {
            id: row.get(0)?,
            user_id: row.get(1)?,
            repo_id: row.get(2)?,
            repo_full_name: row.get(3)?,
            default_branch: row.get(4)?,
            clone_path: row.get(5)?,
            private: row.get::<_, i64>(6)? != 0,
            status: row.get(7)?,
            progress: row.get(8)?,
            stage: row.get(9)?,
            error: row.get(10)?,
            last_synced_at: row.get(11)?,
            head_commit: row.get(12)?,
            created_at: row.get(13)?,
            updated_at: row.get(14)?,
        })
    }

    const GITHUB_IMPORT_COLS: &'static str =
        "id, user_id, repo_id, repo_full_name, default_branch, clone_path, private, \
         status, progress, stage, error, last_synced_at, head_commit, created_at, updated_at";

    /// Fetch a single import scoped to the owning user (prevents cross-user access).
    pub fn get_github_import(&self, user_id: &str, import_id: &str) -> Option<GithubImport> {
        let conn = self.conn();
        let sql = format!(
            "SELECT {} FROM github_imports WHERE id = ?1 AND user_id = ?2",
            Self::GITHUB_IMPORT_COLS
        );
        conn.query_row(&sql, params![import_id, user_id], Self::map_github_import)
            .ok()
    }

    pub fn list_github_imports(&self, user_id: &str) -> Vec<GithubImport> {
        let conn = self.conn();
        let sql = format!(
            "SELECT {} FROM github_imports WHERE user_id = ?1 ORDER BY updated_at DESC",
            Self::GITHUB_IMPORT_COLS
        );
        let mut stmt = conn
            .prepare(&sql)
            .expect("list_github_imports prepare failed");
        stmt.query_map(params![user_id], Self::map_github_import)
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn test_db() -> Database {
        let dir = tempfile::tempdir().unwrap().keep();
        Database::open(&dir.join("cortex.sqlite"))
    }

    /// A panic while the database lock is held used to poison the mutex, which
    /// turned one bad request into a permanent outage for every other caller in
    /// the process. The accessor recovers the guard instead.
    #[test]
    fn a_panic_under_the_database_lock_does_not_disable_the_database() {
        let db = std::sync::Arc::new(test_db());
        assert!(db.schema_version() > 0, "database works before the panic");

        let poisoner = std::sync::Arc::clone(&db);
        let panicked = std::thread::spawn(move || {
            let _guard = poisoner.conn();
            panic!("simulated panic while holding the database lock");
        })
        .join();
        assert!(panicked.is_err(), "the worker thread must have panicked");

        // Poisoned under the old code; every call below would have panicked.
        assert!(
            db.schema_version() > 0,
            "database still works after the panic"
        );
        let conversation = db.create_conversation("clerk_user_1", Some("post-panic"));
        assert!(
            !conversation.id.is_empty(),
            "writes still work after the panic"
        );
    }

    fn task_state(task_id: &str, title: &str) -> serde_json::Value {
        serde_json::json!({
            "tasks": [{
                "id": task_id,
                "groupId": "group-1",
                "title": title,
                "status": "created",
                "priority": "normal",
                "createdAt": "2026-05-25T00:00:00Z",
                "updatedAt": "2026-05-25T00:00:00Z",
                "createdBy": "You"
            }],
            "members": [],
            "activity": [],
            "updatedAt": "2026-05-25T00:00:00Z"
        })
    }

    fn task_state_with_tasks(tasks: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "tasks": tasks,
            "members": [],
            "activity": [],
            "updatedAt": "2026-05-25T00:00:00Z"
        })
    }

    fn path_lease(path: &str) -> ResourceLeaseRequest {
        path_lease_in_repo("default", path)
    }

    fn path_lease_in_repo(repo_key: &str, path: &str) -> ResourceLeaseRequest {
        ResourceLeaseRequest {
            resource_type: "path".to_string(),
            repo_key: repo_key.to_string(),
            resource_key: path.to_string(),
            mode: "write".to_string(),
            reason: Some("test".to_string()),
            metadata: serde_json::json!({}),
        }
    }

    fn task_lease(group_id: &str, task_id: &str) -> ResourceLeaseRequest {
        ResourceLeaseRequest {
            resource_type: "task".to_string(),
            repo_key: "default".to_string(),
            resource_key: format!("{group_id}:{task_id}"),
            mode: "exclusive".to_string(),
            reason: Some("test".to_string()),
            metadata: serde_json::json!({}),
        }
    }

    #[test]
    fn list_deployment_operations_events_returns_cortex_deployments() {
        let db = test_db();
        db.record_deployment_event(
            "deploy.inspected",
            "first",
            &serde_json::json!({ "commit": "abc1234" }),
        );
        db.record_deployment_event(
            "deploy.verified",
            "second",
            &serde_json::json!({ "commit": "def5678" }),
        );

        let events = db.list_deployment_operations_events(10);

        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|event| event.entity_type == "deployment"));
        assert!(events
            .iter()
            .all(|event| event.scope_id.as_deref() == Some("cortex")));
        assert!(events
            .iter()
            .any(|event| event.entity_id == "first" && event.payload["commit"] == "abc1234"));
        assert!(events
            .iter()
            .any(|event| event.entity_id == "second" && event.payload["commit"] == "def5678"));
    }

    fn test_step(
        id: &str,
    ) -> (
        String,
        String,
        String,
        Option<String>,
        String,
        String,
        String,
        i64,
    ) {
        (
            id.to_string(),
            "execute".to_string(),
            "modify".to_string(),
            None,
            "standard".to_string(),
            "low".to_string(),
            "Apply change".to_string(),
            Utc::now().timestamp_millis(),
        )
    }

    #[test]
    fn upsert_group_task_state_with_events_persists_projection_and_event() {
        let db = test_db();
        let state = task_state("task-1", "First task");
        db.upsert_group_task_state_with_events(
            "user-1",
            "group-1",
            &state,
            &[CortexTaskStateEvent {
                task_id: Some("task-1".to_string()),
                event_type: "task.created".to_string(),
                entity_type: "task".to_string(),
                entity_id: "task-1".to_string(),
                payload: serde_json::json!({ "task": { "id": "task-1" } }),
            }],
            None,
        )
        .expect("atomic task state write");

        let projection = db
            .get_cortex_task_projection("user-1", "group-1", "task-1", 25)
            .expect("task projection");

        assert_eq!(projection["task"]["id"], "task-1");
        assert!(projection["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| {
                event["event_type"] == "task.created"
                    && event["entity_type"] == "task"
                    && event["entity_id"] == "task-1"
                    && event["task_id"] == "task-1"
            }));
    }

    #[test]
    fn upsert_group_task_state_with_events_rolls_back_when_attachment_fails() {
        let db = test_db();
        db.upsert_group_task_state("user-1", "group-1", &task_state("task-1", "First task"));
        let next_state = task_state("task-2", "Second task");

        let result = db.upsert_group_task_state_with_events(
            "user-1",
            "group-1",
            &next_state,
            &[CortexTaskStateEvent {
                task_id: Some("task-2".to_string()),
                event_type: "task.created".to_string(),
                entity_type: "task".to_string(),
                entity_id: "task-2".to_string(),
                payload: serde_json::json!({ "task": { "id": "task-2" } }),
            }],
            Some(("missing-task", "conversation-1")),
        );

        assert!(result.is_err());
        assert!(db
            .get_cortex_task_projection("user-1", "group-1", "task-2", 25)
            .is_none());
        let state = db
            .get_group_task_state("user-1", "group-1")
            .expect("previous state remains");
        assert_eq!(state["tasks"][0]["id"], "task-1");

        let summary = db.get_group_operations_summary("user-1", "group-1", 25);
        assert!(!summary["recent_events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["event_type"] == "task.created" && event["entity_id"] == "task-2"));
    }

    #[test]
    fn upsert_group_task_state_indexes_and_reconciles_cortex_tasks() {
        let db = test_db();
        db.upsert_group_task_state("user-1", "group-1", &task_state("task-1", "First task"));

        assert!(db.cortex_task_exists("user-1", "group-1", "task-1"));

        db.upsert_group_task_state("user-1", "group-1", &task_state("task-2", "Second task"));

        assert!(!db.cortex_task_exists("user-1", "group-1", "task-1"));
        assert!(db.cortex_task_exists("user-1", "group-1", "task-2"));
    }

    #[test]
    fn create_run_with_metadata_updates_cortex_task_latest_run() {
        let db = test_db();
        db.upsert_group_task_state("user-1", "group-1", &task_state("task-1", "First task"));
        let conversation = db.create_conversation("user-1", Some("Project Chat"));

        let run_id = db.create_run_with_metadata(
            "user-1",
            "Ship task",
            "auto",
            &[],
            Some("task-1"),
            Some("group-1"),
            Some(&conversation.id),
        );

        let conn = db.conn();
        let (latest_run_id, conversation_id): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT latest_run_id, conversation_id FROM cortex_tasks
                 WHERE user_id = ?1 AND group_id = ?2 AND id = ?3",
                params!["user-1", "group-1", "task-1"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(latest_run_id.as_deref(), Some(run_id.as_str()));
        assert_eq!(conversation_id.as_deref(), Some(conversation.id.as_str()));

        assert!(conn
            .query_row(
                "SELECT 1 FROM cortex_task_chats
                 WHERE user_id = ?1 AND group_id = ?2 AND task_id = ?3 AND conversation_id = ?4",
                params!["user-1", "group-1", "task-1", conversation.id],
                |_| Ok(())
            )
            .is_ok());
        drop(conn);

        let events = db.list_run_operations_events(&run_id, 25);
        assert!(events.iter().any(|event| {
            event.event_type == "run.created"
                && event.entity_type == "run"
                && event.entity_id == run_id
                && event.task_id.as_deref() == Some("task-1")
        }));
        assert!(events.iter().any(|event| {
            event.event_type == "chat.attached"
                && event.entity_type == "chat"
                && event.entity_id == conversation.id
                && event.task_id.as_deref() == Some("task-1")
        }));
    }

    #[test]
    fn create_run_with_steps_acquires_resource_leases_atomically() {
        let db = test_db();
        let step = test_step("step-a");
        let run_id = db
            .create_run_with_steps_and_resource_leases(
                "user-1",
                "Ship path change",
                "auto",
                &["src/main.rs".to_string()],
                None,
                None,
                None,
                &[path_lease("src/main.rs")],
                &[step],
                &[],
            )
            .expect("run should be created");

        let leases = db.list_active_resource_leases_for_run(&run_id);
        assert_eq!(leases.len(), 1);
        assert_eq!(leases[0].resource_type, "path");
        assert_eq!(leases[0].resource_key, "src/main.rs");

        let conn = db.conn();
        let step_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM steps WHERE run_id = ?1",
                params![run_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(step_count, 1);
    }

    #[test]
    fn create_run_with_steps_rejects_conflicting_active_resource_lease() {
        let db = test_db();
        let first = db
            .create_run_with_steps_and_resource_leases(
                "user-1",
                "Ship first path change",
                "auto",
                &["src/main.rs".to_string()],
                None,
                None,
                None,
                &[path_lease("src/main.rs")],
                &[test_step("step-a")],
                &[],
            )
            .expect("first run");

        let second = db.create_run_with_steps_and_resource_leases(
            "user-1",
            "Ship conflicting path change",
            "auto",
            &["src/main.rs".to_string()],
            None,
            None,
            None,
            &[path_lease("src/main.rs")],
            &[test_step("step-b")],
            &[],
        );

        match second {
            Err(CreateRunError::ResourceConflict(conflict)) => {
                assert_eq!(conflict.run_id, first);
                assert_eq!(conflict.resource_key, "src/main.rs");
            }
            other => panic!("expected resource conflict, got {other:?}"),
        }

        let conn = db.conn();
        let leaked_steps: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM steps WHERE id = 'step-b'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(leaked_steps, 0);
    }

    #[test]
    fn resource_lease_path_conflicts_are_scoped_by_repo_key() {
        let db = test_db();
        db.create_run_with_steps_and_resource_leases(
            "user-1",
            "Ship first repo path change",
            "auto",
            &["src/main.rs".to_string()],
            None,
            None,
            None,
            &[path_lease_in_repo("github:hey-vera/heyvera", "src/main.rs")],
            &[test_step("step-a")],
            &[],
        )
        .expect("first run");

        let same_path_other_repo = db.create_run_with_steps_and_resource_leases(
            "user-1",
            "Ship second repo path change",
            "auto",
            &["src/main.rs".to_string()],
            None,
            None,
            None,
            &[path_lease_in_repo(
                "github:claw-net/claw-net",
                "src/main.rs",
            )],
            &[test_step("step-b")],
            &[],
        );

        assert!(same_path_other_repo.is_ok());
    }

    #[test]
    fn resource_lease_path_conflicts_include_parent_and_child_paths() {
        let db = test_db();
        db.create_run_with_steps_and_resource_leases(
            "user-1",
            "Ship directory change",
            "auto",
            &["src".to_string()],
            None,
            None,
            None,
            &[path_lease("src")],
            &[test_step("step-a")],
            &[],
        )
        .expect("first run");

        let child = db.create_run_with_steps_and_resource_leases(
            "user-1",
            "Ship child file change",
            "auto",
            &["src/main.rs".to_string()],
            None,
            None,
            None,
            &[path_lease("src/main.rs")],
            &[test_step("step-b")],
            &[],
        );
        assert!(matches!(child, Err(CreateRunError::ResourceConflict(_))));
    }

    #[test]
    fn terminal_run_status_releases_resource_leases() {
        let db = test_db();
        let run_id = db
            .create_run_with_steps_and_resource_leases(
                "user-1",
                "Ship path change",
                "auto",
                &["src/main.rs".to_string()],
                None,
                None,
                None,
                &[path_lease("src/main.rs")],
                &[test_step("step-a")],
                &[],
            )
            .expect("first run");
        assert_eq!(db.list_active_resource_leases_for_run(&run_id).len(), 1);

        assert!(db.update_run_status(&run_id, "succeeded", None));
        assert!(db.list_active_resource_leases_for_run(&run_id).is_empty());
        let events = db.list_run_operations_events(&run_id, 25);
        assert!(events.iter().any(|event| {
            event.event_type == "run.status_changed"
                && event.entity_type == "run"
                && event.payload["status"] == "succeeded"
        }));
        assert!(events.iter().any(|event| {
            event.event_type == "resource_lease.released"
                && event.entity_type == "resource_lease"
                && event.payload["resource_key"] == "src/main.rs"
        }));

        let second = db.create_run_with_steps_and_resource_leases(
            "user-1",
            "Ship later path change",
            "auto",
            &["src/main.rs".to_string()],
            None,
            None,
            None,
            &[path_lease("src/main.rs")],
            &[test_step("step-b")],
            &[],
        );
        assert!(second.is_ok());
    }

    #[test]
    fn scheduler_mutations_record_operations_events() {
        let db = test_db();
        let run_id = db.create_run("user-1", "Exercise scheduler event coverage", "auto", &[]);
        for worker_id in ["worker-1", "worker-2", "worker-3", "worker-4"] {
            db.register_worker(worker_id, "user-1");
        }

        let planned_step_id = "planned-by-scheduler";
        db.create_step_with_id(
            planned_step_id,
            &run_id,
            "heal",
            "heal",
            "standard",
            "medium",
            "Repair failed work",
            Utc::now().timestamp_millis(),
        );
        db.set_step_earliest_dispatch(planned_step_id, 12_345);

        let unleased_step = db.create_step(&run_id, "implement", "standard", "medium", "Unlease");
        let lease_gen = db
            .lease_step(
                &unleased_step,
                "worker-1",
                Utc::now().timestamp_millis() + 60_000,
            )
            .expect("lease step");
        assert!(db.unlease_step(&unleased_step, lease_gen));

        let cancelled_step = db.create_step(&run_id, "test", "standard", "medium", "Cancel");
        let cancelled_lease_gen = db
            .lease_step(
                &cancelled_step,
                "worker-2",
                Utc::now().timestamp_millis() + 60_000,
            )
            .expect("lease step");
        assert!(db.start_step(&cancelled_step, cancelled_lease_gen));
        assert!(db.cancel_assigned_step(&cancelled_step, "worker shutdown"));

        let orphaned_step = db.create_step(&run_id, "review", "standard", "medium", "Orphan");
        let orphaned_lease_gen = db
            .lease_step(
                &orphaned_step,
                "worker-3",
                Utc::now().timestamp_millis() + 60_000,
            )
            .expect("lease step");
        assert!(db.start_step(&orphaned_step, orphaned_lease_gen));
        db.orphan_step(&orphaned_step);

        let recovered_step = db.create_step(&run_id, "fix", "standard", "medium", "Recover");
        let recovered_lease_gen = db
            .lease_step(
                &recovered_step,
                "worker-4",
                Utc::now().timestamp_millis() + 60_000,
            )
            .expect("lease step");
        assert!(db.fail_step(&recovered_step, recovered_lease_gen, "boom", None));
        assert!(db.mark_step_recovered(&recovered_step));

        db.increment_heal_count(&run_id);
        db.record_run_branch(&run_id, "cortex/run-123");

        let events = db.list_run_operations_events(&run_id, 100);
        assert!(events.iter().any(|event| {
            event.event_type == "step.planned"
                && event.entity_id == planned_step_id
                && event.payload["work_kind"] == "heal"
        }));
        assert!(events.iter().any(|event| {
            event.event_type == "step.dispatch_deferred"
                && event.entity_id == planned_step_id
                && event.payload["earliest_dispatch_at"] == 12_345
        }));
        assert!(events.iter().any(|event| {
            event.event_type == "step.unleased"
                && event.entity_id == unleased_step
                && event.payload["status"] == "pending"
        }));
        assert!(events.iter().any(|event| {
            event.event_type == "step.cancelled"
                && event.entity_id == cancelled_step
                && event.payload["reason"] == "worker shutdown"
        }));
        assert!(events.iter().any(|event| {
            event.event_type == "step.orphaned"
                && event.entity_id == orphaned_step
                && event.payload["status"] == "orphaned"
        }));
        assert!(events.iter().any(|event| {
            event.event_type == "step.recovered"
                && event.entity_id == recovered_step
                && event.payload["status"] == "recovered"
        }));
        assert!(events.iter().any(|event| {
            event.event_type == "run.heal_incremented"
                && event.entity_id == run_id
                && event.payload["heal_attempts"] == 1
        }));
        assert!(events.iter().any(|event| {
            event.event_type == "run.branch_recorded"
                && event.entity_id == run_id
                && event.payload["branch_name"] == "cortex/run-123"
        }));
    }

    #[test]
    fn cascade_failure_records_skipped_step_events() {
        let db = test_db();
        let run_id = db.create_run("user-1", "Cascade failure", "auto", &[]);
        let root_step = db.create_step(&run_id, "implement", "standard", "medium", "Root");
        let child_step = db.create_step(&run_id, "test", "standard", "medium", "Child");
        let grandchild_step = db.create_step(&run_id, "review", "standard", "medium", "Grandchild");
        db.add_step_dependency(&child_step, &root_step, "success_required");
        db.add_step_dependency(&grandchild_step, &child_step, "success_required");

        let skipped = db.cascade_failure(&root_step);

        assert_eq!(skipped, vec![child_step.clone(), grandchild_step.clone()]);
        let events = db.list_run_operations_events(&run_id, 100);
        assert!(events.iter().any(|event| {
            event.event_type == "step.skipped"
                && event.entity_id == child_step
                && event.payload["failed_step_id"] == root_step
                && event.payload["blocked_by_step_id"] == root_step
        }));
        assert!(events.iter().any(|event| {
            event.event_type == "step.skipped"
                && event.entity_id == grandchild_step
                && event.payload["failed_step_id"] == root_step
                && event.payload["blocked_by_step_id"] == child_step
        }));
    }

    #[test]
    fn expired_resource_lease_can_be_reacquired() {
        let db = test_db();
        let run_id = db
            .create_run_with_steps_and_resource_leases(
                "user-1",
                "Ship path change",
                "auto",
                &["src/main.rs".to_string()],
                None,
                None,
                None,
                &[path_lease("src/main.rs")],
                &[test_step("step-a")],
                &[],
            )
            .expect("first run");

        {
            let conn = db.conn();
            conn.execute(
                "UPDATE resource_leases SET expires_at = ?1 WHERE run_id = ?2",
                params![Utc::now().timestamp_millis() - 1, run_id],
            )
            .unwrap();
        }

        assert_eq!(db.expire_stale_resource_leases().len(), 1);

        let second = db.create_run_with_steps_and_resource_leases(
            "user-1",
            "Ship reacquired path change",
            "auto",
            &["src/main.rs".to_string()],
            None,
            None,
            None,
            &[path_lease("src/main.rs")],
            &[test_step("step-b")],
            &[],
        );
        assert!(second.is_ok());
    }

    #[test]
    fn task_resource_lease_blocks_same_task_only() {
        let db = test_db();
        db.create_run_with_steps_and_resource_leases(
            "user-1",
            "Ship first task",
            "auto",
            &["src/a.rs".to_string()],
            Some("task-1"),
            Some("group-1"),
            None,
            &[task_lease("group-1", "task-1")],
            &[test_step("step-a")],
            &[],
        )
        .expect("first run");

        let same_task = db.create_run_with_steps_and_resource_leases(
            "user-1",
            "Ship same task",
            "auto",
            &["src/b.rs".to_string()],
            Some("task-1"),
            Some("group-1"),
            None,
            &[task_lease("group-1", "task-1")],
            &[test_step("step-b")],
            &[],
        );
        assert!(matches!(
            same_task,
            Err(CreateRunError::ResourceConflict(_))
        ));

        let other_task = db.create_run_with_steps_and_resource_leases(
            "user-1",
            "Ship other task",
            "auto",
            &["src/c.rs".to_string()],
            Some("task-2"),
            Some("group-1"),
            None,
            &[task_lease("group-1", "task-2")],
            &[test_step("step-c")],
            &[],
        );
        assert!(other_task.is_ok());
    }

    #[test]
    fn get_cortex_task_projection_returns_task_runs_chats_and_events() {
        let db = test_db();
        db.upsert_group_task_state("user-1", "group-1", &task_state("task-1", "First task"));
        let conversation = db.create_conversation("user-1", Some("Project Chat"));

        let run_id = db.create_run_with_metadata(
            "user-1",
            "Ship task",
            "auto",
            &[],
            Some("task-1"),
            Some("group-1"),
            Some(&conversation.id),
        );

        let projection = db
            .get_cortex_task_projection("user-1", "group-1", "task-1", 100)
            .expect("task projection");

        assert_eq!(projection["task"]["id"], "task-1");
        assert_eq!(projection["task"]["group_id"], "group-1");
        assert_eq!(projection["task"]["latest_run_id"], run_id);
        assert_eq!(projection["task"]["conversation_id"], conversation.id);
        assert_eq!(projection["runs"][0]["id"], run_id);
        assert_eq!(projection["chats"][0]["id"], conversation.id);
        assert!(projection["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["event_type"] == "run.created"
                && event["run_id"] == run_id
                && event["task_id"] == "task-1"));
        assert!(projection["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["event_type"] == "chat.attached"
                && event["run_id"] == run_id
                && event["entity_id"] == conversation.id
                && event["task_id"] == "task-1"));
    }

    #[test]
    fn attach_cortex_task_chat_updates_projection_and_records_event() {
        let db = test_db();
        db.upsert_group_task_state("user-1", "group-1", &task_state("task-1", "First task"));
        let conversation = db.create_conversation("user-1", Some("Durable Project Chat"));

        assert!(db.attach_cortex_task_chat("user-1", "group-1", "task-1", &conversation.id));

        let projection = db
            .get_cortex_task_projection("user-1", "group-1", "task-1", 100)
            .expect("task projection");

        assert_eq!(projection["task"]["conversation_id"], conversation.id);
        assert_eq!(projection["chats"][0]["id"], conversation.id);
        assert!(projection["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["event_type"] == "chat.attached"
                && event["entity_type"] == "chat"
                && event["entity_id"] == conversation.id
                && event["task_id"] == "task-1"));
    }

    #[test]
    fn cortex_approval_requests_are_scoped_and_resolvable() {
        let db = test_db();
        db.upsert_group_task_state("user-1", "group-1", &task_state("task-1", "First task"));

        let request = db.create_cortex_approval_request(
            "user-1",
            "group-1",
            Some("task-1"),
            Some("conversation-1"),
            Some("run-1"),
            "Approve deploy",
            "Ship the verified change?",
            "urgent",
            "cortex",
        );

        assert_eq!(request.status, "pending");
        assert_eq!(request.task_id.as_deref(), Some("task-1"));
        assert_eq!(request.priority, "urgent");

        let user_requests =
            db.list_cortex_approval_requests("user-1", "group-1", Some("pending"), 10);
        assert_eq!(user_requests.len(), 1);
        let other_user_requests =
            db.list_cortex_approval_requests("user-2", "group-1", Some("pending"), 10);
        assert!(other_user_requests.is_empty());

        let resolved = db
            .resolve_cortex_approval_request(
                "user-1",
                "group-1",
                &request.id,
                "approved",
                &serde_json::json!({ "note": "looks good" }),
            )
            .expect("approval resolves");
        assert_eq!(resolved.status, "approved");
        assert_eq!(resolved.decision.as_ref().unwrap()["note"], "looks good");
        assert!(resolved.resolved_at.is_some());
        assert!(db
            .resolve_cortex_approval_request(
                "user-1",
                "group-1",
                &request.id,
                "rejected",
                &serde_json::json!({}),
            )
            .is_none());

        let pending = db.list_cortex_approval_requests("user-1", "group-1", Some("pending"), 10);
        assert!(pending.is_empty());
    }

    #[test]
    fn operations_summary_counts_cancelled_approvals() {
        let db = test_db();
        db.upsert_group_task_state("user-1", "group-1", &task_state("task-1", "First task"));

        let request = db.create_cortex_approval_request(
            "user-1",
            "group-1",
            Some("task-1"),
            None,
            None,
            "Cancel stale ask",
            "This ask is no longer needed.",
            "normal",
            "task-manager",
        );

        db.resolve_cortex_approval_request(
            "user-1",
            "group-1",
            &request.id,
            "cancelled",
            &serde_json::json!({ "source": "test" }),
        )
        .expect("approval cancels");

        let group_summary = db.get_group_operations_summary("user-1", "group-1", 25);
        assert_eq!(group_summary["approvals"]["pending"], 0);
        assert_eq!(group_summary["approvals"]["cancelled"], 1);

        let personal_summary = db.get_personal_operations_summary("user-1", 25);
        assert_eq!(personal_summary["approvals"]["cancelled"], 1);
    }

    #[test]
    fn projection_and_summary_surface_pending_approvals() {
        let db = test_db();
        db.upsert_group_task_state("user-1", "group-1", &task_state("task-1", "First task"));

        let approval = db.create_cortex_approval_request(
            "user-1",
            "group-1",
            Some("task-1"),
            None,
            None,
            "Approve dependency upgrade",
            "Allow Cortex to change the dependency set?",
            "high",
            "task-manager",
        );

        let projection = db
            .get_cortex_task_projection("user-1", "group-1", "task-1", 100)
            .expect("task projection");
        assert_eq!(projection["approvals"][0]["id"], approval.id);
        assert_eq!(projection["approvals"][0]["status"], "pending");

        let summary = db.get_group_operations_summary("user-1", "group-1", 25);
        assert_eq!(summary["approvals"]["pending"], 1);
        assert!(summary["attention"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["kind"] == "approval_pending"
                && item["approval_id"] == approval.id
                && item["task_id"] == "task-1"));
    }

    #[test]
    fn pending_step_approval_blocks_ready_queries_until_approved() {
        let db = test_db();
        db.upsert_group_task_state("user-1", "group-1", &task_state("task-1", "First task"));
        let now = Utc::now().timestamp_millis();
        let run_id = db.create_run_with_steps(
            "user-1",
            "Ship risky task",
            "auto",
            &[],
            Some("task-1"),
            Some("group-1"),
            None,
            &[(
                "step-risky".to_string(),
                "execute".to_string(),
                "ship".to_string(),
                None,
                "execute".to_string(),
                "critical".to_string(),
                "Deploy production change".to_string(),
                now,
            )],
            &[],
        );
        assert!(db.update_run_status(&run_id, "running", None));

        assert_eq!(db.find_ready_steps(&run_id), vec!["step-risky".to_string()]);

        let approval = db
            .ensure_cortex_step_approval_request(
                "user-1",
                "step-risky",
                "autonomy.dispatch",
                "Approve risky Cortex dispatch",
                "Cortex wants to dispatch a critical risk step.",
                "urgent",
                "scheduler-risk-gate",
            )
            .expect("approval request");
        assert_eq!(approval.step_id.as_deref(), Some("step-risky"));
        assert_eq!(approval.ask_type, "autonomy.dispatch");
        assert!(db.has_pending_cortex_step_approval("user-1", "step-risky"));
        assert!(db.find_ready_steps(&run_id).is_empty());
        assert!(db.find_all_ready_steps().is_empty());

        let same_approval = db
            .ensure_cortex_step_approval_request(
                "user-1",
                "step-risky",
                "autonomy.dispatch",
                "Approve risky Cortex dispatch",
                "Cortex wants to dispatch a critical risk step.",
                "urgent",
                "scheduler-risk-gate",
            )
            .expect("approval request");
        assert_eq!(same_approval.id, approval.id);

        db.resolve_cortex_approval_request(
            "user-1",
            "group-1",
            &approval.id,
            "approved",
            &serde_json::json!({ "approved_by": "user-1" }),
        )
        .expect("approval resolves");

        assert!(db.has_approved_cortex_step_approval("user-1", "step-risky", "autonomy.dispatch"));
        assert_eq!(
            db.latest_cortex_step_approval_status("user-1", "step-risky", "autonomy.dispatch")
                .map(|(_, status)| status)
                .as_deref(),
            Some("approved")
        );
        assert!(!db.has_pending_cortex_step_approval("user-1", "step-risky"));
        assert_eq!(db.find_ready_steps(&run_id), vec!["step-risky".to_string()]);
        assert_eq!(db.find_all_ready_steps().len(), 1);
    }

    #[test]
    fn get_group_operations_summary_counts_user_scoped_backend_state() {
        let db = test_db();
        db.upsert_group_task_state(
            "user-1",
            "group-1",
            &task_state_with_tasks(serde_json::json!([
                {
                    "id": "task-urgent",
                    "groupId": "group-1",
                    "title": "Urgent queued task",
                    "status": "created",
                    "priority": "urgent",
                    "assigneeId": null,
                    "createdAt": "2026-05-25T00:00:00Z",
                    "updatedAt": "2026-05-25T00:00:00Z",
                    "createdBy": "You"
                },
                {
                    "id": "task-active",
                    "groupId": "group-1",
                    "title": "Active backend task",
                    "status": "in-progress",
                    "priority": "normal",
                    "assigneeId": "user-1",
                    "createdAt": "2026-05-25T00:00:00Z",
                    "updatedAt": "2026-05-25T00:00:00Z",
                    "createdBy": "You"
                }
            ])),
        );
        db.upsert_group_task_state(
            "user-2",
            "group-1",
            &task_state("task-other-user", "Other user task"),
        );

        let run_id = db.create_run_with_metadata(
            "user-1",
            "Ship active backend task",
            "auto",
            &[],
            Some("task-active"),
            Some("group-1"),
            None,
        );
        assert!(db.update_run_status(&run_id, "failed", Some("test failure")));
        let step_id = db.create_step(&run_id, "implement", "standard", "medium", "Wire summary");
        assert!(db.fail_unleased_step(&step_id, "test step failure", Some("test")));

        let other_run_id = db.create_run_with_metadata(
            "user-2",
            "Other user run",
            "auto",
            &[],
            Some("task-other-user"),
            Some("group-1"),
            None,
        );
        assert!(db.update_run_status(&other_run_id, "failed", Some("should not leak")));

        let summary = db.get_group_operations_summary("user-1", "group-1", 25);

        assert_eq!(summary["group_id"], "group-1");
        assert_eq!(summary["scope"], "group");
        assert_eq!(summary["tasks"]["total"], 2);
        assert_eq!(summary["tasks"]["open"], 2);
        assert_eq!(summary["tasks"]["active"], 1);
        assert_eq!(summary["tasks"]["urgent"], 1);
        assert_eq!(summary["tasks"]["without_run"], 1);
        assert_eq!(summary["tasks"]["completion"]["gated_done_available"], true);
        assert_eq!(summary["tasks"]["completion"]["gated_done"], 0);
        assert_eq!(summary["tasks"]["completion"]["done_without_evidence"], 0);
        assert_eq!(summary["runs"]["total"], 1);
        assert_eq!(summary["runs"]["failed"], 1);
        assert_eq!(summary["runs"]["latest_run_id"], run_id);
        assert_eq!(summary["steps"]["total"], 1);
        assert_eq!(summary["steps"]["failed"], 1);
        assert!(summary["attention"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["kind"] == "urgent_not_active" && item["task_id"] == "task-urgent"));
        assert!(summary["attention"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["kind"] == "failed_run" && item["run_id"] == run_id));
        assert!(summary["recent_events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|event| event["actor_user_id"] == "user-1"));
        assert!(summary["recent_events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["event_type"] == "run.status_changed"
                && event["scope_id"] == "group-1"
                && event["task_id"] == "task-active"
                && event["run_id"] == run_id));
        assert!(summary["recent_events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["event_type"] == "step.failed"
                && event["scope_id"] == "group-1"
                && event["task_id"] == "task-active"
                && event["run_id"] == run_id
                && event["step_id"] == step_id));
    }

    #[test]
    fn get_group_operations_summary_surfaces_active_resource_leases() {
        let db = test_db();
        let run_id = db
            .create_run_with_steps_and_resource_leases(
                "user-1",
                "Ship leased work",
                "auto",
                &["src/main.rs".to_string()],
                Some("task-1"),
                Some("group-1"),
                None,
                &[
                    path_lease_in_repo("github:hey-vera/heyvera", "src/main.rs"),
                    task_lease("group-1", "task-1"),
                ],
                &[test_step("step-a")],
                &[],
            )
            .expect("leased run");

        let summary = db.get_group_operations_summary("user-1", "group-1", 25);

        assert_eq!(summary["resource_leases"]["active"], 2);
        assert_eq!(summary["resource_leases"]["by_type"]["path"], 1);
        assert_eq!(summary["resource_leases"]["by_type"]["task"], 1);
        assert_eq!(summary["resource_leases"]["by_mode"]["write"], 1);
        assert_eq!(summary["resource_leases"]["by_mode"]["exclusive"], 1);
        assert!(summary["resource_leases"]["leases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|lease| lease["run_id"] == run_id
                && lease["resource_type"] == "path"
                && lease["repo_key"] == "github:hey-vera/heyvera"
                && lease["resource_key"] == "src/main.rs"));
    }

    #[test]
    fn get_group_operations_graph_links_tasks_runs_steps_evidence_and_leases() {
        let db = test_db();
        db.upsert_group_task_state("user-1", "group-1", &task_state("task-1", "Ship graph"));
        db.upsert_group_task_state(
            "user-2",
            "group-1",
            &task_state("task-other-user", "Other graph"),
        );
        let conversation = db.create_conversation("user-1", Some("Project Chat"));
        assert!(db.attach_cortex_task_chat("user-1", "group-1", "task-1", &conversation.id));

        let run_id = db
            .create_run_with_steps_and_resource_leases(
                "user-1",
                "Ship graph",
                "auto",
                &["src/main.rs".to_string()],
                Some("task-1"),
                Some("group-1"),
                Some(&conversation.id),
                &[path_lease_in_repo("github:hey-vera/heyvera", "src/main.rs")],
                &[test_step("step-a"), test_step("step-b")],
                &[(
                    "step-b".to_string(),
                    "step-a".to_string(),
                    "success_required".to_string(),
                )],
            )
            .expect("run");
        let report_id = db
            .record_verifier_report(
                "step-a",
                &run_id,
                0,
                None,
                "test",
                "verified",
                "pass",
                r#"{"ok":true}"#,
            )
            .expect("report");
        let approval = db.create_cortex_approval_request_with_gate(
            "user-1",
            "group-1",
            Some("task-1"),
            Some("step-b"),
            Some(&conversation.id),
            Some(&run_id),
            "approval",
            "Approve merge",
            "Ship it",
            "high",
            "cortex",
        );
        let other_run_id = db.create_run_with_metadata(
            "user-2",
            "Other user run",
            "auto",
            &[],
            Some("task-other-user"),
            Some("group-1"),
            None,
        );
        let graph = db.get_group_operations_graph("user-1", "group-1", 50);
        let nodes = graph["nodes"].as_array().unwrap();
        let edges = graph["edges"].as_array().unwrap();

        assert_eq!(graph["group_id"], "group-1");
        assert!(nodes.iter().any(|node| {
            node["id"] == "task:task-1"
                && node["type"] == "task"
                && node["completion"]["run_id"] == run_id
        }));
        assert!(nodes.iter().any(
            |node| node["id"] == format!("chat:{}", conversation.id) && node["type"] == "chat"
        ));
        assert!(nodes
            .iter()
            .any(|node| node["id"] == format!("run:{run_id}") && node["type"] == "run"));
        assert!(nodes
            .iter()
            .any(|node| node["id"] == "step:step-a" && node["type"] == "step"));
        assert!(nodes.iter().any(|node| {
            node["id"] == format!("evidence:{report_id}")
                && node["type"] == "evidence"
                && node["step_id"] == "step-a"
        }));
        assert!(nodes.iter().any(|node| {
            node["id"] == format!("approval:{}", approval.id)
                && node["type"] == "approval"
                && node["step_id"] == "step-b"
        }));
        assert!(nodes.iter().any(|node| {
            node["type"] == "resource_lease"
                && node["run_id"] == run_id
                && node["resource_key"] == "src/main.rs"
        }));
        assert!(nodes
            .iter()
            .all(|node| node["run_id"] != other_run_id && node["entity_id"] != "task-other-user"));

        assert!(edges.iter().any(|edge| {
            edge["from"] == "task:task-1"
                && edge["to"] == format!("run:{run_id}")
                && edge["type"] == "task_run"
        }));
        assert!(edges.iter().any(|edge| {
            edge["from"] == "step:step-a"
                && edge["to"] == "step:step-b"
                && edge["type"] == "step_dependency"
                && edge["edge_type"] == "success_required"
        }));
        assert!(edges.iter().any(|edge| {
            edge["from"] == "step:step-a"
                && edge["to"] == format!("evidence:{report_id}")
                && edge["type"] == "step_evidence"
        }));
        assert!(edges.iter().any(|edge| {
            edge["from"] == "step:step-b"
                && edge["to"] == format!("approval:{}", approval.id)
                && edge["type"] == "step_approval"
        }));
        assert!(graph["recent_events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|event| event["actor_user_id"] == "user-1"));
    }

    #[test]
    fn get_personal_operations_summary_aggregates_user_groups() {
        let db = test_db();
        db.upsert_group(
            "user-1",
            "group-registered",
            "Registered group",
            "team",
            "Tracked group",
            2,
            "#9cc7b8",
            "manual",
            Some("registered"),
        );
        db.upsert_group_task_state(
            "user-1",
            "group-derived",
            &task_state_with_tasks(serde_json::json!([
                {
                    "id": "task-derived",
                    "groupId": "group-derived",
                    "title": "Derived open task",
                    "status": "created",
                    "priority": "urgent",
                    "assigneeId": null,
                    "createdAt": "2026-05-25T00:00:00Z",
                    "updatedAt": "2026-05-25T00:00:00Z",
                    "createdBy": "You"
                }
            ])),
        );
        db.upsert_group_task_state(
            "user-1",
            "group-registered",
            &task_state_with_tasks(serde_json::json!([
                {
                    "id": "task-registered",
                    "groupId": "group-registered",
                    "title": "Registered active task",
                    "status": "in-progress",
                    "priority": "normal",
                    "assigneeId": "user-1",
                    "createdAt": "2026-05-25T00:00:00Z",
                    "updatedAt": "2026-05-25T00:00:00Z",
                    "createdBy": "You"
                }
            ])),
        );
        db.upsert_group_task_state(
            "user-2",
            "group-derived",
            &task_state("other-task", "Other"),
        );

        let run_id = db
            .create_run_with_steps_and_resource_leases(
                "user-1",
                "Ship registered task",
                "auto",
                &["src/lib.rs".to_string()],
                Some("task-registered"),
                Some("group-registered"),
                None,
                &[path_lease_in_repo("github:hey-vera/heyvera", "src/lib.rs")],
                &[test_step("step-registered")],
                &[],
            )
            .expect("leased run");
        assert!(db.update_run_status(&run_id, "running", None));

        let summary = db.get_personal_operations_summary("user-1", 25);

        assert_eq!(summary["scope"], "personal");
        assert_eq!(summary["groups_total"], 2);
        assert_eq!(summary["active_groups"], 2);
        assert_eq!(summary["tasks"]["total"], 2);
        assert_eq!(summary["tasks"]["open"], 2);
        assert_eq!(summary["tasks"]["urgent"], 1);
        assert_eq!(summary["runs"]["total"], 1);
        assert_eq!(summary["runs"]["active"], 1);
        assert_eq!(summary["resource_leases"]["active"], 1);
        assert_eq!(summary["resource_leases"]["by_type"]["path"], 1);
        assert!(summary["groups"]
            .as_array()
            .unwrap()
            .iter()
            .any(|group| group["group_id"] == "group-derived"
                && group["source"] == "derived"
                && group["tasks"]["total"] == 1));
        assert!(summary["groups"]
            .as_array()
            .unwrap()
            .iter()
            .any(|group| group["group_id"] == "group-registered"
                && group["name"] == "Registered group"
                && group["resource_leases"]["active"] == 1));
        assert!(summary["attention"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["group_id"].as_str().is_some()));
    }

    #[test]
    fn authority_scopes_include_personal_and_authorized_org_resources() {
        let db = test_db();
        let personal = db.ensure_personal_authority_scope("user-1");
        assert_eq!(personal.id, "personal:user-1");
        assert_eq!(personal.kind, "personal");
        assert_eq!(personal.role, "owner");

        db.upsert_authority_scope(
            "owner-1",
            "org:github:hey-vera",
            "org",
            "HeyVera",
            "GitHub organization authority",
            "github",
            Some("hey-vera"),
            serde_json::json!({
                "requires_org_handoff": true,
                "allowed_actions": ["read", "plan"],
                "write_policy": "approval_required"
            }),
        );
        db.add_authority_membership("org:github:hey-vera", "user-1", "admin");
        db.add_authority_membership("org:github:hey-vera", "user-2", "viewer");
        db.upsert_authority_resource(
            "org:github:hey-vera",
            "github_repo",
            "github:hey-vera/heyvera",
            "write",
            serde_json::json!({
                "default_branch": "main",
                "requires_lease": true
            }),
        );

        let scopes = db.list_authority_scopes_for_user("user-1");
        assert_eq!(scopes.len(), 2);
        assert!(scopes
            .iter()
            .any(|scope| scope.id == "personal:user-1" && scope.kind == "personal"));
        let org = scopes
            .iter()
            .find(|scope| scope.id == "org:github:hey-vera")
            .expect("org scope");
        assert_eq!(org.role, "admin");
        assert_eq!(org.policy["requires_org_handoff"], true);

        let resources = db.list_authority_resources_for_user("user-1", "org:github:hey-vera");
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].resource_key, "github:hey-vera/heyvera");
        assert_eq!(resources[0].access, "write");
        assert!(db.authority_resource_allows(
            "user-1",
            "org:github:hey-vera",
            "github_repo",
            "github:hey-vera/heyvera",
            "write"
        ));
        assert!(!db.authority_resource_allows(
            "user-2",
            "org:github:hey-vera",
            "github_repo",
            "github:hey-vera/heyvera",
            "write"
        ));
        let discovered = db
            .find_non_personal_authority_resource_scope(
                "user-1",
                "github_repo",
                "github:hey-vera/heyvera",
            )
            .expect("non-personal resource scope");
        assert_eq!(discovered.id, "org:github:hey-vera");
        assert_eq!(discovered.kind, "org");

        let unauthorized = db.list_authority_resources_for_user("user-3", "org:github:hey-vera");
        assert!(unauthorized.is_empty());
    }

    #[test]
    fn run_creation_records_authority_context_in_operations_event() {
        let db = test_db();
        let run_id = db
            .create_run_with_steps_and_resource_leases_with_authority(
                "user-1",
                "Ship org scoped run",
                "auto",
                &["src/lib.rs".to_string()],
                Some("task-1"),
                Some("group-1"),
                None,
                &[],
                &[test_step("step-authority")],
                &[],
                Some(&serde_json::json!({
                    "scope_id": "org:github:hey-vera",
                    "scope_kind": "org",
                    "role": "admin",
                    "handoff_id": "handoff-1",
                    "reason": "operator selected org context"
                })),
            )
            .expect("run");

        let events = db.list_run_operations_events(&run_id, 10);
        let created = events
            .iter()
            .find(|event| event.event_type == "run.created")
            .expect("run.created event");
        assert_eq!(
            created.payload["authority"]["scope_id"],
            "org:github:hey-vera"
        );
        assert_eq!(created.payload["authority"]["handoff_id"], "handoff-1");
    }

    #[test]
    fn run_creation_persists_pr_authority_context_and_write_lease() {
        let db = test_db();
        let run_id = db
            .create_run_with_steps_and_resource_leases_with_authority(
                "user-1",
                "Ship org scoped PR",
                "auto",
                &["src/lib.rs".to_string()],
                Some("task-1"),
                Some("group-1"),
                None,
                &[path_lease_in_repo("github:hey-vera/heyvera", "src/lib.rs")],
                &[test_step("step-authority-pr")],
                &[],
                Some(&serde_json::json!({
                    "scope_id": "org:github:hey-vera",
                    "scope_kind": "org",
                    "role": "admin",
                    "handoff_id": "handoff-1",
                    "reason": "operator selected org context"
                })),
            )
            .expect("run");

        let context = db
            .get_run_pr_authority_context(&run_id, "user-1")
            .expect("PR authority context");
        assert_eq!(context.repo_key.as_deref(), Some("github:hey-vera/heyvera"));
        assert_eq!(
            context.authority_scope_id.as_deref(),
            Some("org:github:hey-vera")
        );
        assert_eq!(context.authority_context["handoff_id"], "handoff-1");
        assert!(db.run_has_pr_write_lease(&run_id, Some("github:hey-vera/heyvera")));

        let leases = db.list_active_resource_leases_for_run(&run_id);
        assert_eq!(
            leases[0].authority_scope_id.as_deref(),
            Some("org:github:hey-vera")
        );
    }

    #[test]
    fn org_authority_resource_leases_conflict_across_users_same_scope() {
        let db = test_db();
        let authority_context = serde_json::json!({
            "scope_id": "org:github:hey-vera",
            "scope_kind": "org",
            "role": "admin",
            "handoff_id": "handoff-1",
        });

        let first = db
            .create_run_with_steps_and_resource_leases_with_authority(
                "user-1",
                "Ship first org change",
                "auto",
                &["src/lib.rs".to_string()],
                Some("task-1"),
                Some("group-1"),
                None,
                &[path_lease_in_repo("github:hey-vera/heyvera", "src/lib.rs")],
                &[test_step("step-org-a")],
                &[],
                Some(&authority_context),
            )
            .expect("first run");

        let second = db.create_run_with_steps_and_resource_leases_with_authority(
            "user-2",
            "Ship conflicting org change",
            "auto",
            &["src/lib.rs".to_string()],
            Some("task-2"),
            Some("group-1"),
            None,
            &[path_lease_in_repo("github:hey-vera/heyvera", "src/lib.rs")],
            &[test_step("step-org-b")],
            &[],
            Some(&authority_context),
        );

        match second {
            Err(CreateRunError::ResourceConflict(conflict)) => {
                assert_eq!(conflict.run_id, first);
                assert_eq!(conflict.resource_key, "src/lib.rs");
            }
            other => panic!("expected org-scoped resource conflict, got {other:?}"),
        }
    }

    #[test]
    fn personal_resource_leases_do_not_conflict_across_users() {
        let db = test_db();
        db.create_run_with_steps_and_resource_leases_with_authority(
            "user-1",
            "Ship personal first change",
            "auto",
            &["src/lib.rs".to_string()],
            None,
            Some("group-1"),
            None,
            &[path_lease_in_repo("github:hey-vera/heyvera", "src/lib.rs")],
            &[test_step("step-personal-a")],
            &[],
            Some(&serde_json::json!({
                "scope_id": "personal:user-1",
                "scope_kind": "personal",
                "role": "owner",
            })),
        )
        .expect("first personal run");

        let second = db.create_run_with_steps_and_resource_leases_with_authority(
            "user-2",
            "Ship personal second change",
            "auto",
            &["src/lib.rs".to_string()],
            None,
            Some("group-2"),
            None,
            &[path_lease_in_repo("github:hey-vera/heyvera", "src/lib.rs")],
            &[test_step("step-personal-b")],
            &[],
            Some(&serde_json::json!({
                "scope_id": "personal:user-2",
                "scope_kind": "personal",
                "role": "owner",
            })),
        );

        assert!(second.is_ok());
    }

    #[test]
    fn run_pr_write_lease_ignores_read_and_expired_leases() {
        let db = test_db();
        let read_lease = ResourceLeaseRequest {
            resource_type: "path".to_string(),
            repo_key: "github:hey-vera/heyvera".to_string(),
            resource_key: "src/lib.rs".to_string(),
            mode: "read".to_string(),
            reason: Some("read-only inspection".to_string()),
            metadata: serde_json::json!({}),
        };
        let run_id = db
            .create_run_with_steps_and_resource_leases(
                "user-1",
                "Inspect only",
                "auto",
                &["src/lib.rs".to_string()],
                None,
                Some("group-1"),
                None,
                &[read_lease],
                &[test_step("step-read-only")],
                &[],
            )
            .expect("run");
        assert!(!db.run_has_pr_write_lease(&run_id, Some("github:hey-vera/heyvera")));

        let write_run_id = db
            .create_run_with_steps_and_resource_leases(
                "user-1",
                "Write then expire",
                "auto",
                &["src/other.rs".to_string()],
                None,
                Some("group-1"),
                None,
                &[path_lease_in_repo(
                    "github:hey-vera/heyvera",
                    "src/other.rs",
                )],
                &[test_step("step-expired")],
                &[],
            )
            .expect("run");
        {
            let conn = db.conn();
            conn.execute(
                "UPDATE resource_leases SET status = 'expired' WHERE run_id = ?1",
                params![write_run_id],
            )
            .unwrap();
        }
        assert!(!db.run_has_pr_write_lease(&write_run_id, Some("github:hey-vera/heyvera")));
    }

    #[test]
    fn get_cortex_task_projection_reports_evidence_gated_completion() {
        let db = test_db();
        db.upsert_group_task_state(
            "user-1",
            "group-1",
            &task_state_with_tasks(serde_json::json!([
                {
                    "id": "task-1",
                    "groupId": "group-1",
                    "title": "Verified task",
                    "status": "done",
                    "priority": "normal",
                    "assigneeId": "user-1",
                    "createdAt": "2026-05-25T00:00:00Z",
                    "updatedAt": "2026-05-25T00:00:00Z",
                    "createdBy": "You"
                }
            ])),
        );
        let run_id = db.create_run_with_metadata(
            "user-1",
            "Ship verified task",
            "auto",
            &[],
            Some("task-1"),
            Some("group-1"),
            None,
        );
        let step_id = db.create_step(&run_id, "implement", "standard", "medium", "Ship it");
        {
            let conn = db.conn();
            conn.execute(
                "UPDATE steps
                 SET status = 'verified'
                 WHERE id = ?1",
                params![step_id],
            )
            .unwrap();
        }
        db.record_verifier_report(
            &step_id,
            &run_id,
            0,
            None,
            "test",
            "verified",
            "pass",
            r#"{"verifier_report":{"verdict":"success"}}"#,
        );
        assert!(db.update_run_status(&run_id, "succeeded", None));

        let projection = db
            .get_cortex_task_projection("user-1", "group-1", "task-1", 100)
            .expect("task projection");

        assert_eq!(projection["task"]["completion"]["raw_done"], true);
        assert_eq!(projection["task"]["completion"]["gated_done"], true);
        assert_eq!(projection["task"]["completion"]["reason"], "passed");
        assert_eq!(projection["task"]["completion"]["run_id"], run_id);
        assert_eq!(
            projection["task"]["completion"]["steps"]["verified_pass"],
            1
        );
    }

    #[test]
    fn cortex_task_has_evidence_backed_completion_requires_verified_success() {
        let db = test_db();
        db.upsert_group_task_state(
            "user-1",
            "group-1",
            &task_state_with_tasks(serde_json::json!([
                {
                    "id": "task-1",
                    "groupId": "group-1",
                    "title": "Verified task",
                    "status": "in-progress",
                    "priority": "normal",
                    "assigneeId": "user-1",
                    "createdAt": "2026-05-25T00:00:00Z",
                    "updatedAt": "2026-05-25T00:00:00Z",
                    "createdBy": "You"
                }
            ])),
        );

        assert!(!db.cortex_task_has_evidence_backed_completion("user-1", "group-1", "task-1"));

        let run_id = db.create_run_with_metadata(
            "user-1",
            "Ship verified task",
            "auto",
            &[],
            Some("task-1"),
            Some("group-1"),
            None,
        );
        let step_id = db.create_step(&run_id, "implement", "standard", "medium", "Ship it");
        assert!(db.update_run_status(&run_id, "succeeded", None));
        assert!(!db.cortex_task_has_evidence_backed_completion("user-1", "group-1", "task-1"));

        {
            let conn = db.conn();
            conn.execute(
                "UPDATE steps
                 SET status = 'verified'
                 WHERE id = ?1",
                params![step_id],
            )
            .unwrap();
        }
        db.record_verifier_report(
            &step_id,
            &run_id,
            0,
            None,
            "test",
            "verified",
            "pass",
            r#"{"verifier_report":{"verdict":"success"}}"#,
        );

        assert!(db.cortex_task_has_evidence_backed_completion("user-1", "group-1", "task-1"));
    }

    #[test]
    fn record_verifier_report_updates_step_and_records_event_atomically() {
        let db = test_db();
        let run_id = db.create_run_with_metadata(
            "user-1",
            "Ship verified evidence",
            "auto",
            &[],
            Some("task-1"),
            Some("group-1"),
            None,
        );
        let step_id = db.create_step(&run_id, "implement", "standard", "medium", "Ship it");

        let report_id = db
            .record_verifier_report(
                &step_id,
                &run_id,
                0,
                Some("worker-1"),
                "test",
                "verified",
                "pass",
                r#"{"verifier_report":{"verdict":"success"}}"#,
            )
            .expect("verifier report");

        let report = db
            .get_latest_verifier_report(&step_id)
            .expect("latest verifier report");
        assert_eq!(report.id, report_id);
        let conn = db.conn();
        let (verification_status, verifier_report_id): (String, Option<String>) = conn
            .query_row(
                "SELECT verification_status, verifier_report_id FROM steps WHERE id = ?1",
                params![step_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(verification_status, "verified_pass");
        assert_eq!(verifier_report_id.as_deref(), Some(report_id.as_str()));
        drop(conn);

        let events = db.list_run_operations_events(&run_id, 25);
        assert!(events.iter().any(|event| {
            event.event_type == "verifier.reported"
                && event.entity_type == "verifier_report"
                && event.entity_id == report_id
                && event.step_id.as_deref() == Some(step_id.as_str())
                && event.payload["step_verification_status"] == "verified_pass"
        }));
    }

    #[test]
    fn record_verifier_report_rolls_back_for_missing_step() {
        let db = test_db();
        let report_id = db.record_verifier_report(
            "missing-step",
            "run-1",
            0,
            None,
            "test",
            "verified",
            "pass",
            r#"{"verifier_report":{"verdict":"success"}}"#,
        );

        assert!(report_id.is_none());
        let conn = db.conn();
        let reports: i64 = conn
            .query_row("SELECT COUNT(*) FROM verifier_reports", [], |row| {
                row.get(0)
            })
            .unwrap();
        let events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM operations_events WHERE event_type = 'verifier.reported'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reports, 0);
        assert_eq!(events, 0);
    }

    #[test]
    fn cortex_task_completion_gate_does_not_trust_step_status_only() {
        let db = test_db();
        db.upsert_group_task_state(
            "user-1",
            "group-1",
            &task_state_with_tasks(serde_json::json!([
                {
                    "id": "task-1",
                    "groupId": "group-1",
                    "title": "Spoofed task",
                    "status": "in-progress",
                    "priority": "normal",
                    "assigneeId": "user-1",
                    "createdAt": "2026-05-25T00:00:00Z",
                    "updatedAt": "2026-05-25T00:00:00Z",
                    "createdBy": "You"
                }
            ])),
        );
        let run_id = db.create_run_with_metadata(
            "user-1",
            "Ship spoofed task",
            "auto",
            &[],
            Some("task-1"),
            Some("group-1"),
            None,
        );
        let step_id = db.create_step(&run_id, "implement", "standard", "medium", "Ship it");
        {
            let conn = db.conn();
            conn.execute(
                "UPDATE steps
                 SET status = 'verified', verification_status = 'verified_pass'
                 WHERE id = ?1",
                params![step_id],
            )
            .unwrap();
        }
        assert!(db.update_run_status(&run_id, "succeeded", None));

        assert!(!db.cortex_task_has_evidence_backed_completion("user-1", "group-1", "task-1"));
    }

    #[test]
    fn get_verifier_report_for_run_step_returns_owned_evidence() {
        let db = test_db();
        let run_id =
            db.create_run_with_metadata("user-1", "Ship evidence", "auto", &[], None, None, None);
        let step_id = db.create_step(&run_id, "verify", "standard", "medium", "Verify it");
        let report_id = db
            .record_verifier_report(
                &step_id,
                &run_id,
                0,
                Some("worker-1"),
                "test",
                "verified",
                "pass",
                r#"{"worker_completed":{"exit_code":0},"verifier_report":{"verdict":"success"}}"#,
            )
            .expect("report id");

        let payload = db
            .get_verifier_report_for_run_step("user-1", &run_id, &step_id, &report_id)
            .expect("report payload");
        assert_eq!(payload["report"]["id"], report_id);
        assert_eq!(payload["report"]["status"], "verified");
        assert_eq!(payload["evidence"]["worker_completed"]["exit_code"], 0);
        assert!(db
            .get_verifier_report_for_run_step("user-2", &run_id, &step_id, &report_id)
            .is_none());
    }

    #[test]
    fn get_group_operations_summary_counts_evidence_gated_done() {
        let db = test_db();
        db.upsert_group_task_state(
            "user-1",
            "group-1",
            &task_state_with_tasks(serde_json::json!([
                {
                    "id": "task-verified",
                    "groupId": "group-1",
                    "title": "Verified done task",
                    "status": "done",
                    "priority": "normal",
                    "assigneeId": "user-1",
                    "createdAt": "2026-05-25T00:00:00Z",
                    "updatedAt": "2026-05-25T00:00:00Z",
                    "createdBy": "You"
                },
                {
                    "id": "task-raw",
                    "groupId": "group-1",
                    "title": "Raw done task",
                    "status": "done",
                    "priority": "normal",
                    "assigneeId": "user-1",
                    "createdAt": "2026-05-25T00:00:00Z",
                    "updatedAt": "2026-05-25T00:00:00Z",
                    "createdBy": "You"
                }
            ])),
        );
        let run_id = db.create_run_with_metadata(
            "user-1",
            "Ship verified done task",
            "auto",
            &[],
            Some("task-verified"),
            Some("group-1"),
            None,
        );
        let step_id = db.create_step(&run_id, "implement", "standard", "medium", "Ship it");
        {
            let conn = db.conn();
            conn.execute(
                "UPDATE steps
                 SET status = 'verified'
                 WHERE id = ?1",
                params![step_id],
            )
            .unwrap();
        }
        db.record_verifier_report(
            &step_id,
            &run_id,
            0,
            None,
            "test",
            "verified",
            "pass",
            r#"{"verifier_report":{"verdict":"success"}}"#,
        );
        assert!(db.update_run_status(&run_id, "succeeded", None));

        let summary = db.get_group_operations_summary("user-1", "group-1", 25);

        assert_eq!(summary["tasks"]["done_raw"], 2);
        assert_eq!(summary["tasks"]["completion"]["gated_done_available"], true);
        assert_eq!(summary["tasks"]["completion"]["gated_done"], 1);
        assert_eq!(summary["tasks"]["completion"]["done_without_evidence"], 1);
        assert!(summary["attention"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["kind"] == "done_without_evidence"
                && item["task_id"] == "task-raw"
                && item["reason"] == "no_run"));
    }

    // --- V3: verdict persistence ---

    fn spec(id: &str, required: bool) -> CheckSpec {
        CheckSpec {
            id: id.to_string(),
            source: CheckSource::Contract,
            command: vec!["cargo".into(), "test".into()],
            timeout_secs: 60,
            required,
        }
    }

    fn execution(spec_id: &str, outcome: CheckOutcome, exit_code: Option<i32>) -> CheckExecution {
        CheckExecution {
            spec_id: spec_id.to_string(),
            exit_code,
            outcome,
            duration_ms: 1200,
            output_digest: "sha256:deadbeef".to_string(),
            output_tail: "ok".to_string(),
            runner_image: "cortex/runner@sha256:abc".to_string(),
        }
    }

    #[test]
    fn migration_v62_creates_execution_jobs() {
        let db = test_db();
        let conn = db.conn();

        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert!(
            version >= 62,
            "fresh database must reach v62, got {version}"
        );

        let found: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'execution_jobs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(found, 1, "execution_jobs must exist after migration");
    }

    #[test]
    fn migration_v64_records_what_egress_was_enforced() {
        // `network_policy` says what was asked for. These say what held. A
        // receipt that names a policy without naming the hosts it resolved to,
        // or what enforced them, is not an answer to "what could this reach".
        let db = test_db();
        let conn = db.conn();

        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(execution_jobs)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        for column in ["effective_egress", "egress_mediator"] {
            assert!(
                columns.iter().any(|c| c == column),
                "execution_jobs must record {column}; have {columns:?}"
            );
        }
    }

    fn sample_execution_job() -> cortex_core::execution_job::ExecutionJob {
        use cortex_core::execution_job::*;
        ExecutionJob {
            job_id: uuid::Uuid::new_v4().to_string(),
            job_version: EXECUTION_JOB_VERSION,
            run_id: "run-1".to_string(),
            step_id: "step-1".to_string(),
            attempt_id: "attempt-1".to_string(),
            lease_gen: 4,
            model_ref: ModelRef::uncatalogued("claude-opus-5"),
            backend_kind: BackendKind::Cli,
            effort: None,
            effort_applied: EffortApplication::NotRequested,
            budgets: Budgets::unquoted(),
            network_policy: NetworkPolicy::Deny,
            capability_grants: Vec::new(),
            context_bundle: None,
            quote_id: None,
            plan_receipt_id: None,
            image_ref: "cortex/sandbox@sha256:abc".to_string(),
            isolation_class: IsolationClass::Container,
            resource_profile: ResourceProfile::default(),
            effective_egress: Some(Vec::new()),
            egress_mediator: None,
        }
    }

    #[test]
    fn execution_job_records_the_provenance_a_receipt_needs() {
        // Invariant 4: a receipt names an immutable image, a resource profile,
        // and what actually ran. None of it was persisted before this table.
        let db = test_db();
        let job = sample_execution_job();
        assert!(db.record_execution_job("run-1", &job));

        let conn = db.conn();
        let (image, isolation, profile_version, model, wall_clock): (
            String,
            String,
            String,
            String,
            i64,
        ) = conn
            .query_row(
                "SELECT image_ref, isolation_class, profile_version, model_catalog_id, wall_clock_ms
                 FROM execution_jobs WHERE attempt_id = ?1",
                params![job.attempt_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();

        assert_eq!(image, "cortex/sandbox@sha256:abc");
        assert_eq!(isolation, "container");
        assert_eq!(profile_version, "rp-1");
        assert_eq!(model, "claude-opus-5");
        assert!(
            wall_clock > 0,
            "an unbounded attempt must not be recordable"
        );
    }

    #[test]
    fn execution_job_is_idempotent_on_attempt_and_lease_gen() {
        // A resubmission under the same attempt is the same logical execution
        // and must not record a second sandbox.
        let db = test_db();
        let first = sample_execution_job();
        let mut second = sample_execution_job();
        second.job_id = uuid::Uuid::new_v4().to_string();

        assert!(db.record_execution_job("run-1", &first));
        assert!(
            !db.record_execution_job("run-1", &second),
            "a duplicate attempt must be reported, not inserted twice"
        );

        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM execution_jobs WHERE attempt_id = ?1",
                params![first.attempt_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn a_new_lease_generation_is_a_new_job() {
        // The other side of idempotency: a genuine retry under a new lease
        // must be recorded, or a receipt loses the attempt that actually ran.
        let db = test_db();
        let first = sample_execution_job();
        let mut retried = sample_execution_job();
        retried.job_id = uuid::Uuid::new_v4().to_string();
        retried.lease_gen = first.lease_gen + 1;

        assert!(db.record_execution_job("run-1", &first));
        assert!(db.record_execution_job("run-1", &retried));
    }

    #[test]
    fn migration_v61_creates_the_verification_tables() {
        let db = test_db();
        let conn = db.conn();

        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert!(
            version >= 62,
            "fresh database must reach v62, got {version}"
        );

        for table in [
            "verification_runs",
            "verification_checks",
            "verification_specs",
        ] {
            let found: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    params![table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(found, 1, "{table} must exist after migration");
        }
    }

    #[test]
    fn claim_verification_is_a_compare_and_swap() {
        let db = test_db();

        let first = db.claim_verification("run-1", "step-1", 1, "tree-abc", "img@sha256:1");
        assert!(first.is_some(), "first claim wins");

        let second = db.claim_verification("run-1", "step-1", 1, "tree-abc", "img@sha256:1");
        assert!(
            second.is_none(),
            "a second claim on the same attempt must lose — this is what mints the ledger key exactly once"
        );

        // A different attempt is a different verdict, and may be claimed.
        let next_attempt = db.claim_verification("run-1", "step-1", 2, "tree-def", "img@sha256:1");
        assert!(next_attempt.is_some());
        assert_ne!(first, next_attempt);
    }

    #[test]
    fn frozen_specs_are_written_once_and_never_overwritten() {
        let db = test_db();
        db.save_check_specs("run-1", "step-1", &[spec("a", true)])
            .expect("freeze");
        // A second derivation must not be able to change the exam.
        db.save_check_specs("run-1", "step-1", &[spec("b", true), spec("c", true)])
            .expect("second freeze is a no-op");

        let loaded = db.load_check_specs("run-1", "step-1");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "a");

        assert!(
            db.load_check_specs("run-1", "missing").is_empty(),
            "no frozen specs reads as empty, not as an error"
        );
    }

    use cortex_core::billing_binding::{ChargeKey, RefundKey};

    #[test]
    fn refund_mirrors_the_charge_and_replays_as_a_no_op() {
        let db = test_db();
        db.init_credit_balance("user-1", 100).expect("balance");

        let charge = ChargeKey::for_verification("v-1");
        let refund = RefundKey::for_verification("v-1");
        db.deduct_credits("user-1", 30, "verified task", &charge)
            .expect("charge");
        let after_charge = db.credit_ledger_totals("user-1");
        assert_eq!(after_charge.0, -30, "subscription bucket drew 30");

        let refunded = db
            .refund_credits("user-1", &charge, &refund, "failed verdict")
            .expect("refund");
        assert_eq!(
            refunded.subscription_remaining, 100,
            "a refund restores exactly what the charge took"
        );
        assert_eq!(
            db.credit_ledger_totals("user-1").0,
            0,
            "the append-only log nets to zero"
        );

        // Replay: the process died between verdict and refund and retried.
        let replay = db
            .refund_credits("user-1", &charge, &refund, "failed verdict")
            .expect("replay is not an error");
        assert_eq!(replay.subscription_remaining, 100, "replay changes nothing");
        assert_eq!(db.credit_ledger_totals("user-1").0, 0);
    }

    #[test]
    fn refund_without_a_matching_charge_moves_no_money() {
        let db = test_db();
        db.init_credit_balance("user-1", 50).expect("balance");

        let out = db
            .refund_credits(
                "user-1",
                &ChargeKey::for_verification("never-charged"),
                &RefundKey::for_verification("x"),
                "no charge",
            )
            .expect("not an error");
        assert_eq!(out.subscription_remaining, 50);
        assert_eq!(db.credit_ledger_totals("user-1"), (0, 0));
    }

    use cortex_core::hotspot::MIGRATION_SEQUENCE;

    /// The failure this exists to remove: two agents each picking "the next"
    /// migration number, both files clean, the merge succeeding, and the
    /// second migration silently skipped.
    #[test]
    fn a_sequence_never_hands_out_the_same_value_twice() {
        let db = test_db();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..50 {
            let v = db
                .allocate_sequence_value("repo-1", MIGRATION_SEQUENCE, 1, None, None)
                .expect("allocation");
            assert!(seen.insert(v), "value {v} was handed out twice");
        }
        assert_eq!(seen.len(), 50);
    }

    #[test]
    fn allocation_starts_where_the_repository_already_is() {
        let db = test_db();
        // A repo already on v65 must be handed 66, not 1.
        assert_eq!(
            db.allocate_sequence_value("repo-1", MIGRATION_SEQUENCE, 66, None, None),
            Some(66)
        );
        assert_eq!(
            db.allocate_sequence_value("repo-1", MIGRATION_SEQUENCE, 66, None, None),
            Some(67)
        );
    }

    /// `start_at` must not pull an advanced sequence backwards.
    #[test]
    fn a_lower_start_does_not_reissue_a_used_value() {
        let db = test_db();
        db.allocate_sequence_value("repo-1", MIGRATION_SEQUENCE, 100, None, None);
        assert_eq!(
            db.allocate_sequence_value("repo-1", MIGRATION_SEQUENCE, 1, None, None),
            Some(101),
            "a stale start_at must not rewind the sequence"
        );
    }

    #[test]
    fn sequences_are_independent_per_repo_and_per_name() {
        let db = test_db();
        assert_eq!(
            db.allocate_sequence_value("repo-1", MIGRATION_SEQUENCE, 1, None, None),
            Some(1)
        );
        // A different repo is a different sequence.
        assert_eq!(
            db.allocate_sequence_value("repo-2", MIGRATION_SEQUENCE, 1, None, None),
            Some(1)
        );
        // So is a different name in the same repo.
        assert_eq!(
            db.allocate_sequence_value("repo-1", "port", 1, None, None),
            Some(1)
        );
    }

    /// An allocation must be traceable to the work that asked for it, or a
    /// number appears in a diff with no explanation.
    #[test]
    fn an_allocation_records_who_asked() {
        let db = test_db();
        db.allocate_sequence_value(
            "repo-1",
            MIGRATION_SEQUENCE,
            1,
            Some("run-1"),
            Some("step-1"),
        );
        let conn = db.conn();
        let (run, step): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT run_id, step_id FROM sequence_allocations WHERE repo_key = 'repo-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("row");
        assert_eq!(run.as_deref(), Some("run-1"));
        assert_eq!(step.as_deref(), Some("step-1"));
    }

    #[test]
    fn the_latest_value_reports_what_was_handed_out() {
        let db = test_db();
        assert_eq!(db.latest_sequence_value("repo-1", MIGRATION_SEQUENCE), None);
        db.allocate_sequence_value("repo-1", MIGRATION_SEQUENCE, 10, None, None);
        db.allocate_sequence_value("repo-1", MIGRATION_SEQUENCE, 10, None, None);
        assert_eq!(
            db.latest_sequence_value("repo-1", MIGRATION_SEQUENCE),
            Some(11)
        );
    }

    fn seed_run_and_step(db: &Database, run_id: &str, step_id: &str, paths: &[&str]) {
        let conn = db.conn();
        conn.execute(
            "INSERT OR IGNORE INTO runs (id, user_id, goal, created_at, updated_at, repo_key)
             VALUES (?1, 'user-1', 'g', 0, 0, 'repo-1')",
            params![run_id],
        )
        .expect("run");
        let seed = serde_json::json!({ "target_paths": paths }).to_string();
        conn.execute(
            "INSERT INTO steps (id, run_id, kind, tier, risk, objective, created_at, updated_at, recipe_seed_json)
             VALUES (?1, ?2, 'execute', 'standard', 'low', 'o', 0, 0, ?3)",
            params![step_id, run_id, seed],
        )
        .expect("step");
    }

    #[test]
    fn a_step_reads_the_paths_its_planner_seed_declared() {
        let db = test_db();
        seed_run_and_step(&db, "run-1", "step-1", &["src/a.rs", "src/b.rs"]);
        assert_eq!(
            db.get_step_target_paths("step-1"),
            Some(vec!["src/a.rs".to_string(), "src/b.rs".to_string()])
        );
        // No seed at all is `None` — unknown, which the caller treats as
        // repo-wide rather than as "writes nothing".
        seed_run_and_step(&db, "run-1", "step-2", &[]);
        assert_eq!(db.get_step_target_paths("step-2"), None);
        assert_eq!(db.get_step_target_paths("step-missing"), None);
    }

    #[test]
    fn the_first_step_takes_the_path_and_the_second_is_refused() {
        let db = test_db();
        seed_run_and_step(&db, "run-1", "step-1", &["src/a.rs"]);
        seed_run_and_step(&db, "run-2", "step-2", &["src/a.rs"]);

        assert!(db
            .acquire_step_path_leases(
                "user-1",
                "run-1",
                "step-1",
                "repo-1",
                &["src/a.rs".to_string()]
            )
            .is_ok());
        let conflict = db
            .acquire_step_path_leases(
                "user-1",
                "run-2",
                "step-2",
                "repo-1",
                &["src/a.rs".to_string()],
            )
            .expect_err("the path is already held");
        assert_eq!(conflict.resource_key, "src/a.rs");
        assert_eq!(conflict.step_id.as_deref(), Some("step-1"));
    }

    /// Disjoint paths must not serialise — that is the whole point.
    #[test]
    fn two_steps_on_different_paths_both_acquire() {
        let db = test_db();
        seed_run_and_step(&db, "run-1", "step-1", &["src/a.rs"]);
        seed_run_and_step(&db, "run-2", "step-2", &["src/b.rs"]);
        assert!(db
            .acquire_step_path_leases(
                "user-1",
                "run-1",
                "step-1",
                "repo-1",
                &["src/a.rs".to_string()]
            )
            .is_ok());
        assert!(db
            .acquire_step_path_leases(
                "user-1",
                "run-2",
                "step-2",
                "repo-1",
                &["src/b.rs".to_string()]
            )
            .is_ok());
    }

    /// A redispatch after a retry must not deadlock the step against itself.
    #[test]
    fn a_step_re_acquiring_its_own_path_is_not_a_conflict() {
        let db = test_db();
        seed_run_and_step(&db, "run-1", "step-1", &["src/a.rs"]);
        let keys = vec!["src/a.rs".to_string()];
        assert!(db
            .acquire_step_path_leases("user-1", "run-1", "step-1", "repo-1", &keys)
            .is_ok());
        assert!(
            db.acquire_step_path_leases("user-1", "run-1", "step-1", "repo-1", &keys)
                .is_ok(),
            "the same step must be able to re-acquire what it already holds"
        );
    }

    /// Releasing is what makes the next step's retry succeed. Without it the
    /// queue never drains and a waiting step spins until the TTL.
    #[test]
    fn releasing_a_steps_leases_lets_the_waiting_step_through() {
        let db = test_db();
        seed_run_and_step(&db, "run-1", "step-1", &["src/a.rs"]);
        seed_run_and_step(&db, "run-2", "step-2", &["src/a.rs"]);
        let keys = vec!["src/a.rs".to_string()];

        db.acquire_step_path_leases("user-1", "run-1", "step-1", "repo-1", &keys)
            .expect("first acquires");
        assert!(db
            .acquire_step_path_leases("user-1", "run-2", "step-2", "repo-1", &keys)
            .is_err());

        assert_eq!(db.release_step_resource_leases("step-1"), 1);
        assert!(
            db.acquire_step_path_leases("user-1", "run-2", "step-2", "repo-1", &keys)
                .is_ok(),
            "the waiting step must get the path once it is freed"
        );
        // Idempotent: releasing twice frees nothing more.
        assert_eq!(db.release_step_resource_leases("step-1"), 0);
    }

    /// Overlap is by directory, so a step holding `src` blocks one wanting
    /// `src/a.rs`. If this ever passes, two steps write the same file.
    #[test]
    fn a_directory_lease_blocks_a_file_inside_it() {
        let db = test_db();
        seed_run_and_step(&db, "run-1", "step-1", &["src"]);
        seed_run_and_step(&db, "run-2", "step-2", &["src/a.rs"]);
        db.acquire_step_path_leases("user-1", "run-1", "step-1", "repo-1", &["src".to_string()])
            .expect("directory acquires");
        assert!(db
            .acquire_step_path_leases(
                "user-1",
                "run-2",
                "step-2",
                "repo-1",
                &["src/a.rs".to_string()]
            )
            .is_err());
    }

    /// Leases are keyed per repo, so the same path in two repos does not
    /// contend.
    #[test]
    fn the_same_path_in_a_different_repo_does_not_contend() {
        let db = test_db();
        seed_run_and_step(&db, "run-1", "step-1", &["src/a.rs"]);
        seed_run_and_step(&db, "run-2", "step-2", &["src/a.rs"]);
        let keys = vec!["src/a.rs".to_string()];
        db.acquire_step_path_leases("user-1", "run-1", "step-1", "repo-1", &keys)
            .expect("repo-1 acquires");
        assert!(
            db.acquire_step_path_leases("user-1", "run-2", "step-2", "repo-2", &keys)
                .is_ok(),
            "a different repo is a different resource"
        );
    }

    #[test]
    fn a_step_with_no_write_set_takes_no_lease() {
        let db = test_db();
        seed_run_and_step(&db, "run-1", "step-1", &[]);
        assert!(db
            .acquire_step_path_leases("user-1", "run-1", "step-1", "repo-1", &[])
            .is_ok());
        assert_eq!(db.release_step_resource_leases("step-1"), 0);
    }

    #[test]
    fn a_run_repo_key_round_trips() {
        let db = test_db();
        seed_run_and_step(&db, "run-1", "step-1", &["src/a.rs"]);
        assert_eq!(db.get_run_repo_key("run-1").as_deref(), Some("repo-1"));
        assert_eq!(db.get_run_repo_key("run-absent"), None);
    }

    /// The routing signal picks its domain from this, and the column holds the
    /// `{:?}` form — so a serde round-trip would return `None` for every row
    /// ever written and silently file every verdict under "Conversation".
    #[test]
    fn the_stored_debug_form_of_an_intent_parses_back() {
        let db = test_db();
        db.record_decision(
            "dec-1",
            "user-1",
            Some("run-i"),
            Some("step-i"),
            // Exactly what scheduler.rs writes.
            &format!("{:?}", cortex_core::routing::Intent::Refactor),
            "low",
            "standard",
            "claude",
            "claude-opus-5",
            None,
            "",
            "auto",
        );

        assert_eq!(
            db.get_step_intent("step-i"),
            Some(cortex_core::routing::Intent::Refactor)
        );
        assert_eq!(db.get_step_intent("step-never-routed"), None);
    }

    #[test]
    fn the_attempt_chain_counts_every_try_not_just_the_last() {
        let db = test_db();
        assert_eq!(db.attempt_chain_spend("step-none"), AttemptChain::default());

        let conn = db.conn();
        // `step_attempts.step_id` is a foreign key, so the chain needs a real
        // step to hang off.
        conn.execute(
            "INSERT INTO runs (id, user_id, goal, created_at, updated_at)
             VALUES ('run-c', 'user-1', 'g', 0, 0)",
            [],
        )
        .expect("insert run");
        conn.execute(
            "INSERT INTO steps (id, run_id, kind, tier, risk, objective, created_at, updated_at)
             VALUES ('step-c', 'run-c', 'execute', 'standard', 'low', 'o', 0, 0)",
            [],
        )
        .expect("insert step");

        // Three attempts: two finished, one still in flight.
        for (n, started, finished) in [
            (1i64, 1_000i64, Some(3_000i64)),
            (2, 4_000, Some(9_000)),
            (3, 10_000, None),
        ] {
            conn.execute(
                "INSERT INTO step_attempts
                    (step_id, run_id, attempt_number, lease_gen, status, started_at, finished_at)
                 VALUES ('step-c', 'run-c', ?1, ?1, 'done', ?2, ?3)",
                params![n, started, finished],
            )
            .expect("insert attempt");
        }
        drop(conn);

        let chain = db.attempt_chain_spend("step-c");
        assert_eq!(
            chain.attempts, 3,
            "the unfinished attempt still cost a dispatch"
        );
        // 2000 + 5000; the in-flight attempt contributes no duration rather
        // than being counted as zero-length.
        assert_eq!(chain.total_duration_ms, 7_000);
    }

    #[test]
    fn an_unsealed_verification_has_no_receipt() {
        // F18: the same input produced two different verdicts on consecutive
        // runs. This is why. The gate is recomputed from the executions
        // recorded *so far*, so an in-flight verification yields a receipt
        // whose verdict changes underneath a caller as checks land --
        // `Inconclusive` with nothing executed, then something else. Anything
        // polling for "a receipt exists" caught whichever it happened to hit.
        //
        // A receipt is the record of a verification that finished.
        let db = test_db();
        let specs = vec![spec("check-pass", true), spec("check-fail", true)];
        db.save_check_specs("run-9", "step-9", &specs)
            .expect("freeze");

        let vid = db
            .claim_verification("run-9", "step-9", 1, "tree-xyz", "img@sha256:9")
            .expect("claim");

        assert!(
            db.get_receipt("run-9", "step-9").is_none(),
            "a claimed but unfinished verification must not serve a receipt"
        );

        db.record_check_execution(
            &vid,
            &specs[0],
            &execution("check-pass", CheckOutcome::Passed, Some(0)),
        )
        .expect("record execution");

        assert!(
            db.get_receipt("run-9", "step-9").is_none(),
            "a partially executed verification must not serve a receipt either -- \
             this is the state that produced the varying verdict"
        );

        db.finish_verification(&vid, Verdict::Failed).expect("seal");

        assert!(
            db.get_receipt("run-9", "step-9").is_some(),
            "a sealed verification must serve its receipt"
        );
    }

    #[test]
    fn receipt_serves_the_shape_the_frontend_types_against() {
        let db = test_db();
        let specs = vec![spec("check-pass", true), spec("check-fail", true)];
        db.save_check_specs("run-1", "step-1", &specs)
            .expect("freeze");

        let vid = db
            .claim_verification("run-1", "step-1", 1, "tree-abc", "img@sha256:1")
            .expect("claim");
        db.record_check_execution(
            &vid,
            &specs[0],
            &execution("check-pass", CheckOutcome::Passed, Some(0)),
        )
        .expect("record execution");
        db.finish_verification(&vid, Verdict::Failed).expect("seal");

        let receipt = db.get_receipt("run-1", "step-1").expect("receipt exists");
        assert_eq!(receipt.verification_id, vid);
        assert_eq!(receipt.attempt, 1);
        assert_eq!(receipt.tree_hash, "tree-abc");

        // The gate is derived, so a required check with no execution row is
        // never silently a pass.
        assert!(receipt
            .gate
            .not_executed
            .contains(&"check-fail".to_string()));

        // Field names are the contract with Receipt.tsx.
        let json = serde_json::to_value(&receipt).expect("serializes");
        for key in [
            "verification_id",
            "run_id",
            "step_id",
            "attempt",
            "tree_hash",
            "gate",
            "executions",
        ] {
            assert!(json.get(key).is_some(), "receipt must carry `{key}`");
        }

        assert!(
            db.get_receipt("run-1", "never-verified").is_none(),
            "no verification means no receipt, not an empty one"
        );

        // A step that ran before scoped egress was recorded carries no egress
        // block at all. Reporting it as "reached nothing" would be a claim we
        // have no evidence for.
        assert!(
            receipt.egress.is_none(),
            "a step with no execution job must not claim an egress"
        );
        assert!(
            json.get("egress").is_none(),
            "an absent egress must be absent from the wire, not null"
        );
    }

    #[test]
    fn the_receipt_reports_what_the_sandbox_could_reach() {
        let db = test_db();
        let specs = vec![spec("check-pass", true)];
        db.save_check_specs("run-2", "step-2", &specs)
            .expect("freeze");

        // A job whose planner granted crates.io, recorded the way the worker
        // records it.
        let mut job = sample_execution_job();
        job.run_id = "run-2".to_string();
        job.step_id = "step-2".to_string();
        job.capability_grants = vec![
            cortex_core::execution_job::CapabilityGrant::ResolveDependencies {
                registries: vec!["crates".to_string()],
            },
        ];
        job.effective_egress = Some(vec![
            "index.crates.io:443".to_string(),
            "static.crates.io:443".to_string(),
        ]);
        job.egress_mediator = Some("cortex/egress:dev".to_string());
        db.record_execution_job("run-2", &job);

        let vid = db
            .claim_verification("run-2", "step-2", 1, "tree-xyz", "img@sha256:1")
            .expect("claim");
        db.record_check_execution(
            &vid,
            &specs[0],
            &execution("check-pass", CheckOutcome::Passed, Some(0)),
        )
        .expect("record execution");
        db.finish_verification(&vid, Verdict::Verified)
            .expect("seal");

        let receipt = db.get_receipt("run-2", "step-2").expect("receipt exists");
        let egress = receipt.egress.clone().expect("the job recorded an egress");

        // What was granted and what was opened, both — so a reader can see them
        // disagree rather than having to trust that they cannot.
        assert_eq!(egress.granted_registries, vec!["crates".to_string()]);
        assert!(egress
            .endpoints
            .contains(&"index.crates.io:443".to_string()));
        assert!(
            !egress.endpoints.iter().any(|e| e.contains("npmjs")),
            "a cargo grant must not show npm on the receipt"
        );
        assert_eq!(egress.mediator_image.as_deref(), Some("cortex/egress:dev"));

        let json = serde_json::to_value(&receipt).expect("serializes");
        assert!(json["egress"]["endpoints"].is_array());
    }

    #[test]
    fn a_receipt_for_a_sandbox_that_opened_nothing_says_so() {
        let db = test_db();
        let specs = vec![spec("check-pass", true)];
        db.save_check_specs("run-3", "step-3", &specs)
            .expect("freeze");

        let mut job = sample_execution_job();
        job.run_id = "run-3".to_string();
        job.step_id = "step-3".to_string();
        // Recorded, and empty. Distinct from the `None` above.
        job.effective_egress = Some(Vec::new());
        job.egress_mediator = None;
        db.record_execution_job("run-3", &job);

        let vid = db
            .claim_verification("run-3", "step-3", 1, "tree-none", "img@sha256:1")
            .expect("claim");
        db.record_check_execution(
            &vid,
            &specs[0],
            &execution("check-pass", CheckOutcome::Passed, Some(0)),
        )
        .expect("record execution");
        db.finish_verification(&vid, Verdict::Verified)
            .expect("seal");

        let egress = db
            .get_receipt("run-3", "step-3")
            .expect("receipt")
            .egress
            .expect("recorded, even though it is empty");

        assert!(egress.endpoints.is_empty(), "nothing was opened");
        assert!(egress.granted_registries.is_empty());
        assert!(
            egress.mediator_image.is_none(),
            "no mediator is stood up for a sandbox with no network"
        );
    }

    #[test]
    fn unknown_outcome_text_cannot_manufacture_a_pass() {
        assert_eq!(
            check_outcome_from_str("something-corrupt"),
            CheckOutcome::NotExecuted,
            "an unreadable row must land on the one outcome that cannot bill"
        );
        assert_eq!(check_outcome_from_str("passed"), CheckOutcome::Passed);
        assert_eq!(check_outcome_from_str("timed_out"), CheckOutcome::TimedOut);
    }
}

/// The truth model: a worker's report is a diagnostic, never a transition.
///
/// Every test here is named for the shortcut it exists to catch. See
/// `docs/adr/ADR-0001-step-truth-model.md`.
#[cfg(test)]
mod truth {
    use super::tests::test_db;
    use super::*;

    #[test]
    fn leasing_to_an_unregistered_worker_is_reported_not_silently_lost() {
        // The bug this pins: the scheduler leased with the literal string
        // "scheduler" as the worker id. `steps.assigned_worker` is a foreign
        // key onto `workers(id)`, so with `PRAGMA foreign_keys = ON` every
        // dispatch raised `FOREIGN KEY constraint failed` — and the error was
        // swallowed into the same `None` that means "another dispatcher won the
        // CAS". No step could ever be leased, so no step could ever run, and
        // the only symptom was one warning per reconcile tick that named the
        // wrong cause.
        //
        // The assertion is deliberately about the *distinction*: leasing to a
        // registered worker must succeed, and leasing to one that does not
        // exist must not. A test that only checked the happy path would have
        // passed against the broken code, because the broken code never got a
        // registered worker id to pass.
        let db = test_db();
        let now = Utc::now().timestamp_millis();
        let run_id = db.create_run_with_steps(
            "user-1",
            "ship it",
            "auto",
            &[],
            None,
            None,
            None,
            &[(
                "step-lease".to_string(),
                "execute".to_string(),
                "ship".to_string(),
                None,
                "execute".to_string(),
                "medium".to_string(),
                "Do the work".to_string(),
                now,
            )],
            &[],
        );
        assert!(!run_id.is_empty());

        assert_eq!(
            db.lease_step("step-lease", "w-does-not-exist", now + 600_000),
            None,
            "leasing to a worker with no row must fail rather than corrupting the \
             foreign key"
        );

        db.register_worker("w-real", "user-1");
        let lease_gen = db
            .lease_step("step-lease", "w-real", now + 600_000)
            .expect("leasing to a registered worker must succeed");
        assert!(lease_gen > 0);
    }

    /// A run with one leased, running step, ready to receive a delivery.
    fn leased_step(db: &Database, step_id: &str) -> (String, i64) {
        let now = Utc::now().timestamp_millis();
        let run_id = db.create_run_with_steps(
            "user-1",
            "ship it",
            "auto",
            &[],
            None,
            None,
            None,
            &[(
                step_id.to_string(),
                "execute".to_string(),
                "ship".to_string(),
                None,
                "execute".to_string(),
                "medium".to_string(),
                "Do the work".to_string(),
                now,
            )],
            &[],
        );
        db.update_run_status(&run_id, "running", None);
        db.register_worker("worker-1", "user-1");
        let lease_gen = db
            .lease_step(step_id, "worker-1", now + 600_000)
            .expect("step leases");
        assert!(db.start_step(step_id, lease_gen));
        (run_id, lease_gen)
    }

    fn status_of(db: &Database, step_id: &str) -> String {
        db.get_step_status(step_id).expect("step exists")
    }

    fn parsed_statuses(
        db: &Database,
        run_id: &str,
    ) -> Vec<(String, cortex_engine::captain::StepStatus)> {
        db.get_all_step_statuses(run_id)
            .into_iter()
            .filter_map(|(id, s)| cortex_engine::captain::StepStatus::from_str(&s).map(|p| (id, p)))
            .collect()
    }

    #[test]
    fn worker_success_cannot_make_step_verified() {
        // The core regression. A worker reports success; the step must be
        // `delivered` and nothing else.
        let db = test_db();
        let (_run, gen) = leased_step(&db, "step-1");

        assert!(db.deliver_step("step-1", "a1", gen, Some("done"), None, None, Some("abc")));

        let status = status_of(&db, "step-1");
        assert_eq!(status, "delivered");
        assert_ne!(status, "verified", "a worker cannot verify its own work");
        assert_ne!(status, "succeeded", "the state that caused this is gone");
        assert_eq!(
            db.get_verification_state("step-1", "a1", gen),
            Some(("delivered".to_string(), 0))
        );
    }

    #[test]
    fn worker_success_cannot_complete_task() {
        // The same shortcut one level up: a run must not reach a terminal
        // status while its only step is merely delivered.
        let db = test_db();
        let (run_id, gen) = leased_step(&db, "step-1");
        assert!(db.deliver_step("step-1", "a1", gen, None, None, None, Some("abc")));

        let parsed = parsed_statuses(&db, &run_id);
        assert_eq!(parsed.len(), 1, "delivered must be a parseable status");
        assert_eq!(
            cortex_engine::captain::check_run_completion(&parsed),
            None,
            "a run whose work nobody checked has not finished"
        );
    }

    #[test]
    fn failed_independent_check_transitions_to_failure() {
        let db = test_db();
        let (_run, gen) = leased_step(&db, "step-1");
        assert!(db.deliver_step("step-1", "a1", gen, None, None, None, Some("abc")));
        assert!(db.begin_verifying_step("step-1", "a1", gen, None));
        assert_eq!(status_of(&db, "step-1"), "verifying");

        assert!(db.record_verification_outcome("step-1", "a1", gen, "failed", Some("c1 failed")));
        assert_eq!(status_of(&db, "step-1"), "failed");
    }

    #[test]
    fn stale_delivery_cannot_alter_active_attempt() {
        // A delivery arriving for an attempt that has already been superseded.
        let db = test_db();
        let (_run, gen) = leased_step(&db, "step-1");

        assert!(
            !db.deliver_step("step-1", "a0", gen - 1, None, None, None, Some("old")),
            "a delivery for a superseded attempt must be a no-op"
        );
        assert_eq!(status_of(&db, "step-1"), "running");
    }

    #[test]
    fn stale_verifier_result_cannot_alter_active_attempt() {
        // The same on the verdict side — the case the existing lease_gen CAS
        // defends, which had to survive this refactor.
        let db = test_db();
        let (_run, gen) = leased_step(&db, "step-1");
        assert!(db.deliver_step("step-1", "a1", gen, None, None, None, Some("abc")));
        assert!(db.begin_verifying_step("step-1", "a1", gen, None));

        assert!(
            !db.record_verification_outcome("step-1", "a1", gen - 1, "verified", None),
            "a verdict for a superseded attempt must not verify the live one"
        );
        assert_eq!(status_of(&db, "step-1"), "verifying");
    }

    #[test]
    fn dependent_write_step_blocked_until_verified() {
        let db = test_db();
        let now = Utc::now().timestamp_millis();
        let step = |id: &str| {
            (
                id.to_string(),
                "execute".to_string(),
                "ship".to_string(),
                None,
                "execute".to_string(),
                "medium".to_string(),
                format!("work {id}"),
                now,
            )
        };
        let run_id = db.create_run_with_steps(
            "user-1",
            "two steps",
            "auto",
            &[],
            None,
            None,
            None,
            &[step("step-a"), step("step-b")],
            &[(
                "step-b".to_string(),
                "step-a".to_string(),
                "success_required".to_string(),
            )],
        );
        db.update_run_status(&run_id, "running", None);
        db.register_worker("worker-1", "user-1");
        let gen = db.lease_step("step-a", "worker-1", now + 600_000).unwrap();
        assert!(db.start_step("step-a", gen));
        assert!(db.deliver_step("step-a", "a1", gen, None, None, None, Some("abc")));

        assert!(
            !db.find_ready_steps(&run_id).contains(&"step-b".to_string()),
            "a delivered tree nobody checked must not unblock a dependent"
        );

        assert!(db.begin_verifying_step("step-a", "a1", gen, None));
        assert!(
            !db.find_ready_steps(&run_id).contains(&"step-b".to_string()),
            "nor must a step that is still being graded"
        );

        assert!(db.record_verification_outcome("step-a", "a1", gen, "verified", None));
        assert!(
            db.find_ready_steps(&run_id).contains(&"step-b".to_string()),
            "a verified dependency unblocks its dependent"
        );
    }

    #[test]
    fn inconclusive_does_not_become_done() {
        let db = test_db();
        let (run_id, gen) = leased_step(&db, "step-1");
        assert!(db.deliver_step("step-1", "a1", gen, None, None, None, Some("abc")));
        assert!(db.begin_verifying_step("step-1", "a1", gen, None));
        assert!(db.record_verification_outcome(
            "step-1",
            "a1",
            gen,
            "inconclusive",
            Some("no runner")
        ));

        assert_eq!(status_of(&db, "step-1"), "inconclusive");
        assert_eq!(
            cortex_engine::captain::check_run_completion(&parsed_statuses(&db, &run_id)),
            None,
            "an unanswered question must not settle as a finished run"
        );
    }

    #[test]
    fn transition_and_event_are_one_transaction() {
        // The projection and its operations event land together. There is no
        // seam to inject a failure into from outside — that is the point — so
        // this asserts the invariant from both directions: every transition
        // that applied has its event, and one that did not apply has none.
        let db = test_db();
        let (_run, gen) = leased_step(&db, "step-1");
        assert!(db.deliver_step("step-1", "a1", gen, None, None, None, Some("abc")));
        assert!(db.begin_verifying_step("step-1", "a1", gen, None));
        assert!(db.record_verification_outcome("step-1", "a1", gen, "verified", None));

        let conn = db.conn();
        for event_type in ["step.delivered", "step.verifying", "step.verdict"] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM operations_events
                     WHERE step_id = ?1 AND event_type = ?2",
                    params!["step-1", event_type],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                count, 1,
                "{event_type} must be recorded with its transition"
            );
        }

        let refused: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM operations_events
                 WHERE step_id = ?1 AND event_type = 'step.execution_failed'",
                params!["step-1"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            refused, 0,
            "a transition that never applied must not have left an event"
        );
    }

    #[test]
    fn version_cas_rejects_concurrent_transition() {
        // Two verdicts race for the same attempt. The first advances the
        // lifecycle row; the second finds a state it may not leave.
        let db = test_db();
        let (_run, gen) = leased_step(&db, "step-1");
        assert!(db.deliver_step("step-1", "a1", gen, None, None, None, Some("abc")));
        assert!(db.begin_verifying_step("step-1", "a1", gen, None));

        assert!(db.record_verification_outcome("step-1", "a1", gen, "verified", None));
        assert!(
            !db.record_verification_outcome("step-1", "a1", gen, "failed", None),
            "the second verdict must not overwrite the first"
        );
        assert_eq!(status_of(&db, "step-1"), "verified");

        let (state, version) = db.get_verification_state("step-1", "a1", gen).unwrap();
        assert_eq!(state, "verified");
        assert_eq!(
            version, 2,
            "delivered -> verifying -> verified advances the row twice"
        );
    }

    #[test]
    fn manual_override_is_not_verified() {
        let db = test_db();
        let (_run, gen) = leased_step(&db, "step-1");
        assert!(db.deliver_step("step-1", "a1", gen, None, None, None, Some("abc")));
        assert!(db.begin_verifying_step("step-1", "a1", gen, None));
        assert!(db.record_verification_outcome("step-1", "a1", gen, "failed", Some("c1")));

        assert!(db.record_manual_override(
            "step-1",
            "a1",
            gen,
            "operator-7",
            "shipping this by hand, the check is broken",
            Some(Utc::now().timestamp_millis() + 86_400_000),
        ));

        let status = status_of(&db, "step-1");
        assert_eq!(status, "manual_override");
        assert_ne!(status, "verified", "a human deciding is a different fact");

        let (actor, reason, expires) = db.get_manual_override("step-1", "a1").expect("recorded");
        assert_eq!(actor, "operator-7");
        assert!(reason.contains("by hand"));
        assert!(
            expires.is_some(),
            "an override with no expiry silently becomes permanent"
        );
    }

    #[test]
    fn execution_failure_is_not_a_customer_failure() {
        // A sandbox that never produced a tree is our problem. It must not
        // arrive as `failed`, which is a judgement about work that exists.
        let db = test_db();
        let (_run, gen) = leased_step(&db, "step-1");

        assert!(db.record_execution_failure("step-1", "a1", gen, "sandbox image missing"));
        assert_eq!(status_of(&db, "step-1"), "execution_failed");

        let (state, _) = db.get_verification_state("step-1", "a1", gen).unwrap();
        assert_eq!(state, "execution_failed");
    }

    #[test]
    fn legacy_report_survives_as_diagnostic() {
        // The worker's own account is kept, never deleted — it is the fastest
        // signal about what the worker thought it did.
        let db = test_db();
        let (run_id, gen) = leased_step(&db, "step-1");
        db.record_attempt(
            "step-1",
            &run_id,
            1,
            "worker-1",
            gen,
            Some("claude"),
            Some("opus"),
        );
        assert!(db.deliver_step(
            "step-1",
            "a1",
            gen,
            Some("I fixed everything"),
            Some("[\"src/main.rs\"]"),
            None,
            Some("abc"),
        ));
        db.deliver_attempt("step-1", gen);

        let snapshots = db.get_run_step_snapshots(&run_id);
        let step = snapshots.iter().find(|s| s.id == "step-1").expect("step");
        assert_eq!(
            step.output_summary.as_deref(),
            Some("I fixed everything"),
            "the worker's report must not be dropped by the refactor"
        );

        let conn = db.conn();
        let attempt_status: String = conn
            .query_row(
                "SELECT status FROM step_attempts WHERE step_id = ?1 AND lease_gen = ?2",
                params!["step-1", gen],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            attempt_status, "delivered",
            "the worker's own attempt record must not claim success either"
        );
    }

    #[test]
    fn migration_backfills_succeeded_to_delivered() {
        // Historical rows become `delivered`, not `verified`. Those steps were
        // never independently verified, and labelling them so would be a false
        // claim about work already delivered to customers.
        let db = test_db();
        let (_run, gen) = leased_step(&db, "step-1");
        {
            let conn = db.conn();
            conn.execute(
                "UPDATE steps SET status = 'succeeded' WHERE id = ?1",
                params!["step-1"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO step_attempts
                     (step_id, run_id, attempt_number, worker_id, lease_gen, status, started_at)
                 VALUES (?1, 'run-x', 9, 'worker-1', ?2, 'succeeded', 0)",
                params!["step-1", gen + 50],
            )
            .unwrap();

            // Re-run the migration against rows that predate it.
            migrate_v63(&conn);

            let leftovers: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM steps WHERE status = 'succeeded'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(leftovers, 0, "no step may keep the old ambiguous status");

            let attempt_leftovers: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM step_attempts WHERE status = 'succeeded'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(attempt_leftovers, 0);
        }

        assert_eq!(
            status_of(&db, "step-1"),
            "delivered",
            "history becomes delivered, never verified"
        );
    }

    #[test]
    fn migration_v63_creates_the_truth_model_tables() {
        let db = test_db();
        let conn = db.conn();
        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        // Raised to 66 with the price list catalog. The floor is the point of
        // this assertion: the shared `schema_version` counter means a
        // collision silently skips whichever migration merged second, and a
        // floor that never moves cannot notice.
        assert!(
            version >= 66,
            "fresh database must reach v66, got {version}"
        );

        for table in ["step_verification_state", "manual_overrides"] {
            let found: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    params![table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(found, 1, "{table} must exist after migration");
        }
    }
}

// --- Durable verification jobs (PR B) ------------------------------------

/// How long a claim survives without a heartbeat before another dispatcher may
/// take it. Long enough that a slow check does not lose its own claim, short
/// enough that a dispatcher killed mid-run does not strand a delivery for the
/// rest of the day.
pub const VERIFICATION_LEASE_MS: i64 = 5 * 60 * 1000;

/// How many times a job may be attempted before it is `dead`.
///
/// Reclaim counts as an attempt, so a job that reliably kills its dispatcher
/// reaches the dead-letter state instead of looping forever.
pub const VERIFICATION_MAX_ATTEMPTS: i64 = 5;

/// What `begin_verifying_step` needs to enqueue the durable job alongside the
/// transition.
///
/// Borrowed rather than owned because it lives exactly as long as the call: it
/// is not a thing to store, it is the argument that keeps the enqueue inside
/// the transaction.
#[derive(Debug, Clone, Copy)]
pub struct VerificationEnqueue<'a> {
    pub job_id: &'a str,
    pub run_id: &'a str,
    /// The commit the worker delivered. Frozen here; never re-resolved.
    pub delivered_commit: &'a str,
    /// Digest of the checks frozen at dispatch, from [`spec_set_digest`].
    pub spec_set_digest: &'a str,
    /// Which runner policy was in force, so a receipt can say what the rules
    /// were rather than what they are now.
    pub runner_policy_ver: &'a str,
}

/// A verification job as the dispatcher sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationJob {
    pub job_id: String,
    pub run_id: String,
    pub step_id: String,
    pub attempt_id: String,
    pub lease_gen: i64,
    /// The commit that was delivered. Immutable, and never re-resolved: a job
    /// that looked the commit up again at claim time could grade a tree the
    /// worker never delivered.
    pub delivered_commit: String,
    /// Digest of the frozen check specs at the moment the job was enqueued.
    ///
    /// The brief asks for a spec *set id*; a digest is the same guarantee
    /// without a second table to keep honest. At claim time the dispatcher
    /// re-digests what it loaded and refuses if it differs, so a job cannot be
    /// graded against a different exam than the one it was promised.
    pub spec_set_digest: String,
    pub state: String,
    pub attempt_count: i64,
    pub claim_token: Option<String>,
    pub next_run_at: Option<i64>,
    pub terminal_reason: Option<String>,
}

/// The digest of a frozen spec set.
///
/// Over the serialized specs rather than over a row id, so "the exam did not
/// change" is checkable without trusting that nothing rewrote the row.
pub fn spec_set_digest(specs: &[cortex_core::verification::CheckSpec]) -> String {
    use sha2::{Digest, Sha256};
    let canonical = serde_json::to_string(specs).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn read_verification_job(conn: &Connection, job_id: &str) -> Option<VerificationJob> {
    conn.query_row(
        "SELECT job_id, run_id, step_id, attempt_id, lease_gen, delivered_commit,
                spec_set_id, state, attempt_count, claim_token, next_run_at, terminal_reason
         FROM verification_jobs WHERE job_id = ?1",
        params![job_id],
        |row| {
            Ok(VerificationJob {
                job_id: row.get(0)?,
                run_id: row.get(1)?,
                step_id: row.get(2)?,
                attempt_id: row.get(3)?,
                lease_gen: row.get(4)?,
                delivered_commit: row.get(5)?,
                spec_set_digest: row.get(6)?,
                state: row.get(7)?,
                attempt_count: row.get(8)?,
                claim_token: row.get(9)?,
                next_run_at: row.get(10)?,
                terminal_reason: row.get(11)?,
            })
        },
    )
    .ok()
}

/// Worker service credentials (`cwk_` keys).
///
/// A worker is a long-lived headless daemon, not a browser session, so it
/// cannot hold a Clerk user JWT. These are the rows that let one prove who it
/// is. Issuance is deliberately absent from the HTTP surface — see the
/// `cortex-worker-key` binary.
impl Database {
    /// Record a newly issued worker key.
    ///
    /// Takes the **hash**, never the plaintext: the caller shows the secret to
    /// its owner once and drops it, and nothing in this process writes it down.
    /// A duplicate `key_hash` is an error rather than an overwrite — see the
    /// UNIQUE constraint in migration v67.
    ///
    /// `expires_at` is epoch milliseconds, or `None` for a key that only ends
    /// by revocation.
    pub fn create_worker_key(
        &self,
        id: &str,
        key_hash: &str,
        key_prefix: &str,
        owner_user_id: &str,
        scope: &str,
        expires_at: Option<i64>,
    ) -> Result<(), String> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO worker_keys
                (id, key_hash, key_prefix, owner_user_id, scope, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                key_hash,
                key_prefix,
                owner_user_id,
                scope,
                Utc::now().timestamp_millis(),
                expires_at
            ],
        )
        .map_err(|e| format!("failed to create worker key: {e}"))?;
        Ok(())
    }

    /// Resolve a presented worker key hash to its owner, and stamp usage.
    ///
    /// Returns `None` — never a reason — when the key is unknown, revoked, or
    /// expired. The caller is an authentication boundary and must not tell a
    /// client which of those it was.
    ///
    /// Revocation and expiry are filtered **in the SQL**, not in Rust after the
    /// fetch, so there is no shape of this function in which a caller forgets
    /// the check. `now_ms` is passed in rather than read here so a test can
    /// place a key in the past or the future without sleeping.
    ///
    /// `last_used_at` is stamped only on a successful resolution, and its
    /// failure is not fatal: an audit timestamp is not worth refusing a
    /// worker that legitimately authenticated.
    pub fn authenticate_worker_key(&self, key_hash: &str, now_ms: i64) -> Option<String> {
        let conn = self.conn();
        let owner: String = conn
            .query_row(
                "SELECT owner_user_id
                 FROM worker_keys
                 WHERE key_hash = ?1
                   AND revoked_at IS NULL
                   AND (expires_at IS NULL OR expires_at > ?2)
                 LIMIT 1",
                params![key_hash, now_ms],
                |row| row.get(0),
            )
            .ok()?;

        if let Err(e) = conn.execute(
            "UPDATE worker_keys SET last_used_at = ?1 WHERE key_hash = ?2",
            params![now_ms, key_hash],
        ) {
            tracing::warn!("could not stamp worker key last_used_at: {e}");
        }

        Some(owner)
    }

    /// Revoke a worker key by its display prefix or id. Returns rows affected.
    ///
    /// Idempotent: `revoked_at IS NULL` in the WHERE means re-revoking an
    /// already-revoked key reports 0 rather than moving the timestamp, so an
    /// audit keeps the moment revocation actually happened.
    pub fn revoke_worker_key(&self, id_or_prefix: &str) -> usize {
        let conn = self.conn();
        conn.execute(
            "UPDATE worker_keys
             SET revoked_at = ?1
             WHERE (id = ?2 OR key_prefix = ?2) AND revoked_at IS NULL",
            params![Utc::now().timestamp_millis(), id_or_prefix],
        )
        .unwrap_or(0)
    }

    /// List worker keys for an operator view. Never includes key material
    /// beyond the non-secret display prefix.
    pub fn list_worker_keys(&self, owner_user_id: Option<&str>) -> Vec<serde_json::Value> {
        let conn = self.conn();
        let mut stmt = match conn.prepare(
            "SELECT id, key_prefix, owner_user_id, scope, created_at,
                    expires_at, revoked_at, last_used_at
             FROM worker_keys
             WHERE ?1 IS NULL OR owner_user_id = ?1
             ORDER BY created_at DESC",
        ) {
            Ok(stmt) => stmt,
            Err(e) => {
                tracing::warn!("could not list worker keys: {e}");
                return Vec::new();
            }
        };
        let rows = stmt.query_map(params![owner_user_id], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "keyPrefix": row.get::<_, String>(1)?,
                "ownerUserId": row.get::<_, String>(2)?,
                "scope": row.get::<_, String>(3)?,
                "createdAt": row.get::<_, i64>(4)?,
                "expiresAt": row.get::<_, Option<i64>>(5)?,
                "revokedAt": row.get::<_, Option<i64>>(6)?,
                "lastUsedAt": row.get::<_, Option<i64>>(7)?,
            }))
        });
        match rows {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(e) => {
                tracing::warn!("could not list worker keys: {e}");
                Vec::new()
            }
        }
    }
}
