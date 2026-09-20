# Cortex Architecture — Design Specification

> Finalized via dual-brain (Claude Opus + GPT 5.5) architectural review.
> Pressure-tested via adversarial GPT 5.5 review — all CRITICAL/HIGH findings resolved.
> This document is the source of truth for the Rust implementation.

---

## ⚠️ Amendment 2026-08-02 — read before relying on any section below

This document was written for a model where **the user supplies provider access**.
Cortex is moving to **operator-funded keys sold as credits**, which inverts the
core principle. Most of this spec survives that change; some of it is now wrong,
and two parts were refuted by reading the code.

Sections are marked below. Where a section says REFUTED, the spec describes
something the implementation does not do — trust the note, not the section.

| § | Subject | Status under operator-funded keys |
|---|---|---|
| 1 | State layer / tables | **Keep.** Add `credit_ledger` + `provider_spend` per [CREDITS.md](../cortex/plan/CREDITS.md). |
| 2 | Deterministic intent resolution | **Keep**, with a caveat — see "sentence splitting" below. |
| 3 | Evaluator + policy split | **Keep the shape.** Constants need re-deriving against measured cost. |
| 4 | Failure taxonomy | **Revise.** `CliNotFound` / `CliNotAuthenticated` / `CliAuthExpired` are user-machine failures. Operator-funded execution fails with HTTP status codes, provider error bodies, and 429s. The *scope* model (Provider / Worker / Task / Workspace) survives; the *kinds* do not. |
| 5 | Ship Captain / DAG / leases | **Keep — and it is real.** Kahn cycle detection, round-robin fairness, `max_concurrent: 5`, per-step `tokio::spawn` all verified present. |
| 6 | Profiles | **Keep**, re-derive budget numbers once cost is measured. |
| 7 | Wire protocol v2 | **Mostly keep.** `ProviderClaim { provider, cli_version }` becomes meaningless when the operator holds the keys — capability is central, not per-worker. |
| 8 | Test strategy | **Keep.** Note all three e2e tests on the live path are `#[ignore]`, and the routing integration test asserts `OK \|\| BAD_REQUEST`, which cannot fail. |
| 9 | Workspace isolation | **Keep, and it becomes load-bearing.** Worktrees move from the user's machine onto Cortex infrastructure. That is a new cost centre and a new security boundary — untrusted LLM-authored code running next to operator API keys. See the sandbox decision in PLAN-2026-08.md §6. |
| 10 | Scheduler | **Keep.** |
| 11 | Scoring constants | **Keep — implemented, with two bugs.** `crates/core/src/evaluator.rs:399-518` implements this faithfully and is live via `scheduler.rs:962`. But `evaluator.rs:339` returns `+18` where this spec says `-18` (sign flip, biases toward OpenAI), and the per-tier token budgets are 4× off spec. |
| 12 | Step context selection | **Keep.** |
| 13 | SQLite performance | **REFUTED.** This section specifies a single write actor plus an r2d2 read pool via `spawn_blocking`. The code has a bare `Mutex<Connection>` with 376 blocking lock sites, no pool and no actor. Two replicas would double-dispatch — pay twice, bill once. Either implement §13 or move to Postgres; do not ship metered billing on what exists. |
| 14 | Security model | **Revise.** "Workers own user-provider access" and "User's CLI tools are untrusted" both assume the user's credentials. Under operator funding the trust boundary moves: the *worker* becomes the untrusted party executing model-authored code, and the keys never leave Brain. |
| 15 | Admin API | **Keep.** |
| 16 | Monitoring alerts | **Keep.** Add per-tenant spend-rate and credit-drawdown alerts. |
| 17 | Deferred list | **Stale.** "x402 billing — log usage events, wire later" is now the product. |

**Two implementation facts this spec does not capture:**

1. ~~**There are two routers.**~~ **DONE 2026-09-04 — there is one.**
   `crates/engine/src/scorer.rs` and `router.rs` are deleted, with
   `/api/route`, `/api/execute` and their handlers. They were dead exactly as
   described: `state.providers` is initialised empty and only ever read, so
   `score_providers` returned `None` and both endpoints answered 400
   unconditionally. Nothing called them — no frontend, no script, no other
   crate. The live path is §11's evaluator, and it is now the only one.

   `classify_intent`, `classify_risk` and `resolve_model` stayed; they are used
   by the live path. `effective_tier` was private to the deleted router and
   went with it.

   **Still true, and not the same thing:** `state.providers` itself remains
   inert. It is initialised empty, never written, and still read by
   `chat.rs`'s suggestions and by the mission-control payload — so both have
   always reported "no providers" and always will. That is a live surface with
   a dead input, which is a different repair from deleting a dead one, and it
   is not done here.
2. **§2's decomposition is thinner than it reads.** The DAG is built by splitting
   the user's sentence on `" and then "` / `" also "`, capped at 5 segments.
   "Fix the login bug" produces one node. The orchestration machinery is real;
   the thing feeding it is a string split.

### Amended core principle

**Brain owns decisions, state, policy, recovery — and provider access.**
**Workers own sandboxed execution of model-authored code, and nothing else.**

---

## Core Principle (original — superseded, retained for diff context)

**Brain owns decisions, state, policy, and recovery. Workers own local execution and user-provider access.**

---

## 1. State Layer — SQLite

One database. WAL mode. All operational state lives here.

### Tables

```sql
-- User & auth
CREATE TABLE users (
    id          TEXT PRIMARY KEY,  -- Clerk user ID or "local"
    email       TEXT,
    created_at  INTEGER NOT NULL,  -- epoch ms
    updated_at  INTEGER NOT NULL
);

-- Conversations (chat UI)
CREATE TABLE conversations (
    id          TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL,
    title       TEXT NOT NULL DEFAULT 'New Chat',
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE messages (
    id              TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role            TEXT NOT NULL,  -- user, assistant, system
    content         TEXT NOT NULL,
    provider        TEXT,
    model           TEXT,
    created_at      INTEGER NOT NULL
);

-- Workers and sessions (Brain-minted identities)
CREATE TABLE workers (
    id              TEXT PRIMARY KEY,  -- Brain-minted, not self-asserted
    user_id         TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'connected',  -- connected, grace, lost
    created_at      INTEGER NOT NULL,
    last_seen       INTEGER NOT NULL
);

CREATE TABLE worker_sessions (
    id              TEXT PRIMARY KEY,
    worker_id       TEXT NOT NULL REFERENCES workers(id),
    connected_at    INTEGER NOT NULL,
    disconnected_at INTEGER,
    last_heartbeat  INTEGER NOT NULL,
    grace_deadline  INTEGER  -- set on disconnect, cleared on reconnect
);

-- Provider capabilities (claims + verification)
CREATE TABLE provider_capabilities (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    worker_id       TEXT NOT NULL REFERENCES workers(id),
    user_id         TEXT NOT NULL,
    provider        TEXT NOT NULL,  -- claude, openai, gemini
    cli_version     TEXT,
    status          TEXT NOT NULL DEFAULT 'claimed',  -- claimed, verified, degraded, cooling_down, unavailable
    last_reported   INTEGER NOT NULL,
    last_verified   INTEGER,
    last_success    INTEGER,
    last_failure    INTEGER,
    failure_streak  INTEGER NOT NULL DEFAULT 0,
    UNIQUE(worker_id, provider)
);

-- User profiles (persisted, not ephemeral)
CREATE TABLE user_profiles (
    user_id             TEXT PRIMARY KEY,
    active_profile      TEXT NOT NULL DEFAULT 'auto',
    auto_mode           TEXT NOT NULL DEFAULT 'normal',  -- normal, protect_budget, protect_quality, recovering
    auto_mode_since     INTEGER,
    custom_overrides     TEXT,  -- JSON
    updated_at          INTEGER NOT NULL
);

-- Artifacts (step outputs, logs, patches)
CREATE TABLE artifacts (
    id              TEXT PRIMARY KEY,
    step_id         TEXT NOT NULL REFERENCES steps(id),
    attempt_number  INTEGER NOT NULL,
    kind            TEXT NOT NULL,  -- log, patch, test_report, file_list, json
    uri             TEXT NOT NULL,
    sha256          TEXT,
    size_bytes      INTEGER,
    created_at      INTEGER NOT NULL
);

-- Idempotency (message deduplication)
CREATE TABLE idempotency_keys (
    key             TEXT PRIMARY KEY,
    result_json     TEXT,
    created_at      INTEGER NOT NULL,
    expires_at      INTEGER NOT NULL
);

-- Decision ledger (routing decisions + outcomes)
CREATE TABLE decisions (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL,
    run_id          TEXT,
    step_id         TEXT,
    timestamp       INTEGER NOT NULL,
    intent          TEXT NOT NULL,
    risk            TEXT NOT NULL,
    tier            TEXT NOT NULL,
    provider        TEXT NOT NULL,
    model           TEXT NOT NULL,
    worker_id       TEXT,
    rationale       TEXT NOT NULL,
    profile         TEXT NOT NULL DEFAULT 'auto'
);

CREATE TABLE outcomes (
    id              TEXT PRIMARY KEY,
    decision_id     TEXT NOT NULL REFERENCES decisions(id),
    timestamp       INTEGER NOT NULL,
    success         INTEGER NOT NULL,  -- 0 or 1
    duration_ms     INTEGER,
    failure_kind    TEXT,              -- TaskFailureKind enum
    failure_scope   TEXT,              -- FailureScope enum
    files_changed   TEXT,             -- JSON array
    exit_code       INTEGER
);

-- Usage tracking (for pressure calculation)
CREATE TABLE usage_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id     TEXT NOT NULL,
    timestamp   INTEGER NOT NULL,
    provider    TEXT NOT NULL,
    tier        TEXT NOT NULL,
    model       TEXT NOT NULL,
    worker_id   TEXT,
    tokens_in   INTEGER,
    tokens_out  INTEGER,
    duration_ms INTEGER
);

CREATE INDEX idx_usage_window ON usage_events(provider, timestamp);

-- Task orchestration (Ship Captain)
CREATE TABLE runs (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL,
    goal            TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'pending',
    -- pending, planning, running, succeeded, failed, cancelled
    profile         TEXT NOT NULL DEFAULT 'auto',
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    started_at      INTEGER,
    finished_at     INTEGER,
    failure_reason  TEXT,
    heal_attempts   INTEGER NOT NULL DEFAULT 0,
    version         INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE steps (
    id              TEXT PRIMARY KEY,
    run_id          TEXT NOT NULL REFERENCES runs(id),
    kind            TEXT NOT NULL,
    -- search, execute, think, test, build, lint, heal, review, gate
    status          TEXT NOT NULL DEFAULT 'pending',
    -- pending, ready, leased, running, succeeded, failed, cancelled, orphaned, skipped
    tier            TEXT NOT NULL,
    risk            TEXT NOT NULL,
    objective       TEXT NOT NULL,
    required_provider TEXT,         -- NULL = any, else specific provider required
    required_repo   TEXT,           -- NULL = any, else specific repo URL
    input_context   TEXT,           -- JSON: compact context from predecessors
    output_summary  TEXT,
    files_changed   TEXT,           -- JSON array
    base_commit     TEXT,           -- git commit step started from
    head_commit     TEXT,           -- git commit step ended at
    attempt_count   INTEGER NOT NULL DEFAULT 0,
    max_attempts    INTEGER NOT NULL DEFAULT 3,
    lease_gen       INTEGER NOT NULL DEFAULT 0,
    lease_deadline  INTEGER,
    assigned_worker TEXT REFERENCES workers(id),
    last_error      TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    version         INTEGER NOT NULL DEFAULT 0
);

-- Normalized step dependencies (replaces JSON depends_on)
CREATE TABLE step_dependencies (
    step_id         TEXT NOT NULL REFERENCES steps(id),
    depends_on_id   TEXT NOT NULL REFERENCES steps(id),
    edge_type       TEXT NOT NULL DEFAULT 'success_required',
    -- success_required: successor runs only if dependency succeeded
    -- completion_required: successor runs when dependency is terminal (succeeded OR failed)
    --   Used by heal steps that need to run AFTER a test fails
    PRIMARY KEY (step_id, depends_on_id)
);

CREATE TABLE step_attempts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    step_id         TEXT NOT NULL REFERENCES steps(id),
    run_id          TEXT NOT NULL,
    attempt_number  INTEGER NOT NULL,
    worker_id       TEXT,
    lease_gen       INTEGER NOT NULL,
    status          TEXT NOT NULL,  -- started, succeeded, failed, orphaned
    provider        TEXT,
    model           TEXT,
    started_at      INTEGER NOT NULL,
    finished_at     INTEGER,
    failure_kind    TEXT,
    error_summary   TEXT
);

-- Score evidence (for explainability and replay)
CREATE TABLE score_evidence (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    decision_id     TEXT NOT NULL REFERENCES decisions(id),
    evaluator       TEXT NOT NULL,  -- intent, risk, budget, provider_fit, quality
    evidence_json   TEXT NOT NULL,  -- full typed evidence as JSON
    score           REAL,
    timestamp       INTEGER NOT NULL
);
```

### Key Query Patterns

```sql
-- Pressure calculation (rolling 5-hour window)
SELECT provider, tier, SUM(tokens_in + tokens_out) as total_tokens
FROM usage_events
WHERE timestamp > ? AND user_id = ?
GROUP BY provider, tier;

-- Provider reliability (last 24h)
SELECT provider, 
    COUNT(*) as total,
    SUM(success) as successes,
    AVG(duration_ms) as avg_duration
FROM outcomes o
JOIN decisions d ON o.decision_id = d.id
WHERE o.timestamp > ? AND d.user_id = ?
GROUP BY provider;

-- Ready steps (all dependencies satisfied per edge type)
SELECT s.* FROM steps s
WHERE s.run_id = ? AND s.status = 'pending'
AND NOT EXISTS (
    SELECT 1 FROM step_dependencies sd
    JOIN steps dep ON dep.id = sd.depends_on_id
    WHERE sd.step_id = s.id
    AND (
        (sd.edge_type = 'success_required' AND dep.status != 'succeeded')
        OR (sd.edge_type = 'completion_required' AND dep.status NOT IN ('succeeded', 'failed'))
    )
);

-- Expire stale leases
UPDATE steps SET status = 'orphaned', version = version + 1
WHERE status = 'leased' AND lease_deadline < ?;

-- CAS step transition
UPDATE steps SET status = ?, version = version + 1, updated_at = ?
WHERE id = ? AND status = ? AND version = ?;
```

---

## 2. Intent Resolution — Deterministic First

Layered resolution, no LLM for parsing:

```
Layer 1: Explicit commands         ("/fix", "/explore", "/review")
Layer 2: Pattern match             (regex on keywords — 90% of cases)
Layer 3: Compound decomposition    ("fix X and test Y" → 2 steps)
Layer 4: Clarification             (ambiguous → ask user)
Layer 5: Template response         (format results without LLM)
```

### Intent Enum

```rust
pub enum Intent {
    Fix,        // fix, repair, patch, resolve, debug
    Add,        // add, create, implement, build, write
    Explore,    // explore, find, search, grep, where, what
    Review,     // review, audit, check, inspect
    Think,      // should we, how should, architecture, design
    Test,       // test, verify, validate, check
    Refactor,   // refactor, clean, simplify, restructure
    Ship,       // ship, deploy, release, publish, PR
}
```

### Compound Decomposition

Split on sentence-level boundaries: `and|also|then|after that|plus`

Rules:
- Never split inside quoted strings or code blocks
- Comma splitting only at clause boundaries, not mid-phrase
- If splitting produces > 5 segments → ask user to clarify instead of blindly decomposing

Each segment gets independent intent + risk classification → becomes a Step in the run DAG.

Ordered language (`then|after|first|before`) → sequential dependencies (`success_required` edges).
No ordering cues + independent subsystems → parallel (no dependencies).

---

## 3. Scoring — Evaluator + Policy Judge

### Architecture

```rust
#[async_trait]
pub trait Evaluator: Send + Sync {
    type Evidence: Serialize + Send + Sync;
    async fn evaluate(&self, ctx: &EvalContext) -> Result<Self::Evidence>;
}

pub struct DecisionEvidence {
    pub intent: IntentEvidence,
    pub risk: RiskEvidence,
    pub budget: BudgetEvidence,
    pub provider_fit: ProviderFitEvidence,
}

pub trait RoutingPolicy: Send + Sync {
    fn decide(&self, evidence: &DecisionEvidence, profile: &Profile) -> RoutingDecision;
}
```

### Evaluators

**IntentEvaluator** — pattern-match keywords → Intent + confidence.

**RiskEvaluator** — three layers:
1. Static keyword patterns (fast, 90%)
2. File path patterns (auth/, billing/, etc.)
3. Historical success rate from `outcomes` table

```rust
pub struct RiskEvidence {
    pub level: RiskLevel,
    pub basis: RiskBasis,  // static, file_path, historical, combined
    pub static_level: RiskLevel,
    pub file_risk: Option<RiskLevel>,
    pub history_success_rate: Option<f64>,
}
```

**BudgetEvaluator** — rolling window pressure per provider/tier:

```rust
pub struct BudgetEvidence {
    pub pressures: HashMap<(ProviderId, Tier), PressureState>,
}

pub enum PressureState {
    Healthy(f64),     // 0-65%
    Warm(f64),        // 65-82%
    Hot(f64),         // 82-95%
    Throttled(f64),   // 95%+
}
```

**ProviderFitEvaluator** — capability match, isolation, coupling:

```rust
pub struct ProviderFitEvidence {
    pub candidates: Vec<CandidateScore>,
}

pub struct CandidateScore {
    pub provider: ProviderId,
    pub worker_id: String,
    pub capability_status: ProviderStatus,
    pub pressure_penalty: f64,
    pub fit_bonus: f64,         // coupling/isolation match
    pub latency_penalty: f64,   // startup overhead for short tasks
    pub reliability: f64,       // from outcome history
    pub total: f64,
}
```

### Policy Function

The policy is profile-aware. It takes all evidence and applies profile-specific weights:

```rust
impl RoutingPolicy for DefaultPolicy {
    fn decide(&self, evidence: &DecisionEvidence, profile: &Profile) -> RoutingDecision {
        // 1. Filter: remove providers that can't serve this tier/risk
        // 2. Score: weight evidence by profile
        // 3. Select: highest scoring candidate
        // 4. Explain: build rationale from evidence
    }
}
```

Both evidence AND decision are persisted to `decisions` + `score_evidence` tables.

---

## 4. Failure Taxonomy

### Two-part model: Worker reports evidence, Brain classifies

Worker sends structured failure:

```rust
pub struct WorkerFailureReport {
    pub task_id: Uuid,
    pub kind: WorkerFailureKind,
    pub exit_code: Option<i32>,
    pub stderr_excerpt: Option<String>,  // last 500 chars
    pub tool: Option<String>,            // claude, codex, gemini
}

pub enum WorkerFailureKind {
    CliNotFound,
    CliNotAuthenticated,
    CliAuthExpired,
    CliRateLimited,
    CliModelUnavailable,
    CliCrashed,
    ProcessTimeout,
    ProcessKilled,
    NetworkError,
    PermissionDenied,
    OutputEmpty,
    Unknown,
}
```

Brain enriches with its own observations:

```rust
pub enum TaskFailureKind {
    // Provider scope — affects provider scoring
    ProviderAuthExpired,
    ProviderQuotaExceeded,
    ProviderRateLimited,
    ProviderModelUnavailable,
    ProviderServiceDown,
    ProviderSafetyRefusal,

    // Worker scope — affects worker reliability
    CliNotInstalled,
    CliNotAuthenticated,
    WorkerCrashed,
    WorkerDisconnected,
    WorkerResourceExhausted,

    // Task scope — no provider/worker penalty
    TaskTimeout,
    TaskCancelled,
    PermissionDenied,
    TestsFailed,
    BuildFailed,
    QualityFailure,
    OutputEmpty,
    OutputInvalid,

    // Workspace scope
    GitConflict,
    GitPushRejected,
    WorkspaceDirty,
    ConflictingEdits,

    Unknown,
}

pub enum FailureScope {
    Provider,
    ProviderAccount,
    ProviderModel,
    Worker,
    WorkerTooling,
    Task,
    Workspace,
    User,
    Unknown,
}
```

### Routing impact matrix

| Failure Kind | Scope | Retry Action | Scoring Impact |
|---|---|---|---|
| ProviderAuthExpired | ProviderAccount | Different provider; ask user to re-auth | Account cooldown until refresh |
| ProviderQuotaExceeded | ProviderAccount | Different provider | Account cooldown until reset |
| ProviderRateLimited | ProviderModel | Same provider after backoff | Temporary pressure increase |
| ProviderModelUnavailable | ProviderModel | Same provider, fallback model | Model penalty (permanent if deprecated) |
| ProviderServiceDown | Provider | Different provider | Temporary global cooldown |
| ProviderSafetyRefusal | Task | Ask user to revise | No penalty |
| CliNotInstalled | WorkerTooling | Different worker/provider | Worker-tooling penalty until fixed |
| CliNotAuthenticated | WorkerTooling | Ask user to auth | Worker-tooling penalty until fixed |
| WorkerCrashed | Worker | Different worker | Temporary worker cooldown |
| WorkerDisconnected | Worker | Wait for reconnect or reassign | Temporary worker cooldown |
| TaskTimeout | Task | Retry with larger model or decompose | No penalty normally; 3+ timeouts on same provider in 1hr → temporary provider cooldown |
| TestsFailed | Task | Heal step (up to 2x) | Quality signal, not reliability |
| BuildFailed | Task | Heal step (up to 2x) | Quality signal |
| GitConflict | Workspace | Ask user or rebase | No penalty |

---

## 5. Ship Captain — Task Orchestration

### Run Lifecycle

```
pending → planning → running → succeeded
                         ↓
                       failed
                         ↓
                      cancelled
```

### Step Lifecycle

```
pending → ready → leased → running → succeeded
                     ↓         ↓
                  orphaned   failed
                     ↓         ↓
                   ready     [Brain may insert heal step with completion_required edge]
                (requeued)   [heal step runs, then retry test step depends on heal]
```

Steps that fail do NOT automatically retry. `failed` is terminal for that step.
Retries happen via Brain inserting new steps into the DAG.
`orphaned` steps (from lease expiry) DO requeue to `ready` if retries remain.

### Step Model

Steps form a DAG via `step_dependencies` table. Two edge types:
- `success_required` — step becomes ready only when dependency `succeeded`
- `completion_required` — step becomes ready when dependency is terminal (`succeeded` OR `failed`)

The `completion_required` edge enables heal workflows: a heal step depends on a failed test step being finished, not succeeded. Brain checks readiness after every step state change.

### Dynamic Heal Insertion

When a test/build step fails:
1. Brain checks `run.heal_attempts < max_heal_attempts` (default: 2)
2. Inserts a new `heal` step that depends on the failed step
3. Inserts a retry of the original test step that depends on the heal step
4. If same failure signature repeats → stop healing, fail the run

```
edit → test(failed) → heal₁ → test_retry₁(failed) → heal₂ → test_retry₂
```

### Lease Model

- Brain assigns `(step_id, lease_gen, deadline)` to a Worker
- Deadline scales by step kind: search=2min, execute=10min, think=15min
- Worker sends heartbeats to extend lease
- If heartbeat stops → Brain waits until deadline → marks `orphaned` → requeues
- Late results with stale `lease_gen` are rejected

### Multi-Worker Selection

```rust
fn select_worker(step: &Step, workers: &[ConnectedWorker]) -> Option<WorkerId> {
    workers.iter()
        .filter(|w| w.has_provider(step.required_provider()))
        .filter(|w| w.capability_status(step.provider) != Unavailable)
        .filter(|w| step.required_repo.is_none() || w.has_repo(step.required_repo))
        .max_by_key(|w| worker_score(w, step))
}
```

Prefer: verified providers > claimed, low queue depth, matching repo.

### Step Output Chaining

Steps produce structured output:

```rust
pub struct StepOutput {
    pub summary: String,
    pub files_found: Vec<String>,
    pub files_changed: Vec<String>,
    pub structured: serde_json::Value,  // step-kind-specific data
}
```

Brain builds compact `StepContext` for successor steps from predecessor summaries + declared structured fields. Full transcripts stay in artifacts, not in context.

### Brain Restart Recovery

1. Load all runs where `status IN ('planning', 'running')`
2. Mark all worker connections as unknown (they'll re-register)
3. Expire leases where `deadline < now()`
4. Requeue orphaned steps (if retries remaining)
5. Recompute ready steps from DAG
6. Resume scheduler loop

All state transitions use CAS: `WHERE version = ? ... SET version = version + 1`

---

## 6. Profiles

Four profiles that configure the policy function:

| Profile | Risk Floor | Heal Attempts | Provider Bias | Budget Warn/Limit |
|---|---|---|---|---|
| auto | adaptive | 2 | balance by pressure | $5 / $10 |
| balanced | medium | 2 | even split | $5 / $10 |
| cost-saver | high | 1 | prefer cheaper | $2 / $5 |
| quality-first | low | 3 | prefer strongest | $15 / $30 |

Natural language aliases: "cheap" → cost-saver, "careful" → quality-first, "fast" → cost-saver, "thorough" → quality-first.

Profiles are persisted per-user in SQLite.

---

## 7. Wire Protocol v2

### Authentication

WebSocket connection is authenticated via the account JWT:
- Worker sends JWT as query param or first message on connect
- Brain verifies JWT, extracts `user_id` from claims
- Brain mints `worker_id` and `session_id` — these are NOT self-asserted
- `user_id` comes from JWT, never from Worker's Register message
- All subsequent messages are authenticated by the connection (TLS + session binding)

### Brain → Worker

```rust
pub enum BrainMessage {
    Welcome {
        session_id: String,
        worker_id: String,       // Brain-minted
        protocol_version: u32,   // for forward compatibility
    },
    ExecuteStep {
        run_id: String,
        step_id: String,
        attempt_id: String,      // Brain-minted, unique per attempt
        lease_gen: i64,
        lease_deadline_ms: i64,
        workspace_id: String,    // Brain-assigned workspace scope
        base_commit: Option<String>,
        allowed_paths: Vec<String>,  // workspace path restrictions
        task: TaskContract,
        decision: RoutingDecision,
        context: StepContext,
    },
    CancelStep { step_id: String, reason: String },
    StaleLeaseNotice {
        step_id: String,
        your_lease_gen: i64,
        current_lease_gen: i64,
        disposition: String,     // "abandon_and_cleanup" or "preserve_for_inspection"
    },
    Ping,
}
```

### Worker → Brain

```rust
pub enum WorkerMessage {
    Register {
        token: String,           // account JWT — Brain extracts user_id from this
        protocol_version: u32,
        providers: Vec<ProviderClaim>,
        workspace_dir: String,
        repos: Vec<RepoInfo>,
    },
    StepStarted {
        message_id: String,      // UUID, for deduplication
        step_id: String,
        attempt_id: String,
        lease_gen: i64,
        provider: String,
        model: String,
    },
    StepOutput {
        step_id: String,
        attempt_id: String,
        lease_gen: i64,
        line: String,
    },
    StepCompleted {
        message_id: String,
        step_id: String,
        attempt_id: String,
        lease_gen: i64,
        exit_code: i32,
        base_commit: Option<String>,
        head_commit: Option<String>,
        output: StepOutput,
    },
    StepFailed {
        message_id: String,
        step_id: String,
        attempt_id: String,
        lease_gen: i64,
        failure: WorkerFailureReport,
    },
    Heartbeat {
        active_steps: Vec<String>,
    },
    Pong,
}

pub struct ProviderClaim {
    pub provider: ProviderId,
    pub cli_version: Option<String>,
}

pub struct RepoInfo {
    pub path: String,
    pub remote_url: Option<String>,
    pub branch: Option<String>,
    pub head_commit: Option<String>,
}
```

### Key changes from v1:
- **Security**: worker_id/user_id are Brain-minted from JWT, not self-asserted
- **Idempotency**: `message_id` on state-changing messages, Brain deduplicates
- **Attempt tracking**: `attempt_id` on all step messages, Brain-minted per attempt
- **Workspace authority**: Brain assigns `workspace_id`, `base_commit`, `allowed_paths`
- **Stale lease handling**: explicit `StaleLeaseNotice` message
- **Forward compatibility**: `protocol_version` on Register and Welcome
- **Git provenance**: `base_commit` + `head_commit` on StepCompleted
- All step messages include `lease_gen` for stale rejection

---

## 8. Test Strategy

### Test Pyramid

```
Unit (70%)     — evaluators, state machine reducer, intent parser, risk classifier
Integration    — Brain + MockWorker via real WebSocket, real SQLite
(25%)            scenario-driven: success, retry, timeout, disconnect, heal
E2E (5%)       — full stack smoke test with mock CLI
```

### Mock Worker

Scenario-driven test Worker that simulates various failure modes:

```rust
pub struct MockWorkerConfig {
    pub brain_url: String,
    pub worker_id: String,
    pub scenario: Scenario,
}

pub enum ScenarioStep {
    Succeed { delay_ms: u64 },
    FailWith { kind: WorkerFailureKind, delay_ms: u64 },
    Timeout,
    Disconnect,
    SlowStream { lines: Vec<String>, delay_per_line_ms: u64 },
}
```

### Routing Replay

Record real routing decisions as fixtures. Replay against new scoring logic:

```rust
#[test]
fn replay_routing_ledger() {
    for entry in load_fixtures("tests/fixtures/routing/*.json") {
        let actual = policy.decide(&entry.evidence, &entry.profile);
        assert_properties(&actual, &entry.expected_properties);
    }
}
```

Semantic assertions over exact score matching:
- "critical risk never routes to search tier"
- "throttled provider never wins"
- "verified provider preferred over claimed"

### State Machine Invariants (proptest)

- Completed run has no pending/running steps
- Failed run has terminal failure reason
- Retry count never exceeds policy
- Terminal states are absorbing
- Every dispatched step has a worker assignment
- Stale lease completions are rejected

---

## 9. Workspace Isolation

### Problem

Worker A gets a step, starts editing files. Network blip → lease expires. Brain requeues to Worker B. Now both Workers may have modified the same files. Brain rejects Worker A's stale `lease_gen`, but the filesystem damage is already done.

### Solution: Git Worktree Per Attempt

Every step attempt runs in an isolated git worktree on a unique branch:

```
refs/cortex/attempts/{step_id}/{attempt_number}
```

- Worker creates worktree before execution, destroys after
- Workers never modify the main working tree directly
- Accepted results are merged by Brain-approved reconciliation (or by the Worker on Brain's instruction after acceptance)
- Stale Workers receive `StaleLeaseNotice` and must stop, not push, not merge, and clean up their worktree
- Workers never push directly to user branches — only to namespaced attempt refs

### Stale Lease Response

```rust
pub enum BrainMessage {
    // ... existing variants ...
    StaleLeaseNotice {
        step_id: String,
        your_lease_gen: i64,
        current_lease_gen: i64,
        disposition: StaleDisposition,
    },
}

pub enum StaleDisposition {
    AbandonAndCleanup,   // delete worktree, stop
    PreserveForInspection, // keep worktree but stop
}
```

### Same Machine vs Different Machine

- Same machine: worktrees provide filesystem isolation automatically
- Different machines: each has its own clone, no conflict possible at filesystem level
- Both cases: only the accepted attempt's changes get merged to the user's branch

### V1 Simplification

For v1 with single-Worker-per-user, worktree isolation is optional (collision is impossible). But the protocol should support it from day one so multi-Worker works when we need it.

---

## 10. Scheduler Architecture

### Hybrid Event-Driven + Reconciliation

Single global scheduler task. Events are hints; SQLite is truth.

```rust
enum SchedulerEvent {
    RunCreated { run_id: String },
    StepCompleted { run_id: String, step_id: String },
    StepFailed { run_id: String, step_id: String },
    WorkerConnected { worker_id: String },
    WorkerDisconnected { worker_id: String },
    Reconcile,
}
```

Scheduler loop:

```rust
loop {
    tokio::select! {
        Some(event) = rx.recv() => {
            state.apply_event(event).await;
            state.schedule_until_blocked().await;
        }
        _ = reconcile_interval.tick() => {
            state.reconcile_from_db().await;
            state.expire_due_leases().await;
            state.schedule_until_blocked().await;
        }
        _ = state.next_lease_deadline() => {
            state.expire_due_leases().await;
            state.schedule_until_blocked().await;
        }
    }
}
```

### Fairness

Round-robin across users with ready work. Per-user concurrency limits.

```rust
struct SchedulerState {
    user_queues: HashMap<UserId, UserQueue>,
    schedulable_users: VecDeque<UserId>,
    workers: WorkerPool,
    lease_deadlines: BinaryHeap<Reverse<LeaseDeadline>>,
}

struct UserQueue {
    ready_steps: VecDeque<StepRef>,
    running_count: usize,
    max_concurrent: usize,  // default: 5
}
```

User A has 10 runs, User B has 1 → B still gets assignments within one scheduling round.

### Backpressure

Only assign when: worker has capacity, user has concurrency budget, global budget exists. If all workers are busy, steps stay in `ready` state.

### Worker Disconnect Grace

```
connected → disconnected_grace (15-60s) → lost
```

On disconnect: stop assigning new work. Start grace timer. If Worker reconnects and proves lease ownership → continue. If grace expires → shorten affected lease deadlines → normal expiry path requeues.

### Brain Restart

1. Open SQLite, run migrations
2. Expire leases past deadline
3. Rebuild per-user counters from `status = leased/running`
4. Rebuild ready queues from dependency satisfaction
5. Start WebSocket acceptor + scheduler loop
6. Send initial `Reconcile` event

---

## 11. Scoring Algorithm — Exact Constants

### Profile Weights

```
              intent  risk  budget  provider_fit
balanced:      0.20   0.25   0.25     0.30
cost-saver:    0.10   0.25   0.45     0.20
quality-first: 0.15   0.35   0.10     0.40
auto(normal):  0.20   0.25   0.25     0.30
```

### Final Score Formula

```
score = 50
  + w.intent       × intent_score
  + w.risk         × risk_score
  + w.budget       × budget_score
  + w.provider_fit × provider_fit_score
```

### Subscore Components

**Intent score:**
- +20 if provider supports requested tier exactly
- +8 if provider can serve higher tier
- -30 if provider only serves lower tier
- Veto if unauthenticated or tier unavailable

**Risk score:**
- +20 if tier >= risk minimum tier
- -40 if tier below minimum
- Veto if critical risk and tier != think

**Budget score:**
- 30 - pressure_penalty + underused_bonus
- Pressure penalty: smooth curve from 0 (healthy) to 100 (throttled)
  - < 65%: 0
  - 65-82%: 5-20 (linear)
  - 82-95%: 20-60 (superlinear)
  - 95%+: 85-100

**Provider fit score:**
- Capability bonus: +22 exact tier, +10 higher tier, -35 lower tier
- Reliability bonus: (success_rate - 0.80) × 60, clamped [-15, +12], requires 20+ samples
- Latency penalty: -18 if OpenAI + task < 90s, -10 if < 180s
- Underused bonus: +12 if provider < 30% pressure and peers > 55%
- Risk alignment: +20 critical/think, +12 high/think, +8 high/execute
- Profile bias: cost-saver favors search (+8), quality-first favors think (+12)

### Tiebreaking (deterministic)

1. Higher recent success rate (last 50 outcomes)
2. Lower pressure in selected tier
3. Lower estimated startup latency
4. Least recently selected for this user/tier
5. Stable order: Claude > OpenAI > Gemini

### Pressure Algorithm

- Rolling 5-hour window
- Per-tier token budgets: Search 5M, Execute 2M, Think 500K per provider
- Token estimation when actual unavailable: Search 2.5K, Execute 5.5K, Think 11K
- Transitions: Healthy(<65%) → Warm(65-82%) → Hot(82-95%) → Throttled(95%+)

### Auto Profile State Machine

```
Normal
  → ProtectBudget    if any tier pressure ≥ 82%
  → ProtectQuality   if risk is High/Critical
  → Recovering       if ≥2 failures in last 5 attempts on selected provider

ProtectBudget → Normal   after all pressures < 65% for 30 minutes
ProtectQuality → Normal  after run completes with no pending High/Critical steps
Recovering → Normal      after 3 successful steps or 30 minutes without failure
```

### Profile Switch Behavior

- Running steps: keep original RoutingDecision
- Ready/queued steps: re-scored immediately
- Leased steps: only cancelled and reissued on safety upgrades (balanced→quality-first on high risk)
- Downgrades never interrupt in-flight work

---

## 12. Step Context Selection

### Core Rule

Direct dependencies get detailed projected slices. Non-adjacent ancestors contribute only durable run facts.

### Context Budget

- Target: 32K chars (~8-12K tokens)
- Hard max: 48K chars
- Heal steps: same budget but priority shifts to failure output

### Per-Transition Data Flow

| From → To | What Flows | What Doesn't |
|-----------|-----------|--------------|
| search → execute | File findings (ranked), symbols, snippets, search confidence | Raw grep output, terminal logs |
| explore → fix | Architecture explanation, constraints, likely fix locations, risk notes | Full codebase analysis |
| execute → test | Files changed, diff summary, commands run, suggested tests, risk | Full worker transcript, full patches |
| test → heal | Failing command, exit code, failure excerpts, failing test names, changed files, original objective, previous heal attempts | Full test suite output |
| heal → test | Heal summary, files fixed, attempt number, pinned test command | Heal reasoning transcript |
| gate → heal | Gate status, issue list, severity, changed files | Gate internal scoring details |
| test → gate | Test command, pass/fail, duration | If failed, gate usually shouldn't run |

### Run Fact Index

Every ancestor may contribute durable facts; only direct dependencies contribute detailed slices:

```rust
struct RunFactIndex {
    relevant_files: Vec<FileFinding>,
    changed_files: Vec<FileChange>,
    decisions: Vec<DecisionFact>,
    constraints: Vec<String>,
    commands_run: Vec<CommandRecord>,
}
```

### Truncation Priority

1. Original objective + current step instruction
2. Failure context (for heal steps)
3. Files changed by direct predecessor
4. Failing command/output excerpts
5. Direct predecessor summary
6. Relevant files from transitive facts
7. Commands run
8. Risk notes/constraints
9. Free-form extra

### Typed Step Outputs

Workers produce typed structured outputs per step kind (not free-form JSON):

```rust
pub struct TestOutput {
    pub passed: bool,
    pub command: Option<String>,
    pub exit_code: Option<i32>,
    pub failing_tests: Vec<String>,
    pub failure_excerpts: Vec<String>,
}

pub struct ExecuteOutput {
    pub files_changed: Vec<FileChange>,
    pub commands_run: Vec<CommandRecord>,
    pub implementation_summary: String,
    pub suggested_tests: Vec<String>,
}

pub struct HealOutput {
    pub healed: bool,
    pub attempts: u32,
    pub files_changed: Vec<FileChange>,
    pub failure_addressed: Option<String>,
}
```

---

## 13. SQLite Performance

### Production PRAGMAs

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
PRAGMA temp_store = MEMORY;
PRAGMA mmap_size = 268435456;  -- 256 MiB
PRAGMA wal_autocheckpoint = 1000;  -- ~4 MiB
```

### Connection Architecture

- 1 single DB write actor (ALL writes serialized via mpsc channel — eliminates contention)
- 4-8 read pool connections via `r2d2` (all reads via `spawn_blocking`)

The single write actor handles both correctness transactions and batched telemetry, serialized naturally. No concurrent writer contention possible.

### Write Batching

Only batch `usage_events` and non-critical telemetry. Correctness writes (step claims, outcomes, decisions) remain strongly transactional.

### Hot Path Indexes

```sql
-- Scheduler: find ready steps
CREATE INDEX idx_steps_status_ready ON steps(status, run_id) WHERE status = 'ready';

-- Lease expiry
CREATE INDEX idx_steps_lease ON steps(status, lease_deadline) WHERE status = 'leased';

-- Pressure calculation
CREATE INDEX idx_usage_window ON usage_events(provider, timestamp);

-- Provider reliability
CREATE INDEX idx_outcomes_provider ON outcomes(timestamp);
CREATE INDEX idx_decisions_user ON decisions(user_id, timestamp);
```

### Retention

- `usage_events`: 7-30 days raw, hourly rollups kept longer
- `step_attempts`: 30-90 days
- `decisions` + `outcomes`: long-term (audit trail)
- Delete in chunks (5000 rows per batch), use `PRAGMA incremental_vacuum`

### WAL Checkpoint

- Auto-checkpoint at 1000 pages (~4 MiB)
- Background `PRAGMA wal_checkpoint(PASSIVE)` every 30-60s
- Alert if WAL > 256 MiB
- `TRUNCATE` checkpoint only during maintenance windows

### Backup

- SQLite backup API every 5-15 minutes during production
- `PRAGMA quick_check` after backup
- Daily offsite retained snapshots
- Periodic restore drills

### Scaling Ceiling

SQLite is sufficient for thousands of users with disciplined writes. Migration trigger: sustained hundreds of writes/sec mixed with correctness transactions, or need for horizontal write scaling. Migration path: repository trait boundary → dual-write → shadow reads → cutover to Postgres.

---

## 14. Security Model

### Trust Boundaries

- **Brain is fully trusted** — owns all state, decisions, and policy
- **Worker is semi-trusted** — authenticated user, but evidence is validated
- **User's CLI tools are untrusted** — Worker reports what CLI says, Brain validates patterns

### Authentication

- WebSocket: JWT verification on connect, Brain mints worker_id/session_id
- HTTP API: Clerk JWT in Authorization header, verified per request
- Admin API: separate admin JWT role claim required

### Worker Evidence Validation

Brain does NOT blindly trust Worker failure reports:
- CLI error patterns validated against known provider error formats
- Repeated identical errors flagged for human review
- Score poisoning detection: if a worker's reports diverge significantly from other workers on same provider, flag anomaly

### Tenant Isolation

- user_id derived from JWT, never self-asserted
- All queries filter by user_id — no cross-user data access
- Workers can only receive tasks for their authenticated user
- Artifacts scoped to user_id + run_id

### Rate Limiting

Per-user limits enforced at API layer:
- `max_active_runs`: 10 (concurrent)
- `max_runs_per_hour`: 30
- `max_steps_per_hour`: 200
- `max_workers_per_user`: 5

Admission control: reject `RunCreated` if user exceeds limits.

---

## 15. Admin API

Operational controls — authenticated with admin JWT role:

```
GET  /admin/runs                          — list runs by status, user, date
GET  /admin/runs/{id}                     — full run detail with steps and attempts
POST /admin/runs/{id}/retry-step/{step_id} — manually retry a failed step
POST /admin/runs/{id}/cancel              — cancel a run and all in-flight steps

GET  /admin/workers                       — list connected workers
POST /admin/workers/{id}/drain            — stop assigning new work, wait for completion
POST /admin/workers/{id}/quarantine       — disconnect and block worker

POST /admin/providers/{id}/disable        — temporarily disable a provider globally
POST /admin/providers/{id}/enable         — re-enable a disabled provider

POST /admin/scheduler/pause               — pause scheduling (in-flight continues)
POST /admin/scheduler/resume              — resume scheduling

GET  /admin/metrics                       — scheduler queue depth, worker counts, etc.
```

---

## 16. Monitoring Alerts

Production alerts that should exist from day one:

| Alert | Condition | Severity |
|-------|-----------|----------|
| Stuck run | Run in `running` state > 1 hour with no step progress | HIGH |
| Scheduler stall | No assignments made in 5 minutes despite ready steps | CRITICAL |
| WAL growth | WAL file > 256 MiB | HIGH |
| Worker disconnect storm | > 50% of workers disconnect in 5 minutes | CRITICAL |
| Provider degradation | Provider success rate < 50% over 30 minutes | HIGH |
| Disk space | < 10% free disk | CRITICAL |
| Lease expiry spike | > 10 leases expired in 5 minutes | MEDIUM |
| Write queue depth | DB write actor queue > 1000 pending | HIGH |
| Error rate | > 5% of API requests returning 5xx | HIGH |

---

## 17. What This Design Does NOT Include (Intentionally Deferred)

- LLM-powered intent parsing (deterministic is enough for v1)
- Multi-Brain / distributed coordination (single process is fine at scale)
- x402 billing integration (log usage events, wire later)
- VeraAI observation pipeline (schema ready, integration deferred)
- Soma reputation wiring (event schema ready, implementation deferred)
- Gemini provider support (enum exists, executor stubbed)
- Desktop app (Tauri, after web is solid)
- Mobile app (iOS, after desktop)
- Agent hierarchy / manager layers
- Subscription pooling / team features
