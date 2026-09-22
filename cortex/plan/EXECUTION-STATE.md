# Cortex execution state

Running checkpoint for the actualization of
`cortex/plan/HARNESS-EXCELLENCE-PLAN-2026-08.md`. Update this file as work
lands; an interrupted session should be able to resume from it without
re-deriving anything.

**Last updated:** 2026-09-14 (Cortex extraction route-ownership PR 2 implemented locally)
**Base commit at start:** `c8ca2941` (main — "clear all seven open dependency advisories (#498)")
**Wave 2 base:** `3db58b13` (main — "make the sandbox check able to block a merge (#504)")

---

## Status

| Task | What | State |
|---|---|---|
| 1 | Land the harness plan on `main` | **done** — PR [#500](https://github.com/hey-vera/heyvera/pull/500) merged |
| 1a | Split-out auth commit from the same branch | branch `fix/auth-remove-subscription-flow` pushed, **PR deliberately not opened** — see F1 |
| 2 | Briefs for PR C, PR A, PR B | **done** — PR [#501](https://github.com/hey-vera/heyvera/pull/501) merged |
| 3 | Implement PR C (execution sandbox) | **done** — PR [#502](https://github.com/hey-vera/heyvera/pull/502) merged |
| 4 | Promote `sandbox` to a required status check | **done** — PR [#504](https://github.com/hey-vera/heyvera/pull/504) merged |
| 5 | Implement PR A (truth model) | **done** — PR [#505](https://github.com/hey-vera/heyvera/pull/505) merged |
| 6 | Brief + implement PR C2 (scoped egress) | **done** — PRs [#506](https://github.com/hey-vera/heyvera/pull/506), [#507](https://github.com/hey-vera/heyvera/pull/507) merged |
| 7 | Handle PR #499 (28 cargo bumps) | **done** — reviewed, not merged; grouping fixed in PR [#508](https://github.com/hey-vera/heyvera/pull/508) |
| 8 | Implement PR B (durable verifier) | **done** — PR [#513](https://github.com/hey-vera/heyvera/pull/513) merged |
| **Wave 3** | | |
| 9 | Fence Soma behind a cargo feature, default off | **done** — see "Wave 3 / Task 1" below |
| 9a | Corrections to the fence: soma-core, `/api/vera/simulate`, frontend | **done** — PR [#517](https://github.com/hey-vera/heyvera/pull/517) merged |
| 10 | Derive the egress allowlist at plan time | **done** — PR [#518](https://github.com/hey-vera/heyvera/pull/518) merged |
| 11 | Restore the routing signal from the verdict | **done** — PR [#519](https://github.com/hey-vera/heyvera/pull/519) merged |
| 12 | Governance: required checks, CODEOWNERS, environments | **done** — see "Wave 3 / Task 4" below. **Two recommendations need Josh.** |
| 13 | Rebase PR #105 and report what is true | **done, not merged** — see "Wave 3 / Task 5" below. **#105 cannot be rebased; one real gap survives it.** |
| 14 | Task 6 — PR U (provenance typing) | **done** — see "Wave 3 / Task 6" below. Six of seven deliverables; the seventh needs the context wired. |
| 15 | PR R part 1 — plan-derived write sets | **done** — see "Wave 3 / Task 7" below. Step-scope leases and queueing remain. |
| 16 | PR R part 2 — step-scoped leases, queue on conflict | **done** — see "Wave 3 / Task 8" below. Hotspots/sequence resources remain. |
| 17 | PR R part 3 — hotspots, scheduler-allocated sequences | **done** — see "Wave 3 / Task 9" below. **PR R is complete.** |
| **Wave 4** | | |
| 18 | Task 1 — prove the F7 claim against the database | **done** — see "Wave 4 / Task 1" below. **Nothing has ever executed.** |
| 19 | Task 2 — provider egress from the routing decision | **done** — PR [#527](https://github.com/hey-vera/heyvera/pull/527) merged. G3 + ADR-0004. |
| 20 | Task 3 — wire the context through (F8), close PR U deliverable 7 | **done** — see "Wave 4 / Task 3" below. |
| 20a | Task 4 — one step dispatched end to end, and **F0** found | **done** — see "Wave 4 / Task 4" below. **The scheduler could never lease a step.** |
| 21 | Decision 3 — production environment reviewer | **awaiting governance decision** — the original billing failure occurred while the repository was private; it is now public, but `production` still has no required reviewer. **Needs Josh to choose/authorize the reviewer.** |
| 22 | Decision 4 — close #105, write longform by handle | #105 **closed**; the route is its own PR. |
| **Wave 5** | | |
| 23 | Task 1 - one real model invocation, end to end | **built, not run** - PRs [#530](https://github.com/hey-vera/heyvera/pull/530), [#531](https://github.com/hey-vera/heyvera/pull/531) merged. **Needs the provider key.** F9 + F10 found. |
| 24 | Task 2 - deploy production, with a worker | **deployed; execution still blocked** - production is at v66, healthy, zero rows moved. A worker service exists and cannot authenticate (**F11**), no sandbox image existed on the host, and no provider key is set. See "Task 2 outcome" below. |
| 25 | Task 3 - catalog and estimator, price list as versioned data | **done** - migration v66, `provisional` prices that quote and never charge. PR J and PR Q deliberately not built. |
| 26 | Task 4 - Phase 35 (teaching layer) written | **done** - written only, per the brief. PR AU added to the delivery list. |
| **Wave 6** | | |
| 27 | Task 2 - reconcile the two Phase 35 drafts; round 6 amendments | **done** - branch `docs/round6-amendments`. Docs only. `docs/round5-teaching` merged by hand and deleted. |
| 28 | Task 4 - one task end to end with a stubbed provider | **done, nothing merged** - branch `feat/stub-provider-e2e`. Green path proven at zero API cost. **Six findings, F13-F18.** F14 is the one that matters: a failing diff is never delivered, so `Verdict::Failed` is unreachable. |
| **2026-09-10 repair** | | |
| 29 | Fail closed on unknown frozen-exam integrity | **done** — PR [#640](https://github.com/1xmint/heyvera/pull/640) squash-merged at `ea9b562e`; see "2026-09-10 verifier integrity repair" below. |
| 30 | Private single-provider spend gateway and stubbed reservation/reconciliation | **done** — PR [#642](https://github.com/1xmint/heyvera/pull/642) squash-merged at `6bea4888`; migration v68; `$0` provider spend. |
| 31 | Deliver a scoped gateway capability and expose a stub-only Messages listener | **done** — PR [#643](https://github.com/1xmint/heyvera/pull/643) squash-merged at `61a87ecd`; `$0` provider spend. |
| 32 | Prove the real Claude CLI gateway request shape with no network | **done** — PR [#644](https://github.com/1xmint/heyvera/pull/644) squash-merged at `e01e32b0`; `$0` provider spend. |
| 33 | Make the stub listener compatible with the measured streaming shape | **done** — PR [#645](https://github.com/1xmint/heyvera/pull/645) squash-merged at `54ff6e10`; `$0` provider spend. |
| 34 | Prove the pinned CLI consumes the real Rust stub listener | **done** — PR [#646](https://github.com/1xmint/heyvera/pull/646) squash-merged at `1193c981`; `$0` provider spend. |
| 35 | Cortex extraction PR 1 — sever Socials from the Cortex ledger | **implemented locally, not pushed** — branch `fix/socials-sever-cortex-ledger`; new Pulse drafts and Socials checkout/usage fail closed with 503 while existing lifecycle remains. |
| 36 | Cortex extraction PR 2 — make route ownership executable | **implemented locally, not pushed** — stacked branch `refactor/product-route-ownership`; literal 92/98/15 ownership manifest, negative route probes, separate routers, and explicit Caddy routing. |
| 37 | Cortex extraction PR 3 — create the independent Socials database boundary | **implemented locally, not pushed** — stacked branch `refactor/socials-database-boundary`; 43-table checksummed baseline, read-only inspector/exporter, FTS rebuild, and mixed-v68 compatibility proof. |

## PR C — what landed, and what it deliberately did not

Six commits, each independently green:

1. `crates/core/src/execution_job.rs` — the full field set, version 1.
2. `crates/worker/src/sandbox/` — trait, container implementation, policy.
3. `crates/worker/src/executor.rs` — sandbox routing, fallback deleted.
4. Executor regression tests.
5. Migration v62 + `record_execution_job` + protocol field.
6. Adversarial tests against a real runtime + `Dockerfile.sandbox` + CI job.

**Two exposures closed, not one.** The known one was the worktree fallback. The
second was found during implementation: `run_required_checks` executed
`sh -lc <command>` on the host, in the worktree the agent had just written to.
A check like `npm test` runs repository-defined scripts, so a task only had to
write a file to get host execution. Checks now run in the sandbox, one per
check. This was the same invariant and the same interface, so it landed here
rather than becoming a separate PR.

**Known gaps, stated rather than papered over:**

- ~~**The wire protocol has no `blocked` state.**~~ **Closed by PR A.** The
  protocol has `StepBlocked` (version 2 to 3), and a refusal is recorded as
  `execution_failed` against Cortex rather than as the customer's step failing.
  The `BLOCKED:` prefix is gone.
- **Isolation is `Container`, not `MicroVm`.** Phase 32.4 requires kernel-level
  isolation and the container class is recorded on every job so no receipt
  overstates it. The `SandboxRunner` trait carries no container vocabulary, so
  the microVM implementation is a swap. **Phase 32.4 asks for a date on that
  commitment; the date is Josh's to set.**
- ~~**The `sandbox` CI job is not in the required-checks list.**~~ **Closed
  2026-08-09** — see "Task 4" below.
- **`run_id` on the job is empty at the worker.** The worker does not know it;
  the API supplies it when recording. Harmless today, but a `NOT NULL` column
  fed from a `resolve_run_id` that can return `None` will want revisiting when
  PR D reads these rows.
- ~~**Scoped egress is not implemented.**~~ **Closed by PR C2 (#507).** The
  adversarial test inverted as predicted: a granted registry is now reachable
  and everything else is not.

**Three things CI found that local testing could not**, worth remembering
because they are the class of bug a container unit test cannot reach:

1. bollard reports a **non-zero exit as a wait error**, so every legitimately
   failing task was being classified `SandboxUnavailable` — the exact
   Failed/Blocked confusion the design set out to prevent.
2. A bind mount **carries host ownership through unchanged**, so a sandbox
   running as a different unprivileged user could not write to its own
   workspace. Every task would have delivered nothing. The sandbox now runs as
   the workspace owner, never uid 0.
3. The allowlist path named a per-attempt Docker network that **nothing
   provisions**, so it failed at submit rather than silently opening egress.

## Task 4 — `sandbox` is now a required check

Required contexts on `main` are now:

```
heyvera, rust, cortex, npm-audit (cortex), npm-audit (heyvera), cargo-deny, sandbox
```

A check that runs but cannot block a merge is not a gate. That is the precise
shape of the fault that let seven dependency advisories accumulate before #498,
and the `sandbox` job is the only thing standing between a task and host
execution — so it had to become blocking rather than advisory.

Two properties made it safe to require rather than merely desirable:

- The job is **unconditional**. It has no `paths:` filter and no change-detection
  gate, so it reports on every pull request. A required check that sometimes
  does not report never goes green and wedges the repository — the failure mode
  that killed the old `web` job in reverse.
- It **cannot pass vacuously**. The adversarial tests skip silently when
  `CORTEX_SANDBOX_IT` is unset, so the job counts the passing tests and fails
  below ten (`.github/workflows/ci.yml:146`). A green `sandbox` therefore means
  the tests ran, not that they were skipped.

Branch protection is read-modify-write: `PUT .../branches/main/protection`
replaces the entire object, and every field omitted from the payload is reset to
its default. The current settings were read first, the one context added, and
the rest resent unchanged. Re-read afterwards to confirm the settings that were
not the point of the change survived: `required_conversation_resolution` still
enabled, `allow_force_pushes` and `allow_deletions` still disabled, `strict`
still false, no review requirement or push restriction introduced.

## PR A — what landed, and what it deliberately did not

Three commits. Migration **v63**, not the v62 the brief anticipated: PR C
landed first and took v62.

1. `crates/engine/src/captain.rs` + `docs/adr/ADR-0001-step-truth-model.md` —
   the states, and why they exist.
2. `crates/api/src/db.rs`, `storage.rs`, `ws.rs`, `scheduler.rs`,
   `verification_driver.rs`, `run_payload.rs` — migration v63, the
   transactional transitions, and the wiring.
3. `crates/core/src/protocol.rs`, `crates/worker/src/bin/worker.rs`,
   `cortex/src/**` — `StepBlocked` and the truthful panes.

**The consequential decision.** The migration rewrites every historical
`succeeded` step to `delivered`, never `verified`. Those rows were never
independently verified, so labelling them verified would assert a claim about
work already delivered to customers that we never checked. Old runs now show
fewer verified steps. Forward-only: reversing it means re-asserting a claim
that was never true.

**Known gaps, stated rather than papered over:**

- ~~**A step can strand in `verifying`.**~~ **Closed by PR B (#513).** The job
  row is inserted in the same transaction as the transition, and a dispatcher
  that dies leaves a claim that expires rather than a delivery that is lost.
- **The positive routing signal is suspended.** `record_step_completed`
  derived its outcome from the worker's exit code, which invariant 6 forbids
  from improving a score, so it no longer fires on delivery. It does not fire
  from the verdict path either: `VeraTracker` lives in `AppState` by value and
  cannot be moved into the spawned task. Dropping a metric beats crediting
  unverified work. **PR B restores it against a real verdict.**
- **The verdict emits no live event.** Same constraint — the spawned task holds
  a `Database` and nothing else, so it cannot reach the Mission Control
  channel. A pane shows `verifying` until it refreshes. Run completion is
  reconciled on the 30s scheduler tick (`reconcile_run_completion`) so a run
  whose last step verified in the background does finish.
- **`success_required` serialises more than strictly necessary.** Every
  dependent waits for `verified`, including read-only ones. The scheduler
  cannot establish that a dependent does not write, and unknown scope is never
  optimistically unblocked. Narrowing this needs write-scope on the step.
- **`inconclusive` holds its dependents indefinitely.** It is not terminal and
  not accepted, so a run with an inconclusive step waits for an operator. Its
  retry policy is PR B's.

## PR C2 — what landed, and what it deliberately did not

Three commits. Migration **v64**.

1. `crates/egress/` (the mediator binary) + `crates/worker/src/sandbox/egress.rs`
   (its lifecycle) + the policy rules that decide what gets opened.
2. `ExecutionJob.effective_egress` / `.egress_mediator` + `Dockerfile.egress`.
3. Ten adversarial tests, the CI wiring, and `docs/adr/ADR-0002-egress-mediation.md`.

**The decision.** Enforcement is network topology, not client configuration. The
sandbox joins a Docker network created `internal: true` — Docker installs no
gateway, so nothing on it can reach off the host by any route — and the mediator
is the only dual-homed member. A task that unsets `HTTPS_PROXY` and dials an
allowlisted host's IP does not get filtered; it gets no route.

`egress_a_direct_connection_bypassing_the_proxy_fails` is the test that proves
it, and it passed against a real runtime in the `sandbox` job. If it ever goes
red, the allowlist is advice.

**Known gaps, stated rather than papered over:**

- **Nothing issues a capability grant.** The planning path never decides that a
  task needs npm, so `effective_egress` records an empty set in practice. The
  enforcement is complete and currently unused. Issuing grants is a planning
  decision, not a boundary one, and it is the next thing that makes the sandbox
  useful rather than merely capable.
- **DNS resolves inside the sandbox even under `internal`.** Docker's embedded
  resolver forwards through the daemon, so a lookup can succeed where no packet
  can follow it. Harmless — resolution is not access — and it is why the tests
  assert at the TCP level rather than on `getent`.
- **Registry host sets drift.** `REGISTRIES` in `policy.rs` needs an owner and a
  review cadence; a stale entry looks to a customer like a broken build.
- **Container-specific.** The microVM phase needs its own mediator attachment,
  most likely over vsock. `SandboxRunner` still carries no container vocabulary.

## Task 7 — PR #499, reviewed rather than merged

28 cargo crates in one Dependabot group, 15 of them major. It was already red:
`bollard` 0.18 → 0.21 moved half its module tree and `crates/worker/src/sandbox/`
imports six of the moved items, so `rust` and `sandbox` both failed. The gate
worked; it was never going to merge unread.

Two findings from reading it, both recorded on the PR:

1. **`sentry` 0.35 → 0.49 pulls `sentry-actix`**, and with it `actix-web` plus
   seven more actix crates, into the lockfile of an axum product. Behind an
   optional feature so it does not compile by default — but it is in
   `Cargo.lock` and therefore in `cargo-deny`'s surface.
2. **Five RustCrypto majors** land in `soma-crypto`, whose test vectors cannot
   detect a behavioural change. See F5.

The grouping that produced it is the actual defect, and PR #508 fixes it: minor
and patch stay grouped (which is what makes them safe to automerge), majors
arrive alone. The rule was already written in `dependabot.yml` for `rusqlite`;
it now applies to every crate.

## PR B — what landed, and what it deliberately did not

Two commits. Migration **v65**.

1. `verification_jobs` + the claim/heartbeat/reclaim/retry/seal SQL, and the
   enqueue threaded through `begin_verifying_step` so it is inside PR A's
   transition transaction.
2. `crates/api/src/verification_dispatcher.rs`, the startup wiring in both
   binaries, the single-node assertion, and the metrics.

**The durability argument.** The job row is inserted inside the
`delivered -> verifying` transaction. If the transition commits the job exists;
if it rolls back neither happened. `enqueue_verification_job_in` is private and
takes a transaction for exactly that reason — a public enqueue is a way to end
up with a job without a state, or a state without a job.

**⚠️ Deployment changed.** `CORTEX_SINGLE_NODE=1` is required and the server
**exits** without it. The SQLite store is a `Mutex<Connection>` two processes do
not share, so a second dispatcher grades every delivery twice. Set in
`deploy/cortex-api.service`, `docker-compose.yml`, `docker-compose.dev.yml`, and
`.env.example`. **Anything that deploys Cortex outside those files needs it
added.**

**Known gaps, stated rather than papered over:**

- ~~**The positive routing signal is still suspended.**~~ **Closed by wave 3 /
  Task 3.** The premise was also wrong: `run_claimed_job` takes
  `state: &AppState`, so neither the `Arc` nor the scheduler round-trip was
  needed.
- **The verdict still emits no live Mission Control event**, for the same
  reason. A pane shows `verifying` until it refreshes, and run completion is
  reconciled on the 30s scheduler tick.
- **One dispatcher, one process, by assertion.** The claim CAS is written so a
  Postgres `FOR UPDATE SKIP LOCKED` port has the same semantics, but the rest of
  the write path is not yet safe for two writers. The assertion is the guard
  until the write-actor + read-pool work in ARCHITECTURE.md §13 exists.
- **Rollback requires draining.** Dropping `verification_jobs` with jobs in
  flight loses the queue.

## Wave 3 / Task 1 — the Soma fence

`docs/adr/ADR-0003-soma-feature-fence.md` carries the reasoning. What landed:
`soma` is an optional dependency of `crates/api` **and** `crates/worker`, behind
a `soma` feature that is not in `default`. `crate::soma_fence` is the single
place the two projects meet.

~~**`soma-core` is deliberately not fenced.** It is the trust arithmetic behind
`VeraTracker` — no identity, no delegation, no keys, no network — and fencing it
would delete the routing signal that Task 3 exists to restore.~~ **This
justification was false of the crate and is corrected in Task 1a below.**

**Two things found while doing it, neither of which the brief anticipated:**

1. **The worker refuses every step without a delegation.**
   `crates/worker/src/bin/worker.rs:180` fails the step `PermissionDenied` when
   `delegation` is `None`, gated on `SOMA_ENFORCE_DELEGATION` which **defaults
   to on** and is set nowhere in `deploy/`. Turning the feature off in the API
   alone would have stopped Cortex executing any work at all. The worker is
   fenced by the same feature, so the two must be built the same way; a
   mismatched pair fails loudly rather than degrading.
2. **A `{`-prefixed worker token could have reached the `Ok("local")`
   fallback.** `authenticate_worker` returns `Ok("local")` when
   `clerk_secret_key` is `None`. Had the Soma branch simply been deleted, a JSON
   token in a no-Clerk deployment would have authenticated. It is now rejected
   by format check before that path, with a test that leaves
   `clerk_secret_key` unset on purpose.

**Guarded against rot:** a `no-default-features` CI job builds and tests the
whole workspace with everything off and counts its fence assertions (floor 6),
and a step in `rust` compiles `--features soma` on both binaries. The
`no-default-features` job should be added to the required-checks list — see
wave 3 / Task 4.

**Customer-facing consequence for Josh:** `cortex/src/lib/cortexApi.ts` calls
six `/api/soma/*` endpoints that now 404 in a default build, and `/api/health`
no longer carries a `soma` block. Nothing in Cortex's execution, routing, or
billing path depends on them. **Decided and closed in Task 1a: fence the
frontend, do not opt the deploy into `--features soma`.**

## Wave 3 / Task 1a — three corrections to the fence

Three commits on `fix/cortex-owns-trust-arithmetic`. No migration.

### The fence was half a fence

`crates/api/Cargo.toml` said `soma-core` "carries no identity, delegation, keys,
or network." That is false of the crate: `crates/soma-core/src` holds
`delegation.rs`, `heart.rs`, `envelope.rs`, `trust.rs`, `death.rs`,
`pulse_tree.rs`, `room.rs`, and its manifest depends on `soma-crypto` and
`rand_core`/`getrandom`. The true claim was narrower — the path Cortex
exercised, `VeraTracker`, touched none of it.

The gap was not cosmetic. Confirmed before changing anything:

```
$ cargo tree -p cortex-api --no-default-features -i soma-crypto
soma-crypto v0.1.0
└── soma-core v0.1.0
    └── cortex-api v0.1.0
```

`soma-crypto` was linked into **every default Cortex build**, including
`--no-default-features` — the crate whose test vectors assert nothing (was F5).

Fixed by moving, not by fencing. `crates/core/src/vera.rs` is a verbatim copy of
soma-core's `trust`, `compaction`, and `vera` modules plus the three plain types
they need, so every value it computes is the value computed before. `crates/api`
declares no `soma-core` dependency. `crates/soma`, `crates/soma-core` and
`crates/soma-crypto` are unmodified — the "do not modify crates/soma\*"
instruction held; the arithmetic was copied out, not edited in place.

**The generalisable lesson, and the reason this is worth the space:** a
`#[cfg(feature = ...)]` gate fences the code it is written on and says nothing
about what is *linked*. No Rust construct can see a dependency graph, so no test
in the fence could ever have caught this. The `no-default-features` job now
asserts the graph directly — `cargo tree -p <bin> --no-default-features -i
<crate>` for all three soma crates against both binaries, where a *successful*
invocation is the failure — plus the opposite direction, so the feature cannot
silently become a no-op.

soma-core had **no tests at all** (`grep -rn "cfg(test)" crates/soma-core/src`
returns nothing), so the arithmetic behind the routing signal was entirely
unexercised. The copy lands with ten, covering what Task 3 will lean on: decay
saturates rather than underflowing on a future timestamp; a failure lowers trust
without lowering warmth; and one repeated observer cannot reach high coherence
(C=0.05 gamed vs 1.0 diverse) — the anti-gaming property that makes the
bottleneck multiplicative rather than additive.

### `/api/vera/simulate` was a denial-of-service lever, and unauthenticated

Worse than the brief recorded. It was not "any authenticated caller": the
handler took **no auth extractor at all** and sat in the public,
un-rate-limited block. `agents=1000&per_agent=100` is 100,000 synthetic
interactions, each a SHA-256 and a `format!`, run synchronously on an async
handler.

The precise mechanism, since it matters for the fix: it does not take the
database lock. It occupies a Tokio **worker thread** for the whole run, and
enough concurrent calls starve the runtime that every database handler shares —
so the effect is a stalled process, reached by thread starvation rather than by
lock contention.

Both halves, because neither alone is sufficient: the route moved to
`/api/admin/vera/simulate` behind `require_admin_middleware`, and the bound
moved *into* `simulate_ecosystem` and onto the **product** rather than each
factor — which is why 1000 and 100 each looked modest. `authorize_admin` fails
closed in production but returns `Ok(())` when no admin list is configured *and*
`CLERK_SECRET_KEY` is unset, so the internal bound is what covers a
misconfigured deployment.

Also bounded the compaction history: level-2 values were retained for the life
of the process, one per hundred interactions, and serialised in full into every
response from the public `/api/vera/network`.

### The frontend is fenced, and the deploy is not

`SOMA_API_ENABLED` (off unless `VITE_CORTEX_SOMA_ENABLED=true`) follows the
existing `MEMORY_API_ENABLED` pattern. Three live call sites go quiet: the
sign-in session POST, the spend hooks, and the "Soma spend" settings tab.
Nothing is deleted — Soma returns, and the flag turns on with the cargo feature.

**Found while doing it:** `setSomaDelegation` is guarded inside the setter, not
just at the call site, because `authedFetch` sends `Authorization: Soma <json>`
*instead of* the Clerk bearer token whenever a delegation is set. With the
backend fence up, that scheme is refused — a delegation arriving from anywhere
would not degrade Soma features, it would **unauthenticate the entire app**.
Latent today only because the session POST 404s before it can set one.

On the dropped `/api/health` soma block: no change needed and none made.
Nothing under `cortex/src` ever read a soma field off the health response.

**Verified:** `cortex-api` lib 335/335. `soma_isolation` 3/3 with default
features off. The five new `vera_surface` tests pass. `cortex-core` vera 10/10.
`cargo check -p cortex-api --features soma` still compiles. `cargo tree -i`
reports all three soma crates absent from `cortex-api` and `cortex-worker` with
default features off, and `soma` present with the feature on. `npm run build`
passes; `npm run lint` reports 11 problems, identical to an unmodified tree.

## Wave 3 / Task 2 — the egress allowlist is derived at plan time

Three commits on `feat/plan-time-egress-grants`. No migration — v64 already
added `effective_egress` and `egress_mediator`.

Closes PR C2's "enforcement complete and unused": the planning path now issues
capability grants, so `effective_egress` records real endpoints instead of an
empty set.

### The rule

`cortex_core::egress::derive_egress` requires two conditions, and **neither is
authored by the task**:

1. **The step must be able to change the tree.** Search, Think, Review and Gate
   steps never resolve a dependency. `step_changes_the_tree` is reused rather
   than re-expressed — the same predicate that decides whether verification
   attaches — because writing it twice is how the two would stop agreeing.
2. **The repository must contain the manifest.** A repo with no `Cargo.toml`
   gets no route to crates.io regardless of what the objective claims to need.

So an objective saying "install the dependencies" opens nothing; a
`package.json` opens npm.

### The registry table moved, rather than being copied

`REGISTRIES` and `expand_registry` now live in `cortex_core::egress`. Both ends
need them — the planner names the allowlist, the worker refuses any entry no
grant justifies — and two copies would agree only until one was edited. The
failure mode is silent and misdiagnosable: the sandbox opens strictly *less*
than the planner believed, so a task fails looking like a broken network rather
than like a policy mismatch. Enforcement is unchanged and now reads the same
table the planner did.

`EcosystemManifests` is deliberately **not** `check_derivation::EcosystemFacts`.
The two answer different questions — a `go.mod` justifies the module proxy and
contributes nothing to the check floor — and keeping them apart means a change
to what egress opens cannot change what verification requires, or the reverse.
`check_derivation` is untouched. It also let the manifest probe cover pypi and
go, which the check floor does not know about.

### Protocol version stays at 3

A bump is for a change an old peer cannot handle safely; this one it can.
`ExecuteStep.egress` is `serde(default)` and the default is `Deny`, so an old
worker ignoring it behaves exactly as today and a new worker against an old
brain gets the same. **Both directions of a version skew fail closed.** Bumping
would break every running worker's handshake to announce a change whose failure
mode is already closed.

### The receipt reports both halves

`granted_registries` (what the planner decided) and `endpoints` (the
intersection the sandbox enforced). If they ever disagree a reader can see it
rather than trusting they cannot. Absent means *no record*; present-but-empty
means *reached nothing* — collapsing those would let a receipt claim "no
network" for a step where we do not know.

**Verified:** api lib 337, core 94, worker 56, engine 111, context 44 — all
green, run cold after trimming 26.2 GiB of build cache. Frontend builds.

## Wave 3 / Task 3 — the routing signal is back, from the verdict

One commit on `feat/routing-signal-from-verdict`. No migration.

### Only the independent verdict rewards

`outcome_for_verdict` is the whole rule, and two of its four arms emit **nothing**:

| Verdict | Signal | Why |
|---|---|---|
| `Verified` | positive | The only source of a positive reward. |
| `Failed` | negative | Checks ran and did not pass. |
| `Inconclusive` | **none** | Our infrastructure failed, not the model. |
| `Unverified` | **none** | No ground truth existed; nothing was proven either way. |

Recording `Inconclusive` as a failure is how a routing table learns to avoid a
model for *our* outage. `None` is an answer, not a missing case.

### Spend is the whole attempt chain

A step verified on its fourth try cost four dispatches. A signal that only sees
the attempt that happened to succeed cannot tell a model that gets it right
first time from one that needs coaxing — and the second is the expensive one,
which is exactly what the signal exists to notice.

`attempt_chain_spend` counts every attempt *including one still in flight*, and
sums duration only over finished ones, so an open attempt contributes to the
count and not to the time rather than being counted as zero-length. Mass is the
attempt count — integer-denominated, per CREDITS.md — with wall time carried
alongside as evidence rather than as the unit.

### Two things the checkpoint had wrong

1. **The stated blocker was already gone.** PR B's entry says the dispatcher
   "reaches a `Database` and not `AppState`, where `VeraTracker` lives by
   value". `run_claimed_job` takes `state: &AppState` and already did. Neither
   the `Arc` nor the scheduler round-trip that entry anticipated was needed.
2. **A serde round-trip on `intent` would have silently disabled the domain
   split.** `record_decision` is called with `format!("{:?}", intent)`
   (`scheduler.rs`), so the column holds `"Refactor"` — while `Intent` carries
   `#[serde(rename_all = "snake_case")]` and accepts only `"refactor"`. Parsing
   through serde compiles, type-checks, and returns `None` for every row ever
   written, filing every verdict under "Conversation". The parse is an explicit
   match accepting either spelling, with a test that writes the `{:?}` form
   exactly as the scheduler does.

### The asymmetry in ws.rs is intended

A self-reported *failure* is still recorded; a self-reported success is not.
Invariant 6 forbids a self-report from improving a score, and nobody reports
themselves failing in order to look better. It cannot double-count either:
`completion_accepted = verified_success && step_transitioned`, so a delivery
rejected there never transitions to `verifying`, never enqueues a job, and never
produces a verdict that could record the same failure twice. Both facts are now
comments at the site rather than inferences.

**Verified:** api lib 346, core 94, worker 56, engine 111, context 44.

## Wave 3 / Task 4 — governance

### What changed on `main`

One read-modify-write on the protection object. Required contexts are now:

```
heyvera, rust, cortex, npm-audit (cortex), npm-audit (heyvera),
cargo-deny, sandbox, no-default-features
```

and `strict` is **true** (branches must be up to date before merging).

**What `strict` actually costs, measured rather than assumed.** The question was
whether GitHub's auto-merge updates a stale branch by itself. It does not. PRs
#520 and #521 were both opened from the same base; #520 merged, and #521
immediately went to `mergeStateStatus: BEHIND` with auto-merge still armed and
every check green. It sat there until the branch was rebased by hand, at which
point it re-ran checks and proceeded.

So the standing cost is: **when two PRs are open at once, the second needs a
manual rebase after the first merges.** That is a real tax on a workflow that
runs PRs back to back, and it is the argument for either merging one at a time
or adopting a merge queue. It is not a reason to turn `strict` off — the
protection it buys is that a PR green against a stale base cannot merge into a
main it was never tested against — but it should be a deliberate cost rather
than a surprise.

`no-default-features` had to become blocking for the same reason `sandbox` did
in wave 2: it is the only thing that checks the Soma fence in the configuration
the fence is *for*, and a check that cannot block a merge is not a gate. It is
also the job carrying the `cargo tree -i` graph assertion from Task 1a — the one
check that can catch a soma crate re-entering the default build, which no
`#[cfg]` gate can see.

**The read-modify-write, done the same careful way as wave 2.** `PUT
.../branches/main/protection` replaces the entire object and resets every
omitted field to its default. The live object was re-read immediately before
building the payload — not reused from an earlier snapshot, since a stale read
would silently revert whatever changed meanwhile — the two intended fields were
mutated, and every other field was resent unchanged. Re-read afterwards and
confirmed field by field:

| Field | Before | After |
|---|---|---|
| `required_status_checks.strict` | false | **true** |
| contexts | 7 | **8** |
| `required_pull_request_reviews` | null | null |
| `required_conversation_resolution` | true | true |
| `allow_force_pushes` | false | false |
| `allow_deletions` | false | false |
| `enforce_admins` | false | false |
| `required_linear_history` | false | false |
| `restrictions` | null | null |
| `block_creations` / `lock_branch` / `allow_fork_syncing` | false | false |
| `required_signatures` | false | false |

`.github/CODEOWNERS` was rewritten to paths that exist — the previous file
listed `/src/`, which has never existed in this repository. A pattern that
matches nothing reads as coverage and provides none.

### Recommendation 1 - required reviews: **do not enable yet**

Recommended against for now, and the reason is mechanical rather than
philosophical: **it would stop these PRs auto-merging.** `automerge.yml` arms
auto-merge on every PR, and a review requirement with a single human owner means
every PR waits for Josh. That converts an agent-driven workflow into a queue in
front of one person, and the predictable outcome is that the requirement gets
bypassed or removed rather than satisfied.

CODEOWNERS is now correct and ready, so enabling it later is a one-field change.

**What would make it worth enabling:** a second reviewer, or narrowing the
requirement to the paths where "looks fine" is not a sufficient review - the
sandbox, egress policy, `db.rs`, billing, and the verdict path. Classic branch
protection cannot express "reviews required, but only for these paths".
Rulesets can.

### Recommendation 2 - rulesets: **worth migrating, as its own change**

The repository is on classic branch protection. Rulesets would be better here
for three specific reasons, not as general modernisation:

1. **They remove the whole-object-replace hazard.** Every protection change so
   far has been a careful RMW where forgetting a field silently disables it.
   Ruleset rules are individually editable; that class of accident disappears.
2. **Evaluate mode answers the reviews question with data.** A ruleset can run
   in "evaluate" and report what it *would* have blocked without blocking
   anything. That turns recommendation 1 from a judgement call into an
   observation - run required-reviews in evaluate for a week and count.
3. **Path-scoped rules.** The narrowing that would make required reviews
   affordable is expressible in a ruleset and not in classic protection.

**The cost, stated honestly:** rulesets and classic protection can both be
active, and the effective policy is their union. During a migration it is easy
to believe a rule is off when it is on, or the reverse. That is why this should
be its own change with its own verification pass, not folded into a wave.

### Production is already behind an Environment - with a gap

`deploy-production.yml` already declares `environment: production`, and the
environment exists. Its only protection rule is a **branch policy**. There is no
required reviewer and no wait timer, so the environment currently constrains
*which branch* can deploy and not *whether a human agreed*.

**Recommendation:** add a required reviewer to the `production` environment.
Unlike branch-protection reviews this does not create a queue in front of the
agent workflow - it gates only `workflow_dispatch` deploys, self-approval is
permitted, and the deploy is already a deliberate manual action. Not enabled
here because it changes how deploys work, which is Josh's to decide.

### The deploy hazard recorded in wave 2 is resolved

Worth correcting explicitly, because it was recorded as blocking. PR #411
merged: the live database is no longer a tracked file, `CORTEX_DB_PATH` points
outside the repository, and the script snapshots a legacy in-tree database
before the `git reset --hard` / `git clean -fd`. That snapshot matters more than
it looks - `clean -fd` deletes *untracked* files, so untracking the database
without it would have converted a revert into a deletion. "Do not run Deploy
Production" no longer applies.

## Wave 3 / Task 5 — PR #105 cannot be rebased, and should not be

**Not merged.** Not as a judgement about the feature, but because there is
nothing mergeable there: the PR targets a repository that no longer exists.

### What it is

`feat(social): P09 usability completion — member directory, leave community,
longform by handle`. Opened **2026-05-11**, three months stale. Six files, +464
/ −47. Its merge base is `1faee091`.

At that merge base this repository was a TypeScript project with `src/`,
`web/`, `dashboard/`, `packages/`, and `internal/` at the root. **None of those
directories exist on `main` today.** The Node backend was archived to
`archive/src-nodejs/` and rewritten in Rust; the web frontend moved to
`heyvera/src/`.

### What the rebase actually does

Attempted, rather than predicted. `git rebase origin/main` on the branch:

- **The backend changes land in `archive/`.** Git's rename detection follows
  `src/` → `archive/src-nodejs/` and cleanly auto-merges
  `archive/src-nodejs/{db/index.ts, db/social.ts, routes/social.ts}`. That is
  the worst possible outcome, because it *succeeds*: the PR would merge green,
  having added three months-old features to a directory nothing compiles or
  runs.
- `tests/unit/social-p09.test.ts` conflicts as `file location`, with git
  suggesting `archive/tests-nodejs/unit/` — same problem.
- `web/src/api/social.ts` is a modify/delete conflict; the file moved to
  `heyvera/src/api/social.ts`.
- `heyvera/src/components/app/VeraSocials.tsx` is a genuine content conflict
  against three months of divergence.

A rebase is the wrong operation here. This is a port, and the backend half has
no destination — its runtime was replaced.

### What is actually still missing

Checked feature by feature against `main` rather than assumed from the title:

| Feature | Status on `main` |
|---|---|
| Member directory | **Exists.** `GET /v1/social/communities/{id}/members` → `social::list_community_members` |
| Leave community | **Exists.** `DELETE /v1/social/communities/{id}/leave` → `social::leave_community` |
| Longform by handle | **Missing.** No `/profiles/{handle}/longform`, and `get_longform` takes a `FeedQuery` of limit/cursor only — there is no author filter by any spelling. |

So two of the three shipped independently during the Rust rewrite, and one did
not.

### The one gap, and why it is worth a small change

`ProfileStats.longformCount` is rendered in two places
(`heyvera/src/components/app/VeraSocials.tsx:802` and `:2089`, both "N
longform"). A profile therefore displays a count of longform entries with no
endpoint that can list them — a number the reader cannot click through.

**Recommendation:** close #105 and open a small change against the Rust API for
`GET /v1/social/profiles/{handle}/longform`, sized to that one gap. Porting a
three-month-old TypeScript PR to recover a single missing route costs more than
writing the route, and carries three months of unreviewed divergence with it.

**Not closed here.** Closing a PR is a product call on the Socials lane, and
Socials is Josh's — this is a report, not an action.

## Wave 3 / Task 6 — PR U, provenance-typed context

Four commits on `feat/provenance-typed-context`. No migration.

### The rule, made unbypassable

Only the task contract may be read as an instruction. Everything else —
repository files, a prior step's output, summaries of either — is data, and data
phrased as a command is still data.

That cannot live in a comment, because the failure is silent: prompt text
assembled from a repository file reads exactly like prompt text assembled from
the contract, and once both are `String` nothing downstream can tell them apart.
So provenance is a **type**; every render goes through one function; and that
function matches exhaustively with no wildcard arm, so a new variant will not
compile until someone decides how it is framed.

`ContextItem` has no constructor that omits provenance, and the raw accessor is
named `raw_unframed` — deliberately awkward, so that anything bypassing `render`
catches a reviewer's eye in a diff.

### Ordering is load-bearing, not tidiness

`Provenance` derives `Ord` most-authoritative-first. `render_bundle` uses it to
put the contract before any observed content, which is what makes "your
instructions are the contract above" true rather than aspirational. `compact`
reuses the same order to drop the least authoritative first.

### Verified evidence is never dropped

Everything else can be re-derived — a repository re-read, a summary regenerated
— but a verdict is the outcome of a check that ran once, against a tree, at a
commit. When only evidence is left, `compact` returns an over-budget bundle
rather than dropping it: that is the honest failure, and the caller can see it.
Silently discarding the only independently established facts produces a bundle
that looks fine and is not.

The budget is measured on **rendered** length, matching what reaches the model.
Budgeting on raw content would undercount the framing — an off-by-a-wrapper that
only surfaces as a truncated production prompt.

### The directive scan is not the boundary

Crude lowercased substring matching, chosen over anything cleverer on purpose.
The framing in `render` is the defence and holds whether or not the scan fires;
the scan exists to make an attempt **visible**, so a miss costs an alert rather
than the protection. It skips the contract and the user's own words — both are
entitled to contain instructions, and flagging them would train whoever reads
these findings to ignore them.

### Assumptions

Every `AssumptionCheck` variant is decidable by looking at the repository, and
that is the whole constraint: an assumption whose check is "ask an agent" is not
an assumption, it is another unverified claim wearing a checked one's clothes.

`Indeterminate` is separate from `Violated` and **only `Violated` blocks** —
the same distinction the verdict model draws between `Failed` and
`Inconclusive`. Our inability to read a file is not evidence a premise is false,
and blocking on it converts an infrastructure failure into a product failure.

`paths_to_read` returns only files the assumptions name, because a plan-time
check that walked the tree would cost time proportional to repository size on
every plan.

### The artifact is typed

`Artifact` carries `Provenance::AgentOutput`. `confidence` stays for retrieval
ordering, but no longer carries the weight of "how much should this be trusted":
the 0.5/0.1 ternary encodes *the worker said it worked*, which is the
self-report invariant 6 refuses to let improve a score. **A float cannot stop a
downstream renderer treating text as an instruction; a type can.**

### What is not done, and why

Deliverable 7 — per-attempt composition **recorded** — is half done. `compose`
and `BundleRef.composition` exist and are tested, but nothing populates them:
the worker builds the `ExecutionJob` and discards the context (**F8**), so
nothing on the execution path holds both the bundle and the job. Populating it
is part of connecting the context, which this PR deliberately does not do. The
field exists so that work has somewhere to land rather than growing a new one.

**Verified:** core 120, api 346, engine 111, worker 56, context 44.

## Wave 3 / Task 7 — PR R, part 1: plan-derived write sets

One commit on `feat/plan-derived-write-sets`. No migration. **This is part of PR
R, not all of it** — see "what remains" below.

### A live bug, not just groundwork

Lease keys were built with `path.trim_matches('/')`, while the conflict checker
(`path_keys_overlap`, `db.rs:4707`) tests for a prefix followed by `/`. So
`src/` and `src/a.rs` were reported as **not** overlapping, and two steps
writing the same directory could both hold a lease. Keys now go through
`write_set::normalise`, folding `src`, `/src`, `src/`, `./src` and `/src/` to
one key.

### Canonical acquisition order

Requests are sorted before being returned. Two transactions taking the same
locks in opposite orders can deadlock. Today that is latent rather than live —
acquisition happens inside a single transaction that checks every conflict
before inserting anything — but it depends on caller assembly order, which is
not a property anyone is maintaining. Sorting makes every acquirer agree without
coordinating, and costs nothing.

This changed the order requests come back in, so one scheduler test now asserts
by content rather than position. The old assertions were encoding assembly order
as though it meant something.

### `Empty` and `Unknown` are different, deliberately

`WriteSet::Empty` means *this step writes nothing* and can run beside anything.
`WriteSet::Unknown` means *we cannot tell* and is treated as repo-wide.
Collapsing them would let a step with undeclared paths run concurrently with one
touching the same file.

Undeclared paths stay repo-wide, exactly as today: declaring nothing must not be
rewarded with more concurrency than declaring something. The behaviour only
improves for plans that declare.

**The main win** is `derive_write_set(changes_tree: false, ..) == Empty` —
read-only steps stop taking a repo-wide lease. Search/Think/Review steps are
common, and today any of them serialises every other run against the same repo
whenever the plan declares no paths.

### Overlap is not an error

`find_overlaps` returns every overlapping pair as *input to scheduling*, not as
a rejection. A plan where every step writes the same file is a perfectly good
plan that happens to have no parallelism; failing it would reject most real
work. It returns all pairs rather than the first, so a plan with four mutually
overlapping steps is reported once with four facts instead of fixed and re-run
four times.

### Parity is pinned by a test

`keys_overlap` is a copy of `path_keys_overlap`, and a test asserts they agree
across a key matrix. Two different answers to "do these paths conflict" is a
scheduler granting concurrency the conflict checker would refuse — the exact
failure PR R exists to prevent. If the db-side rule is ever edited, that test
fails.

### What remains in PR R

1. **Step-scoped leases.** `resource_leases` already has a `step_id` column and
   `holder_type`; `acquire_run_resource_leases_tx` writes `'run'` with
   `step_id NULL`. The schema supports the move; the acquisition path does not
   do it yet.
2. **Queue on conflict instead of failing.** Today a conflict returns
   `CreateRunError::ResourceConflict` at run creation. Queueing means a waiting
   state and a wake-up when the holder releases — the largest piece.
3. **Hotspot declaration and scheduler-allocated sequence resources.**

**Verified:** core 136; workspace **878 passed / 0 failed** across all targets
(`--all-targets`, matching CI — `--lib` alone does not compile
`crates/api/tests/`).

## Wave 3 / Task 8 — PR R, part 2: step-scoped leases and queueing

One commit on `feat/step-scoped-path-leases`. No migration — `resource_leases`
already had `step_id` and `holder_type`, and nothing had ever written `'step'`
into them.

### Scope moved

A run-scoped path lease is held from run creation until the run finishes, so two
runs touching the same directory serialise end to end even when only one step in
each writes there. Leases are now taken at **dispatch**, by the step, and
released when the step reaches a terminal state.

### Queueing came free, and that was the surprise

`dispatch_step` already had `DispatchOutcome::RetryLater` — the step stays
pending and is retried on the next tick — used for a missing worker and a closed
billing gate. A path conflict returns the same thing.

That is queue-on-conflict with no new waiting state, no wake-up plumbing, and
nothing extra to keep correct. The piece of PR R that looked largest turned out
to be a three-line decision once the lease call was in the right place. The
earlier estimate ("a waiting state and a wake-up when the holder releases — the
largest piece") was wrong because it assumed the scheduler had no retry loop,
and it has had one all along.

### The release rides the transition

Releasing inside the same transaction as the state change, not after it. A
separate commit could leave a path held by a finished step if the process died
between the two, and nothing would free it but the TTL. Terminal states are
`verified`/`failed`/`inconclusive`, and the release rides the CAS already
guarding the transition, so a superseded attempt cannot free a live step's paths.

### Two failure modes closed rather than open

An unopenable transaction and a failed conflict check both return **conflict**,
so the step waits rather than dispatching unleased. Failing open there would put
two steps in the same directory — the exact thing being prevented.

A step re-acquiring a path it already holds is not a conflict, or a redispatch
after a retry would deadlock the step against itself.

### What remains of PR R

Hotspot declaration and scheduler-allocated sequence resources. Nothing else.

**Verified:** workspace **887 passed / 0 failed** across all targets.

## Wave 3 / Task 9 — PR R, part 3: hotspots and sequence allocation

One commit on `feat/hotspots-and-sequence-allocation`. **PR R is complete.**

No migration: `sequence_allocations` is created by the schema block rather than
a versioned migration, so it does not touch the counter shared with Socials —
which would have been a poor irony for this particular table.

### Two problems, two answers

Phase 11.5 bundles them, and they are not the same:

- A **hotspot** is a file with real content that many changes touch. Concurrent
  edits conflict textually, and the conflict is *visible*.
- A **sequence** is a number that must be unique and increasing. Nothing is
  merged; a value is chosen. Two branches each choosing the next migration
  number both produce a syntactically clean file, the merge **succeeds**, and
  the one that lands second is silently skipped.

`HotspotKind::Counter::is_textually_mergeable() == false` is that written down.
It is the one kind a stricter lease cannot save, because the failure mode is a
clean merge rather than a conflict. This organisation has already lost time to
it on this repository.

### The allocator rests on the index, not the read

Two callers can read the same max; only one insert can commit against the unique
index over `(repo_key, sequence, value)`, and the loser retries onto the next
value. A `SELECT max` plus an unguarded insert would be the same race the
mechanism exists to remove, moved one layer down. The retry is bounded at 16 so
an unexpected constraint failure cannot spin.

`start_at` seeds an empty sequence — a repo already on v65 must be handed 66,
not 1 — and a stale `start_at` cannot rewind a sequence that has moved past it.
Allocations record the run and step that asked, so a number in a diff traces
back to the work that requested it.

### What was not built

- **Hotspots as short-duration exclusive leases.** The agent would have to take
  and release the lease mid-execution, which needs worker protocol support. The
  classification exists; the lease behaviour does not.
- **Git-history inference** of additional hotspots.
- **A repo config file format** for declaring them. `classify` already takes a
  declared list and prefers it over the well-known one, so the plumbing is
  there — what is missing is where the list comes from, and that is a format
  decision.

**Verified:** core 143; workspace **898 passed / 0 failed** across all targets.

## Wave 4 / Task 1 — nothing has ever executed

**The answer is none, and it is stronger than "none since PR C".**

Queried the live production database directly
(`/home/guardian/claw-net/.cortex/cortex.db` on `clawguard`, read-only):

| Fact | Value |
|---|---|
| Steps in a terminal state — ever | **0** |
| Steps by status | 5, all `pending` |
| `step_attempts` rows | **0** |
| `outcomes` rows | **0** |
| `verifier_reports` rows | **0** |
| Runs | 3, all stuck `running` |
| Last step activity of any kind | **2026-05-29** |
| Last worker connection | **2026-05-19** |

No step has succeeded. No step has failed. No step has been attempted.

### But this does not confirm F7's mechanism, and saying it did would be wrong

The query answers the question that was asked, and it is worth being precise
about what it does *not* establish.

**Production is running commit `3cafad6b`, deployed 2026-06-03.** Its
`schema_version` is **42**; `main` is at **v65**. So the deployed binary predates
PR C (v62), PR A (v63), PR C2 (v64) and PR B (v65). There is no
`execution_jobs` table on that database, no `verification_jobs`, no sandbox, no
egress mediator.

**Production has therefore never run the code F7 describes.** The reason nothing
has executed in production is not that the sandbox had no route to the provider —
it is that production has not been deployed in over two months and no worker has
connected since May. That is upstream of F7 entirely, and it is its own finding.

So: F7 is **unfalsified, not corroborated**. It was established by reading three
lines (`executor.rs:103`, `executor.rs:142`, `container.rs:128-135`) and it is
still correct about the code on `main`. What it lacks is evidence, and this query
is not that evidence. **Wave 4 / Task 4 is the evidence**, which is precisely why
the task exists.

This distinction is the standing correction applied to itself: the query is near
the boundary, not at it. A row count proves what the database recorded; only a
step that runs proves a step can run.

### A second finding, not in the plan

There is **no `cortex-worker` service on the production host** — only
`cortex.service`. Even with current code deployed, there would be nothing to
dispatch to. Whatever else wave 4 establishes locally, production needs a deploy
and a worker before any of it is true there.

## Wave 4 / Task 2 — provider egress, derived from the routing decision

One commit on `feat/provider-egress-grant`. **No migration** — see below.

### The shape

Two derivations, deliberately not one:

| | `derive_egress` | `derive_provider_egress` |
|---|---|---|
| Input | the repository's manifests + step kind | a `ProviderId`, and nothing else |
| Grant | `ResolveDependencies { registries }` | `ReachProvider { provider }` |
| Opens | the ecosystem's registries | exactly one host |

`derive_provider_egress` takes a `ProviderId` and no other argument. That is the
security property rather than a signature convenience: it **cannot** be
influenced by the repository, the objective, the task contract or the step kind,
because none of them is in scope. A step routed to one provider cannot reach
another, and nothing a customer can put in a repository adds a provider host.

The grant names the *provider*; `PROVIDER_ENDPOINTS` in `cortex_core::egress`
decides what that name reaches — the same rule as `REGISTRIES`, so a grant naming
an unknown provider expands to nothing rather than being trusted as a hostname.

### They meet once, at the sandbox edge

`EgressPlan::union` is the only place the two plans combine, and it combines them
as late as possible: in `build_job`, on the worker, from two fields that
travelled the wire separately. Both arrive as finished values, so the union
changes what is *reachable* and cannot change what either side *decided*.

The grants stay unflattened. That is what lets a receipt say **why** each host
was open rather than only that it was — `granted_registries` and
`granted_provider` are read back out of the same persisted list, separately.
A merged host list would have destroyed that irreversibly.

### No migration, on purpose

The migration counter is a live hazard and this change did not need to touch it.
`capability_grants` is already persisted as JSON on `execution_jobs`, and the new
grant is a new variant inside it; `effective_egress` is already a JSON array and
now includes the provider host. So the receipt gained `granted_provider` with no
schema change at all. Max on `main` stays **v65**.

### Every path that builds a sandbox, not just the scheduler

Found while wiring it: `scheduler.rs` is not the only dispatch path.
`state.rs`'s direct dispatch and `sse.rs`'s spawned execution both build a
sandbox from a routing decision, and both would have kept F7 if only the
scheduler were fixed. All three now derive the provider grant. This is the same
class of miss as the three the standing correction names — a fix that is correct
at the site it was written and absent everywhere else.

### The credential — accepted exposure, named gate

The CLI runs inside the sandbox, so the key goes inside the sandbox.
`sandbox::policy::sanctioned_env` was `Vec::new()` and unconditional; it is now
an allowlist of **exactly one variable**, chosen by the routed provider.

Three things bound it, and the first is the one that matters:

1. **The key is admitted by the same grant that opened the host.** The provider
   is read off `CapabilityGrant::ReachProvider` on the job, not from a
   configuration flag or the routing decision — so a sandbox that cannot reach a
   provider never holds a credential for one. The two facts cannot drift apart
   because they have one source.
2. One provider's variable, never two. A registry grant carries no credential.
3. An unset key is not invented. The CLI fails as unauthenticated, truthfully.

This violates Phase 32.4 and it shipped anyway, because the alternative was an
orchestrator that cannot execute a step — which is not more secure, only
untested. Recorded as gate **G3** in `ADR-0003`, with
`docs/adr/ADR-0004-provider-credential.md` stating the target and being honest
that it is not free: the mediator is CONNECT-only and never terminates TLS, so
credential injection means terminating TLS for the provider host — acquiring a
CA key, the ability to read every prompt and completion, and a second security
path for one host. That was avoided deliberately and should not be reversed to
satisfy a checklist item.

**The mitigation that applies now is Josh's:** the shortest-lived,
narrowest-scoped provider key the API supports. Egress narrowing already stops a
key being *exfiltrated* from inside the sandbox; only scope and lifetime bound it
being *used*.

### G3 is filed in ADR-0003 and does not belong to Soma

Recorded as instructed, and the section framing was corrected rather than left
implying otherwise: ADR-0003's gate list is now "Gates", with G1/G2 gating the
Soma feature and G3 explicitly gating Phase 32.4's isolation commitment. Filing a
provider-credential gate under "gates on re-enabling Soma" would have been a
tidy-looking inaccuracy.

### Verified

Workspace **920 passed / 0 failed** across all targets (`--all-targets`, matching
CI). Was 887 at the end of wave 3.

The new assertions are at the boundary rather than near it, which is the whole
point of this wave:

- `effective_egress` on a job built from a routed step contains the provider's
  endpoint and no other provider's — asserted on the value the runtime is handed.
- The container `Config.env` carries exactly the routed provider's key. The test
  **sets** the variable first, so it fails when the credential stops arriving
  rather than passing vacuously on a machine that has no key. That vacuum is the
  failure mode of the three incidents the standing correction names.
- Five new adversarial tests against a real runtime, including
  `egress_the_routed_provider_is_reachable` and
  `egress_a_step_routed_to_one_provider_cannot_reach_another`. The CI floor moved
  20 → 25, because a gated test that never runs reads like coverage.

## Wave 4 / Task 3 — the model finally sees the context

One commit on `feat/wire-step-context`. No migration.

### What was actually wrong

`worker.rs` destructured `ExecuteStep` and let `context` fall into the `..`. The
API assembled the repository map, the user's goal and every predecessor summary,
serialised them, sent them over the socket — and the worker discarded them before
building the prompt. **The model has never seen any of it.**

That is F8, and Task 1 is the reason nobody noticed: nothing has ever executed,
so nothing was ever missing anything.

### The mapping is the security decision, so it lives with the renderer

`items_from_step_context` is in `cortex_core::provenance`, not in the worker. The
choice of which provenance a field carries is not plumbing — it decides whether
a repository's text can be read as an instruction — so it sits next to `render`
where the two are read and changed together, and the worker cannot invent a
provenance without editing that module.

| `StepContext` field | Provenance | Why |
|---|---|---|
| the task contract | `Contract` | the only thing that may instruct |
| `user_goal`, `conversation_excerpt` | `UserMessage` | the human's words, not an agreed objective |
| `repo_map` | `RepositoryContent` | it came out of a tree nobody here has read; a path in it is attacker-controlled text |
| `predecessor_summaries` | `AgentOutput` | an earlier model's account of its own work |

**There is no arm producing `VerifiedEvidence`.** `StepContext` carries no
verification id, and inventing one to make a summary look authoritative would
forge exactly the claim that variant exists to protect.

`build_prompt` renders through `render_bundle`, which sorts most-authoritative
first — so "your instructions are the contract above" is a fact about the string
rather than a hope. The raw accessor stays unused on this path.

### PR U deliverable 7 is closed

`build_job` is the first point that holds both the bundle and the job, so
`BundleRef.composition` is populated there — composed over **the same items
`build_prompt` renders**. Building a second bundle for the record would produce
numbers that look like evidence and describe something else.

A bundle is recorded even when the context is empty. A step told nothing but its
contract is a fact worth having; absent would mean "no record", which is a
different claim.

### One real finding, from writing the attack the way an attacker would

The injection test used "ignore **your** previous instructions" — and it matched
none of the three existing patterns, each of which assumed the possessive was
absent. The canonical phrasing was the one getting through. Added, with the
`disregard` form.

This costs an alert rather than the defence — the framing in `render` holds
whether or not the scan fires — but it is the second time this wave that writing
the realistic case found something the plausible case did not.

**Verified:** workspace **924 passed / 0 failed** across all targets.

## Wave 4 / Task 4 — a step dispatched, and the reason none ever had

**The wave found its real showstopper here, and it was not F7.**

### F0. The scheduler leased every step to a worker that does not exist

`scheduler.rs:515` called `db.lease_step(&step.step_id, "scheduler", deadline)` —
the literal string `"scheduler"` where the worker id belongs. The worker had
already been resolved forty lines earlier and was sitting in scope.

`steps.assigned_worker` is a foreign key onto `workers(id)`. No worker is ever
called `"scheduler"`. With `PRAGMA foreign_keys = ON` — which `db.rs:5178` sets —
the statement raised `FOREIGN KEY constraint failed` on **every dispatch**.

And `lease_step` ended its `execute` with `.unwrap_or(0)`. So a schema violation
returned zero rows, which is byte-for-byte what losing the CAS to another
dispatcher looks like. The caller returned `None`, the scheduler logged

```
CAS lease failed for step <id> — skipping
```

and waited for the next reconcile tick. Then did it again. Forever.

**No step could ever be leased, so no step could ever be dispatched.** Not to a
sandbox, not to a worker, not anywhere. This sits *upstream of F7 entirely* — the
step never got far enough to discover it had no route to a model.

The whole failure was one warning per thirty seconds that named the wrong cause,
in a system where nothing was expected to be running anyway.

### Why nothing caught it

`lease_step` has direct unit tests and they pass. They pass because they call it
with `"worker-1"` after `db.register_worker("worker-1", ...)` — a registered
worker id, which is the one thing the scheduler never passed. The unit test was
correct, the function was correct, and the caller handed it a value no test ever
handed it.

That is the standing correction again, in its fifth instance: **the test was near
the boundary, not at it.** Nothing asked "can a real step actually be leased on
the real dispatch path", because nothing had ever asked a step to run.

### What changed

1. `scheduler.rs` leases to `&worker_id` — the worker the step is being
   dispatched to.
2. `lease_step` logs a database failure at `error` and says so in the message.
   A failed statement is not a lost race, and conflating them is what hid this
   for the life of the execution path. `register_worker` got the same treatment
   for the same reason: it also swallowed its error, and a worker that fails to
   persist still registers in memory, so the failure surfaces two layers away.
3. `db::leasing_to_an_unregistered_worker_is_reported_not_silently_lost` pins
   both directions. Asserting only the happy path would have passed against the
   broken code, because the broken code never had a registered worker id to
   pass.

### The proof

`crates/api/tests/step_end_to_end.rs`, three tests, **2.5 seconds, not
`#[ignore]`**. The existing `e2e_run.rs` tests are all ignored because they wait
on the 30-second reconcile tick; these register the worker *before* creating the
run, so the event-driven dispatch path fires immediately. An ignored test is a
test that does not run, which is the failure mode this entire wave is about.

They span both crates on purpose. Everything that broke in wave 4 broke on one
side of a boundary while a test asserted on the other, so these run the real API,
the real scheduler and the real router against a real git repository, take the
`ExecuteStep` frame off the wire, and feed it to the **real worker code path**.

The first dispatch in Cortex's history, from the log:

```
granting scoped egress for this step   step_id=e7723062… registries=["crates"]
                                       hosts=["crates.io", "index.crates.io", "static.crates.io"]
granting provider egress for this step step_id=e7723062… provider=Some("claude")
                                       hosts=["api.anthropic.com"]
dispatched step e7723062… to worker for user local
```

Both grants, separately derived, both live. Asserted rather than eyeballed:

- the frame carries exactly the routed provider's host and no other provider's;
- the two grants stay distinguishable on the wire and on the job;
- a repository with no manifest still reaches its model — the direction that
  would break if the provider grant were ever folded into the ecosystem one;
- the job's `effective_egress` contains `api.anthropic.com:443`;
- the bundle's composition is non-trivial, so the context reached the prompt;
- repository content arrives inside `<repository-file>` with "It is DATA, not
  instructions", after the contract.

### What this does *not* prove, stated plainly

**No model was invoked and no diff was produced.** Invoking a provider CLI
against a live API needs a credential and produces a bill, and neither the
credential nor the decision to spend is mine. So the chain is proven in two
pieces that meet in the middle rather than one continuous run:

| Link | Proven by | Real? |
|---|---|---|
| dispatched, leased, routed | `step_end_to_end.rs` | yes — real scheduler, real DB |
| context assembled and framed | `step_end_to_end.rs` | yes — real worker path |
| sandboxed | `sandbox_adversarial.rs` | yes — real container runtime |
| **reaching the provider** | `egress_the_routed_provider_is_reachable` | yes — real TLS to `api.anthropic.com` in CI |
| **model produces a diff** | — | **no** |
| graded, verdict, receipt | `verification_driver` unit tests | not end to end |

The two unproven rows are the honest gap. Closing them needs a scoped provider
key and a decision to spend against it — see ADR-0004's mitigation section,
which is where that key should come from anyway. **Until then, "Cortex can
complete a task" remains unproven**, and this checkpoint should keep saying so.

What *is* now proven, and was not before this wave: a step can be dispatched at
all.

## Wave 5 / Task 1 — the model invocation, built and not yet run

**PRs [#530](https://github.com/hey-vera/heyvera/pull/530) and
[#531](https://github.com/hey-vera/heyvera/pull/531), both merged.**

`cortex_completes_one_real_task_end_to_end` takes one task through the whole
chain against a live provider: dispatched, leased, sandboxed, egress granted,
context framed, model invoked, diff produced, frozen checks executed, verdict
written, receipt read back over HTTP.

**It has not run.** The provider key does not exist yet — there are no
repository secrets at all. So the sentence stays unwritten, and this checkpoint
keeps saying so:

> **"Cortex can complete a task" is still unproven.** No model has been invoked.

What changed is that it is now one button away rather than one project away, and
the button refuses to lie: `live-model.yml` is `workflow_dispatch`, it fails
before doing anything if `ANTHROPIC_API_KEY` is absent, and after the run it
greps the log for the skip banner and fails if it finds it.

### The subject is chosen so a pass cannot be vacuous

A zero-dependency crate whose only test fails, because `add` subtracts.

- The test asserts the required check **fails before the model runs**. Without
  that, `Verified` at the end is a statement about cargo rather than about a
  model.
- Zero dependencies means the frozen checks need no registry, so this proves the
  model path instead of measuring a `cargo fetch`. Cheapest real task that still
  has an executable ground truth.

### The gate, in both directions

| Condition | Behaviour | Verified |
|---|---|---|
| `CORTEX_LIVE_MODEL_IT` unset | skip + stderr banner saying the claim is UNPROVEN | yes, locally |
| gate set, key or image missing | **panic** | yes, locally |
| workflow run, test skipped | job **fails** on the grep | in CI when run |

### F9 and F10 — the same shape, at two more boundaries

Neither was in the plan. Both were found by asking what would actually happen
when the last two links ran, and both would have made the proof impossible.

**F9 — no verification check could ever execute.** `ecosystem:cargo-check` and
`ecosystem:cargo-test` write to `target/`. The check container mounts the tree
read-only, sets `readonly_rootfs`, passes an empty env, and had no other mount.
There was no writable path in the container, so every cargo check failed before
compiling a line.

The failure did not look like infrastructure. A non-zero exit is
`CheckOutcome::Failed`, which is a verdict **about the customer's work** — and
`Verdict::Failed` is the branch that triggers a refund. The verifier's behaviour
was to blame the customer for its own missing mount.

**F10 — the sandbox had nowhere to write either.** Worse consequence. The
worktree was the sandbox's *only* writable path, with no `HOME` and no `TMPDIR`.
A provider CLI in that container either refuses to start or writes its config and
cache **into the tree it was asked to change**, where they land in the diff, in
the git evidence, and in what the frozen checks grade.

Both fixed with a `/scratch` tmpfs and a compile-time environment constant. The
constant matters: the credential allowlist is untouched, and the tests assert the
environment is *exactly* the constant rather than *does not contain a secret* —
an absence assertion passes when somebody quietly adds a fourth variable.

**Why nothing caught either.** Every driver test uses a `ScriptedRunner`, and
reaching the real container config required a Docker daemon, so nothing ever did.
`container_config` is now a free function taking the image, which is why the four
new tests exist and why none of them is gated.

That is the standing correction in its sixth and seventh instances: **the tests
were near the boundary, not at it.**

### Known gap, not papered over

`ecosystem:npm-ci` installs into `node_modules/` *inside* the tree, so scratch
does not rescue it. Redirecting npm's prefix changes resolution semantics and a
writable tree copy would give up the read-only property. The npm floor stays
unexecutable and says so in the module docs.

### What Josh needs to do

Add `ANTHROPIC_API_KEY` as a repository secret, scoped per **ADR-0004's
mitigation**: shortest-lived, narrowest scope, spend cap on the key itself. Then
run the `live model` workflow.

## Wave 5 / Task 2 — the deploy, rehearsed and then blocked

**The rehearsal is clean. The deploy has not happened, and it must not happen
until one config line is set.**

### The rehearsal

`cargo run -p cortex-api --bin rehearse-migration -- <copy>` — a tool rather than
a one-off script, because this is not the last deploy that will be behind.

Run against a copy of the live production database, pulled with its `-wal`. The
WAL mattered: it was 127KB and modified the same day, so a copy of the `.db`
alone would have rehearsed against a tree production does not have.

```
schema before:  42
schema after:   65

GROUPED COUNTS
  runs.status/running                        3         3
  steps.status/pending                       5         5
  steps.verification_status/unverified       5         5
TABLE COUNTS      (steps, runs, step_attempts, outcomes, verifier_reports,
                   credit_*, decisions, workers, user_credentials, social_*)
                                        all unchanged

v63 — historical `succeeded` steps rewritten to `delivered`
  succeeded before: 0
  delivered after:  0
  → no rows to rewrite; this migration is a no-op on this database

RESULT: migrated 42 → 65
```

**Twenty-three migrations moved zero rows.** v63 was the wave's stated riskiest
item — a forward-only, customer-visible rewrite of history. Measured, that
history is empty: production has never had a `succeeded` step, because it has
never executed one. The risk was correct to assume and is now measured to be nil.

### The backup, verified rather than asserted

`/home/guardian/backups/pre-wave5/cortex_20260811T042450Z.db`, taken with
`sqlite3 .backup` rather than `cp`. `PRAGMA integrity_check` → `ok`; row counts
match the live database across six tables; and a copy of it was opened and
queried at a scratch path, so "it can be restored" is a thing that was done
rather than a property of the file's existence.

### Two things found on the host, and one of them stops the deploy

**`CORTEX_SINGLE_NODE` is absent from `/etc/cortex/cortex.env`.** The server
exits without it (PR #513). Deploying current code onto this host as it stands
takes production down at startup.

The repository's claim that it is "set in `deploy/cortex-api.service`" is true of
a file **this host does not use**: the live unit is
`/etc/systemd/system/cortex.service`, which predates that file and carries an
`EnvironmentFile=/etc/cortex/cortex.env` that has no such line. A setting present
in the repo and absent from the machine is the same as absent.

**There is a `cortex-worker` binary and no `cortex-worker` service.** The binary
has been at `/usr/local/bin/cortex-worker` since 2026-06-03 and nothing runs it;
`/etc/systemd/system/` holds `cortex.service` alone. `scripts/cortex-install-worker.sh`
exists and defaults `CORTEX_USER=deploy`, while this host runs Cortex as
`guardian` out of `/home/guardian/claw-net` — so it needs `CORTEX_USER=guardian`
or it installs a service pointing at a directory that does not exist.

Also corrected: the host **does** have full passwordless sudo
(`(ALL : ALL) ALL`), so the earlier note that `/etc/cortex/cortex.env` was
unreadable no longer holds.

### Where it stopped

The write to `/etc/cortex/cortex.env` was refused by this session's permission
classifier. That is a reasonable thing to gate and it was not worked around. The
deploy is blocked on exactly one line, and the order matters — this before the
deploy, not after:

```
CORTEX_SINGLE_NODE=1
```

Remaining, in order, once that lands: deploy `main`, then
`sudo CORTEX_USER=guardian bash scripts/cortex-install-worker.sh`, then report
schema version, worker connected, and whether a step can dispatch.

## Wave 5 / Task 2 outcome — production is current, and cannot execute

**Deployed.** `cbeb9c05` is live on `clawguard`, schema **42 -> 66**, verified
after the fact rather than assumed:

| Check | Result |
|---|---|
| Service | `active (running)`, 0 restarts |
| Schema version | **66** |
| `CORTEX_SINGLE_NODE` | present in the **running process**, not only in the file |
| Verification dispatcher | started (it exits the process without the flag) |
| Health | `status: ok`, database ok, not degraded |
| Data | 5 pending steps, 3 running runs, 2 social posts - unchanged |
| v63 history rewrite | 0 rows, exactly as rehearsed |
| Price list | v1 in production, 88 classes, all `provisional` |

Rollback on the host: `cortex-server.3cafad6b.bak` and
`cortex_predeploy_20260813T182044Z.db`.

Deployed **without** `scripts/deploy-cortex.sh`, deliberately. The live database
is a *tracked file* in `/home/guardian/claw-net`, and that script snapshots it
and then runs `git reset --hard` + `git clean -fd`, which restores the committed
dev snapshot over production and never restores the live one. The safe path is
`git fetch` (refs only, never touches the working tree), a separate
`git worktree` at `origin/main`, build there, `sudo cp` the binary.

### F11. There is no worker credential a service can hold

**The finding that explains why production has never had a worker**, and it is
not a configuration mistake.

`cortex-worker.service` is installed and running. It connects, registers, and is
disconnected within a millisecond, every five seconds, minting a new worker id
each time:

```
WARN cortex_api::ws: worker auth failed: empty auth token
WARN cortex_api::ws: worker w-9325911b-67c: registration failed, closing connection
```

The worker reads its token from `CORTEX_TOKEN`.
`scripts/cortex-install-worker.sh` writes a `worker.env` containing
`CORTEX_BRAIN_URL`, `SOMA_ENFORCE_DELEGATION` and `RUST_LOG` - and **no token at
all**. So the worker sends `token: ""`.

Setting the variable does not fix it, which is the actual finding.
`authenticate_worker` (`ws.rs:1387`) accepts exactly three things:

1. an **empty** token - accepted only when `clerk_secret_key.is_none()`, i.e.
   when authentication is switched off entirely. Production has Clerk
   configured, so this is refused;
2. a token starting with `{` - a **Soma delegation**, and the `soma` feature is
   fenced off by ADR-0003 and must not be enabled;
3. anything else - a **Clerk JWT**, a short-lived end-user session token.

A systemd service cannot hold any of them. It cannot hold a JWT that expires in
about a minute, and the worker has no refresh path - there is no `refresh` or
`expire` handling anywhere in `crates/worker/src/bin/worker.rs`. Nothing
consults `user_api_keys` either.

**So Cortex has no service-identity credential, and the worker path has never
been exercised against an authenticated API.** Every local test registers with
an empty token against an `AppState` built with `None` for the Clerk secret -
the one configuration in which the empty token is accepted. The tests were near
the boundary, not at it, for the eighth time.

Closing it needs a design decision, not a config change: a worker service token
with its own issuance, scope and revocation. It grants the right to execute
steps, so it is a security decision and it is deliberately not improvised here.

**Do not "fix" this by unsetting `CLERK_SECRET_KEY`.** That would make the empty
token authenticate as user `local` and would disable authentication for the
entire production API.

### F12. The provider CLI will not start without a writable HOME

Found by building the sandbox image on the host, and it confirms F10 against the
real CLI rather than by reasoning:

```
EACCES: mkdir '/home/sandbox/.claude/debug'
```

`claude --version` fails - not a task, the version flag - because the sandbox
user is created `--no-create-home`. It also exposed a flaw in F10's own fix:
`SCRATCH_ENV` pointed `HOME` at `/scratch/home`, and the tmpfs is mounted *over*
`/scratch` at start, hiding anything the image created there. Every value now
points at the mount point itself, which always exists, is mode 1777, and needs
nothing to have gone right beforehand.

The build-time smoke test in `Dockerfile.sandbox-provider` now runs with the
exact environment the runner supplies, because a smoke test run under different
variables tests a configuration that never happens.

### F9, F10 and F12 proven against a real runtime, not against a config struct

The unit tests assert what `container_config` *returns*. That is the right test
and it is not proof, because the whole class of bug being fixed is "the runtime
does something the configuration did not imply". So all three were run on the
production host, in the exact configuration the code builds at dispatch:
`--user sandbox --read-only --network none --cap-drop ALL
--security-opt no-new-privileges:true`, a `/scratch` tmpfs with the real mount
options, and the real `SCRATCH_ENV` values.

**The sandbox image starts its agent** — F10 and F12:

```
$ docker run ... cortex/sandbox:dev claude --version
2.0.14 (Claude Code)
exit=0
```

Before the fix this same command failed with
`EACCES: mkdir '/home/sandbox/.claude/debug'`.

**The check runner executes a real check against a read-only tree** — F9, the
direct proof:

```
$ docker run ... -v $TREE:/work:ro cortex/runner:phase-a cargo check --locked
    Checking subject v0.1.0 (/work)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.21s
```

And the property the read-only mount exists to protect survived it — the tree
afterwards contains `Cargo.lock`, `Cargo.toml`, `src` and **no `target/`**. The
build output went to the tmpfs and died with the container, so a check still
cannot edit its way to green.

That is the first time in this project's history that a frozen check has been
executed by the real runner against a real tree.

All three images are now on the host under the names the code defaults to, so
no environment variable is needed: `cortex/sandbox:dev` (723MB),
`cortex/egress:phase-a` (1.35MB), `cortex/runner:phase-a` (1.36GB).

### What is true about production now

- The API is current, healthy, and at v66.
- A worker service exists and **cannot authenticate** (F11).
- The sandbox images did not exist on the host at all. All three are now built
  and **verified running**, under the names the code defaults to.
- `claude` 2.1.143 and `codex` 0.131.0 are installed on the host.
- **No provider API key is set anywhere.** Even with F11 closed, a dispatched
  step would sandbox, reach the provider host, and fail unauthenticated.

So: **a step still cannot execute in production**, and the reason is now three
named, specific things rather than "no worker service". That is the honest
answer to Task 2's item 5.

## Wave 5 / Task 3 — the price list, as versioned data

The reason `quoted_credits` has been `None` everywhere was never the charging
code. It was that there was no price that was not invented, and inventing one is
never right.

### Three properties, each checkable

1. **It is data, not code.** `usage::model_rates` matches on substrings of model
   ids (`m.contains("haiku")`) — invariant 11's exact prohibition, pricing code
   carrying hardcoded model names. Those rates are now rows in
   `price_list_models`.
2. **It is versioned and immutable.** Invariant 23, enforced by triggers in
   migration v66 rather than asserted in a comment. An `UPDATE` or `DELETE` on a
   published price is an error at the storage layer; republishing a version
   number fails rather than overwriting. A test fires each trigger.
3. **It is labelled with how much it is worth trusting.** Every seeded class is
   `provisional` with `sample_count = 0`, and a provisional class **quotes but
   does not charge**.

### The graduation gate is the design, not a footnote

| Status | Quoted | Charged |
|---|---|---|
| `provisional` | yes | **no** |
| `committed` | yes | yes |

So `quoted_credits` is unblocked *and* billing is not silently switched on.
Moving a class to `committed` is a deliberate, recorded, commercial act — Phase
31.3's gate, expressed as the only difference between two statuses.
`StepQuote::billable_credits()` is the single accessor the driver uses, so
reading the raw number and charging for a provisional class is not something a
call site can do by accident.

### `TaskClass` — and why tier is not one of its axes

`work_kind : risk : verifiable`, in `cortex_core::task_class`. 88 classes.

Tier is deliberately excluded even though it is available. Tier is a *routing
decision*, and folding it into the price key would make the customer's price
depend on Cortex's own choice of model — a router that can raise the price by
routing badly. That is invariant 11's failure one level up and it would make
invariant 12 unenforceable.

`verifiable` is an axis because invariant 22 says work with no executable ground
truth is never priced as though it had been proven. Keeping it in the key is what
makes that a price rather than a footnote — and it is taken from the frozen check
set at dispatch, not from an intention.

### The quote is frozen at dispatch, and a retry does not re-price

Frozen beside the check specs, for the same reason: a price resolved at verdict
time is a price the work could have influenced. `UNIQUE(run_id, step_id)` rather
than per attempt — Cortex absorbing the cost of its own second attempt is the
entire content of an outcome guarantee, and re-quoting on retry would bill the
customer for Cortex having been wrong the first time.

`billable` is stored, never recomputed: a class that graduates between dispatch
and verdict must not retroactively make a step billable that the customer was
told was free.

### One finding, from the tests rather than from review

The first seeded list valued a credit at one dollar, and **every modelled class
collapsed to exactly one credit** — a trivial gate and a critical refactor priced
identically. Every row still held a plausible number, so the list looked fine and
had simply stopped being a price list.

`micros_per_credit` is now a published field of the list rather than a constant,
for the invariant-11 reason (it is a fact, so it lives in the one table) and for
this one (it is the resolution of the whole price space).
`the_price_list_actually_distinguishes_classes` asserts over the whole class
space rather than over two rows, because two rows is what the previous test
checked and it did not notice.

**The commercial value of a credit is not set here.** The seed is chosen so the
modelled spread survives rounding, the list is `provisional` so it cannot charge,
and what a credit is worth is Josh's.

### What this deliberately is not

PR I only. **PR J (cost-aware routing) and PR Q (run forecast) are not built** —
both need this table and neither is here. The seeded per-class spend is *modelled*
from a per-`WorkKind` token profile, a risk multiplier and an unverifiable
discount; it is not measured, `sample_count` is 0 on all 88 rows, and that is
exactly what keeps every class provisional.

## Wave 5 / Task 4 — Phase 35 written, not implemented

`HARNESS-EXCELLENCE-PLAN-2026-08.md` gains **Phase 35 — the teaching layer,
derived rather than narrated**, plus PR AU in the delivery list. Written only;
no code.

The thesis: the explanation is derived from the decision record, never inferred
by a model watching another model. The argument is not that models confabulate —
it is *who the feature is for*. A narration layer's errors are invisible to
precisely the audience that cannot check them, so a user who could catch the
mistake did not need the narrator, and a user who could not is miscalibrated with
the full authority of the system that just did the work.

Covered: what a teaching artifact may assert (a two-column table, and the
"may never" column matters more), how it degrades when the record is thin
(shorter, never vaguer — "not recorded" and "did not happen" are different
sentences), its disclosure tier under invariant 31 (per-section, and *nothing*
at operator tier — a teaching artifact must never be why a mechanism is allowed
to stay complicated), and why it is not a chat surface (a chat box promises
answers the record cannot give, and hands the burden of knowing what to ask back
to the person who could not evaluate the plan).

Explicitly rejected with reasons: a local vector index (Phase 29.2 settled it on
measured grounds, and the teaching case is *weaker* than the exploration case
already rejected — similarity search over a structure you hold the schema of is a
worse index than the schema), and a model-inferred rationale stream. The
distinction drawn against Phases 27 and 30: an inspecting judge produces
*recorded, gradeable* evidence; a rationale stream produces prose that bypasses
the record and goes straight to a user.

Scheduled last deliberately. 35.7 lists what has to exist first; built early it
would render mostly "not recorded", and a feature judged useless is a feature
somebody later fills in with a model.

**Superseded in structure by wave 6 / task 2.** This section describes the draft
that landed on `main`. A second, different Phase 35 existed on
`docs/round5-teaching` at the same time; the two were hand-merged in wave 6 and
renumbered, so the section numbers quoted above no longer resolve — "what has to
exist first" is now 35.14. The arguments recorded here all survive the merge.

## Wave 6 / Task 2 — one Phase 35, and the evidence graded

Branch `docs/round6-amendments`, off `3585c711`. **Docs only — nothing under
`crates/` was touched.** Ran in parallel with `fix/worker-service-credential`;
the file sets are disjoint.

### The reconciliation, and why it was not a rebase

`main` and `docs/round5-teaching` each grew a **different Phase 35** from the
same merge base (`a466bb1f`), using the same section numbers for different
arguments. A rebase would have conflicted on every line of the phase and
resolved to whichever draft the resolver liked. It was hand-merged instead:

- **main's draft was the architectural spine** — the derived-never-narrated
  thesis, the may-assert / may-never-assert tables, thin-record degradation,
  disclosure tiers, why it is not a chat surface.
- **the branch held the evidence and the product shape** — the RCT, the
  pull-not-push rule, the surfaces ranked by evidence, the expertise split, the
  interruption cost, the cold start, how it dies.

Neither was discarded. The merged phase runs 35.1–35.17 and
`docs/round5-teaching` is **deleted, local and remote** (no PR existed on it), so
there is one draft rather than two. `ROUND5-STATE.md` was ported unchanged — it
is the round-5 resume file and it records reasoning the merged phase does not
restate.

**One error corrected while merging.** The branch claimed `--lib` "compiles and
passes while running none of them". It does run every unit test in
`crates/api/src/` — hundreds. What it never does is *compile* `crates/api/tests/`,
so the integration suite where every cross-crate finding in waves 4 and 5 lived
is silently absent from a green result. Both places in the plan that recommend a
test command now say `--all-targets` and say why.

### What round 6 actually attacked

Not a new axis. **Evidence quality** — whether the numbers the plan leans on say
what the plan says they say. Several did not, and two of those would have
mispriced the product.

**The capability thesis is bounded harder than it was.**

| Was | Is |
|---|---|
| "N samples from different providers correlate far less" | Cross-vendor decorrelates *measurably more* than temperature sampling, but frontier models show **high** correlation across providers and architectures, and correlation **rises with capability** — ~60% agreement when both err (Correlated Errors in LLMs, ICML 2025, [arXiv:2506.07962](https://arxiv.org/abs/2506.07962), n > 350) |
| `1 − (1 − a)^N` as the sampling gain | An **upper bound Cortex will not reach**, recorded as one on the receipt and in the estimator |
| A race arm is a model | A race arm is `(model_ref, method_ref, context_rendering_ref)`; complementarity is measured over the **tuple**, and a race varying only `model_ref` is recorded `single_axis_diversity` and may not claim pass@N above single-arm |

The `ultra` position gains a specific requirement: **at least one arm re-derives
its context from the `TaskFrame`** rather than inheriting the shared rendering.
Every arm anchored to one context inherits that context's errors — the
shared-anchor term — and one re-deriving arm is the only sample of it Cortex will
ever get. **Context diversity is unmeasured in the literature for code**, so this
is also the cheapest original result available: a Phase 30 suite configuration,
not a research programme.

**The published negative result now has an answer on the page.** Mixed-model MoA
underperforms self-MoA by 6.6% ([arXiv:2502.00674](https://arxiv.org/abs/2502.00674)).
It does not bind because MoA aggregates by **synthesis** — a weak candidate
contaminates the output — while Cortex aggregates by **execution**, where a weak
candidate fails its checks and is discarded. The quality–diversity tradeoff does
not bind on a selector. That makes Phase 12.10's "aggregation is execution, not
voting" load-bearing rather than stylistic: **if aggregation ever drifts toward
synthesis, the 6.6% result starts applying in full.**

**The existence proof was missing and is now in 28.2.** CodeMonkeys
([arXiv:2501.14723](https://arxiv.org/abs/2501.14723)) pooled candidate edits
across five agent systems: ensemble selection **66.2%** against a best individual
member of **62.8%** — combining models beat the best model. The third number is
the product thesis: **pooled coverage 80.8%**, so roughly **14 points** of correct
answers were generated and thrown away because the selector could not identify
them. The pool is cheap; the selector is the moat.

### The finding that costs the most to have missed

**Invariant 34 — an evaluation environment is sealed, and the seal is recorded
with the result.** Phase 30.2's post-cutoff restriction closes **training**
contamination and nothing else, and 30.2's own construction — tasks *are* merged
PRs — puts the reference solution in the repository's own git history, one
`git log` from the agent being graded.

Cursor's June 2026 measurement, on sealing a previously-open environment:
**87.1% → 73.0%** and **74.7% → 54.0%**, with 57% of runs performing an upstream
lookup, 9% mining git history, and **63% of resolutions retrieved rather than
derived**. A twenty-point swing is larger than every capability effect this plan
proposes to measure. An unsealed run is not a noisy measurement of capability; it
measures a different quantity, and may not be compared to a sealed one.

**PR C2's egress allowlist already closes the web-lookup channel** — the 57% row
— in shipped code, derived at plan time and enforced at the sandbox boundary. It
was built for tenancy and safety and has never been claimed as an evaluation
asset. What remains is git truncation (at the merge base, with no reachable
object after it — reflogs, remote refs, packed objects, tags and sibling branches
all count) and the retrieval-leak probe.

### Three smaller corrections

- **28.5's falsification test was not a valid experiment.** The baseline is now
  each single model dropped into **Cortex's own scaffold**, cost-matched, sealed,
  post-cutoff. A vendor's published score measures the model *and* the scaffold
  its vendor built around it, and the same family shows a ~17-point spread
  between vendor scaffold and standardised harness — larger than the effect being
  claimed. A **human-acceptance arm** is added: METR (March 2026) found automated
  grading overstates merge-worthiness by **24.2 points**, so a battery-only number
  overstates the deliverable by a known margin.
- **PR AM's budget has a direction now.** Execution-grounded selection beats
  output-pattern voting by **19–52 points**; aggregation-rule variants sit within
  **±0.79pp** of each other (p > 0.05); sketch-based input construction beats
  random fuzzing by **11.3pp**. Effort goes into discriminating inputs, not into
  clever voting math — and the same machinery that builds a discriminating check
  for Phase 27.5's differential control builds a discriminating input for
  selection.
- **Phase 31's loss-ratio prior is seeded rather than guessed.** 15.7% of patches
  passing SWE-bench Verified were erroneous under augmented tests (UTBoost,
  [arXiv:2506.09289](https://arxiv.org/abs/2506.09289)); **77%** of instances
  retain a surviving mutant ([arXiv:2603.00520](https://arxiv.org/abs/2603.00520))
  — which is `battery_power` measured on someone else's corpus and coming back
  weak; ~50% of test-passing PRs were rejected by maintainers (METR). **Cortex's
  own false-accept rate is its gross margin** and must be measured before a
  guarantee is priced.

### What changed in Phase 35 beyond the merge

The teaching amendments were applied *during* the reconciliation rather than
after it, so the merged phase never carried the uncorrected claims:

- **The headline number is now the preregistered one** — 50% vs 67%, d = 0.738,
  p = 0.01, n = 52, widest gap in debugging, ~2 min time difference not
  significant — replacing the looser "17% lower".
- **The population is corrected.** Table 1 says a majority had **7+ years**;
  "mostly junior" does not survive it. The effect is about **domain novelty, not
  seniority** — the situation of every senior touching an unfamiliar library,
  which is a much larger market. Conceptual Inquiry was also the second-fastest
  pattern overall, so the learning-vs-speed tradeoff may be illusory (flagged
  n = 7, exploratory).
- **The mode taxonomy is a hypothesis, never a finding.**
  [arXiv:2601.20245](https://arxiv.org/abs/2601.20245) is exploratory, n = 2–7 per
  pattern, self-selected and therefore confounded with prior skill.
- **35.1 leads with the perception gap**, from four independent designs — the AI
  arm rating the task easier; METR's +20% perceived against −19% measured; Perry
  et al. CCS '23 ([arXiv:2211.03622](https://arxiv.org/abs/2211.03622)); Lee et al.
  CHI 2025. **METR's February 2026 update qualifies the −19% and it must not be
  cited as a standing fact.**
- **The thesis gained two structural legs.** Rudin
  ([arXiv:1811.10154](https://arxiv.org/abs/1811.10154)): a post-hoc explanation
  cannot be perfectly faithful by construction — if it were, it would *be* the
  model. Turpin ([arXiv:2305.04388](https://arxiv.org/abs/2305.04388)): CoT
  explanations systematically misrepresent the true reason, omit the bias that
  changed the answer, accuracy drops up to 36%, and they **increase** trust. The
  audience argument stays alongside as leg one.
- **The verifier is the teacher.** Rustlings is the compiler; Gossip Glomers is
  Maelstrom injecting partitions; Exercism is test runners with humans optional
  (19,603 mentors, ~0.65% of 61M submissions); protohackers is a protocol checker.
  Cortex already owns a frozen battery and a sandbox, so **Phase 35 is a rendering
  of the core asset, not an addition to it.**
- **Two surfaces added.** Contrastive cross-language mapping under five required
  conditions — semantic not syntactic, break point in the same sentence as the
  analogy, ≥2 analogies, falsifiable against a runnable program, anchored to the
  diff in front of the user. And **spaced retrieval over the user's own
  receipts**, which **reverses round 5's flashcard cut** and says so: 26,258
  practising physicians (58.0% vs 43.2% learning, 58.3% vs 52.4% transfer, both
  p < .001) and a 2026 meta-analysis at SMD 0.78, n = 21,415. Every question asks
  **why** a check failed, never whether it passed — Brown's IRT analysis, and the
  reason Rustlings' make-it-compile format is the low-discrimination kind.
- **Practice rooms rejected** on deliberate practice explaining <1% of variance in
  professions (Macnamara 2014), transfer being predicted by work-environment
  support (Blume 2010, 89 studies), and single-digit completion for optional dev
  curricula (~5% AoC day1→day25; 2% of 62,526 Brown Rust Book readers reached
  ch19). **"10% of training transfers", "learning in the flow of work" and
  70-20-10 are named as folklore and banned from Cortex material.** Faded
  exercises anchored to the user's own diff are explicitly *not* rejected.
- **The teaching layer is never a priced line item** — Pluralsight (~$3.5–3.9B,
  equity to zero by 2024), Katacoda (shut down 2022), Replit Teams for Education
  (killed at 18 months). A metered faded exercise bills as **sandbox execution**,
  not as tuition.

### What this task deliberately did not do

- **No Rust, no schema, no migration.** Nothing here consumes a migration number,
  so the shared counter is untouched at v66.
- **PR AU is not implemented.** Phase 35 remains written-only, and 35.14 lists
  what has to exist first.
- **Invariant 34 is not implemented.** The seal is specified; git truncation and
  the retrieval-leak probe are Phase 30 work under PR AP.
- **No round-6 state file.** Round 5 kept `ROUND5-STATE.md` because it was an
  open review; round 6 landed in one pass and this section is its record.

### For round 7

The plan's own closing section now says it: round 5 opened the human axis and did
not finish it. **Round 7 should attack what Cortex does to a *team's* practice
over months, rather than to one developer inside one task** — and it should keep
round 6's habit of grading the evidence as hard as the argument.

## Wave 4 / governance — decision 3 is blocked, and not by us

**Required reviewer on the `production` environment could not be enabled.**

```
PUT /repos/hey-vera/heyvera/environments/production
→ HTTP 422: "Failed to create the environment protection rule. Please ensure
   the billing plan supports the required reviewers protection rule."
```

The repository is **private** and the `hey-vera` org is on the **team** plan.
Deployment protection rules on private repositories need a higher plan.

Verified afterwards that the failed `PUT` changed nothing: `protection_rules` is
still `[branch_policy]` and the branch policy is still `main`. The call is
atomic, so there is no half-applied state to clean up.

**Needs Josh**, and it is a billing decision rather than a technical one: an
Enterprise plan, or making the repository public. Neither is worth doing *for
this* — the value on offer is a self-approvable confirmation on a
`workflow_dispatch` deploy. Recorded so it is not silently dropped, and so the
next person does not spend the same twenty minutes discovering the same 422.

Decisions 1 (required reviews: not yet) and 2 (rulesets: own pass, evaluate
mode) are unchanged and remain queued behind this wave.

## Next — wave 2 is complete

Every task in the wave-2 handoff has landed. The plan's delivery order from
here:

- **PR I, J, Q** — the catalog, the cost objective, the forecast. The whole
  economic claim, and the reason `quoted_credits` is still `None` everywhere.
- **PR R** — step-level leases. Cheap, and it unlocks the speed dial.
- **PR U** — provenance typing. The plan's highest consequence-to-effort item
  and the prerequisite for pointing Cortex at unvetted repositories.

Two things worth doing before or alongside those, both found during this wave:

- ~~**Issue capability grants** (see PR C2's gaps). The egress enforcement is
  complete and unused until the planning path decides a task needs npm.~~
  **Closed by wave 3 / Task 2.**
- ~~**Fix the soma-crypto vectors** (F5).~~ Out of Cortex's lane since Task 1a
  removed soma-crypto from the dependency graph. Now gate **G2** in ADR-0003.
  Still blocks the five RustCrypto majors in #499.

Briefs live in `cortex/plan/briefs/`. An implementer reads only the brief.

## Migration numbers — the shared-counter hazard

`schema_version` is one counter shared with the HeyVera Socials product. The
hazard is real and it bit three times this wave: PR C took **v62**, PR A took
**v63** against a brief that said v62, PR C2 took **v64**, and PR B took
**v65**. **Re-check the maximum before claiming a number** — a collision means
whichever branch merges second has its migration silently skipped.

Wave 5's Task 3 takes **v66** (the price list catalog), re-checked against `main`
at rebase time rather than at design time, which is the whole discipline. Max on
`main` is therefore **v66** once that merges.

## What was verified directly (not inherited)

Re-checked against the tree at `c8ca2941`, per the plan's "Verify before you
change" rule:

- `crates/worker/src/executor.rs:49-70` — worktree isolation is **best-effort**
  and falls back to the caller's `working_dir` on any worktree failure. Confirmed
  verbatim; this is the live safety exposure PR C removes.
- `crates/worker/src/executor.rs:74` — `Command::new(&cmd)` spawns the provider
  CLI as a **host process** with the worker's inherited environment.
- `crates/worker/src/executor.rs:520` — `ProviderId::Gemini => Ok(("gemini", vec![]))`
  drops the routed model. The plan's claim holds at the stated line.
- Migration counter: latest is `migrate_v61` (`crates/api/src/db.rs:3445`); the
  fresh-database assertion at `db.rs:25375` requires `>= 61`. **Next free number
  is v62.**
- Branch protection required checks: `heyvera`, `rust`, `cortex`,
  `npm-audit (cortex)`, `npm-audit (heyvera)`, `cargo-deny` — and, since
  2026-08-09, `sandbox`. Auto-merge is armed on every PR by
  `.github/workflows/automerge.yml`; the required checks are the only gate.

## Findings — plan/repo inconsistencies

Recorded per the handoff rule (record and keep going; do not redesign mid-flight).

### F1. The plan branch was not docs-only *(action taken, needs Josh)*

`origin/docs/concurrency-assessment` carried one substantive code commit,
`865dbfae fix(auth): remove the provider-subscription credential flow`, mixed in
among sixteen docs commits. It removes the backend subscription auth flow, the
container CLI login path, `PendingContainerAuth`, `start_login_exec`,
`complete_login_exec`, `AppState::pending_container_auths`, and rewrites
onboarding's `ProviderStep.tsx`.

It was **split out of the docs PR** onto `fix/auth-remove-subscription-flow` and
pushed. Its PR was **not** opened, because opening a PR arms auto-merge and this
is a customer-facing onboarding change. Awaiting Josh.

### F2. `CONCURRENCY-ASSESSMENT.md` was already on main

The handoff states the plan docs exist only on `docs/concurrency-assessment`.
`cortex/plan/CONCURRENCY-ASSESSMENT.md` was in fact already on `main` with
byte-identical content, so commit `d437ff5d` was dropped during the rebase.
Cosmetic; noted so the next reader is not confused by a 16-commit branch
producing 15 commits.

### F8. The worker discards the assembled context *(closed by wave 4 / Task 3)*

**Closed.** The context is wired through, rendered only by
`cortex_core::provenance`, and `BundleRef.composition` is populated per attempt.
The precondition held: PR U (#523) landed before the field was connected, so
repository content reached a model already typed as data.

The last paragraph below asked whether the context *should* be wired through,
noting it had been dead long enough that nobody noticed. Task 1 answered that:
it was not dead because it was unwanted, it was dead because nothing has ever
run.



Found while grounding PR U. `crates/worker/src/bin/worker.rs:143-145`
destructures `ExecuteStep` as `{ step_id, attempt_id, lease_gen, task, decision,
delegation, egress, .. }` — `context` falls into the `..`. There is no `context`
identifier anywhere in `crates/worker/src/`, `build_task_prompt`
(`executor.rs:684`) builds from `TaskContract` alone, and `TaskContract`
(`crates/core/src/task.rs:9`) has no context field.

So the API assembles context — `ContextBus`, the repo map, predecessor summaries
— serialises it, sends it over the socket, and the worker throws it away. **The
model never sees any of it.** The whole context-flow investment is inert on the
execution path.

Two consequences:

1. **The injection hole PR U is meant to close is not open through this path
   yet.** The plan's launch gate (line 6698) says repository content is an
   unreviewed injection surface. That is right about what will happen and not
   about what happens today, because the field carrying repository content is
   dropped before the prompt is built.
2. **It makes PR U cheaper and more urgent at once.** Cheaper: there is no
   rendering path to retrofit, so the typing can be designed in before anything
   consumes it. More urgent: the moment someone wires `context` through — a
   one-line change to that destructure, which looks like a bug fix — the hole
   opens with no typing in place.

**PR U must land before the context is connected, not after.** Recorded in
`cortex/plan/briefs/PR-U-provenance-typing.md` so an implementer does not read
the disconnected field as an oversight to fix in passing.

Separately: whether the context *should* be wired through is a real product
question this does not answer. It has been dead for long enough that nobody
noticed, which is its own signal about how much the current prompt depends on it.

### F7. The sandboxed provider CLI has no route to the model API *(closed by wave 4 / Task 2)*

**Closed.** The provider grant is derived from the routing decision at dispatch
and enforced at the sandbox edge; the first of the two shapes below was taken.
The credential half is not closed and is not pretending to be — it is gate **G3**
in ADR-0003 with a target in ADR-0004.

Worth keeping the original text because of the last paragraph: it asked whether
any step had in fact executed since PR C merged, and the answer turned out to be
"none, ever" — see wave 4 / Task 1, including why that answer does *not*
corroborate the mechanism described here.



Found while grounding Task 2. Not fixed there, and Task 2 does not fix it —
registries are not provider endpoints.

The chain, each link verified at the stated line:

- `executor.rs:103` builds the `SandboxRequest` from `invocation.program` — the
  provider CLI itself — so the agent runs **inside** the sandbox.
- `executor.rs:142` uses `ContainerSandbox` unconditionally. PR C deleted the
  host fallback deliberately; there is no other runner in production.
- `container.rs:128-135`: with no grants the config is `network_mode: "none"`
  and `network_disabled: true`.

So a step dispatched to a sandbox with no egress grant runs an agent CLI that
cannot reach `api.anthropic.com`. Task 2 grants registries when the repo has a
manifest, which does not help: a cargo grant opens crates.io, not the provider.

**Why this is not fixed here.** Opening egress to provider endpoints is a
security-boundary decision, not one derivable from a repository's manifests —
it is the same class of judgement as the isolation-class date in Phase 32.4.
Guessing it inside a task about ecosystem-derived allowlists would put a
permanent hole in the boundary as a side effect of a feature. **Josh's call.**

The plausible shapes, for when it is taken: a provider grant issued at plan time
from the routing decision (the planner already knows which provider was
chosen), or the CLI moved out of the sandbox with only its filesystem effects
sandboxed — which is a different architecture and would need its own ADR.

Worth checking before acting: whether any step has in fact executed
successfully since PR C merged. If the answer is no, this is why, and it means
the sandbox path has never run a real step end to end.

### F5. The soma-crypto test vectors do not test anything *(reclassified — now gate G2)*

**No longer a Cortex to-do.** Since Task 1a, `soma-crypto` is not in any Cortex
binary's dependency graph, so this cannot affect a Cortex build. It is recorded
as **G2 in ADR-0003's "Gates on ever re-enabling the feature"** — a condition on
turning `--features soma` back on, not a task in anyone's queue. It remains a
prerequisite for the five RustCrypto majors in #499, which are soma-crypto's
bumps.

Kept below in full because the reasoning is the evidence for the gate.

Upgraded from F4, which recorded only the symptom.

`export_test_vectors` in `crates/soma-crypto/src/{aead,composite,hash,pulse}.rs`
generates a **fresh random keypair**, signs, and writes the result to
`crates/soma-crypto/test-vectors/*.json`. It asserts nothing. The files are
named as though they were the regression protection for the crypto primitives
and are in fact exported samples that overwrite themselves on every run — which
is also why `cargo test --workspace` leaves a dirty tree.

Consequence: a behavioural change in `aes-gcm`, `chacha20poly1305`, `sha2`,
`hmac`, or `ed25519-dalek` would be absorbed silently by the artifact meant to
catch it. This blocks taking those five majors from #499 with any confidence.

Not fixed here — it is a soma-crypto change, not a Cortex-harness one — but it
should be fixed before those bumps move.

### F6. The worker verifies a delegation against itself *(reclassified — now gate G1)*

**No longer a to-do.** It is reachable only with `--features soma`, which no
deployment sets, so leaving it on a task list would mean carrying an item that
cannot be triggered — the reliable way for a known defect to become a forgotten
one. It is recorded as **G1 in ADR-0003's "Gates on ever re-enabling the
feature"**: a condition that must be cleared *before* the feature can come back,
written down where someone turning the feature on will be reading.

Kept below in full because the reasoning is the evidence for the gate.

`crates/worker/src/bin/worker.rs:154` builds its `InvocationContext` with
`invoker_did: deleg.issuer_did` — the issuer taken from the delegation being
checked. `verify_delegation` therefore confirms the signature is consistent with
the DID the token itself claims, and nothing establishes that the DID is
Cortex's. A self-issued delegation verifies.

The API side does check this (`ws.rs`: `delegation.issuer_did != heart.did()`
is rejected), so the exposure is worker-side only and requires an attacker
already able to send `ExecuteStep` frames over an authenticated socket.

Not fixed in the fence PR: fixing it means changing the worker authentication
path, which is precisely what ADR-0003 exists to avoid doing while Track A
lands. It matters whenever the `soma` feature is turned back on.

### F4. A test run mutates a tracked file *(superseded by F5)*

### F3. PR C is Wave 2, but is being implemented before PR A

The plan's delivery order puts PR C in "Wave 2 — execution and durability
(needs A)". The handoff schedules PR C first, on the grounds that it is the only
live safety exposure rather than a missing capability.

This is **compatible** rather than a violation: Wave 2's dependency on A is real
for **PR B** (which drives the `delivered → verifying` transition and therefore
needs A's states to exist), and not for PR C. PR C's outward contract is a typed
`Blocked` outcome on sandbox-setup failure, which the current executor can return
without any lifecycle-state change. PR C's brief states this explicitly and
fences it: **PR C must not introduce lifecycle states — those are PR A's.**

Reconciled, not deferred. No plan change needed.

## Wave 6 / Task 4 — one task end to end, with a stubbed provider

Branch `feat/stub-provider-e2e`. Zero API cost: no model was consulted, and the
stub refuses to run unless `ANTHROPIC_API_KEY` is exactly a sentinel non-key, so
it can never stand in front of a spendable credential.

The premise held. Every finding below is in the wiring, none is in the model
call, and all five were found by *running* the chain rather than by reading it.
The stub itself worked first time.

**What is now proven, at zero cost:** a task dispatched, leased, sandboxed,
handed to a real executable through the real argv, its stdout parsed by the real
extractors, its diff committed, delivered, graded by the frozen exam in the
runner image, and the receipt read back over HTTP — verdict `Verified`, with
`ecosystem:cargo-test` recorded as `Passed` against the delivered tree.

**What is now proven not to work:** the other direction. See F14.

### F13. Neither published sandbox image can execute a required check, and that silently suppresses delivery

`runner_image()` (`crates/worker/src/executor.rs:785`) reads
`CORTEX_SANDBOX_IMAGE`, so the worker's own required checks run **inside the
sandbox image** — the same image the provider CLI runs in, not the check-runner
image.

Neither published sandbox image carries a toolchain. `Dockerfile.sandbox` is
debian + git + curl + netcat; `Dockerfile.sandbox-provider` adds node, because
the Claude CLI is distributed on npm. Nothing adds rust. So against a Rust
repository, every required check exits **127**:

```
CheckEvidence { name: "ecosystem:cargo-check", exit_code: Some(127),
                stderr_excerpt: Some("sh: 1: cargo: not found") }
CheckEvidence { name: "ecosystem:cargo-test",  exit_code: Some(127),
                stderr_excerpt: Some("sh: 1: cargo: not found") }
```

That is not merely useless evidence. `required_checks_allow_commit`
(`executor.rs:1090`) requires `exit_code == Some(0)` from every required check,
and the auto-commit is skipped when it returns false. So the chain is:

> no toolchain in the image → every required check 127 → commit suppressed →
> `head_commit == base_commit` → nothing delivered → nothing graded → no receipt

Measured directly: the first stubbed run produced a correct diff
(`- a - b` / `+ a + b` in the git evidence, `status_porcelain: " M src/lib.rs"`)
and committed nothing at all.

**This applies to the live path unchanged.** `live-model.yml` builds
`Dockerfile.sandbox-provider`, which has no cargo either. A real model producing
a perfect diff would hit exactly this. The live test has never been run to a
pass, and this is what it would find.

`Dockerfile.sandbox-stub` installs cargo purely to get past F13 so the rest of
the chain could be exercised. That is the one place it deliberately differs from
the provider image by more than the stub, and it is commented as such at the
point of installation.

**RESOLVED 2026-09-04.** The worker now builds a *second* sandbox for its own
required checks, from `check_runner_image()` — `CORTEX_RUNNER_IMAGE`, the same
variable the verification driver reads, whose image is built from
`Dockerfile.runner` and does carry a toolchain. The agent still runs in
`CORTEX_SANDBOX_IMAGE`, which still carries the provider CLIs and no toolchain.
Both workflows already build both images, so nothing in CI changed.

The check job is stripped rather than inherited: `NetworkPolicy::Deny` and no
capability grants. Because `policy::sanctioned_env` derives the provider key
*from* the grant, removing the grant removes the key — so a check runs with no
network and no credential. A check that could reach the network could fetch a
passing result.

`Dockerfile.sandbox-stub` no longer installs cargo, and its absence is now the
assertion: a stubbed run that still reaches a verdict from an image with no
cargo in it proves the checks ran somewhere else.

**F13 had a second half, and the first fix did not clear it.** A stubbed run
against the change above still produced:

```
CheckEvidence { name: "ecosystem:cargo-check", exit_code: Some(127),
                stderr_excerpt: Some("sh: 1: cargo: not found") }
```

— in an image built `FROM rust:1.97.1-slim-bookworm`, which plainly contains
cargo. The cause is `sh -lc`. A **login** shell sources `/etc/profile`, which on
Debian *overwrites* PATH with a fixed list that does not include
`/usr/local/cargo/bin`, so the toolchain is present and unreachable. It is now
`sh -c`, which inherits the environment Docker composed: the image's own PATH,
plus `SCRATCH_ENV`.

The verifier never hit this because a `CheckSpec` is argv and never goes
through a shell at all — `Dockerfile.runner` says exactly that at the point
where it declines to set an `ENTRYPOINT`. Two components running "the same
checks" differed in a way neither's tests could see.

**What that run also exposed, still open.** With the commit gate gone (F14) the
diff was committed and delivered correctly — and the brain then logged
`scheduler: step failed` for a step the worker had reported `Completed`,
inserted a heal chain, and never dispatched verification, so no receipt was
ever minted. The 127s are the obvious cause and the shell fix should remove it,
but *whether the brain fails a step on worker-reported check evidence* is worth
confirming directly: if it does, that is F14's shape a second time, on the
other side of the wire, where ADR-0001 says a worker's report is a diagnostic
and not a transition guard. Confirm against a green run before assuming it is
gone.

### F14. The worker refuses to deliver work that fails its own checks, so `Verdict::Failed` is unreachable

**The finding this task was for**, and it is not a variant of F13 — it survives
fixing F13 and gets worse.

With a toolchain present the checks execute properly, and the commit gate then
does what it was written to do: it suppresses the commit for any work that fails
them. Run with a diff the exam genuinely rejects (`a * b`, which satisfies
`add(2,2)==4` and fails `add(10,5)==15`):

```
[stub FAIL] Completed { exit_code: 0,
    base_commit: Some("1cff1e4f…"), head_commit: Some("1cff1e4f…") }
```

Identical commits. The bad diff was written, judged, and discarded **inside the
worker**. It was never delivered, so the verification dispatcher never saw it,
so no receipt exists and no verdict was ever minted.

The consequences are structural, not cosmetic:

1. **The independent verification is tautological.** The verifier can only ever
   grade trees that already passed the same checks in the sandbox. It re-runs an
   exam whose passing was a precondition of delivery.
2. **`Verdict::Failed` cannot be reached from this path.** Its documented
   purpose is "a required check executed and failed. No charge; refund if
   charged" — and the refund path added in V3 cannot fire from a worker
   delivery, because a delivery that would fail never arrives.
3. **This is why every end-to-end assertion in the repository has only ever
   asserted green.** Not because nobody wrote the red case, but because the red
   case cannot occur. A pipeline that can only produce green has not been
   tested.

The stubbed test now asserts this behaviour explicitly and says, in the
assertion message, that it must be inverted when F14 is fixed:

```rust
assert_eq!(fail.commits, 1,
    "F14 may be FIXED: the failing diff produced a commit …");
assert!(fail.receipt.is_none(),
    "F14 may be FIXED: a receipt exists for the failing scenario …");
```

**The design question this raises is a product decision, not a bug fix.** Either
the worker stops gating delivery on its own checks and lets the verifier be the
judge (which is what an independent verifier is *for*), or the frozen exam is
acknowledged as a pre-delivery filter and the refund promise is re-scoped
accordingly. Both are defensible; the current state is the one that is not,
because it ships the refund promise and the machinery that makes it unreachable.

**RESOLVED 2026-09-04 (D2, PR #560, in this branch).** Josh took the first
option: the verifier alone judges. `required_checks_allow_commit` is gone and
the worker's checks are evidence rather than a gate, so a failing diff is
delivered, graded, and recorded as `Verdict::Failed`. The path policy still
gates the commit, because that is a contract about where work may touch rather
than a judgement about whether it is good. The end-to-end test's FAIL half is
inverted and now asserts the red direction it was written to pin.

Note what this does *not* fix: the checks whose results are now reported as
feedback still run in an image with no toolchain, so they still exit 127 and
the feedback is still worthless. That is F13, and it is still open.

### F15. The worker's own checks build into the customer's tree, and the artifacts are committed

Once cargo exists in the sandbox, the required checks run `cargo test --locked
--workspace` **in the worktree**, and the auto-commit then sweeps up everything
they produced. The delivered diff from a one-line source change:

```
files_changed: ["src/lib.rs", "target/.rustc_info.json", "target/CACHEDIR.TAG",
  "target/debug/.cargo-lock", "target/debug/.fingerprint/subject-…/…",
  "target/debug/deps/libsubject-….rlib", "target/debug/incremental/…", …]
```

Hundreds of build artifacts, committed as the customer's delivery.

This is a solved problem *elsewhere in the same run*: the frozen-check runner
executes with its target directory on the scratch tmpfs
(`/scratch/target/debug/deps/subject-…` appears in the receipt's `output_tail`),
so the authoritative checks leave the graded tree clean. The worker-side checks
do not. `Dockerfile.sandbox-provider` already warns in prose about a tool
writing "into the tree it was asked to change — where they land in the diff, in
the git evidence, and in what the frozen checks grade". That is precisely what
the worker's own checks do.

F13 currently masks this: with no cargo, nothing builds and nothing is
committed. **Fixing F13 without fixing F15 turns every delivery into a diff
containing a build directory.**

**RESOLVED 2026-09-04, in the same change as F13** — which is the only safe
order, per the warning above.

`SCRATCH_ENV` in `crates/worker/src/sandbox/policy.rs` gains
`CARGO_TARGET_DIR=/scratch/target`, `CARGO_HOME=/scratch/cargo` and
`npm_config_cache=/scratch/npm`, so build output lands on the scratch tmpfs
that already existed and dies with the container, rather than in the worktree.
The verifier's own runner has had the same three variables since F9; this is
the worker catching up to it.

These are the deliberate exception to the "every value is the mount point
itself" rule documented above them, and they can be: cargo and npm both create
their own directories, unlike the provider CLI that forced that rule.

The npm gap named above is unchanged — `ecosystem:npm-ci` still installs into
`node_modules/` inside the tree, and a cache directory does not rescue it.

### F16. A step that delivered nothing reports success

In both F13 and F14 the worker emits:

```
Completed { exit_code: 0, base_commit: Some(X), head_commit: Some(X) }
```

`execute_sandboxed` returns `Ok(0)`. Nothing in the protocol distinguishes "the
provider succeeded and delivered a diff" from "the provider succeeded and the
worker then threw the work away". The suppression is recorded only as a
`tracing::warn!` on the worker — invisible to the brain, to the receipt, and to
the customer.

A caller cannot detect this without comparing `base_commit` to `head_commit`
itself, which is exactly the check the test had to add to find it. Whatever is
decided for F14, the non-delivery needs to be a typed outcome rather than a log
line.

**RESOLVED 2026-09-04.** An execute-tier step that exits 0 and leaves the tree
unchanged now emits `WorkerEvent::Failed` with the new
`WorkerFailureKind::NothingDelivered`, instead of `Completed` with
`head_commit == base_commit`. The reason is stated in `stderr_excerpt` rather
than quoted from it, because a clean exit leaves stderr empty and a report that
says only "something went wrong" is not much better than the log line it
replaced.

Scoped to the execute tier deliberately: search and think steps are supposed to
leave the tree alone, and failing them for that would be failing them for
working correctly.

`NothingDelivered` classifies to `TaskFailureKind::OutputEmpty` — same
`FailureScope::Task`, same retry policy, since both mean "the step produced
nothing usable" and that is a fact about the task rather than about Cortex. The
worker-side distinction is kept because the two are diagnosed differently: an
empty stdout is a CLI problem, an unchanged tree is not.

### F17. `ExecutionJob.run_id` is empty

Observed on every dispatch:

```
ExecutionJob { job_id: "77dd8960-…", job_version: 1, run_id: "", step_id: "…" }
```

The job is the record of what ran, and it cannot name the run it belongs to. The
comment on `build_job` says every field is set "including the ones nothing
populates yet, because the job is the record of what ran and a field added later
cannot describe work that already happened" — the field is present and carries
an empty string, which is the same problem wearing the right shape.

Small, and worth fixing while the surrounding code is open.

**RESOLVED 2026-09-04.** Pure plumbing, as suspected: `BrainMessage::ExecuteStep`
has always carried `run_id`, and the worker binary destructured it into `..` and
then wrote `String::new()` onto the job. `StepExecution` now carries it and
`build_job` reads it.

The `sse.rs` path plans no run, so it mints one for the single step it
dispatches rather than passing an empty string — a run of one is the truth
there. A unit test pins the value on the built job, because the failure mode
was a field that looked set to anything checking only its shape.

### F18. The same input produced two different verdicts *(observed, not root-caused)*

Two consecutive runs of the identical test against identical images:

| Run | PASS scenario verdict |
|---|---|
| 3 | `Inconclusive`, `executions: []` |
| 4 | `Verified`, `required_passed=2` |

`Inconclusive` with an empty execution set has exactly one source in
`verify_delivery`: `TreeCheckout::create` failed and returned before any check
ran. That is `git worktree add --detach <path> <head_commit>` against the
workspace.

The obvious suspect — the worker's cleanup deleting the step branch
(`git branch -D`) before the dispatcher checks the commit out — was tested
directly and **is not it**: `git worktree add --detach` resolves a bare SHA
fine after the branch is gone. Run 4's log shows the cleanup taking the
`keeping branch alive for PR creation` path instead, so the two runs did not
even take the same cleanup branch.

Reported rather than diagnosed. It is a race between worker cleanup and the
verification dispatcher over the delivered commit, it is non-deterministic, and
`Inconclusive` is the fail-safe direction (no charge, no refund) — but a verdict
that varies with timing on identical input is not a verdict. The tracing
subscriber added to the test in this branch is what makes the next occurrence
diagnosable; it was invisible before because integration tests install no
subscriber and the driver's `tracing::error!` went nowhere.

**ROOT-CAUSED AND RESOLVED 2026-09-04. It was not a race over the commit.**

`Database::get_receipt` does not read a stored receipt — it *assembles* one,
recomputing the gate with `compute_verdict(&specs, &executions)` from whatever
`verification_checks` rows exist **at read time**. The `verification_runs` row
is created when the dispatcher *claims* the job, so from that instant a receipt
exists, and its verdict changes underneath the reader as checks land:
`Inconclusive` with `executions: []`, then whatever the executions say.

Anything polling for "a receipt exists" gets whichever state it happens to hit.
That is the whole of F18: not two verdicts for one input, but one verdict read
too early. The suspected worker-cleanup race was correctly ruled out and was
never involved.

It was caught by running the chain with F19 fixed: the FAIL scenario reached the
verifier, the dispatcher logged `claimed a verification job` at `00:10:53.232`,
and the test read the receipt at `00:10:53.645` — 413 ms later, finding
`Inconclusive` with only `ecosystem:cargo-check` recorded, while
`ecosystem:cargo-test` had not finished.

The fix is one clause: `get_receipt` selects only rows with
`finished_at IS NOT NULL`. Until `finish_verification` seals it there is no
receipt, and the API returns 404 rather than a preview. Reading the verdict
from the stored column instead would not have fixed it — that column says
`pending` until the same moment.

Recomputing the gate rather than storing it is still right, and the comment on
`get_receipt` explaining why still stands; it was the *unsealed* read that was
wrong, not the derivation.

A unit test pins all three states — claimed, partially executed, sealed —
because the failure mode was timing-dependent and therefore invisible to any
test that did not construct the intermediate state deliberately.

### F19. The brain rejects a delivery on the worker's own check evidence, so `Verdict::Failed` was still unreachable

F14 a second time, one hop further on, and it survived fixing F14.

Found by running the stubbed chain after the commit gate was removed. The
worker did its part correctly — auto-committed the diff, `head_commit !=
base_commit`, `Completed` — and then:

```
cortex_api::scheduler: scheduler: step failed a9cc3300-… in run 6c2f9722-…
cortex_api::scheduler: inserted heal chain for step a9cc3300-…
```

No receipt was ever minted, and the test failed on the PASS scenario with *"no
receipt exists … Delivery reached the brain and the verification dispatcher
never graded it."*

The path: `ws.rs` calls `verify_worker_completion`, which builds a
`VerifierInput` whose evidence is **the worker's own report** — its check
outcomes, its command outcomes — and runs `verify_step` on it.
`decide_verdict` (`crates/engine/src/verifier.rs:437`) returns
`VerifierVerdict::Failed` when `!required_check_summary.failed.is_empty()`.
That becomes `verified_success = false`, which calls
`db.fail_step(…, "VerifierRejected")` instead of `db.deliver_step(…)`. A step
that is never delivered never reaches `status = 'verifying'`, so the
verification dispatcher never sees it.

So the independent verifier could still only ever grade work that had already
passed the worker's own checks — the exact property F14 was raised to remove —
and `Verdict::Failed`, with the refund behind it, was still unreachable.

Note the asymmetry that hid this: the *real* verification path
(`verify_from_executions`) already treats worker evidence correctly. It decides
from `compute_verdict(specs, executions)` and keeps the worker's account in a
field called `worker_hints`, with a test named
`worker_evidence_survives_as_a_hint` asserting it is "demonstrably not
load-bearing". Only the pre-screen in `ws.rs` gave it authority.

**RESOLVED 2026-09-04.** `passed` in `verify_worker_completion` is now
`!matches!(report.verdict, VerifierVerdict::Blocked)`. What that boolean
decides is whether the step is delivered *for grading*, not whether it is any
good, so it turns on contract violations only — edits outside the allowed
paths, or a stale base. That is the same line the worker's own auto-commit
draws: where the work was permitted to touch is a contract, whether it is
correct is the verifier's call.

`decide_verdict` is deliberately **unchanged**. It is also what produces
`worker_hints` on the real path, and it is telling the truth — by the worker's
own account the checks failed. The report is still recorded verbatim. It simply
no longer decides a transition.

### What this branch does not claim

No model was consulted. Nothing here is evidence that Cortex completed a task —
that claim belongs to `live-model.yml` and to nothing else. The receipt is
printed with the disclaimer inside the JSON rather than beside it, the image tag
is `cortex/sandbox:STUBBED`, the workflow artifact is named
`STUBBED-NOT-EVIDENCE-of-task-completion`, and the workflow refuses to run at
all if a real provider credential is in scope.

One gap worth naming: **the `Receipt` struct has no field for provider identity,
so the receipt cannot record that the provider was stubbed** in a
machine-readable way. `Receipt` lives in `db.rs`, which this task was scoped out
of, so the disclaimer is injected into the printed JSON by the test instead.
That is sufficient for a log a human reads and insufficient for a receipt a
program trusts. Worth closing when `db.rs` is next open.

## 2026-09-10 verifier integrity repair

**Branch:** `fix/verifier-unknown-exam-integrity`, based on remote `main` at
`6faf4776`. The Cortex parent was unchanged from inspected local `f3e4bb1d`;
the two intervening commits changed HeyVera frontend dependencies only.

**Brief:**
`cortex/plan/briefs/PR-verifier-unknown-exam-integrity.md`.

`verification_driver.rs` no longer represents exam inspection as
`Option<Vec<String>>`. It now distinguishes intact, permitted authored work,
modified protected exam, and unknown integrity with a reason. Missing and
unreadable contracts, missing strong-contract bases, and failed `git diff`
inspection seal `Inconclusive` before checkout or grading. The reason is
persisted in `step_verification_state.terminal_reason`. That path calls neither
the check runner nor billing; the existing routing layer emits no signal for an
inconclusive verdict.

The database read path now preserves the difference between a missing contract
row and malformed contract JSON for the verifier while retaining the existing
`Option` compatibility method for other callers.

**Local evidence (Windows gnullvm toolchain):**

- Targeted driver filter: 16 passed, 0 failed.
- `step_end_to_end`: 5 passed, including the existing stubbed PASS, FAIL, and
  NOOP procedure; no provider key or live model was used.
- `cargo test --workspace --all-targets --locked`: passed.
- `cargo test --workspace --all-targets --no-default-features --locked`:
  passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `cargo fmt --all --check`: not green locally because the checkout reports
  the repository-wide pre-existing Windows newline-style mismatch documented
  below. The two intended Rust files were formatted with `cargo fmt --all`;
  the unrelated CRLF-only worktree changes it created were removed and
  `git diff --check` passed.

No migration, provider spending, model execution, gateway, billing policy,
deployment, or PR was created.

## Rules in force

- Never push to `main`; never bypass a required check.
- Commit and push after each coherent unit.
- Rust: `cargo +stable-x86_64-pc-windows-gnullvm test -p cortex-api --all-targets`.
  **`--all-targets`, never `--lib`** - the integration tests are where every
  cross-crate finding in waves 4 and 5 lived, and `--lib` cannot see them.
  `cargo fmt --all` is **required**: `fmt` is a required check and runs
  `cargo fmt --all --check`. Run it before pushing. Locally `--check` alone
  reports CRLF noise on this machine, so trust the CI job over a local run.
- `jq` is not installed on this machine and MSYS mangles slashes. Use `gh`'s
  built-in `--jq`. A monitor that reports `unknown` is a loop failing silently,
  not an unknowable answer.
- Trim build caches between PRs. The disk hit 100% full during wave 5 and
  stopped a build; a stale worktree's `target/` was holding 12GB.
- The handoff lists `validate::tests::normalizes_dot_segments` as a known local
  failure. It **passed** on this machine during PR A (316/316 on the api lib).
  Treat any failure of it as suspect rather than expected.
- If a brief and the plan disagree, the plan wins and the brief is fixed.

### F20. A check that ran and failed was recorded as a check that could not run

The last place the same mistake lived, and the one that kept `Verdict::Failed`
unreachable after both F14 and F19 were fixed.

Found by running the chain with F18 and F19 fixed. The FAIL scenario reached
the independent verifier, the verifier ran the exam, and:

```
WARN  check runner failed; retrying  spec_id=ecosystem:cargo-test attempt=0
      error=runner failed while executing: wait failed for check
            ecosystem:cargo-test: Docker container wait error
ERROR check could not be executed after retries; verdict will be inconclusive
INFO  verification complete verdict=Inconclusive required_passed=1 required_total=2
```

Three retries, all identical, and only ever for the one check that exits
non-zero. The PASS tree's `cargo test` exits 0 and completes normally.

**Bollard does not report a non-zero container exit as an `Ok` response.** It
reports it as `Error::DockerContainerWaitError`, carrying the code. So
`check_runner.rs`'s wait — which read `Ok` as an exit and `Err` as a failure of
ours — classified every check that ran and failed as a check that could not be
run: `CheckOutcome::NotExecuted`, and `Inconclusive` as the fail-safe.

`CheckOutcome::Failed` was therefore unreachable from the container runner. A
`Verdict::Failed` needs a check recorded as failed, so the refund path stayed
unreachable even after the worker stopped discarding failing work (F14) and the
brain stopped rejecting it (F19).

**RESOLVED 2026-09-04.** The wait error is inspected for the exit code it
carries and treated as an exit; anything else stays `Inconclusive`, because a
Docker failure is genuinely ours to absorb and charging a customer for our
outage would be worse than the bug. The mapping is split into a pure function
and tested in both directions without a daemon.

Why it survived: nothing in `check_runner.rs`'s tests ever reached the
container path — they all exercise `container_config`, which is pure. That is
the same shape as F9, whose own comment in this file warns about exactly this
("every test of the driver uses a `ScriptedRunner` … the one place a spec
becomes a running process had no test at all"). The warning was written and the
gap it named was still there one layer down.

---

## Wave 7 — Phases 28–35 built as types (2026-09-05 → 2026-09-07)

**Landed:** PR [#630](https://github.com/hey-vera/heyvera/pull/630), a merge of 38
commits (#592–#629). Main moved `5708f1b7` → `7a44a890`.

### Why one PR and not thirty-eight

All 38 were written while GitHub Actions refused to start jobs — a billing state
on the account, since resolved by making the repo public. They could not merge,
and each adds a line to `crates/core/src/lib.rs`, so landing them individually
under `strict: true` meant 37 rebases. Cherry-picking onto one branch produced
exactly two conflicts, both trivial. Merged with a merge commit so all 38
messages survive.

### The method

Every plan rule became **a type that makes its violation unrepresentable**,
rather than a test or a doc comment. The rules are all of the form "X must always
be stated / never claimed", and a doc comment does not survive the second caller.

| Rule | Enforcement |
|---|---|
| `1 − (1 − a)^N` is an upper bound | `DeliveredCorrectBound` has only `as_upper_bound()` |
| a model may never create a pass | `PassedCandidate::from_verdict` accepts only `Verified`; `select` returns a borrow out of its input |
| the corpus may not cross a tenant | `EcosystemFact` is a closed enum of objective shapes — customer content has no variant to inhabit |
| five sentences may never be asserted | `Assertion::source()` returns `None` for exactly those five |
| a charge and a refund are different keys | `ChargeKey` / `RefundKey` (landed earlier, #586) |

### What landed, by phase

| Phase | Modules |
|---|---|
| 28 | `capability`, `capability_claim`, `weak_verifier`, `check_cache`, `race_arm`, `router_objective`, `plan_lint` |
| 29 | `comprehension`, `repo_understanding`, `localization`, `corpus`, `context_budget` |
| 30 | `eval_seal`, `suite_statistics`, `honesty_suite`, `held_out` |
| 31 | `loss_model`, `credit_lifecycle`, `guarantee_graduation` |
| 32 | `review_bundle`, `dependency_gate`, `stop_control`, `external_effect`, `receipt_recall` |
| 33 | `intake_question`, `human_contention`, `throughput`, `disclosure_tier` |
| 34 | ledger tests (#621); the Socials seam measured, split and classified |
| 35 | `teaching_artifact`, `teaching_scope`, `teaching_assertion`, `expertise`, `teaching_render` |

`crates/core` went from ~25 to 59 modules. `cortex-api` from 403 to 414 tests.

### Phase 34.2 — the Socials extraction, three steps in

The plan calls this "mechanical". It is not, and the survey in
`docs/proposals/cortex-socials-split-inventory.md` now records why in three
passes:

1. **There is no router to lift.** 0 of the Cortex router's 162 routes are
   Cortex-only; 70 are `/v1/social/*`.
2. **The seam is two functions, not forty-two tables** (#627). Of 429 `Database`
   methods, eight were reached from both products, from two callers.
3. **`integrations.rs` was misclassified** (#628). Two thirds of its 2,808 lines
   were the Cortex task-state machine. Split into `cortex_groups.rs` (2,164) and
   `integrations.rs` (662). **After that split the cross-product runtime surface
   is `deduct_credits` and `record_usage`, both from `pulse.rs`, and nothing
   else.**
4. **All 162 routes classified** (#629): Socials 88, Cortex 53, shared 21, with a
   recommendation to duplicate the shared 21 rather than build a third crate.

**One question gates the rest: does Socials keep charging Cortex's ledger?**
`pulse.rs` deducts credits for a Pulse draft with no verification behind it.
Answer it and the remaining extraction is mechanical.

`crates/api/src/db` is the other prerequisite and it lifts cleanly — 29,905 lines
importing only `crate::lock` and `crate::social_policy` from the api crate. That
makes a shared `cortex-db` crate a small, verifiable change whenever it is
wanted.

### What is NOT done, and cannot be closed by writing code

Phase 34.4's own list, unchanged:

- **`p_fa` has no measured value.** Needs the Phase 30 suites run against real
  API credits.
- **The decomposition crossover has no calibrated threshold.** Needs per-leaf
  priors; the plan puts this several quarters out.
- **Nobody has watched a solo builder complete a task.** The plan is explicit
  that this is the only real evidence, and it needs a person who has not read
  the document.

Phases 35.11–35.17 are rejections-with-reasons, sequencing and risk framing.
They are prose in the plan and have no code to write.

### On "is the plan complete"

The plan answers this itself, and the answer is no by design:

> It does **not** claim that no further gaps exist. Four rounds of review have
> each found real ones. … The rate of discovery is not obviously decreasing …
> **A fifth review should be expected to find more, and should deliberately
> attack an axis none of the first four used.**

Rounds five and six then did exactly that and found more. So "complete" is not a
state this document has; the honest statement is that **every mechanism it
specifies that can be built without a product decision or a measurement is now
built and on main.**

### Traps this wave (for the next session)

- **`cargo fmt -p <crate>` reformats the whole crate.** On this repo that shows
  ~35 files as modified when only 3 changed; `git diff --stat` is the truth, and
  stage explicit paths.
- **`git branch --merged` lies after a squash merge.** Use
  `git cherry origin/main <branch>` — 22 branches that looked unmerged were
  fully contained.
- **`heyvera-current` is the repo's main working tree**, not a stale worktree.
  `heyvera-social-audit` is a linked worktree of it.

## 2026-09-10 private provider spending gateway

Brief: `cortex/plan/briefs/PR-provider-spend-gateway.md`.

Implemented on `feat/cortex-provider-spend-gateway` from clean `main` at
`3de498e2`:

- Migration v68 separates supplier capacity, per-attempt spending
  authorizations, and per-request reservations from customer credit balances.
- `provider_gateway` signs and verifies capabilities scoped to tenant, run,
  attempt, one provider, one model, one authorization, and an expiry.
- Request forms require an exact model and finite `max_tokens`; streaming and
  tools remain unsupported. The input side is conservatively bounded by
  serialized bytes, so the gateway does not trust a caller-supplied token count.
- A transaction checks authorization exposure and global funded capacity before
  inserting the reservation. Replays cannot invoke the supplier twice, and
  concurrent requests cannot overdraw the same finite capacity.
- Observed usage settles exactly once into `provider_spend`. Timeouts and
  missing/invalid usage remain unresolved at the full reservation. Explicit
  reconciliation can settle or release them; contradictions become a visible
  mismatch rather than overwriting history.
- The sandbox no longer admits real supplier keys. The exact public sentinel
  used by the zero-cost stub image is retained; any other provider key is
  refused even when the job carries a provider grant.

Local evidence, using the installed Windows LLVM toolchain because the default
GNU linker still lacks `libgcc`/`libgcc_eh`:

```text
cargo +stable-x86_64-pc-windows-gnullvm test -p cortex-api --all-targets --locked provider_gateway
  8 passed, 0 failed
cargo +stable-x86_64-pc-windows-gnullvm test -p cortex-worker --all-targets --locked sandbox
  35 passed, 0 failed (35 filtered)
cargo +stable-x86_64-pc-windows-gnullvm test --workspace --all-targets --locked
  passed
cargo +stable-x86_64-pc-windows-gnullvm test --workspace --all-targets --no-default-features --locked
  passed
cargo +stable-x86_64-pc-windows-gnullvm clippy --workspace --all-targets --locked -- -D warnings
  passed
git diff --check
  passed
```

`cargo fmt --all --check` is locally blocked by the checkout's pre-existing
CRLF newline mismatch across untouched Rust files; the intended diff is
formatted and the repository CI remains the authoritative formatting gate.

No provider key was used and no provider request was made. This boundary was
squash-merged in PR #642 at `6bea4888`.

## 2026-09-11 stub gateway capability delivery

Brief: `cortex/plan/briefs/PR-gateway-capability-delivery.md`.

Implemented locally on `feat/cortex-gateway-capability-delivery`:

- `ExecuteStep` carries an optional bearer envelope whose `Debug` form is
  redacted; the worker uses it only on the sandbox request and never copies it
  into the persisted execution job or receipt.
- Both version-skew directions default to no envelope and therefore no gateway
  authentication.
- The final sandbox boundary checks run, attempt, model, provider, expiry,
  gateway host, grant, and non-empty bearer before admitting the documented
  Claude gateway environment variables.
- Claude egress now names `cortex.heyvera.org`; `api.anthropic.com` is absent
  from the provider grant.
- Explicit stub mode publishes a finite authorization before dispatch and the
  authenticated Messages listener executes the durable reserve/settle engine.
- Existing immutable price lists are not edited. An installation missing the
  router's current Claude model ids receives a new version beside the old one.

The listener still rejects streaming and tool-bearing request forms and has no
supplier transport. The next step is CLI request-shape compatibility and an
adversarial container proof; a live call remains separately authorized and has
not occurred.

Local evidence (Windows LLVM toolchain):

```text
cargo +stable-x86_64-pc-windows-gnullvm test -p cortex-core --all-targets --locked egress
  17 passed, 0 failed
cargo +stable-x86_64-pc-windows-gnullvm test -p cortex-worker --all-targets --locked sandbox
  37 passed, 0 failed (35 filtered)
cargo +stable-x86_64-pc-windows-gnullvm test -p cortex-api --all-targets --locked provider_gateway
  10 passed, 0 failed
cargo +stable-x86_64-pc-windows-gnullvm test -p cortex-api --test pricing_integration --locked
  8 passed, 0 failed
cargo +stable-x86_64-pc-windows-gnullvm test --workspace --all-targets --locked
  passed
cargo +stable-x86_64-pc-windows-gnullvm test --workspace --all-targets --no-default-features --locked
  passed
cargo +stable-x86_64-pc-windows-gnullvm clippy --workspace --all-targets --locked -- -D warnings
  passed
```

This boundary was squash-merged in PR #643 at `61a87ecd` after the real
adversarial sandbox CI job proved that a Claude grant cannot contact Anthropic
directly.

## 2026-09-11 Claude CLI gateway request-shape proof

Brief: `cortex/plan/briefs/PR-claude-cli-shape-proof.md`.

Implemented on `feat/cortex-claude-cli-shape-proof`:

- The provider sandbox pins Claude Code `2.1.268`.
- A CI proof runs that real CLI with `--network none`, a read-only root
  filesystem, no supplier credential, and a loopback-only synthetic Messages
  server.
- The proof requires the configured gateway URL to receive Bearer capability
  authentication, the Cortex attempt header, the routed model, and a positive
  integer `max_tokens` bound.
- It also records the compatibility gap instead of hiding it: this real
  print-mode invocation requires streaming and sends an empty `tools` array. The
  current stub listener deliberately rejects streaming.

No provider request can occur in this proof because the container has no
external network. Supplier transport, broader tool-bearing shapes, and a live
call remain separately authorized and out of scope.

This boundary was squash-merged in PR #644 at `e01e32b0` after the new
`gateway-cli-shape` job and every existing required check passed.

## 2026-09-11 stub gateway streaming compatibility

Brief: `cortex/plan/briefs/PR-stub-gateway-streaming.md`.

Delivered in PR #645 from `feat/cortex-stub-streaming`:

- Bounded validation admits the measured boolean `stream` field and empty
  `tools` array while rejecting malformed streaming values and all non-empty
  or non-array tool forms before reservation.
- The stub transport completes durable reconciliation before its already
  buffered synthetic message is encoded as Anthropic SSE.
- The HTTP proof covers the measured 32,000-token bound, the complete event
  sequence, settled spend, and an idempotent replay that cannot create a second
  spend row.
- Non-empty tools remain fail-closed before any reservation is written.

There is still no supplier transport, live provider key, live request,
purchase, or deployment. The streamed body is synthetic and `$0`.

This boundary was squash-merged in PR #645 at `54ff6e10` after every required
check passed.

## 2026-09-11 CLI through the Rust stub listener

Brief: `cortex/plan/briefs/PR-cli-rust-listener-proof.md`.

Delivered in PR #646 from `feat/cortex-cli-rust-listener`:

- A feature-gated proof binary wraps the production stub handler, binds only
  loopback, and refuses to start without an explicit proof-mode guard.
- The pinned Claude CLI runs against the real Rust Messages route in a
  `--network none`, read-only, capability-dropped container.
- The proof requires a successful CLI exit and reads the same SQLite database
  to require one to four reservations with every row settled.
- The proof exposed an omitted contract: after its initial empty-tools calls,
  the pinned CLI sends its built-in tool definitions. The gateway now admits
  only arrays of at most 64 uniquely named, structurally valid definitions;
  malformed forms fail before reservation. This does not grant tool execution,
  which remains confined to the sandbox.
- The original capture stage remains in place, so the exact request-shape
  assertions are still independently enforced.

No supplier transport or credential exists in this path. The container cannot
reach an external provider and spend remains `$0`.

## 2026-09-12 autonomous completion boundary

The repository is at the limit of what can be completed autonomously without
changing product policy, spending money, deploying production, or substituting
synthetic evidence for the measurements the plan requires. There is no further
code-only task queued by this plan.

| Open outcome | Why it remains open | Authority or evidence required |
|---|---|---|
| Production environment reviewer | GitHub rejected the protection rule while the repository was private. The repository is now public, but `production` still has no required reviewer; the old billing diagnosis has not been retried. | Josh chooses/authorizes the reviewer before this deployment-affecting rule is changed. |
| One real model invocation, end to end | The path is built, but there is no provider credential or approved supplier budget. | Josh supplies a narrow, short-lived key and explicitly authorizes the bounded spend. |
| Production execution | Historical deployment evidence found an unauthenticated worker, no host sandbox image, and no provider key. | Josh explicitly authorizes a current production deployment and the required secret/configuration changes. |
| `p_fa`, decomposition crossover, and solo-builder evidence | These are measurements, not missing implementations. | Real API credits, calibrated per-leaf priors, and an observed independent user. |
| Socials/Cortex extraction | Josh decided Socials does not retain the Cortex ledger. PRs 1–3 are implemented locally on stacked branches: billing fails closed, both routers match the literal route manifest, and the additive `heyvera-db` can safely export the 43-table Socials v1 schema. | Land and verify the three boundary PRs, then move Socials into its independent API/state graph; repository creation, pushes, production movement, and deployment still require separate authorization. |

Therefore **the implementation plan is autonomously complete, but the product
validation plan is not complete**. A separate Cortex repository is premature
until the ledger-ownership decision is made and the runtime/database/billing
boundary has been extracted and proven in this repository first.

Read-only GitHub reconciliation on 2026-09-12 found the repository public, the
latest recorded `production` deployment at `7a44a890`, no repository-level
Actions secrets, and only the two Tailscale connection secrets in the
`production` environment. In particular, there is still no recorded provider
credential to make the live-model checkpoint runnable.

## 2026-09-22 live voice drives the agent (M-D-0013 to M-D-0015)

Goal (Josh): a dictation button and, to its right, a continuous live-voice
toggle next to the chat box, with live voice able to use tools. Both buttons
already exist in `ChatComposer.tsx`. Cortex's own Claude runs the tools through
the gateway, charged in credits at actual cost; gpt-live-1 only listens and
speaks.

- **M-D-0013 (this PR):** sessions are created with client delegation. OpenAI's
  `session.delegation.created` carries no request text, so the server keeps the
  user's words from `session.input_transcript.delta` and hands them to the same
  paid agent loop chat uses. The answer goes back as `session.commentary.append`
  (the voice paraphrases it). Confirm-risk tools are withheld from voice turns
  until M-D-0014. One delegation at a time per session. Voice turns are not
  saved to a conversation: voice sessions have no conversation id yet.
- **M-D-0014:** server-checked spoken yes. It must start after the prompt's
  speech ends (the client reports this; GPT-Live has no end-of-speech event),
  within 45 s, as a strict bare yes, with one pending action, used once; a new
  proposal voids the old one. The card always shows the exact action, because
  the voice paraphrases.
- **M-D-0015:** voice-mode confirm card with a countdown.
- Then more Confirm tools: start a run, resolve an approval.

Unconfirmed: the text field name inside `session.input_transcript.delta`
(the parser accepts `delta` or `text`).
