# PR A — Truth model: `delivered`, `verifying`, and no completion before verification

> Self-contained. Do not read `HARNESS-EXCELLENCE-PLAN-2026-08.md` to execute
> this. Plan sections are cited only for provenance if a decision is challenged.
> Source: Phase 0.2.

## Objective

Today a worker's own report of success is what makes a step succeed. On a
delivery message, `complete_step` runs
`UPDATE steps SET status = 'succeeded' …` (`crates/api/src/db.rs:8713`) — the
worker said it worked, so the system records that it worked. `StepStatus`
(`crates/engine/src/captain.rs:51`) has no state between `Running` and
`Succeeded`, and `Succeeded` is terminal. `TaskStatus`
(`crates/core/src/task.rs:166`) goes straight to `Completed`. Independent
verification runs *afterwards*, off to the side, and cannot gate anything.

After this PR the worker's report is a **diagnostic**, not a transition guard.
A step that has produced a commit is `delivered`, then `verifying`, and only
Cortex's own independent runner can move it to `verified`. Nothing downstream —
`done` in the UI, a dependent write step, a PR creation, a charge, or a routing
reward — may act on a merely delivered result.

This PR writes the state-transition ADR, adds the states and transitions, makes
every transition a single transaction with a version CAS, and blocks the
downstream consumers. **It changes no billing behaviour and adds no verifier
durability** — those are PRs F and B.

## Do not do

- **Do not build the verification job table, outbox, claim/lease, heartbeat, or
  recovery loop.** That is **PR B**, and it depends on this PR. This PR leaves
  the existing `tokio::spawn` verifier call site at `crates/api/src/ws.rs:521`
  functionally where it is; it only changes *which state* the step is in when
  that call happens and *what the result is allowed to do*.
- **Do not touch the sandbox or `ExecutionJob`.** That is **PR C**.
- **Do not activate billing, quotes, or the ledger.** `DeliveryFacts.quoted_credits`
  stays `None` (`crates/api/src/ws.rs:518`). Do not invent a price to make a
  transition look complete.
- **Do not build the receipt API or redesign the receipt UI.** That is **PR D**.
  This PR changes state *labels* in existing panes only as far as is needed to
  stop them asserting `verified`/`done` for a delivered result.
- Do not add the manual-override *product surface*. Model the override in the
  schema with actor, reason, and expiry — build the data, not the UI.
- Do not touch the HeyVera Socials lane.

## Prerequisites

**Merged:** none strictly. PR A is Wave 1 and has no dependency on B or C. It is
the prerequisite *for* PR B.

**Verify present before starting:**

1. `crates/api/src/db.rs:8699` — `complete_step` still sets `status = 'succeeded'`
   directly, guarded by `AND lease_gen = ?7 AND status IN ('leased','running')`.
   **That CAS on `lease_gen` is correct and must be preserved**; it is the reason
   a stale delivery cannot bill today.
2. `crates/engine/src/captain.rs:51` — `StepStatus` still lacks `Delivered` /
   `Verifying`, and `Succeeded` is still in `is_terminal()`.
3. `crates/core/src/task.rs:166` — `TaskStatus` still has `Completed` with no
   verification gate.
4. `crates/api/src/ws.rs:521` — the verifier is still launched via `tokio::spawn`.
5. Current max migration is **v61** (`crates/api/src/db.rs:3445`, asserted at
   `db.rs:25375`). Claim the next free number; `schema_version` is a single
   counter shared with the HeyVera Socials product, so re-check before claiming.

## Files expected to change

| Path | Change |
|---|---|
| `docs/adr/` *(new file)* | The state-transition ADR. The state machine below, the rationale for `verifying` existing, and the rule that a worker report is never a guard. Confirm the repo's actual ADR directory before creating one. |
| `crates/engine/src/captain.rs` | Add `Delivered`, `Verifying`, `Verified`, `Inconclusive`, `ExecutionFailed` to `StepStatus`. Remove `Succeeded` from `is_terminal()` or remove the variant — see Design decision 3. Update `as_str`/`from_str` exhaustively. |
| `crates/core/src/task.rs` | `TaskStatus`: `Completed` becomes reachable only from a verified projection. |
| `crates/api/src/db.rs` | `complete_step` writes `delivered`, not `succeeded`. New transition functions, each one transaction, each with a version CAS. Migration v62. |
| `crates/api/src/storage.rs` | The `complete_step` trait signature at `:74` and `:229` follows the rename. |
| `crates/api/src/ws.rs` | The delivery handler moves the step to `delivered` then `verifying` in one transaction before the verifier is invoked. |
| `crates/api/src/scheduler.rs` | Dependency logic requires `verified`, not `succeeded`, for write-dependent edges. |
| `crates/api/src/verification_driver.rs` | The terminal verdict drives the projection transition instead of only recording a verdict. |
| `cortex/src/**` | Render `delivered`/`verifying` truthfully. Minimum viable: no pane may show "done" or a verified badge for a delivered-but-unverified step. |

## Invariants this PR must not violate

Copied verbatim from the plan's invariant list. Not paraphrased.

> 2. **No successful completion before independent verification.** The UI may say
>    `delivered; verifying`, but it may not say `done`, unlock dependent writes,
>    create a PR, charge, or improve a model's score before the required verdict.

> 3. **One durable work item has one durable terminal outcome.** Retries and
>    restarts are idempotent; a claimed verifier job is recoverable rather than
>    silently stranded.

> 6. **The independent verdict is the sole truth for completion, billing, and
>    learning.** Worker-reported checks remain useful diagnostics and are labelled
>    as such; they cannot create a verification badge or positive routing reward.

> 7. **Every UI state identifies its source and freshness.** A browser-local draft
>    can never be styled as shared live operations data.

> 22. **Cortex verifies what can be executed, and says UNVERIFIED otherwise.**
>     Where no executable ground truth exists, the work may still be done, is
>     labelled, and is never priced as though it had been proven.

> 27. **Executed checks are the only thing that can create a pass.** A model acting
>     as a verifier may rank candidates within the already-passing set, raise a
>     finding, or lower confidence. It can never move a `failed` to a `verified`,
>     create a badge, or emit a positive routing reward.

Invariant 3's "recoverable rather than silently stranded" is **PR B's** to
deliver. This PR must not *claim* it. Where a step can currently strand in
`verifying` because the spawned verifier died, that is a known gap this PR
creates and PR B closes — say so in the PR body rather than papering over it
with a timeout that fabricates a verdict.

## Design decisions already made

Conclusions only. Reasoning is in Phase 0.2.

1. **The state machine is exactly this:**

   ```text
   planned -> queued -> leased -> running -> delivered -> verifying
                                                |              |
                                                |              +-> verified -> review/done
                                                |              +-> failed -> retry/failed
                                                |              +-> inconclusive -> retry/attention
                                                +-> execution_failed -> retry/failed
   ```

   `delivered` means the sandbox produced a commit. `verified` means Cortex's
   independent runner accepted the frozen checks. Nothing else means either.

2. **This supersedes the earlier V3 launch guidance.** That guidance avoided a
   `verifying` state to keep the model smaller. It was wrong: omitting it makes
   the product state false, and the Operations Room contract already defines
   `verifying`. Implement it.

3. **`Succeeded` must stop being a terminal step state.** Prefer removing the
   variant outright over leaving it as an alias, so the compiler finds every
   consumer. If an alias is unavoidable for wire compatibility, it maps to
   `delivered` on read and is never written.

4. **The legacy worker report is preserved as an attached diagnostic artifact**,
   never deleted. It is labelled "Worker-reported diagnostics" wherever it
   surfaces and can never render as a verification badge.

5. **One transaction per transition.** Each transition updates the
   step/attempt/run/task projection *and* writes the operations event/outbox
   record in the **same** transaction. A projection that moved without an event,
   or an event without a projection, is the bug this rule exists to prevent.

6. **Every transition uses `lease_gen` plus a version CAS.** The existing
   `complete_step` guard (`AND lease_gen = ?7 AND status IN (...)`) is the
   pattern; extend it, do not replace it. A stale delivery and a stale verifier
   result must both be no-ops against an advanced attempt.

7. **Dependent *write* edges require `verified`.** Read-only dependents may
   proceed on `delivered` only if the scheduler can show the dependent does not
   write; if that cannot be established, serialise. Unknown scope is never
   optimistically unblocked.

8. **`inconclusive` never silently becomes done.** It is a first-class state. Its
   retry policy is PR B's; this PR only guarantees it exists and is not
   collapsible into success.

9. **Manual override is modelled explicitly** — actor, reason, expiry — and
   appears in the state as `manual_override`, never as `verified`. A human
   deciding to ship is a legitimate act and a different fact.

## Schema / migration

**Migration v62** (verify v61 is still the max before claiming this number; PR C
targets the same number, so whichever lands second takes the next one — this is
the shared-counter merge-order hazard, resolve it at rebase, not at design time).

```sql
-- Widen the step status domain. SQLite stores these as TEXT already, so this is
-- a data migration plus a constraint, not a type change.
UPDATE steps SET status = 'delivered' WHERE status = 'succeeded';

CREATE TABLE step_verification_state (
  step_id           TEXT NOT NULL,
  attempt_id        TEXT NOT NULL,
  lease_gen         INTEGER NOT NULL,
  state             TEXT NOT NULL,   -- delivered|verifying|verified|failed|inconclusive|execution_failed|manual_override
  entered_at        INTEGER NOT NULL,
  version           INTEGER NOT NULL DEFAULT 0,
  terminal_reason   TEXT,
  PRIMARY KEY (step_id, attempt_id, lease_gen)
);

CREATE TABLE manual_overrides (
  step_id      TEXT NOT NULL,
  attempt_id   TEXT NOT NULL,
  actor_id     TEXT NOT NULL,
  reason       TEXT NOT NULL,
  expires_at   INTEGER,
  created_at   INTEGER NOT NULL,
  PRIMARY KEY (step_id, attempt_id)
);
```

**Backfill:** existing `succeeded` rows become `delivered`, **not** `verified`.
This is the whole point — those rows were never independently verified and
labelling them so would be a false claim (invariant 6). State this explicitly in
the PR body, because it is a visible product regression to anyone reading old
runs, and it is the correct one.

**Rollback:** the additive tables drop cleanly. The `UPDATE steps` is reversible
only by re-asserting a claim that was never true, so treat it as forward-only and
say so.

## Test matrix

| Test | Catches |
|---|---|
| `truth::worker_success_cannot_make_step_verified` | **The core regression.** A worker reports success; assert the step is `delivered`, never `verified` or `succeeded`. |
| `truth::worker_success_cannot_complete_task` | The same shortcut one level up — `TaskStatus::Completed` reachable from a worker report. |
| `truth::failed_independent_check_transitions_to_failure` | A delivered attempt surviving a failing independent verdict. |
| `truth::stale_delivery_cannot_alter_active_attempt` | A delivery arriving with an outdated `lease_gen` moving a newer attempt. |
| `truth::stale_verifier_result_cannot_alter_active_attempt` | The same on the verdict side — the case the current `lease_gen` CAS defends and which must survive the refactor. |
| `truth::dependent_write_step_blocked_until_verified` | A dependent write unblocking on `delivered`. |
| `truth::inconclusive_does_not_become_done` | `inconclusive` collapsing into success on any path. |
| `truth::transition_and_event_are_one_transaction` | A projection updating without its operations event. Inject a failure between the two writes; assert neither landed. |
| `truth::version_cas_rejects_concurrent_transition` | Two concurrent transitions both succeeding. |
| `truth::manual_override_is_not_verified` | An override rendering or recording as a verification. |
| `truth::legacy_report_survives_as_diagnostic` | The legacy worker report being dropped by the refactor. |
| `truth::migration_backfills_succeeded_to_delivered` | The migration labelling historical rows `verified`. |

Run `cargo +stable-x86_64-pc-windows-gnullvm test -p cortex-api --lib`. Known
pre-existing failure, not a regression:
`validate::tests::normalizes_dot_segments` (Windows path separators).

## Done criteria

- [ ] `grep -rn "status = 'succeeded'" crates/api/src/db.rs` returns nothing on
      the step path.
- [ ] `StepStatus::Succeeded` no longer exists, or exists only as a read-side
      alias that is never written, and `is_terminal()` does not include it.
- [ ] Every state in the machine above exists and is reachable only by its
      stated transition.
- [ ] Every transition function is one transaction, writes its operations event
      inside that transaction, and carries a `lease_gen` + version CAS.
- [ ] `scheduler.rs` requires `verified` for write-dependent edges.
- [ ] No pane under `cortex/src` renders "done" or a verified badge for a step
      in `delivered` or `verifying`; the legacy report renders only under a
      "Worker-reported diagnostics" label.
- [ ] Migration applies cleanly on a fresh database and on a copy of the current
      production schema; the fresh-database assertion in `db.rs` is updated.
- [ ] Every test in the matrix exists, is named as written, and passes.
- [ ] The ADR is committed and describes the machine as implemented.
- [ ] The PR body names the strand-in-`verifying` gap that PR B closes.
- [ ] All seven required checks green: `heyvera`, `rust`, `cortex`,
      `npm-audit (cortex)`, `npm-audit (heyvera)`, `cargo-deny`, `sandbox`.
- [ ] `cargo fmt` was **not** run.

## Open questions

None.
