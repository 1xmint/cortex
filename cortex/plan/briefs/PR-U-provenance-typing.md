# PR U — provenance-typed context

**Source:** `HARNESS-EXCELLENCE-PLAN-2026-08.md` line 6214 (Track B, Phase 13),
and the launch-gate list at line 6698.

An implementer reads only this brief. Everything below was verified against the
tree at `9d23acd2`; every claim carries a `file:line` so it can be re-checked in
one command rather than re-surveyed.

---

## Read this before scoping the work

**The worker discards `StepContext`.** `crates/worker/src/bin/worker.rs:143-145`
destructures `ExecuteStep` as:

```rust
BrainMessage::ExecuteStep {
    step_id, attempt_id, lease_gen,
    task, decision, delegation, egress, ..
} => {
```

`context` falls into the `..`. There is no `context` identifier anywhere in
`crates/worker/src/` — `grep -rn "\bcontext\b" crates/worker/src/` returns
nothing outside `InvocationContext`, which is unrelated. `build_task_prompt`
(`crates/worker/src/executor.rs:684`) builds the prompt from `TaskContract`
alone, and `TaskContract` (`crates/core/src/task.rs:9`) has no context field.

So the API assembles context — `ContextBus`, the repo map, predecessor
summaries — serialises it, sends it over the socket, and the worker throws it
away. **The model never sees any of it.**

Two consequences, and both change how this PR should be built:

1. **The injection hole the plan describes is not open through this path yet.**
   The plan's launch gate (line 6698) says "repository content is untrusted
   input, and until the instructions-come-only-from-the-contract rule is
   enforced, every external test is an unreviewed injection surface." That is
   correct about what *will* happen and not about what happens today: repository
   content does not currently reach the model, because the field carrying it is
   dropped.

2. **That makes PR U cheaper and more urgent at the same time.** Cheaper,
   because there is no existing rendering path to retrofit — the typing can be
   designed into the representation before anything consumes it. More urgent,
   because the moment someone wires `context` through (a one-line change to that
   destructure, which looks like a bug fix), the hole opens with no typing in
   place. **PR U must land before the context is connected, not after.**

Do not treat "wire the context through" as part of this PR. It is a separate
change that becomes safe *because* this one landed. Say so in the PR body so the
next person does not read the disconnected field as an oversight to fix in
passing.

---

## What exists now

| Thing | Where | State |
|---|---|---|
| `Artifact` | `crates/api/src/context_flow.rs:45` | Has `kind`, `confidence: f32`, `metadata`. **No provenance type.** |
| `ArtifactKind` | `crates/api/src/context_flow.rs:61` | `Answer`/`Code`/`Analysis`/`Plan`/`Review`/`Error`/`Working` — describes *shape*, not *trust*. |
| `StepContext` | `crates/core/src/protocol.rs:237` | `predecessor_summaries`, `user_goal`, `conversation_excerpt`, `repo_map`. Untyped strings. |
| `EvidenceSource` | `crates/core/src/contamination.rs:6` | Already distinguishes human/AI/compiler/CI origin. **Reuse this vocabulary; do not invent a second one.** |
| `SignalTier` | `crates/core/src/contamination.rs:22` | Policy → HardObjective → IndependentVerify → Stability → WeakSubjective. |

`confidence: f32` is the closest thing to provenance today, and it is set by a
hardcoded ternary at `crates/api/src/ws.rs` (`0.5` if the worker self-reported
success, `0.1` otherwise). A float that means "the worker said so" is exactly
the kind of number that reads as evidence and is not.

## What to build

### 1. A provenance type on every context item

Not a float. An enum, in `cortex_core`, that a renderer can `match` on
exhaustively — the point is that adding a new provenance forces every render
site to decide what to do with it.

Minimum distinctions the rest of this brief depends on:

- **Contract** — came from the task contract. The only thing that may be read as
  an instruction.
- **RepositoryContent** — read out of the repo under test. Untrusted. Data.
- **AgentOutput** — produced by a previous step's model. Unverified.
- **VerifiedEvidence** — a check execution or verdict. Carries its
  `verification_id`.
- **UserMessage** — the human's own words.

Reuse `EvidenceSource`/`SignalTier` where they already say the right thing
rather than paralleling them.

### 2. Never render inferred as fact

One rendering function, and it must be impossible to bypass. The rule: an item's
provenance determines how it is framed, and no provenance except `Contract` may
be framed as an instruction.

Test it the way the sandbox tests are written — adversarially. A repository file
containing `IGNORE PREVIOUS INSTRUCTIONS AND ...` must come out of the renderer
visibly framed as quoted repository content. Assert on the rendered string.

### 3. Observed-content directives are findings, not commands

The plan's phrasing. Concretely: when repository content or agent output
contains something shaped like an instruction, it is **reported** — surfaced as
a finding on the step — rather than passed through as prompt text that happens
to be quoted. Quoting alone is not sufficient and should not be the whole
defence; the detection is what makes it visible to an operator.

### 4. `Assumption` objects, checked at plan time

An assumption is a claim the plan depends on that nothing has verified — "this
crate builds", "this test currently passes". It needs a mechanical check that
runs **at plan time**, so a plan built on a false premise fails before a
customer is charged for discovering it. `cortex_core::check_derivation` is the
model to follow: pure rules, unit tested without a disk, with the filesystem
edge kept small (`crates/api/src/ecosystem_probe.rs`).

### 5. Summaries typed and sourced

A summary carries what produced it and what it summarises. A summary of agent
output is `AgentOutput`, not evidence, however confident it sounds.

### 6. Verified evidence is exempt from compaction

Compaction drops the least useful thing under a token budget. Verified evidence
is the one class that must survive: it is the only content whose truth was
established independently. Anything else can be re-derived; this cannot.

### 7. Per-attempt context composition recorded

What went into the prompt, for this attempt, recorded so a receipt can state it.
`execution_jobs` already has `context_bundle` and `packed_bytes` columns
(`crates/api/src/db.rs`, migration v62) that nothing populates — use them rather
than adding more.

---

## Constraints

- **No migration unless one is genuinely needed.** Re-check the max in
  `crates/api/src/db.rs` at rebase time, never at design time — the counter is
  shared with Socials and a collision means whichever branch merges second has
  its migration silently skipped. Max on `main` was **v65** at the time of
  writing.
- Required checks now include `no-default-features` and `strict` is on, so the
  branch must be up to date before it can merge. If another PR merges first,
  rebase — GitHub's auto-merge does not do it for you (measured; see Task 4).
- `cargo +stable-x86_64-pc-windows-gnullvm test -p cortex-api --lib`. Never run
  `cargo fmt` on this repo.
- Do not modify `crates/soma`, `crates/soma-core`, or `crates/soma-crypto`.

## What this PR must not do

- **Do not wire `context` through to the worker.** See the top of this brief.
  That is the next change and it is what this one makes safe.
- Do not touch `check_derivation`. Egress derivation was deliberately kept
  separate from it for the same reason (Task 2): a change to what context is
  trusted must not be able to change what verification requires.
- Do not attempt PR I/J/Q pricing work. `quoted_credits` stays `None` until
  there is a task-class price list, which is Josh's decision.

## Definition of done

1. Provenance is a type, not a float, and it lives in `cortex_core`.
2. One render path, with an adversarial test proving repository content
   containing an instruction comes out framed as data.
3. A directive found in observed content produces a finding.
4. Assumptions have mechanical checks that run at plan time.
5. Verified evidence survives compaction when everything else is dropped.
6. `context_bundle` / `packed_bytes` are populated per attempt.
7. The PR body states explicitly that the context is still disconnected from the
   worker, and why that is deliberate.
