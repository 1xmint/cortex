# Is the money path safe on today's SQLite? — 2026-08-07

CREDITS.md lists a hard gate:

> Do not ship any of this on SQLite's current `Mutex<Connection>`
> (`ARCHITECTURE.md` §13 specifies a write-actor and read pool; the code has
> neither — 376 blocking lock sites). Two replicas would double-dispatch: pay
> twice, bill once.

That gate was written before V3 existed. This is the assessment it asks for,
against main at `d81eda3f`.

## What is actually true now

**The lock sites are still there, and there are more of them.** `conn.lock().unwrap()`
appears **423** times in `crates/api/src/db.rs` (up from the 376 recorded in
ARCHITECTURE.md). No write-actor, no read pool. That part has not improved.

**But the failure the gate names — "pay twice, bill once" — is now blocked by
three independent mechanisms, none of which depend on a write-actor:**

| Mechanism | Where | What it prevents |
|---|---|---|
| `UNIQUE(run_id, step_id, attempt)` + `ON CONFLICT DO NOTHING` | `claim_verification` | Two verifiers producing a verdict for one attempt. Losing the claim returns `None`, and the driver stops. The ledger key is derived from the verification id, so it is minted exactly once. |
| `idempotency_key TEXT NOT NULL UNIQUE` | `credit_transactions` (v60) | A replayed charge or refund. Re-deriving the same key makes the write a no-op rather than a second row. |
| `complete_step` CAS on `lease_gen` | `ws.rs` completion handler | A superseded delivery reaching the billing path at all — verification is triggered only in the branch where the CAS held. |

Each is a database constraint, not a lock discipline. They hold across
connections and across processes.

**Concurrent connections are already configured for.** `Database::open` sets
`journal_mode = WAL`, `busy_timeout = 5000`, `synchronous = NORMAL`. WAL allows
one writer with concurrent readers; the busy timeout makes a blocked writer wait
up to five seconds rather than fail. This matters because the verification
driver deliberately opens its **own** connection — `AppState` holds `Database`
by value and the websocket handler only borrows it, so a spawned task cannot
share the handle. That second connection is safe by construction, not by luck.

## Recommendation

**Single-node launch: safe to enable refund-on-failure.** The three mechanisms
above make double-charging a constraint violation rather than a race. The
write-actor's absence costs *latency* — 423 serialized lock acquisitions under
load — not correctness of the ledger.

**Multi-replica: still gated.** Two API replicas would double-dispatch *work*
(the scheduler is not leader-elected), which wastes provider spend even though
the customer is only billed once. That is the deployment shape the write-actor
and ARCHITECTURE.md §13 exist for, and it should not ship without them.

**What would change this answer:** any billing path that writes without an
idempotency key, or any verdict path that writes `verification_runs` without
going through `claim_verification`. Both are worth a review gate.

## Still genuinely open

The other CREDITS.md gate — "do not enable refund-on-failure until the verifier
gates on real checks" — is **satisfied**: V2 removed the empty-vec default that
made "verified" mean "the CLI exited 0", and V3 freezes the derived `CheckSpec`s
at dispatch and executes them in the container runner.

What is *not* satisfied is pricing. No per-step credit quote is persisted
anywhere, so `DeliveryFacts.quoted_credits` is always `None` and the driver
records the verdict then skips the ledger. Refund-on-failure cannot be exercised
until a task-class price list exists, which CREDITS.md deliberately leaves to a
human decision.
