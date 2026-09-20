# ADR-0001 — A worker's report of success is a diagnostic, not a transition guard

**Status:** accepted
**Date:** 2026-08-09
**Supersedes:** the V3 launch guidance that deliberately omitted a `verifying`
state to keep the model smaller.

## Context

Until this decision, a step succeeded because the worker that ran it said so.
On a `StepCompleted` message the API wrote `UPDATE steps SET status =
'succeeded'` and the step was done. `StepStatus` had no state between `Running`
and `Succeeded`, and `Succeeded` was terminal. Independent verification existed —
it ran the frozen checks against the delivered commit and recorded a verdict —
but it ran *afterwards and off to the side*, and nothing waited for it.

Everything downstream keyed off that self-reported success: the UI said done, a
dependent write step unblocked, a branch became eligible for a pull request, the
routing layer learned that the model had done well, and — once pricing exists —
a charge would fire.

This is the central product claim inverted. Cortex sells verified work. A system
whose completion signal is the worker's own opinion of its work sells a claim it
does not check.

## Decision

**A worker's report is evidence about a delivery. It is never the thing that
makes a step succeed.**

The step lifecycle is exactly:

```text
planned -> queued -> leased -> running -> delivered -> verifying
                                              |            |
                                              |            +-> verified -> review/done
                                              |            +-> failed -> retry/failed
                                              |            +-> inconclusive -> retry/attention
                                              +-> execution_failed -> retry/failed
```

with one state off to the side:

```text
                                        (any) -> manual_override
```

Each state means one thing and only that thing:

| State | Means |
|---|---|
| `delivered` | The sandbox produced a commit. Nothing has been checked. |
| `verifying` | Cortex's own runner is executing the checks frozen at dispatch. |
| `verified` | Those checks were executed by us and the required ones passed. |
| `failed` | Those checks were executed by us and a required one did not pass. |
| `inconclusive` | We could not obtain a verdict. Our problem, not the customer's. |
| `execution_failed` | The delivery never happened — the sandbox or the harness failed. |
| `manual_override` | A human decided to ship it anyway. A real and different fact. |

Nothing may treat `delivered` or `verifying` as success: not the UI, not a
dependent write step, not pull-request creation, not the ledger, not a routing
reward.

### Why `verifying` exists as a state rather than as a background detail

Because it is the honest answer to "what is happening to my step right now", and
because the alternative is a lie of omission. If the only states are `running`
and `succeeded`, then during verification the product must either claim the step
is still running (false — the work is delivered) or that it succeeded (false —
nothing has been checked). The Operations Room contract already defines
`verifying`; the data model simply did not have it.

### Why `inconclusive` is first-class and never collapses

An inconclusive verdict means our infrastructure failed to produce an answer.
Folding it into success charges a customer for a claim we did not verify.
Folding it into failure blames the customer for our outage. It stays its own
state, visible, and it is never silently resolved by a timeout — a timeout that
invents a verdict is the same fault as trusting the worker, one layer down.

### Why `manual_override` is not `verified`

A human choosing to ship unverified work is legitimate and routine. Recording it
as `verified` would make the verification badge mean two different things, one
of which is "somebody decided". The override carries actor, reason, and expiry
so it can be audited and so it does not silently become permanent.

### Why every transition is one transaction with a CAS

Two rules, both learned from the shape of bugs this system already had:

1. **The projection and its operations event are written in the same
   transaction.** A step whose status moved without an event is invisible to the
   audit trail; an event without a projection is a claim about a state that
   never existed. Either one alone is the bug.

2. **Every transition carries the `lease_gen` and a version CAS.** `lease_gen` is
   already the attempt discriminator, and the existing `complete_step` guard
   (`AND lease_gen = ?7 AND status IN ('leased','running')`) is the reason a
   stale delivery cannot bill today. That guard is extended, not replaced. A
   stale delivery *and* a stale verifier result must both be no-ops against an
   attempt that has moved on.

### What happens to the old worker report

It is kept, never deleted, and surfaced only under the label
**"Worker-reported diagnostics"**. It is genuinely useful — it is the fastest
signal about what the worker thought it did — and it can never render as a
verification badge or emit a positive routing reward.

## Consequences

**Historical rows become `delivered`, not `verified`.** The migration rewrites
every existing `succeeded` step to `delivered`. Those steps were never
independently verified, and labelling them `verified` would be a false claim
about work already delivered to customers. Anyone reading old runs will see
fewer verified steps than they saw yesterday. That is the correct regression:
the number did not change, the honesty of the label did.

The `UPDATE` is forward-only. Reversing it means re-asserting a claim that was
never true, so the rollback path is the additive tables dropping cleanly and the
status column staying where it is.

**A step can strand in `verifying`.** The verifier is still launched with
`tokio::spawn`. If the process dies between the transition and the verdict, the
step sits in `verifying` with nothing to resume it. This decision does not fix
that, and deliberately does not paper over it with a timeout that fabricates a
verdict. Durable verification jobs — claim, lease, heartbeat, recovery — are the
next change.

**A run cannot report success until every step is verified.** `Succeeded` is
gone from the step states, so `check_run_completion` cannot see an all-terminal
run whose steps were merely delivered.

**`success_required` edges serialise more than they did.** A dependent unblocks
on `verified`, not on `delivered`, for every edge — including dependents that
only read. The scheduler cannot currently establish that a dependent does not
write, and unknown scope is never optimistically unblocked.

## Alternatives rejected

- **Keep `succeeded` as an alias for `delivered`.** Rejected: an alias is a
  second name for the state that caused the problem, and it hides every
  remaining consumer from the compiler. Removing the variant is what makes the
  build enumerate the call sites.
- **Verify synchronously inside the completion handler.** Rejected: checks run
  in a container, and the websocket handler must not hold a connection open for
  container time.
- **Time out `verifying` into `failed`.** Rejected: that charges the customer
  for our outage, and it is the same mistake as trusting the worker.
