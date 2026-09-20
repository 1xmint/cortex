# What a Cortex credit is

> Decision record. Written 2026-08-02 against `origin/main` @ `a7bb716e`, grounded
> in the credit code that already exists rather than in a clean-sheet design.
> Resolves contradiction #3 in [PLAN-2026-08.md](PLAN-2026-08.md).

---

## The decision

**A credit buys a unit of completed work, not a quantity of tokens.**

Tokens are a cost input. They are never the customer-facing unit, never quoted in
the UI, and never the denomination of a balance.

The good news, established by reading the code rather than assuming: **this is
already how the system works.** `PULSE_DRAFT_CREDIT_COST: f64 = 1.0`
(`crates/api/src/pulse.rs:185`) charges one credit per draft created — a fixed
price per product action, with token cost invisible to the user. The only
production caller of `deduct_credits` already follows the rule.

So this is not a redesign. It is an extension of an existing convention to LLM
work, plus four defects that must be fixed before real money flows through it.

---

## Why it has to be this way

Anthropic's Commercial Terms (effective 2025-06-17), verified verbatim:

- **§D.4** — *"Customer may not and must not attempt to (a) access the Services to
  build a competing product or service, including to train competing AI models
  **or resell the Services except as expressly approved by Anthropic**"*
- **§A.1** — *"Anthropic gives Customer permission to use the Services, **including
  to power products and services Customer makes available to its own customers
  and end users**"*

Both clauses are real and they draw a line that runs straight through the credit
schema:

| Denomination | What the customer is buying | Reading |
|---|---|---|
| "500,000 Opus tokens" | metered model access | **resale** — §D.4 |
| "one verified task" | a product that happens to use models | **product** — §A.1 |

A `credit_transactions` row saying `-1.0, "1M claude tokens"` is a resale receipt.
A row saying `-3, "verified task: run_9f2c"` is a product receipt. The schema is
the argument.

Two independent reasons this is also just correct commercially:

1. **Per-token markup is worth zero.** OpenRouter, Vercel AI Gateway, and
   Cloudflare all pass provider rates through at cost; OpenRouter monetises a
   5.5% fee on *credit purchase*, not tokens. Competing bundles sell at $10/mo
   (OpenCode Go) and $18/mo (GLM Coding Plan). If Cortex prices tokens, it is
   selling a commodity at a commodity's margin.
2. **Outcome pricing is the only unit routing savings accrue to.** If a task is
   priced at 3 credits and the router serves it for half the token cost, the
   saving is Cortex's. If it is priced per token, the saving is the customer's
   and the router is a cost centre.

This is the same conclusion the research reached from the other direction: the
defensible claim is *verified outcome at half the frontier bill*, not *smarter
routing*. Pricing has to match the claim.

---

## What already exists, honestly assessed

`crates/api/src/db.rs:9685`, `deduct_credits`. Genuinely good work:

- `BEGIN IMMEDIATE` — takes the write lock up front, so concurrent deductions
  cannot lose updates.
- Refuses to invent a balance: no row → `"no credit balance row — unmetered
  (refusing to invent a balance)"`. Fails closed. This is the right instinct and
  most implementations get it wrong.
- `UPDATE`-only, with `updated == 0` checked — a deduction can never create an
  account.
- Insufficient-balance check before mutation.
- Spends `subscription_remaining` before `pack_remaining` — correct ordering,
  since the monthly allotment expires and purchased packs should not.
- Writes a `credit_transactions` row per balance bucket touched.

Four defects, all of which must be fixed before this carries real money:

### 1. Money is stored as `f64` — must become integer

`amount: f64`; `credit_balances.subscription_remaining REAL`;
`credit_transactions.amount REAL`.

Binary floating point cannot represent decimal fractions exactly. Over many small
deductions `subscription_remaining` drifts from `SUM(credit_transactions.amount)`,
and because the balance column is authoritative, **the drift is invisible and
unrecoverable** — there is no reconciliation path that can tell you which number
is wrong.

Fix: credits are integers. Store `INTEGER`, deduct `i64`. If sub-credit precision
is ever needed, add decimal places to the *unit* (1 credit = 1000 millicredits),
never to the *type*.

### 2. No idempotency — retries double-charge

`deduct_credits(user, amount, description)` has no idempotency key.

This is not hypothetical. The orchestrator retries by design: steps carry
`max_attempts: 3` and orphaned steps requeue after lease expiry
(`docs/ARCHITECTURE.md` §5). A step that completes, charges, and then has its
result rejected for a stale `lease_gen` will be re-run and **charged again**.

The primitive to fix it already exists and is unused here: `check_idempotency` /
`set_idempotency` at `db.rs:8023`/`8035`, backed by the `idempotency_keys` table.

Fix: every deduction carries a caller-supplied key. For task work the natural key
is `(run_id, step_id, attempt_id)` — Brain-minted and already unique per attempt.

### 3. The mutable balance is authoritative; the ledger is a side-log

`credit_balances` holds the truth and `credit_transactions` is written alongside
it. That inverts the correct relationship, and combined with (1) it means there is
no way to answer "why is my balance this number".

Fix: the ledger is authoritative and append-only. The balance column becomes a
materialised cache with a reconciliation check — `SUM(ledger) == balance` — run on
a schedule and asserted in tests. Keep the cache; SQLite will not enjoy summing a
ledger on every request. Just stop treating it as the source of truth.

### 4. `reset_subscription_credits` panics on DB error

`db.rs:9784` ends in `.expect("failed to reset subscription credits")`. In a
request path that takes down the handler. Return `Result`.

---

## Target schema

> **Reconciliation — read this before writing code against the names below.**
> The SQL in this section was the *design draft*. PR #437 (migration v60)
> implemented the decision differently in naming and shape, and **the
> implementation is what you build against**:
>
> | Drafted here | Actually shipped in v60 |
> |---|---|
> | new table `credit_ledger` | `credit_transactions` itself, rebuilt (`credit_transactions_v60` → renamed in place) |
> | `delta INTEGER` | `amount INTEGER` |
> | `bucket` | `balance_type` |
> | `user_id` | `clerk_user_id` |
> | `credit_balances.*_remaining_i` | `credit_balances` rebuilt as `credit_balances_v60` → renamed, integer columns keep their original names |
>
> `idempotency_key TEXT NOT NULL UNIQUE`, the `reason` column, and `run_id` /
> `step_id` all shipped as designed; pre-existing rows carry
> `reason = 'legacy'` and `idempotency_key = 'legacy:' || id`. `provider_spend`
> shipped as written. **Every decision in this document holds** — integers,
> idempotency, append-only ledger, two-ledger separation — only the names
> changed, and rebuild-in-place replaced the additive-column approach below.
> [VERIFIER.md](VERIFIER.md)'s billing binding already uses the shipped name.

Additive migration. Nothing below drops a column; the existing REAL columns are
kept until the integer columns are backfilled and reconciled.

```sql
-- Append-only. This is the truth.
CREATE TABLE credit_ledger (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL,
    -- Signed, in whole credits. Negative = spend, positive = grant.
    delta           INTEGER NOT NULL,
    bucket          TEXT NOT NULL,      -- 'subscription' | 'pack'
    reason          TEXT NOT NULL,      -- 'task_verified' | 'task_failed_refund'
                                        -- | 'pulse_draft' | 'grant_monthly'
                                        -- | 'grant_purchase' | 'adjustment'
    -- Exactly-once. For task work: run_id:step_id:attempt_id.
    idempotency_key TEXT NOT NULL UNIQUE,
    run_id          TEXT,
    step_id         TEXT,
    created_at      INTEGER NOT NULL
);

CREATE INDEX idx_credit_ledger_user ON credit_ledger(user_id, created_at);

-- Materialised cache of SUM(credit_ledger.delta) per bucket. Not authoritative.
ALTER TABLE credit_balances ADD COLUMN subscription_remaining_i INTEGER;
ALTER TABLE credit_balances ADD COLUMN pack_remaining_i         INTEGER;
ALTER TABLE credit_balances ADD COLUMN ledger_checked_at        INTEGER;
```

**Separately, and never in the same table — what it cost us:**

```sql
-- COGS. Token-denominated, provider-denominated, internal only.
-- Never surfaced as a customer balance. This is the table that answers
-- "is our pricing above cost", and it is the reason the two must not be merged.
CREATE TABLE provider_spend (
    id                TEXT PRIMARY KEY,
    user_id           TEXT NOT NULL,
    run_id            TEXT,
    step_id           TEXT,
    provider          TEXT NOT NULL,      -- claude | openai | gemini | ...
    model             TEXT NOT NULL,
    cost_type         TEXT NOT NULL,      -- 'platform' | 'byok' | 'byos'
    tokens_in         INTEGER NOT NULL,
    tokens_out        INTEGER NOT NULL,
    tokens_cached_in  INTEGER NOT NULL DEFAULT 0,
    -- Micro-USD. Integer, from provider-reported usage, never estimated
    -- once the response is in hand.
    cost_micro_usd    INTEGER NOT NULL,
    created_at        INTEGER NOT NULL
);

CREATE INDEX idx_provider_spend_user ON provider_spend(user_id, created_at);
CREATE INDEX idx_provider_spend_run  ON provider_spend(run_id);
```

### The two-ledger rule

**`credit_ledger` is what the customer owes. `provider_spend` is what we paid.
They are never the same row and never the same unit.**

That separation is not bookkeeping fastidiousness — it is what makes this a
product rather than a resale. The moment a credit is defined as a function of
tokens consumed, the customer is buying tokens again and §D.4 is back in play.
It is also the only way to see margin per task class, which is the number that
tells you whether the pricing works.

`cost_type` gains `'platform'`, which does not exist today —
`CostEstimator::get_cost_type` (`crates/api/src/cost_estimator.rs:96`) returns only
`"byok"` or `"byos"`, which is why operator-funded spend currently has nowhere to
be recorded. See PR #415.

---

## Pricing

**Not decided here, and deliberately so.**

Prices must be calibrated against measured cost. That measurement does not exist
yet — task 1.4 in [PLAN-2026-08.md](PLAN-2026-08.md), roughly $300 to instrument
3–5 representative tasks. No credible public figure exists for what a real project
costs on a tuned harness; the numbers in circulation are consulting estimates, not
token costs.

Setting a price before the cost curve is known is precisely the mistake that
produced every public repricing of 2025–26 — Cursor's June 2025 reversal and
refunds, Copilot's Opus multiplier going 7.5× → 27×, Replit's move to effort-based.
Each followed the same arc: price set on intuition, agentic usage exceeded it,
emergency repricing, trust damage.

What *is* decided:

- Credits are integers. A task costs a whole number of them.
- Task classes are priced, not tasks. A small fixed set — a user should be able to
  predict the cost before pressing go.
- **A failed task is refunded.** If verification does not pass, the credits return.
  This is what makes "verified" the product and not a marketing word, and it is
  the mechanism that aligns the router's incentives with the customer's: cheap
  routing that fails costs Cortex twice.
- The monthly allotment stays (`subscription_remaining`, currently 200) and packs
  stay (`pack_remaining`). Spend order stays: allotment first.

---

## Consequences to accept

**Prepaid credits are a liability, not revenue.** Unspent balances are deferred
revenue and may fall under US state unclaimed-property law. Recognising breakage
early is a restatement risk. Talk to an accountant before designing expiry — cheap
now, expensive after an audit.

**Refunds make the verifier load-bearing.** If verification is weak, refunds either
leak money (false failures refunded after real spend) or erode trust (false passes
charged). Today `infer_required_checks` (`crates/api/src/scheduler.rs:619`) returns
empty for Execute steps below High risk, so "verified" currently means "the CLI
exited 0". **Refund-on-failure must not ship before that is fixed.**

**Margin is not SaaS margin.** Roughly 45–55% gross before infrastructure and
35–45% after, and it compresses as model prices fall. That is a real business, and
it is not the business a flat-rate SaaS plan describes.

---

## What not to do

- Do not quote tokens in the UI, in receipts, or in the API. Not "you used 40k
  tokens" — "this task cost 3 credits".
- Do not compute credit cost as a function of observed token usage. That is
  token resale with an exchange rate. Price the class; absorb the variance. If a
  class's variance is unbearable, split the class.
- Do not let `provider_spend` and `credit_ledger` share a table, a unit, or a
  migration.
- Do not enable refund-on-failure until the verifier gates on real checks.
- Do not ship any of this on SQLite's current `Mutex<Connection>`
  (`ARCHITECTURE.md` §13 specifies a write-actor and read pool; the code has
  neither — 376 blocking lock sites). Two replicas would double-dispatch: pay
  twice, bill once.
