# PR C — Sandbox boundary and `ExecutionJob`

> Self-contained. Do not read `HARNESS-EXCELLENCE-PLAN-2026-08.md` to execute
> this. Plan sections are cited only for provenance if a decision is challenged.
> Source: Phase 0.1, amended by Phase 6 and Phase 32.4.

## Objective

Today the worker executes provider CLIs as **host processes**. It creates a git
worktree for isolation, but that isolation is explicitly best-effort: when
worktree creation fails for any reason, it logs at `debug` and runs the agent
directly in the caller's working directory
(`crates/worker/src/executor.rs:49-70`). The spawned process
(`crates/worker/src/executor.rs:74`) inherits the worker's full environment,
network, filesystem, and credentials.

After this PR, model-authored code never runs on the host. Every provider
invocation is a versioned `ExecutionJob` handed to a sandbox runner that owns
the boundary: fresh per task attempt, non-root, read-only base image, bounded
CPU/memory/pids/disk/wall-clock, default-deny egress with an explicit per-task
allowlist, no Docker socket, no host namespaces, no inherited credentials, and
mandatory teardown. There is **no fallback path**: if the sandbox or the
worktree cannot be established, the step returns a typed `Blocked` outcome and
the provider CLI is never invoked.

`ExecutionJob` is cut **once**, with the full field set below — including the
fields nothing reads yet. This struct is the narrowest waist in the system and
re-cutting it after PRs E, F, I, K, and M join on it is expensive.

## Do not do

- **Do not introduce lifecycle states.** `delivered`, `verifying`, `verified`,
  `execution_failed` and the transition machine are **PR A**. This PR adds
  exactly one new outcome shape, `Blocked`, and does not touch the projection
  logic that consumes step outcomes.
- **Do not build the verification job table, outbox, lease, or recovery loop.**
  That is **PR B**.
- **Do not implement the effort dial, `ExecutionPolicy`, the model catalog, or
  routing changes.** This PR *carries* `effort`, `model_ref`, and budget fields
  and *enforces* the budgets it can. It does not decide their values, and it
  does not add a policy table. Those are PRs I and K.
- **Do not implement the signed runner registry / OCI digest pinning.** That is
  Phase 0.3, a separate PR. This PR records whatever image reference it was
  given; it does not add digest validation at startup.
- **Do not add capability-grant issuance or the Plan Receipt.** `capability_grants`
  is carried and enforced at the sandbox edge; it is *populated* by Phase 2.1.
- Do not change pricing, the ledger, or anything under `cortex/src` beyond what a
  new `Blocked` outcome requires to render without crashing.
- Do not touch the HeyVera Socials lane.

## Prerequisites

**Merged:** none. This PR is scheduled ahead of its nominal wave deliberately —
it is the only item in the plan that closes a live safety exposure rather than
adding a capability. It has no code dependency on PR A: its outward contract is
a typed outcome, not a state transition.

**Verify present before starting** (the tree moves; re-check, do not assume):

1. `crates/worker/src/executor.rs:49-70` still contains the worktree fallback to
   `dir.to_path_buf()`. If it does not, the exposure was already closed and this
   brief must be re-scoped before writing code.
2. `crates/worker/src/executor.rs:74` still spawns via `Command::new(&cmd)`.
3. `crates/worker/src/executor.rs:520` still reads
   `ProviderId::Gemini => Ok(("gemini".to_string(), vec![]))`.
4. Current max migration is **v61** (`crates/api/src/db.rs:3445`, asserted at
   `db.rs:25375`). Claim the next free number; if another branch has taken v62,
   take the next one. `schema_version` is a single counter shared with the
   HeyVera Socials product.
5. A container runtime is available on the VPS. If it is not, that is a finding
   to report, not something to route around with a host-execution fallback.

## Files expected to change

| Path | Change |
|---|---|
| `crates/worker/src/executor.rs` | Remove host `Command::new` spawn and the worktree fallback. Build an `ExecutionJob` and submit it to the runner. Fix the Gemini arm at `:520` to carry the routed model — same interface, so it lands here. |
| `crates/worker/src/sandbox/mod.rs` *(new)* | `SandboxRunner` trait: `submit(ExecutionJob) -> Result<ExecutionOutcome, Blocked>`. The trait is the seam that lets a Firecracker/microVM implementation replace the container one without touching callers. |
| `crates/worker/src/sandbox/container.rs` *(new)* | Rootless-container implementation. Owns argv construction, mount set, resource limits, seccomp profile, network policy application, egress logging, and teardown. |
| `crates/worker/src/sandbox/policy.rs` *(new)* | `NetworkPolicy`, `ResourceProfile`, `CapabilityGrant` types and their defaults. Default egress is deny. |
| `crates/core/src/execution_job.rs` *(new)* | The `ExecutionJob` struct and its enums, versioned. Lives in core because api, worker, and later the receipt projection all need it. |
| `crates/worker/src/worktree.rs` | Worktree creation becomes **required**. Failure returns a typed error that maps to `Blocked`; it no longer degrades. |
| `crates/api/src/db.rs` | Migration v62: `execution_jobs` persistence (see Schema). |
| `crates/api/src/state.rs` | Wire the runner's configuration (image ref, resource profile version) into `AppState`. |
| `deploy/` or `scripts/` | Runner image build + the deployment change that makes the sandbox available. Confirm actual path in-tree before writing. |

## Invariants this PR must not violate

Copied verbatim from the plan's invariant list. Not paraphrased.

> 1. **No host execution of untrusted task code.** A code-writing agent never
>    receives host credentials, a Docker socket, the production database, or
>    arbitrary host filesystem access.

> 4. **Receipts are reproducible.** Every receipt names an immutable source tree,
>    OCI image digest, check argv, timeout, resource profile, result, output
>    digest, and artifact locations.

> 8. **Parallel work is conflict-safe by policy, not best effort.** If a required
>    isolation, lease, base commit, or sandbox cannot be obtained, dispatch fails
>    closed and explains the blocker.

> 10. **A receipt never claims a setting the backend did not apply.** If an
>     execution backend cannot honour a requested effort level or model, it
>     declares that at dispatch and the receipt records what was actually applied,
>     not what was asked for.

> 14. **Spend cannot surprise.** Every run carries a forecast and a hard cap
>     enforced at the sandbox boundary. At the cap Cortex stops and asks; it never
>     silently continues and never silently abandons. Cortex may lower effort or
>     cost without asking and must record it; it may never raise them without
>     consent.

> 18. **Instructions come only from the contract; everything else is data.**
>     Repository content, issue bodies, dependency metadata, and tool output are
>     `observed` information and can never direct behaviour, regardless of what
>     they contain. Apparent directives in observed content are reported as
>     findings.

> 30. **Every autonomous capability has an off switch and an inverse.** Dispatch
>     stops at four scopes and by automatic breaker; a stop leaves every in-flight
>     item in a truthful state attributed to the operator rather than to the
>     customer's work; every external effect is compensable with a declared inverse
>     or declared irreversible at plan time and ordered last; and every receipt is
>     recallable by the artifact versions that produced it.

Invariant 14's clause "enforced at the sandbox boundary" is this PR's
responsibility for the mechanism. Populating the budget with a real forecast is
PR F's. An unset budget means *no cap enforced*, which must be logged, not
silently treated as unlimited-and-fine.

Invariant 30's off-switch is out of scope as a product surface, but the runner
must support **kill-and-teardown of an in-flight sandbox** so Phase 32.1's stop
has something to call. Build the primitive; do not build the surface.

## Design decisions already made

Conclusions only. Reasoning is in Phase 0.1, Phase 6.1–6.2, and Phase 32.4.

1. **Isolation strength: kernel-level is the requirement.** A shared-kernel
   container is a resource boundary, not a security boundary against
   model-authored code running adjacent to other customers' source. The initial
   implementation **may** ship a rootless container runtime provided every
   enforced property below holds, and **the `SandboxRunner` trait must not leak
   container-specific concepts**, so a microVM implementation replaces it without
   touching callers. Record the isolation class on the job so a receipt can state
   which boundary actually ran.
2. **One sandbox per task attempt, never reused across tenants.** No writable
   state survives an attempt. No shared content-addressed cache across tenants —
   a cross-tenant check-result cache is an oracle for private source. Per-tenant
   scoping is architecture here, not policy.
3. **Egress is default-deny**, opened only by an explicit task-scoped allowlist
   (typically a package registry, and exactly a package registry). The applied
   policy is recorded on the job so it can reach the receipt.
4. **No credentials in the sandbox.** Not the customer's, and not Cortex's own —
   a sandbox holding a provider key can spend Cortex's money. **The runner
   service, not the model process, owns Git credentials** and performs sanctioned
   fetch/push on the sandbox's behalf.
5. **No fallback, ever.** Sandbox setup failure and worktree failure both produce
   a typed `Blocked` outcome carrying a machine-readable reason. The provider CLI
   must not be invoked on either path. This is the single most important
   behavioural change in the PR.
6. **`ExecutionJob` ships complete on day one**, including fields nothing reads
   yet. `Option<T>` for not-yet-populated values; never omit the field.
7. **Effort is fixed for the lifetime of an attempt.** Two independent reasons
   agree: provider guidance is that effort shapes the rendered prompt, so varying
   it mid-session discards the cached prefix; and the fresh-context retry rule
   requires escalation to start a new attempt anyway. Implement as one rule:
   **escalation creates a new attempt; it never mutates a running job's effort.**
8. **The CLI-vs-HTTP effort question is settled here, in the brief, not during
   implementation.** CLI backends (`claude -p`, `codex exec`) are invoked with
   model flags only and may not expose an effort control. Therefore:
   `ExecutionJob` carries `effort: Option<EffortLevel>` **and**
   `effort_applied: EffortApplication`, and the backend reports which of
   `Applied | Downgraded { to } | Unsupported | NotRequested` occurred, together
   with `backend_kind: Cli | Http`. **A CLI backend must never silently swallow
   an effort request** — it returns `Unsupported` and that reaches the receipt.
   Routing effort-controlled work down the HTTP path
   (`crates/api/src/llm_client.rs`) is PR K's decision to make with this data;
   this PR only guarantees the data is truthful.
9. **The Gemini model-drop fix belongs in this PR**, not a separate one, because
   it is the same interface: `executor.rs:520` returns a bare `"gemini"` command
   with no model argument, so the routed model is unforgeable only once the job
   carries `model_ref` and the invocation is built from it.

### `ExecutionJob` field set — ship all of it

| Field | Type | Populated by |
|---|---|---|
| `job_version` | `u32` | this PR; bump on any breaking field change |
| `attempt_id` | `AttemptId` | existing `StepExecution` |
| `run_id`, `step_id` | ids | existing `StepExecution` |
| `lease_gen` | `i64` | existing `StepExecution` |
| `model_ref` | `ModelRef { catalog_id, catalog_version }` | routing; `catalog_version` is `None` until PR I |
| `backend_kind` | `Cli \| Http` | this PR |
| `effort` | `Option<EffortLevel>` | `None` until PR K |
| `effort_applied` | `EffortApplication` | the backend, at dispatch |
| `token_budget` | `Option<u64>` | `None` until PR F |
| `wall_clock_budget` | `Option<Duration>` | this PR — set a hard default; unbounded is not acceptable |
| `max_tool_calls` | `Option<u32>` | `None` until PR F |
| `network_policy` | `NetworkPolicy` | this PR; default `Deny` |
| `capability_grants` | `Vec<CapabilityGrant>` | empty until Phase 2.1 |
| `context_bundle_ref` | `Option<BundleRef>` + `packed_bytes: Option<u64>` | `None` until the context work |
| `quote_id` | `Option<QuoteId>` | `None` until PR F |
| `plan_receipt_id` | `Option<PlanReceiptId>` | `None` until Phase 2.1 |
| `resource_profile` | `ResourceProfile` + `profile_version` | this PR |
| `image_ref` | `String` | this PR; digest *validation* is Phase 0.3 |
| `isolation_class` | `Container \| MicroVm` | this PR |

`wall_clock_budget` is deliberately not `Option`-in-practice: give it a
configured default so no job is ever unbounded, even before PR F exists.

## Schema / migration

**Migration v62** (verify v61 is still the max before claiming this number).

```sql
CREATE TABLE execution_jobs (
  job_id            TEXT PRIMARY KEY,
  job_version       INTEGER NOT NULL,
  run_id            TEXT NOT NULL,
  step_id           TEXT NOT NULL,
  attempt_id        TEXT NOT NULL,
  lease_gen         INTEGER NOT NULL,

  model_catalog_id  TEXT,
  model_catalog_ver TEXT,
  backend_kind      TEXT NOT NULL,
  effort_requested  TEXT,
  effort_applied    TEXT NOT NULL,

  token_budget      INTEGER,
  wall_clock_ms     INTEGER NOT NULL,
  max_tool_calls    INTEGER,

  network_policy    TEXT NOT NULL,
  capability_grants TEXT NOT NULL DEFAULT '[]',
  context_bundle    TEXT,
  packed_bytes      INTEGER,

  quote_id          TEXT,
  plan_receipt_id   TEXT,

  image_ref         TEXT NOT NULL,
  isolation_class   TEXT NOT NULL,
  resource_profile  TEXT NOT NULL,
  profile_version   TEXT NOT NULL,

  submitted_at      INTEGER NOT NULL,
  terminated_at     INTEGER,
  outcome           TEXT,
  blocked_reason    TEXT,

  UNIQUE(attempt_id, lease_gen)
);

CREATE INDEX idx_execution_jobs_step ON execution_jobs(run_id, step_id);
```

The `UNIQUE(attempt_id, lease_gen)` key is what makes a re-submitted job
idempotent under retry — it mirrors the existing `UNIQUE(run_id, step_id, attempt)`
claim already used on the dispatch path.

**Backfill:** none. Existing rows predate the sandbox and must not be
retroactively labelled as having run inside one — that would be a false
provenance claim (invariant 4).

**Rollback:** the table is additive and write-only in this PR's read paths, so
rollback is a `DROP TABLE`. The *behavioural* change is not rollback-safe by
schema and must be gated on the runner being deployed — see Done criteria.

## Test matrix

Adversarial tests are the point of this PR. Each names the failure it catches.

| Test | Catches |
|---|---|
| `sandbox::cannot_read_host_secret` | A task reading a host env var or a file outside its workspace. Plant a canary secret on the host; assert it is unreachable. |
| `sandbox::no_docker_socket` | `/var/run/docker.sock` reachable from inside — container escape to full host control. |
| `sandbox::egress_denied_by_default` | An outbound connection to an unapproved host succeeding when no allowlist was granted. |
| `sandbox::egress_allowlist_is_scoped` | An allowlist for a package registry silently permitting arbitrary hosts. |
| `sandbox::no_writes_outside_workspace` | A task writing to a host path via a mount that should not exist. |
| `sandbox::no_state_persists_to_next_task` | Sandbox reuse. Write a marker in attempt 1; assert attempt 2 cannot see it. |
| `sandbox::not_root_and_base_is_readonly` | Privilege and image-mutation regressions. |
| `sandbox::resource_limits_enforced` | An unbounded fork/memory/disk consumer taking down the host. |
| `sandbox::wall_clock_budget_terminates` | A job that never exits. Invariant 14's cap being decorative. |
| `sandbox::no_provider_credentials_present` | A provider key leaking into the sandbox environment — the case that can spend Cortex's money. |
| `executor::sandbox_failure_blocks_and_does_not_invoke_cli` | **The core regression.** Force sandbox setup to fail; assert outcome is `Blocked` with a typed reason and that the provider CLI was never spawned. |
| `executor::worktree_failure_blocks_and_does_not_fall_back` | The exact behaviour at `executor.rs:49-70` today. Force worktree creation to fail; assert `Blocked`, not execution in the caller's directory. |
| `execution_job::carries_full_field_set` | A future refactor quietly dropping a field. Assert every field in the table above round-trips through persistence. |
| `execution_job::gemini_invocation_carries_model_ref` | The `executor.rs:520` regression returning to a bare `"gemini"` with no model. |
| `execution_job::cli_backend_reports_effort_unsupported` | A CLI backend silently swallowing an effort request — invariant 10. |
| `execution_job::idempotent_on_attempt_and_lease_gen` | Duplicate submission creating two sandboxes for one attempt. |
| `sandbox::teardown_runs_on_panic_and_on_error` | Leaked sandboxes accumulating on the host. |

Run with `cargo +stable-x86_64-pc-windows-gnullvm test -p cortex-api --lib` plus
the worker crate's suite. Tests that genuinely require a container runtime must
be feature-gated so the Windows dev loop stays usable, and **must run in CI** —
a gated adversarial test that never executes is worse than no test.

Known pre-existing failure, not a regression:
`validate::tests::normalizes_dot_segments` (Windows path separators; passes on
Linux CI).

## Done criteria

Mechanically checkable. No judgment calls.

- [ ] `grep -n "Command::new" crates/worker/src/executor.rs` returns no
      provider-CLI spawn. Any remaining occurrence is inside the sandbox runner
      or is a git invocation, and is annotated as such.
- [ ] No code path in `executor.rs` maps a worktree or sandbox error to
      continued execution. The string "falling back" does not appear.
- [ ] `crates/worker/src/executor.rs:520`'s Gemini arm passes `model_ref`.
- [ ] `ExecutionJob` contains every field in the table above, and
      `execution_job::carries_full_field_set` asserts it.
- [ ] Every test in the matrix exists, is named as written, and passes.
- [ ] The adversarial tests run in CI, not only locally.
- [ ] Migration v62 applies cleanly on a fresh database and on a copy of the
      current production schema; the fresh-database assertion in `db.rs` is
      updated to the new max.
- [ ] Default `NetworkPolicy` is `Deny` and a test asserts the default, not just
      the explicit case.
- [ ] No provider credential, Git credential, or host env var is present in the
      sandbox environment; asserted by test, not by inspection.
- [ ] Every job persists an `image_ref`, `isolation_class`, `resource_profile`,
      and `profile_version` — the fields a reproducible receipt will need.
- [ ] The runner supports kill-and-teardown of an in-flight sandbox.
- [ ] The deployment change that makes the runner available is in the PR, and
      the PR body states what must be true on the VPS before merge takes effect.
- [ ] All seven required checks green: `heyvera`, `rust`, `cortex`,
      `npm-audit (cortex)`, `npm-audit (heyvera)`, `cargo-deny`, `sandbox`.
- [ ] `cargo fmt` was **not** run.

## Open questions

None. Every decision this PR needs is resolved above, including the CLI-vs-HTTP
effort question that the plan's handover-readiness table flagged as the one item
requiring care.

One item is **deliberately deferred, not open**: the microVM implementation has
no date here. The `SandboxRunner` trait makes it a swap rather than a rewrite,
and Phase 32.4 asks for a dated commitment — that date is Josh's to set and does
not block this PR.
