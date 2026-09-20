# The Plan Receipt

Status: specified, not built. Depends on V3 (persisted verdicts + refunds) and on
a task-class price list, which is Josh's decision (see CREDITS.md).

---

## The competitive question this answers

The two products people compare Cortex to sit at opposite ends of one axis, and
both are weak in the same place.

**Devin** takes a ticket and returns a pull request. Maximum delegation: its own
VM, its own plan, its own debugging, a human only at review. The weakness is not
autonomy — it is that **the artifact it returns carries no independent evidence**.
A PR is a claim. The reviewer still has to establish, themselves, that the work is
correct. Autonomy that ends in "please check my homework" has moved the work, not
removed it.

**Kiro** goes the other way: a forced planning phase producing requirements,
design and discrete tasks in EARS notation, with three human approval gates. The
weakness is that **the plan is a document, not a commitment**. `tasks.md` is not
priced, is not binding, and names no criterion that will later decide whether the
work passed. You approve prose. What arrives is judged by the same human eyeballs
that judged Devin's PR.

So: Devin gives you autonomy without evidence, Kiro gives you governance without
teeth. Neither can tell you, before work starts, *what it will cost and what it
will be graded against* — and neither can tell you afterwards, from independent
evidence, whether it passed.

Cortex can do both, because V3 gives it something neither has: a verdict produced
by executing checks itself, and a ledger bound to that verdict.

---

## What a Plan Receipt is

The decomposition of a goal into steps, presented for approval like a pull
request, where every step carries four properties. All four are load-bearing;
drop any one and this degrades into `tasks.md`.

1. **Priced.** Each step shows its cost in whole credits, by task class, before
   anything runs. The total is what the customer agrees to. (CREDITS.md: price
   the class, absorb the variance, never quote tokens.)

2. **Checked.** Each step shows the `CheckSpec`s it will be graded against —
   the actual argv, derived from the ecosystem, the contract and the risk level.
   This is the exam, shown before the exam. It is the same frozen set that V3
   later executes in the container runner, not a paraphrase of it.

3. **Lease-annotated.** Each step shows which paths it intends to touch, so path
   conflicts between steps are visible at approval time rather than discovered
   as a mid-run collision.

4. **Binding.** On approval the plan is frozen. Checks cannot be added or
   relaxed afterwards — an agent that can edit its own exam does not have one.
   Changing the plan means a new Plan Receipt and a new approval.

The output artifact of a run is then the **verdict receipt** V3 already builds:
per-step verdict, the checks that ran, their tails, the pinned runner image, and
the ledger entry the verdict produced. Approval and outcome are the same
vocabulary, so a customer can lay them side by side.

---

## Why this is not copyable

Not "hard to copy" — structurally unavailable, for a specific reason each.

- **Pricing a step requires being willing to refund it.** A vendor metering
  tokens, effort or attempts cannot quote a fixed price per step, because their
  margin story depends on passing variance to the customer. Quoting a fixed price
  is a promise to absorb that variance. The 2026 category went the other way.

- **Showing the checks up front requires deriving them without the model.**
  Cortex's derivation is pure and runs before the worker sees the task
  (`cortex_core::check_derivation`). A product whose plan is written *by* the
  agent cannot show a trustworthy exam, because the examinee wrote it.

- **A binding plan requires an independent grader.** Freezing checks only means
  something if something other than the worker executes them. That is V1 + V3.
  Without it, "binding" is a promise the same party both makes and judges.

- **Lease annotations require modelled path ownership.** Cortex already has
  resource leases; a product whose unit of work is "a session in a VM" has no
  object to annotate.

---

## Sketch

Reuses what exists. No new verification machinery.

**Derivation.** `decompose_goal` already produces steps; `derive_checks` already
produces `CheckSpec`s per step (`scheduler.rs`, at dispatch). The Plan Receipt
moves that derivation earlier — to plan time — and persists it, which is what
`verification_specs` (migration v61) already stores. The freeze becomes the
approval, rather than a side effect of dispatch.

**Storage.** One table, `plan_receipts`: `id`, `run_id`, `status`
(`proposed | approved | superseded`), `total_credits`, `created_at`,
`approved_at`. Per-step rows reuse `verification_specs` keyed by
`(run_id, step_id)` — the specs shown at approval are literally the specs
executed later, not a copy that can drift.

**API.**
- `POST /api/runs/plan` → build and return a Plan Receipt; nothing is dispatched.
- `POST /api/runs/plan/{id}/approve` → freeze and start the run.
- `GET /api/runs/{id}/plan` → the receipt as approved.

**UI.** A pane that reads like a PR: steps as the diff, price as the summary,
checks as the status list, leases as the touched-files list. One approve button.
After the run, the same rows gain their verdicts, so the plan becomes the receipt.

---

## Open decisions

- **The task-class price list is Josh's.** CREDITS.md deliberately does not set
  prices ("setting a price before the cost curve is known is precisely the
  mistake that produced every public repricing of 2025–26"). The Plan Receipt
  cannot ship without one, and must not invent one.
- **What happens when a step is `Unverified`.** VERIFIER.md prices it at task
  rate without the verified badge and without the refund promise. The plan must
  say so at approval time, per step, or the badge means nothing.
- **Re-planning mid-run.** A step that fails and is re-planned needs either a
  new approval or a stated allowance. Silent re-planning would void the binding
  property, which is the whole point.
