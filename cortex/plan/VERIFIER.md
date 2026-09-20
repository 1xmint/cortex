# What "verified" means

> Decision record. Written 2026-08-05 against `origin/main` after PR #439,
> grounded in the verifier, scheduler, and container code that exists today.
> Companion to [CREDITS.md](CREDITS.md) — this document defines the event that
> the credit ledger charges and refunds on. Phase 2.2 of
> [PLAN-2026-08.md](PLAN-2026-08.md).

---

## The decision

**A step is verified when Cortex has re-executed its required checks in an
execution environment Cortex controls, and every one of them passed. Nothing
else counts.**

Not the worker's word. Not a model's opinion. Not an exit code reported over a
websocket by the same process that wrote the code. The verdict that charges a
customer is computed from executions the platform ran itself, and the evidence
is retained so the claim "the tests ran and passed" is provable after the fact.

---

## What "verified" means today, honestly

Three facts, all checked against the code on 2026-08-05:

1. **The verifier never executes anything.** `crates/engine/src/verifier.rs`
   (910 lines) is well-typed evidence *summarization* — `CommandEvidence` and
   `CheckEvidence` structs carry exit codes in; verdicts come out. There is no
   `Command::new`, no spawn, no sandbox anywhere in the file. It grades claims.
2. **The claims come from the party being graded.** Step evidence arrives over
   the worker channel (`crates/api/src/ws.rs`) from the same worker that
   produced the code. In the server-held-key model that worker is executing
   LLM-authored output — the least trustworthy witness available.
3. **Below High risk, nothing is required at all.** `infer_required_checks`
   (`crates/api/src/scheduler.rs:613-625`) returns an empty vec for every
   Execute step under High risk. For most billable work, "verified" currently
   means "the CLI exited 0, says the CLI."

CREDITS.md already drew the consequence: **refund-on-failure must not ship on
this.** This document is the fix it demanded.

## Why it has to be this way

The product claim (PLAN §2.3) is *a verified engineering outcome at roughly
half the frontier bill*. Routing supplies the cost half; this verifier supplies
the whole first word. Three properties are non-negotiable, and each one rules
out a tempting shortcut:

- **Independent** — the check runs on infrastructure the worker cannot touch,
  from a clean tree at the delivered commit. Rules out: trusting worker
  evidence, however well-structured.
- **Deterministic** — the verdict is a pure function of executed check results.
  Rules out: LLM-as-judge anywhere in the gate. Models may *propose* checks;
  they never grade them. (A verifier that always says PASS is the same disease
  as a CI gate that cannot fail — PLAN standing rule 2.)
- **Attributable** — every verdict binds to `(run_id, step_id, attempt)` and
  retains the evidence that produced it. Rules out: verdicts that cannot be
  replayed or audited when a customer disputes a charge. The refund promise is
  only as good as the receipt.

There is also a margin reason to do this well: verification compute is COGS
that competitors selling raw tokens do not carry. It is affordable only because
checks are cheap relative to generation — a test run costs cents of CPU; the
tokens that produced the diff cost dollars. The design below keeps it that way
(one verification per delivered attempt, no speculative re-runs).

---

## Trust model

| Party | Trust level | May do | May never do |
|---|---|---|---|
| Worker / executing CLI | **Untrusted** | Deliver a tree + *suggest* checks | Supply evidence that reaches a verdict |
| Model (any provider) | **Untrusted** | Propose acceptance checks at plan time | Grade results; veto or soften a verdict |
| Check runner (sandbox) | **Trusted, minimal** | Execute checks; report exit/output | Reach the network; see upstream keys |
| Verifier core | **Trusted** | Derive checks, order runs, compute verdicts | Skip a required check for any reason |

Worker-reported evidence does not disappear — it becomes a *hint* (which
commands the worker believes prove the work) and a fast-fail signal (a worker
reporting its own failure short-circuits straight to FAILED without spending
sandbox time). It just never substitutes for execution.

### The gap this table does not close: untrusted as a *witness* is not untrusted as an *executor*

Added 2026-08-05. The table above solves who may produce a verdict. It says
nothing about what the worker may *do while producing the work*, and that is a
separate and currently open problem.

The worker executes LLM-authored code, on infrastructure Cortex owns, holding
operator provider credentials. Under BYOK that risk was the user's — their
machine, their key. Operator-funded keys move it onto us, and
`ARCHITECTURE.md`'s amendment already flags that worktrees migrating onto our
infrastructure is "new cost and a **new security boundary**". The verifier
work hardened the *check runner* thoroughly — fresh container, `--network
none`, read-only tree, `cap_drop: ALL`, empty environment — and gave the
*execution* sandbox none of that treatment. PLAN task 3.4 says egress-denied
by default and never put upstream keys in the sandbox env; that is a plan, not
code.

Stated plainly so it cannot be mistaken for solved: **an execution sandbox
that can reach the network and read our provider keys is the "unattended
laptop" inside our own architecture.** It does not affect verdict integrity —
a compromised worker still cannot manufacture a pass — but it is the thing an
enterprise security review will ask about first, and it precedes SOC 2 rather
than being part of it.

Useful external framing, from the 3Cs (Sekaran & Aleyner, Feb 2026):
*contain, curate, control*. Mapping honestly onto what exists:

| | Where Cortex stands |
|---|---|
| **Contain** | Done for verification (V1). **Not done for execution** (3.4) — the gap above. |
| **Curate** | Strongest of the three, and arrived from the other direction: CONTEXT.md's C1–C4 restrict what a step sees for *quality* reasons and get the security property free. Tool-surface curation per step does not exist. |
| **Control** | Partial. Resource leases (now claiming computed impact sets, C4), `budget_enforcer`, per-tenant spend caps (2.6), risk overlays. Outbound secret scanning (2.7) is unbuilt. **No general policy engine, deliberately** — building one speculatively is how a control plane becomes shelfware. |

The framework's blind spot is worth noting too, because it is our product:
containment bounds what a failure *costs*; it says nothing about whether the
work was *correct*. A receipt does. Prevention is table stakes; proof is the
thing a customer pays for.

---

## The gate

Deterministic verdict rules, in order:

| Condition | Verdict | Billing effect |
|---|---|---|
| Every required check executed and passed | **VERIFIED** | charge fires (exactly once) |
| Any required check executed and failed | **FAILED** | no charge; if already charged, refund fires |
| Any required check could not be executed (runner/infra error) | **INCONCLUSIVE** | no billing effect; bounded retry, then surfaced to operator |
| Check exceeded its timeout | **FAILED**, timeout recorded as the failure | as FAILED |
| No required checks derivable for the task | **UNVERIFIED** (not a verdict — a label) | charged at task rate, sold *without* the verified badge and *without* the refund promise |

Notes that carry weight:

- **INCONCLUSIVE is not FAILED.** A dead runner must never refund a customer or
  charge one. It retries (bounded, then pages the operator). Conflating infra
  failure with task failure either leaks money or breaks trust; this line is
  where that is prevented.
- **UNVERIFIED is priced honesty.** Some tasks genuinely have no derivable
  check ("rewrite this README"). They remain sellable, clearly labeled, at
  normal credit rates — but the refund promise and the verified badge attach
  exclusively to verified work. The alternative (pretending everything is
  verifiable) is how "verified" becomes a marketing word. *Pricing-adjacent —
  Josh signs off on this row before it reaches a pricing page.*
- Timeouts are failures, not infra errors, because a hung test is exactly the
  kind of defect the customer is paying to be protected from. Timeout evidence
  is retained like any failure.

---

## Check derivation

`infer_required_checks`'s empty-vec default dies. Its replacement derives, in
order, and the union is the required set:

1. **Ecosystem floor** — detected from the delivered tree, not from
   `allowed_paths` (the current path-sniffing misses repo-level effects):
   `Cargo.toml` → `cargo check --locked` + `cargo test --locked`;
   `package.json` → install + `npm test`/`npm run build` when the scripts
   exist; both → both. The floor is mandatory for every Execute step that
   touched a file the ecosystem owns.
2. **Contract checks** — acceptance criteria from the `TaskContract`, proposed
   at plan time (models may help write these), frozen before execution starts.
   A check added after the worker has seen the task is advisory only —
   otherwise the optimizing process gets to choose its own exam.
3. **Risk overlays** — at High/Critical risk: `allowed_paths` enforcement
   (already in `verifier.rs` as `find_allowed_path_violations` — it graduates
   from advisory summary to required check) and lint with warnings denied.

An Execute step whose union is empty after all three passes is UNVERIFIED by
construction, labeled as such at plan time — the user knows what they are
buying *before* the run, matching the CREDITS.md rule that a user should be
able to predict cost (and now certainty) before pressing go.

---

## Execution substrate

Two phases, one contract, so the upgrade never touches the verifier core:

```rust
/// The only interface the verifier core sees. Phase A and B both implement it.
trait CheckRunner {
    /// Execute one check against an immutable snapshot of the delivered tree.
    /// Infallible infra is not assumed — Err is what INCONCLUSIVE is made of.
    async fn run(&self, tree: &TreeSnapshot, check: &CheckSpec) -> Result<CheckExecution, RunnerError>;
}
```

**Phase A — now.** Containers via the existing `ContainerManager`
(`crates/api/src/docker.rs:24`, `exec_stream` at `:294`), hardened for this
duty: fresh container per verification, tree mounted read-only from a clean
checkout of the delivered commit (never the worker's working directory),
`--network none`, CPU/memory/pids/time limits, **no upstream provider keys in
the environment** (PLAN §5, task 3.4's rule applies from day one). The
`docker-compose.yml` capability-drop posture (PLAN §8.6) is the baseline.

**Phase B — before paying strangers at scale.** Firecracker-class microVMs
(E2B per PLAN §6). Same `CheckRunner` contract; kernel-level isolation replaces
namespace isolation. The switch is a deployment decision, not a redesign.

The runner image is pinned by digest and recorded in the evidence — "it passed"
is only reproducible if *where it passed* is pinned.

---

## Evidence and receipts

Two tables, SQLite-shaped now, Postgres-ready (they move in Phase 3.1):

```sql
CREATE TABLE verification_runs (
    id              TEXT PRIMARY KEY,      -- verdict id; the refund's receipt
    run_id          TEXT NOT NULL,
    step_id         TEXT NOT NULL,
    attempt         INTEGER NOT NULL,
    tree_hash       TEXT NOT NULL,         -- commit/tree the checks ran against
    runner_image    TEXT NOT NULL,         -- image digest, Phase A/B tag
    verdict         TEXT NOT NULL,         -- verified | failed | inconclusive
    started_at      INTEGER NOT NULL,
    finished_at     INTEGER,
    UNIQUE(run_id, step_id, attempt)       -- one verdict per attempt, CAS'd
);

CREATE TABLE verification_checks (
    id              TEXT PRIMARY KEY,
    verification_id TEXT NOT NULL REFERENCES verification_runs(id),
    source          TEXT NOT NULL,         -- ecosystem | contract | risk
    command         TEXT NOT NULL,
    exit_code       INTEGER,               -- NULL = could not execute
    duration_ms     INTEGER,
    output_digest   TEXT NOT NULL,         -- hash of full output
    output_tail     TEXT NOT NULL          -- last N KB, human-readable receipt
);
```

Full logs are digest-addressed and retention-limited; the tail is what a
dispute reads first. This is the artifact behind "can prove the tests ran."

---

## Billing binding

The verdict is the billing event. One state machine, CAS-guarded like every
lease transition in ARCHITECTURE:

```
Delivered ──> Verifying ──> VERIFIED ──── charge(key = verify:{verification_id})
                   │
                   ├──────> FAILED ────── if charged: refund(key = refund:{verification_id})
                   │
                   └──────> INCONCLUSIVE ─ requeue (bounded); no ledger writes ever
```

- The `UNIQUE(run_id, step_id, attempt)` row is claimed with a CAS before
  running — two verifier processes cannot both produce a verdict, so the
  ledger key derived from `verification_id` is minted exactly once.
- The ledger's `idempotency_key UNIQUE` (PR #437) is the backstop if the
  process dies between verdict and charge: replaying the transition re-derives
  the same key and the ledger makes the write a no-op.
- A refund is a positive `credit_transactions` row with reason
  `task_failed_refund` (already enumerated in CREDITS.md), never an UPDATE of
  the spend row — the ledger stays append-only.

This is the exactly-once money path PLAN task 3.2 asks Temporal to guarantee.
The design works on today's single SQLite node *because* both sides are
idempotent; Temporal later makes the retries durable, it does not change the
keys.

---

## What not to do

- Do not let any model grade any check. Propose, never judge.
- Do not run verification in the worker's environment, ever — including "just
  this once" for speed. The moment a worker can influence its own verdict, the
  product claim is void.
- Do not treat runner failure as task failure (or success). INCONCLUSIVE
  exists so infra problems cost Cortex time, not customers money.
- Do not derive required checks after the worker has seen the task.
- Do not put provider keys, Clerk secrets, or the production DB path in the
  runner environment. The runner gets a tree and a command.
- Do not gate Search-tier (read-only) work on this machinery — verification
  attaches to steps that change trees.

---

## Build plan (hand-off)

| # | Task | Effort |
|---|---|---|
| V1 | `CheckRunner` trait + Phase A container runner (`--network none`, limits, read-only tree mount, image digest capture) | Opus 5 · high |
| V2 | Check derivation: ecosystem floor + contract freeze + risk overlays; delete the empty-vec default at `scheduler.rs:624` | Opus 5 · high |
| V3 | `verification_runs` / `verification_checks` migration (**v61 — after the v53–v59 Socials train and v60 credits; same single-counter rule as #437**) | Opus 5 · medium |
| V4 | Verdict state machine + billing binding via #437 keys; INCONCLUSIVE requeue with bound + operator alert | Opus 5 · high |
| V5 | Wire `verifier.rs` core: worker evidence demoted to hints; verdicts computed from `CheckExecution` rows only | Opus 5 · high |
| V6 | Receipts surface: verdict + check tails on the run API for the frontend to render | Sonnet 5 · medium |
| V7 | Red-team pass: worker attempts to influence verdict (poisoned check suggestions, path escapes, resource exhaustion) | Opus 5 · xhigh |

Sequencing: V1+V2 unblock everything; V4 must not merge before V3; **refund
copy appears in the UI only after V7 passes.** Josh's sign-off gates the
UNVERIFIED pricing row before any pricing page ships.

---

## Amendments — 2026-08-06 (grounding in [RESEARCH-2026-08.md](RESEARCH-2026-08.md))

Three engineering amendments from the research pass. Pricing/positioning
recommendations from the same brief route through JOSH-ACTIONS.md §7, not here.

**A1 — Held-out checks (extends §check derivation; ships as a V2 extension).**
All required checks remain frozen at plan time — that rule is unchanged. What
changes: derivation may emit a *withheld subset* — property-based or
metamorphic variants of the visible contract checks — that never appears in
the worker's prompt. The worker sees what must be true; it does not see every
probe. Rationale: a frozen, fully-visible check set degrades under
optimization pressure ("The Verification Horizon," arXiv 2606.26300 — the
proxy-intent gap widens as generator capability grows), and Kiro's
property-based testing proves the variant technique is productizable.
Held-out checks are still frozen before execution and still deterministic —
this amends *visibility*, never the freeze or the gate.

**A2 — The fresh-context retry rule (binds to V4's requeue path and the heal
path).** A FAILED attempt's transcript is never continued. A retry is a new
worker with fresh context, packed with exactly: the task contract, the failed
receipt's check tails (the executed evidence of what broke), and the step's
C3 context slices. Measured grounding: models do not recover from their own
wrong turns (−39% multi-turn degradation, no recovery, arXiv 2505.06120), and
self-correction without an external signal makes things worse (ICLR 2024,
arXiv 2310.01798). The receipt tail *is* the external signal — retries fed
executed evidence converge; retries fed their own transcript compound the
error. This is an architectural principle, not an optimization: no step
inherits a transcript, the same way no model grades a check.

**A3 — Instrument UNVERIFIED-rate from day one (metrics on
`verification_runs`).** Per task class and per ecosystem, from the first
verdict row. The billing-honesty story collapses silently if a large share of
real tasks falls out of check derivation; this number decides where
derivation investment goes next and gates which task classes the refund
promise is marketed on. One rollup query — cheap now, impossible to
reconstruct later.
