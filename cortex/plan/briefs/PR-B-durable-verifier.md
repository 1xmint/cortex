# PR B — Durable verifier: job outbox, lease, and recovery

> Self-contained. Do not read `HARNESS-EXCELLENCE-PLAN-2026-08.md` to execute
> this. Plan sections are cited only for provenance if a decision is challenged.
> Source: Phase 1.1 and 1.2.

## Objective

Verification today is a fire-and-forget task. On delivery, `crates/api/src/ws.rs:521`
calls `tokio::spawn`, opens its own database handle, constructs a
`ContainerCheckRunner`, and runs `verify_delivery`. If the process restarts, the
node is deployed, the container runner is unavailable, or the task panics, the
verification simply never happens — and the current code says so out loud:
`"no container runner available; delivery left unverified"`
(`crates/api/src/ws.rs:527`). Nothing retries it, nothing reconciles it, and
nothing surfaces that a delivered change is permanently stranded.

After this PR, every delivered change has a **durable** verification job. The
job is inserted in the same transaction that moves the step to `verifying`. A
dispatcher claims jobs with database CAS semantics, heartbeats while it works,
and reclaims claims that expire. On startup and on an interval, every
non-terminal job is reconciled. A crash at any point recovers automatically and
produces **at most one** receipt, charge, or refund.

`tokio::spawn` is never again the source of durability.

## Do not do

- **Do not add or change lifecycle states.** `delivered`, `verifying`,
  `verified`, `inconclusive`, `execution_failed` are **PR A's** and must already
  exist. This PR makes the `delivered → verifying` transition durable; it does
  not define it.
- **Do not touch the sandbox or `ExecutionJob`.** That is **PR C**.
- **Do not activate billing.** The transition transaction must be *shaped* so a
  ledger write can join it later exactly once, but `DeliveryFacts.quoted_credits`
  stays `None` and no ledger row is written. Inventing a price to exercise the
  path is prohibited.
- **Do not build the receipt API or UI.** That is **PR D**. This PR exposes the
  recovery attempt in the operations timeline and stops there.
- **Do not implement the Postgres dispatcher.** Design the claim semantics so
  `FOR UPDATE SKIP LOCKED` maps onto them cleanly, and write the SQLite one.
- **Do not enable provider work if the sandbox is not ready** — keep the
  existing gating; this PR must not become the reason unsandboxed execution runs.
- Do not touch the HeyVera Socials lane.

## Prerequisites

**Merged: PR A.** This PR is not startable without it. Specifically, it needs
`verifying` to exist as a state and the transition to be a single transaction
with a version CAS — this PR inserts the job row *into that transaction*.

**Verify present before starting:**

1. PR A's transition function exists and is transactional. If the
   `delivered → verifying` move is still more than one write, stop: that is PR A
   being incomplete, and it is a finding, not something to work around.
2. `crates/api/src/ws.rs:521` still uses `tokio::spawn` for verification. This
   PR deletes that call site.
3. `crates/api/src/verification_driver.rs` (593 lines) still exposes
   `verify_delivery` and `DeliveryFacts`. The driver's *logic* is reused; only
   its invocation becomes durable.
4. Whether the deployment asserts single-node mode. If there is no such
   assertion, adding one is in scope — see Design decision 5.
5. Current max migration number. **v61** was the max on 2026-08-09
   (`crates/api/src/db.rs:3445`); PRs A and C also claim numbers, so re-check and
   take the next free one at rebase time.

## Files expected to change

| Path | Change |
|---|---|
| `crates/api/src/db.rs` | Migration for `verification_jobs`. Claim/heartbeat/reclaim/terminal SQL, each a CAS. The job insert joins PR A's transition transaction. |
| `crates/api/src/verification_dispatcher.rs` *(new)* | The claim loop, heartbeat, expiry reclaim, startup + interval reconciliation, backoff, and the dead-letter path. |
| `crates/api/src/verification_driver.rs` | `verify_delivery` becomes a function the dispatcher calls with a claimed job, not something spawned from a socket handler. On terminal verdict it performs the single idempotent transition of 1.2. |
| `crates/api/src/ws.rs` | Delete the `tokio::spawn` block at `:521`. Delivery enqueues; it does not execute. |
| `crates/api/src/state.rs` | Dispatcher handle and its configuration. Startup asserts single-node mode. |
| `crates/api/src/metrics.rs` *(or existing metrics module)* | Queue depth, claim age, reclaim count, dead-letter count, time-to-verdict. Confirm the actual module before adding. |
| `crates/api/src/main.rs` | Start the dispatcher; run startup reconciliation before serving. |

## Invariants this PR must not violate

Copied verbatim from the plan's invariant list. Not paraphrased.

> 2. **No successful completion before independent verification.** The UI may say
>    `delivered; verifying`, but it may not say `done`, unlock dependent writes,
>    create a PR, charge, or improve a model's score before the required verdict.

> 3. **One durable work item has one durable terminal outcome.** Retries and
>    restarts are idempotent; a claimed verifier job is recoverable rather than
>    silently stranded.

> 4. **Receipts are reproducible.** Every receipt names an immutable source tree,
>    OCI image digest, check argv, timeout, resource profile, result, output
>    digest, and artifact locations.

> 6. **The independent verdict is the sole truth for completion, billing, and
>    learning.** Worker-reported checks remain useful diagnostics and are labelled
>    as such; they cannot create a verification badge or positive routing reward.

> 21. **An unreliable check never decides anything.** A check that produces
>     differing outcomes on a byte-identical tree is quarantined: it still runs and
>     still reports, but it cannot fail a paid task, move the ledger, or reach any
>     learning signal — and the receipt names it.

> 27. **Executed checks are the only thing that can create a pass.** A model acting
>     as a verifier may rank candidates within the already-passing set, raise a
>     finding, or lower confidence. It can never move a `failed` to a `verified`,
>     create a badge, or emit a positive routing reward.

Invariant 21's quarantine mechanism is **not built here** — flakiness detection
is Phase 24.1. What this PR must not do is build a retry policy that *launders*
flakiness into a pass: a job that produced differing outcomes across attempts on
a byte-identical tree resolves `inconclusive`, never `verified`. Retrying until
green is exactly the failure mode.

## Design decisions already made

Conclusions only. Reasoning is in Phase 1.1 and 1.2.

1. **`verification_jobs` carries the frozen inputs, not references that can
   drift.** Job ID; run/step/attempt/lease generation; the **immutable delivered
   commit**; the frozen spec set ID; quote ID; runner policy version. A job that
   re-resolves its spec set at claim time can verify against a different exam
   than the one that was promised.

2. **States are exactly:** `queued | claimed | retry_wait | succeeded | failed |
   inconclusive | dead`. Plus claim token, claimed-at, lease/heartbeat,
   attempt count, next-run-at, and a typed terminal reason.

3. **A unique key covers the logical verification attempt.** This is what makes
   "at most one receipt/charge/refund" true under concurrent enqueue and under
   replay. `UNIQUE(run_id, step_id, attempt_id, lease_gen)`.

4. **The job is inserted in PR A's `delivered → verifying` transaction.** Not
   after it, not in a callback. If the transition commits, the job exists; if it
   rolls back, neither happened. This is the entire durability argument.

5. **Exactly one verifier dispatcher on SQLite, and startup fails if the
   deployment does not assert single-node mode.** This is a hard failure, not a
   warning — two dispatchers on the current `Mutex<Connection>` double-dispatch.
   The future Postgres dispatcher uses `FOR UPDATE SKIP LOCKED` or equivalent,
   and **its semantics must match this implementation**, so write the claim as a
   conditional update on `(state, claim_token)` rather than anything
   SQLite-specific.

6. **Claims expire and are reclaimed; they are never trusted indefinitely.** A
   dispatcher that dies mid-check leaves a claim whose heartbeat stops; after the
   lease window another dispatcher reclaims it. Reclaim increments the attempt
   count so a job that repeatedly kills its dispatcher reaches `dead` rather than
   looping forever.

7. **Reconciliation runs on startup and on an interval**, over *every*
   non-terminal job — not only the ones this process enqueued. This is what
   recovers work stranded by the previous deploy.

8. **Terminal verification performs one idempotent transition** that: seals the
   verification report; transitions attempt/step/run/task projections; unblocks
   or cancels dependent steps; emits `verification.completed` with source
   `cortex_independent`; and queues the next scheduler action. All of it, or none
   of it.

9. **`inconclusive` gets a bounded automatic retry for infrastructure-class
   failures only** — runner unavailable, sandbox setup failure, timeout of the
   harness rather than of the check. A check that *ran* and was inconclusive does
   not get retried into a verdict. After the bound, the job enters `attention`
   with an explanation visible to both the operator and the customer.

10. **A verification failure never quietly marks a step successful.** It produces
    a diagnostic branch and a retry policy. There is no path from `failed` to
    `verified` that does not involve executed checks passing.

11. **The "no container runner available" case becomes a durable
    `retry_wait`,** not a log line. That message at `ws.rs:527` is the current
    silent-stranding bug in one string, and its disappearance is a done criterion.

## Schema / migration

**Migration number: take the next free one at rebase time.** v61 was the max on
2026-08-09; PRs A and C are also claiming numbers, and `schema_version` is a
single counter shared with the HeyVera Socials product.

```sql
CREATE TABLE verification_jobs (
  job_id             TEXT PRIMARY KEY,
  run_id             TEXT NOT NULL,
  step_id            TEXT NOT NULL,
  attempt_id         TEXT NOT NULL,
  lease_gen          INTEGER NOT NULL,

  delivered_commit   TEXT NOT NULL,     -- immutable; never re-resolved
  spec_set_id        TEXT NOT NULL,     -- frozen at dispatch
  quote_id           TEXT,              -- NULL until PR F
  runner_policy_ver  TEXT NOT NULL,

  state              TEXT NOT NULL,     -- queued|claimed|retry_wait|succeeded|failed|inconclusive|dead
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

CREATE INDEX idx_verification_jobs_claimable
  ON verification_jobs(state, next_run_at);
CREATE INDEX idx_verification_jobs_reclaim
  ON verification_jobs(state, lease_expires_at);
```

**Backfill:** enqueue a job for every step currently sitting in `verifying` with
no terminal verdict — those are the deliveries the `tokio::spawn` path stranded.
Do **not** enqueue for historical `delivered` rows backfilled by PR A; those
predate the verifier and re-verifying them against a moved tree would produce a
meaningless verdict. State the distinction in the PR body.

**Rollback:** the table drops cleanly. Deleting it while jobs are in flight loses
the queue, so rollback requires draining first — say so in the PR body.

## Test matrix

Failure injection is the substance of this PR. Each crash point must recover
automatically, produce **at most one** receipt/charge/refund, and expose the
recovery attempt in the operations timeline.

| Test | Catches |
|---|---|
| `verifier::crash_after_enqueue_recovers` | A job enqueued but never claimed because the process died. |
| `verifier::crash_after_claim_reclaims` | A claim held by a dead dispatcher stranding the job forever. |
| `verifier::crash_after_first_check_recovers` | Partial check execution producing a partial verdict. |
| `verifier::crash_after_verdict_seal_recovers` | A sealed verdict whose projection transition never ran. |
| `verifier::crash_after_ledger_write_is_idempotent` | The double-charge case. Must hold even though billing is inactive — the shape is what is being tested. |
| `verifier::at_most_one_receipt_under_replay` | Duplicate enqueue or duplicate claim producing two receipts. |
| `verifier::job_insert_shares_transition_transaction` | The job being inserted outside PR A's transaction. Roll back the transition; assert no job row. |
| `verifier::expired_claim_is_reclaimed_and_counted` | Reclaim not incrementing `attempt_count`, allowing an infinite loop. |
| `verifier::exhausted_attempts_reach_dead_not_retry` | A poison job looping forever. |
| `verifier::startup_reconciles_foreign_non_terminal_jobs` | Reconciliation only handling jobs this process enqueued. |
| `verifier::second_dispatcher_fails_startup_on_sqlite` | Two dispatchers double-dispatching on `Mutex<Connection>`. |
| `verifier::runner_unavailable_is_retry_wait_not_lost` | The current `ws.rs:527` silent-stranding bug returning. |
| `verifier::infrastructure_inconclusive_retries_bounded` | Unbounded retry, and retry of a check that actually ran. |
| `verifier::executed_inconclusive_does_not_retry_into_pass` | **Retry laundering.** Differing outcomes across attempts resolving `verified` instead of `inconclusive`. |
| `verifier::failed_verdict_never_becomes_verified` | Any path from `failed` to `verified` without passing checks. |
| `verifier::terminal_transition_is_all_or_nothing` | A sealed report without its projection, or vice versa. |
| `verifier::spec_set_is_frozen_at_enqueue` | A job re-resolving its spec set at claim time and verifying against a different exam. |
| `verifier::recovery_attempt_visible_in_operations_timeline` | Silent recovery — correct behaviour that nobody can see. |

Run `cargo +stable-x86_64-pc-windows-gnullvm test -p cortex-api --lib`. Known
pre-existing failure, not a regression:
`validate::tests::normalizes_dot_segments` (Windows path separators).

## Done criteria

- [ ] `grep -n "tokio::spawn" crates/api/src/ws.rs` returns nothing on the
      verification path.
- [ ] The string `"no container runner available; delivery left unverified"` no
      longer exists; that condition is a durable `retry_wait`.
- [ ] The job row is inserted inside PR A's `delivered → verifying` transaction,
      asserted by `verifier::job_insert_shares_transition_transaction`.
- [ ] Every crash point in the matrix has a test, and each asserts **at most one**
      receipt/charge/refund.
- [ ] Startup fails, loudly, if single-node mode is not asserted.
- [ ] Startup reconciliation runs before the server accepts traffic.
- [ ] The claim is expressed as a conditional update on `(state, claim_token)`,
      with no SQLite-specific construct that a `FOR UPDATE SKIP LOCKED` port
      could not match.
- [ ] Metrics exist for queue depth, claim age, reclaim count, dead-letter
      count, and time-to-verdict.
- [ ] There is no code path from `failed` or `inconclusive` to `verified` that
      does not run executed checks.
- [ ] Backfill enqueues stranded `verifying` steps and **not** PR A's
      historical `delivered` rows.
- [ ] Migration applies cleanly on a fresh database and on a copy of the current
      production schema; the fresh-database assertion in `db.rs` is updated.
- [ ] All seven required checks green: `heyvera`, `rust`, `cortex`,
      `npm-audit (cortex)`, `npm-audit (heyvera)`, `cargo-deny`, `sandbox`.
- [ ] `cargo fmt` was **not** run.

## Open questions

None.
