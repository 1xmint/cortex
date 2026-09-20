> **Provenance.** Produced by a read-only Explore pass on 2026-08-06 against `origin/main`, recovered from the agent transcript after the session was lost. Line citations were valid at commit `a7323c48`; re-verify any line number before relying on it. See `RECOVERY-2026-08-06.md`.

# Verifier wiring map — `heyvera-social-audit` @ `origin/main`

**Precondition confirmed:** working tree (`docs/handoff-true-state`) is byte-identical to `origin/main` for all Rust code. `git diff origin/main...HEAD` touches only `cortex/plan/HANDOFF-OPUS5.md`. All citations below are valid against main.

**The unwired claim, quantified:**
| Probe | Result |
|---|---|
| `verification_runs` in `crates/api/src/db.rs` | **0** |
| `billing_binding` in `crates/api/src/` | **0** |
| `ContainerCheckRunner` outside its own file | **0** |
| `verify_from_executions` callers | **0** |
| `compute_verdict` callers outside core+engine | **0** |

---

## 1. Migrations — `crates/api/src/db.rs`

**Mechanism:** a `schema_version` table holding a single integer, read as `MAX(version)`, followed by a flat ladder of `if current < N { migrate_vN(conn); }` statements. Not match arms, not a vec of migrations. Each `migrate_vN` is a free function that ends by writing its own version number inside the same `execute_batch`, so the version bump is atomic with the DDL.

- `apply_migrations` — `crates/api/src/db.rs:305`
- Bootstrap + counter read — `db.rs:306-319`
- Ladder — `db.rs:321` through `db.rs:484` (`if current < 52 { migrate_v52(conn); }` is the last arm)

**Highest migration on main: `v52`** — `fn migrate_v52` at `crates/api/src/db.rs:2387`.

**Stale constant worth knowing about:** `const SCHEMA_VERSION: i64 = 36;` at `db.rs:302` is the *only* occurrence in the entire workspace — it is dead and 16 versions behind the real ladder. It is not the counter; the ladder is.

**Verbatim migration entry** (`db.rs:2387-2415`) — note the trailing `UPDATE schema_version SET version = N;` inside the batch, the `.expect(...)` naming the migration, and the `tracing::info!` echo. This is the exact shape a v61 entry must copy:

```rust
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
```

**Counter contention for v61** (single shared counter, so these are ordered, not parallel):
- v53–v59 — PR **#438**, branch `fix/socials-message-integrity`, **OPEN** (title: "WIP(socials): message integrity — DM consent, activity clock, message requests (v53-v59)")
- v60 — PR **#437**, branch `fix/credit-metering-idempotency`, **OPEN**. `fn migrate_v60` at line 2425 of that branch's `db.rs`.
- v61 — V3, unwritten. Planned schema is specified verbatim in `cortex/plan/VERIFIER.md:216-237`.

---

## 2. V4 core — `crates/core/src/billing_binding.rs` (crate `cortex-core`, 254 lines)

Exported at `crates/core/src/lib.rs:12`. Pure; depends only on `crate::verification::Verdict`.

```rust
pub enum BillingEffect {                       // :25
    Charge { idempotency_key: String },
    Refund { idempotency_key: String },
    None,
}

pub enum BillingState { Unbilled, Charged, Refunded }   // :40

pub mod reason {                               // :52
    pub const TASK_VERIFIED: &str = "task_verified";
    pub const TASK_FAILED_REFUND: &str = "task_failed_refund";
    pub const TASK_UNVERIFIED: &str = "task_unverified";
}

pub fn charge_key(verification_id: &str) -> String   // :62 → format!("verify:{verification_id}")
pub fn refund_key(verification_id: &str) -> String   // :68 → format!("refund:{verification_id}")

pub fn billing_effect(                         // :95
    verdict: Verdict,
    state: BillingState,
    verification_id: &str,
) -> BillingEffect

pub fn ledger_reason(verdict: Verdict) -> Option<&'static str>   // :120
```

**Inputs:** no traits, no DB handle, no structs beyond its own enums. It needs exactly three scalars: a `Verdict`, a `BillingState` the caller must derive from ledger state, and a `verification_id` string. The `verification_id` is the primary key of the not-yet-existing `verification_runs` row — which is precisely why V4 cannot be called before V3.

**Transition table** (`:100-116`): `Refunded` is terminal for every verdict; `Inconclusive` is inert in every state; `Verified|Unverified × Unbilled` → `Charge`; `Verified|Unverified × Charged` → `None`; `Failed × Charged` → `Refund`; `Failed × Unbilled` → `None`.

Module doc at `:5` states the blocker in-band: *"Storing verdicts needs migration v61 and is blocked behind the shared counter."*

---

## 3. V5 core — `crates/engine/src/verifier.rs`

`compute_verdict` itself lives in **`crates/core/src/verification.rs:206`**, not in engine. Engine wraps it.

```rust
// crates/core/src/verification.rs:206
pub fn compute_verdict(specs: &[CheckSpec], executions: &[CheckExecution]) -> VerdictReport
```

**Consumes:** `&[CheckSpec]` (`:40`) and `&[CheckExecution]` (`:89`) — plain slices, no DB, no clock, no async.
**Returns:** `VerdictReport { verdict, required_total, required_passed, failed: Vec<String>, not_executed: Vec<String> }` (`:169`).

Semantics that constrain the wiring:
- Filters to `spec.required` only; empty required set → `Verdict::Unverified` (`:209-217`).
- For each required spec it folds **every** matching execution through `worst_of` (`:195`), not `find` — Failed/TimedOut(2) > NotExecuted(1) > Passed(0). This is the V7 fix (#479); duplicate rows cannot be laundered by ordering.
- A required spec with **no** execution row is `not_executed`, never a pass (`:248`, test at `:343`).
- Precedence: any `failed` → `Failed`; else any `not_executed` → `Inconclusive`; else `Verified` (`:252-258`).
- `Verdict::has_billing_effect()` at `:161` — false only for `Inconclusive`.

**Engine wrapper** — `crates/engine/src/verifier.rs:952`:

```rust
pub fn verify_from_executions(
    contract: &VerifierWorkContract,
    worker_evidence: &StructuredStepEvidence,
    specs: &[CheckSpec],
    executions: &[CheckExecution],
) -> ExecutedVerification
```

Returns `ExecutedVerification { verdict: VerifierVerdict, next_action: VerifierNextAction, gate: VerdictReport, worker_hints: VerifierReport }` (`:921`). Mapping at `:960-968`: `Verified→Success/Accept`, `Failed→Failed/FixAndRetry`, `Inconclusive→Blocked/FixAndRetry`, `Unverified→NeedsEvidence/AddEvidence`. The worker's own account is computed via `verify_contract_evidence` and stored as `worker_hints` — structurally incapable of changing `verdict`. **Zero callers.**

The legacy path still in production is `verify_step` (`verifier.rs:240`) / `verify_contract_evidence` (`:244`), which grade worker-reported evidence.

---

## 4. V1/V2 — runner trait, derivation, and the scheduler seam

### CheckRunner trait — `crates/core/src/verification.rs:126`

```rust
pub trait CheckRunner: Send + Sync {
    fn run(
        &self,
        tree: &TreeSnapshot,
        check: &CheckSpec,
    ) -> impl Future<Output = Result<CheckExecution, RunnerError>> + Send;

    fn runner_image(&self) -> &str;
}
```

RPITIT (not `async_trait`), so it is **not object-safe** — `dyn CheckRunner` will not compile. Callers must be generic over `R: CheckRunner` or use a concrete type.

`Err` is reserved strictly for infra failure and is what `Inconclusive` is made of (`RunnerError`, `:108`). A check that runs and fails is `Ok(CheckOutcome::Failed)`.

### Container implementation — `crates/api/src/check_runner.rs`

`pub struct ContainerCheckRunner { docker: Docker, image: String }` (`:51`), constructed via `ContainerCheckRunner::new(image: impl Into<String>) -> Result<Self, String>` (`:59`). `impl CheckRunner for ContainerCheckRunner` at `:155`.

Hardening (`container_config`, `:68-97`): `network_disabled`, `network_mode: "none"`, `env: Some(Vec::new())` (empty, not filtered), read-only bind `{path}:{/work}:ro`, `readonly_rootfs`, `cap_drop: ALL`, `no-new-privileges`, 2 GiB memory, 1 CPU, `pids_limit: 512` (`:44-49`). Timeout handled at `:202-212` → `CheckOutcome::TimedOut` (a verdict about the work, with output collected before teardown). `wait` errors → `RunnerError::ExecutionFailed` (`:219-232`).

Declared `pub mod check_runner;` at `crates/api/src/lib.rs:22`. **Never instantiated anywhere.**

### Check derivation — V2 is the one piece that IS wired

`crates/core/src/check_derivation.rs:233`:
```rust
pub fn derive_checks(input: &DerivationInput) -> Vec<CheckSpec>
pub fn is_unverified_by_construction(checks: &[CheckSpec]) -> bool   // :257
```
`DerivationInput { facts: EcosystemFacts, contract_commands: Vec<Vec<String>>, risk: RiskLevel, has_allowed_paths: bool }` (`:58`). Union of ecosystem floor + contract checks + risk overlays, deduped on argv keeping first (`:239-249`).

Edge probe: `pub fn probe_ecosystem(workspace_dir: &Path) -> EcosystemFacts` — `crates/api/src/ecosystem_probe.rs:28`.

**The scheduler already calls it** — `crates/api/src/scheduler.rs:637`:
```rust
fn infer_required_checks(
    kind: StepKind,
    risk: RiskLevel,
    allowed_paths: &[String],
    workspace_dir: &Path,
) -> Vec<RequiredCheck>
```
Guarded by `step_changes_the_tree(kind)` (`:614`) — Execute/Test/Build/Lint/Heal true; Search/Think/Review/Gate false.

**The lossy seam:** at `scheduler.rs:654-661` the `Vec<CheckSpec>` is immediately downgraded to `Vec<RequiredCheck>` (`crates/core/src/task.rs:44`), collapsing argv into a display string (`spec.command.join(" ")`) and **dropping `source` and `timeout_secs` entirely**. Doc comment at `:633-636` acknowledges this: *"`CheckSpec` carries argv; `RequiredCheck.command` is a display string."* From there the checks go into `build_work_recipe` (`:664`) as `AcceptanceVerification::RequiredCheck { check_name }` (`:693`) — i.e. they become *instructions handed to the worker*, never executed by Cortex. The `CheckSpec` values needed to drive `ContainerCheckRunner` are discarded at this line.

### Step state machine — `crates/engine/src/captain.rs`

```rust
pub enum StepStatus {                          // :51
    Pending, Ready, Leased, Running, Succeeded,
    Failed, Recovered, Cancelled, Orphaned, Skipped,
}
```
`is_terminal()` at `:64`; `as_str`/`from_str` round-trip to the DB strings. `pub enum RunStatus { Pending, Planning, Running, Succeeded, Failed, Cancelled }` at `:11`.

**There is no `Delivered` or `Verifying` state.** The machine goes `Leased → Running → Succeeded|Failed` with no seam for an asynchronous verification pass.

### Where a verification pass would be triggered — `crates/api/src/ws.rs`

This is the delivery hook. Worker completion arrives over the websocket; the sequence in the completion handler:

1. `ws.rs:426` — `let mut verified_success = exit_code == 0;` (the worker's own exit code is the initial truth)
2. `ws.rs:430-447` — `verify_worker_completion(db, run_id, &step_id, &attempt_id, lease_gen, worker_id, exit_code, &output, base_commit, head_commit, branch, &truncated_summary, &message_id)` → `(String, String, String, String, bool)`. Definition at `ws.rs:831-845`. It reads `output.evidence` — **worker-reported** check evidence (`:856-870`) — and calls the engine's contract-evidence verifier.
3. `ws.rs:449-452` — `verified_success = report_passed;`
4. `ws.rs:455-464` — `db.record_verifier_report(...)` with `verifier = "engine_verifier"`
5. `ws.rs:468-476` — on success, `db.complete_step(&step_id, lease_gen, ...)`, CAS'd on `lease_gen`; stale lease_gen returns false (`:477-487`)
6. `ws.rs:494+` — on failure, `db.record_failed_step_output(...)`

**The insertion point for a real verification pass is between (1) and (5)** — after `head_commit`/`branch` are known (a tree to snapshot exists) and before `complete_step` transitions the step. That is the only place in the codebase where a delivered tree and a `lease_gen` are both in scope.

Note `db.complete_step` is CAS'd on `lease_gen`, and `billing_binding`'s module doc (`:11-14`) calls out exactly this: a step that completes, charges, then gets rejected for a stale `lease_gen` will be re-run and must not be charged twice.

---

## 5. Ledger — `credit_transactions`

### On main
Table created in `migrate_v6` — `crates/api/src/db.rs:814-820`. **No `idempotency_key` column, no `reason` column, `amount REAL`:**
```sql
CREATE TABLE IF NOT EXISTS credit_transactions (
    id TEXT PRIMARY KEY,
    clerk_user_id TEXT NOT NULL,
    amount REAL NOT NULL,
    balance_type TEXT NOT NULL,
    description TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```
Writes on main are raw inserts inside `deduct_credits` at `db.rs:9751` and `db.rs:9764` with no idempotency at all.

There is a **separate, unrelated** `idempotency_keys` table (`db.rs:737`, accessors at `8027`/`8039`/`8048`) — a generic HTTP request-replay cache with TTL expiry. Not the ledger mechanism.

### On PR #437 — branch `fix/credit-metering-idempotency` (OPEN, unmerged)

`migrate_v60` rebuilds both tables: `REAL → INTEGER` whole credits, and adds `reason`, `idempotency_key TEXT NOT NULL UNIQUE`, `run_id`, `step_id`:

```sql
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
```
Legacy rows backfill as `'legacy:' || id`. Also adds a separate `provider_spend` table for token-denominated COGS, deliberately kept out of `credit_transactions`.

**The charge function** (there is no refund function on either branch — only a deduction):
```rust
pub fn deduct_credits(
    &self,
    clerk_user_id: &str,
    amount: i64,
    description: &str,
    idempotency_key: &str,
) -> Result<CreditBalanceRecord, String>
```
Doc comment specifies `run_id:step_id:attempt_id` as the intended key format. Because one deduction can span both buckets and two rows cannot share one UNIQUE key, it derives `{key}:subscription` and `{key}:pack` per bucket. Rejects `amount < 0` and empty keys. `BEGIN IMMEDIATE`, explicit replay check, insert with `reason = 'spend'`.

Also on that branch: `pub fn credit_ledger_totals(&self, clerk_user_id: &str) -> (i64, i64)` — sums the append-only log per bucket for reconciliation against the cached balance columns.

**Impedance mismatch to note for the spec:** `billing_binding::charge_key` produces `verify:{verification_id}` and `refund_key` produces `refund:{verification_id}`, while `deduct_credits` documents `run_id:step_id:attempt_id` and appends its own `:subscription`/`:pack` suffixes. And `BillingEffect::Refund` has no corresponding function — nothing in the workspace credits a balance back for a failed verdict.

---

## 6. Run/step API surface

Routers are built in **`crates/api/src/lib.rs`** (the only file with `.route(` calls). Two near-duplicate router blocks: `:398-404` and `:820-829`.

| Route | Handler | Line |
|---|---|---|
| `GET/POST /api/runs` | `routes::list_runs` / `create_run` | `lib.rs:398`, `:820` |
| `POST /api/runs/estimate` | `routes::estimate_run` | `lib.rs:399`, `:821` |
| `GET /api/runs/{id}` | `routes::get_run` | `lib.rs:400`, `:822` |
| `GET /api/runs/{id}/events` | `routes::get_run_events` | `lib.rs:401`, `:823` |
| `GET /api/runs/{run_id}/steps/{step_id}/verifier-report/{report_id}` | `routes::get_verifier_report` | `lib.rs:402`, `:825-827` |
| `POST /api/runs/{id}/pr` | `routes::create_pr` | `lib.rs:403`, `:828` |
| `GET /api/runs/{id}/stream` | `run_stream::stream_run` | `lib.rs:404`, `:829` |

**The receipt endpoint does not exist.** The closest thing, `routes::get_verifier_report` (`crates/api/src/routes.rs:740`), serves the **legacy worker-evidence** report, not a verdict + check tails:

```rust
pub struct VerifierReport {          // crates/api/src/db.rs:72
    pub id: String,
    pub step_id: String,
    pub run_id: String,
    pub lease_gen: i64,
    pub worker_id: Option<String>,
    pub verifier: String,
    pub status: String,        // stringly-typed
    pub verdict: String,       // stringly-typed: "pass" | "fail" | "blocked"
    pub evidence_json: String,
    pub created_at: i64,
    pub updated_at: i64,
}
```
`is_verified_success()` at `db.rs:87` → `status == "verified" && verdict == "pass"`. This is an entirely different type from `cortex_core::verification::VerdictReport` despite the name collision with `engine::verifier::VerifierReport`.

Persistence for the legacy path: `verifier_reports` table created in `migrate_v11` (`db.rs:1053-1077`), index added at `:1133`. Accessors: `record_verifier_report` (`db.rs:8586`, 8-arg stringly-typed), `get_latest_verifier_report` (`db.rs:8699`), `get_verifier_report_for_run_step` (`db.rs:8728`, ownership-scoped by `user_id`). Step status mapping at `db.rs:8626-8633` maps `(status, verdict)` → `verified_pass | verified_fail | verified_blocked | needs_evidence | verification_error | unverified`.

Storage trait indirection: `crates/api/src/storage.rs:102` (trait method), `:261`/`:272` (impl delegating to `Database::record_verifier_report`).

**Frontend (V6) is built and waiting.** `cortex/src/components/mission/Receipt.tsx` declares TS interfaces that mirror `cortex_core::verification` field-for-field — `CheckExecution` (`:19`), `VerdictReport` (`:29`), and the wrapper the API must serve:
```ts
export interface Receipt {          // Receipt.tsx:37
  verification_id: string;
  run_id: string;
  step_id: string;
  attempt: number;
  tree_hash: string;
  gate: VerdictReport;
  executions: CheckExecution[];
}
```
That shape is exactly `verification_runs` ⋈ `verification_checks` per the planned v61 schema.

It is **not rendered**. The route `/receipts` (`cortex/src/App.tsx:1084`) mounts `ReceiptsPane` from `cortex/src/components/mission/panes.tsx:22`, which is a `PaneStub` whose `blockedOn` prop reads verbatim: `"V3 — persisting verdicts (migration v61)"` (`panes.tsx:24`). Nav entry at `MissionControl.tsx:21`.

---

## 7. Complete reference list — `verification_runs` / `verification_checks` / `VerificationRun` / `CheckExecution`

Workspace-wide, excluding `target/`. Counts are match counts per file.

**Rust (4 files):**
- `crates/core/src/verification.rs` — 10 (the `CheckExecution` definition at `:89` plus test constructors)
- `crates/engine/src/verifier.rs` — 4 (import `:915`, signature `:956`, test helper `:1005-1006`)
- `crates/api/src/check_runner.rs` — 4 (import `:33`, return types `:142-143`, `:160`)

**TypeScript (1 file):**
- `cortex/src/components/mission/Receipt.tsx` — 3 (`:19` interface, `:44` field, `:82` param)

**Docs (3 files):**
- `cortex/plan/VERIFIER.md` — 6 (schema `:216`, `:229`, `:231`; build plan `:298`)
- `cortex/plan/SURFACE.md` — 1 (`:29`)
- `cortex/plan/HANDOFF-OPUS5.md` — 1 (`:128`)

**`VerificationRun` as a Rust type does not exist anywhere.** `verification_runs` and `verification_checks` appear **only** in `cortex/plan/*.md` — zero occurrences in any `.rs` or `.sql` file. There is no migration, no struct, no accessor.

A parallel set of identical hits exists under `.claude/worktrees/agent-a0e082feb10ffb3d9/` — that is a Claude Code agent worktree checkout, not workspace source.
