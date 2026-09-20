# Cortex — Product Vision

> What Cortex is and who it is for.
> Last updated: 2026-08-02. Supersedes [VISION-2026-05-BYOK.superseded.md](VISION-2026-05-BYOK.superseded.md).
>
> **This doc is scope, not status.** For what is actually built, read
> [PLAN-2026-08.md](PLAN-2026-08.md) and the audit files under
> `~/.claude/projects/C--Users-Josh-Desktop-GitHub/cortex-audit/`.
> The previous version carried a self-reported "what's working" table that
> independent review contradicted in several places. Status claims belong where a
> check can falsify them, not in a vision doc.

---

## What Cortex is

**Cortex delivers verified engineering work at roughly half the frontier bill, and
shows you the tests that prove it.**

You describe a task. Cortex decomposes it, routes each piece to whichever model
suits that piece, executes in an isolated sandbox, and **runs real checks against
the result** before calling it done. You pay in credits, per unit of completed
work. You never see a token count, never manage an API key, and are not billed for
a task that failed its checks.

## What Cortex is not

**Not a model gateway.** OpenRouter, Vercel AI Gateway, and Cloudflare already
pass provider tokens through at zero markup. That business is commoditised;
Cortex should not enter it.

**Not "smarter than the best model."** The 2026 evidence does not support that
claim. Routers reach *parity* with frontier models at roughly half the cost — the
best measured result is 75/100 against 74/100 for unrouted Opus, which is inside
the noise band — and a badly calibrated router costs **three times more** than no
router at all. One commercial router measured **−24.7%** against simply using the
best single model.

Claiming "best at everything" would mean selling something we cannot demonstrate.
"Same result, half the bill, and here is the test output" is defensible,
differentiated, and true.

**Not BYOK.** Users do not bring keys. Cortex holds the provider accounts and
sells access to *outcomes*, which is a different product from selling access to
*models* — see [CREDITS.md](CREDITS.md) for why that distinction is load-bearing
rather than cosmetic.

---

## The wedge: verification

Competitors forward your request to a model and return what comes back. None of
them run your test suite and refuse to charge you when it fails.

That gap is the product, and it is the one lever the research consistently
supports: best-of-N sampling only beats single-shot when something can *identify*
the winner. Without an execution-based verifier, parallel attempts give
diminishing returns, because the theoretical ceiling rises with N while your
actual success rate stays gated on picking the right answer. With a verifier,
cheaper models become good enough — which is also what makes the economics work.

So verification is not a feature bolted onto a router. **It is the thing that
makes the router safe to point at a cheaper model.**

This carries a pricing consequence, deliberately accepted: **a task that fails
verification is refunded.** That is what stops "verified" from being a marketing
word, and it aligns incentives — routing cheap and failing costs Cortex twice.

⚠️ Refund-on-failure must not ship before the verifier gates on real checks.
Today `infer_required_checks` (`crates/api/src/scheduler.rs:619`) returns empty for
Execute steps below High risk, so "verified" currently means "the CLI exited 0".

---

## Who it is for

**Primary: developers who want the work done, not the tokens managed.** People who
would otherwise drive Claude Code or Codex directly, and who care about the result
and the bill rather than which model produced it.

**Secondary: teams.** Shared projects, visible task state, and no credential
sharing — because there are no user credentials to share.

> **Josh's call, not settled here:** price points, whether the free NPX tool from
> the old vision still ships, and how hard to lean on teams versus individuals at
> launch. The old `$6.99/mo unlimited` is void — unlimited plans against agentic
> usage produced every public repricing of 2025–26 — but what replaces it depends
> on measured cost (task 1.4 in PLAN-2026-08.md).

---

## The three surfaces

Carried forward from the previous vision, and still right. Three views of one
engine, not three products.

### 1. Project chat — private, per user, per project

Where you work. Talk about the codebase, argue about approach, ask for things.
Cortex classifies intent and routes accordingly: conversation is cheap, real work
costs credits and says so before spending them.

Code work runs in isolated branches and never touches `main` directly.

### 2. Personal task manager — your command centre

Everything in flight, across every project and team. Status queries are database
reads rather than model calls, so "how is my week going" is free and instant.

### 3. Team task manager — shared orchestration

Where private work becomes visible. Everyone sees tasks, status, blockers, and who
holds what. Conversations stay private; **only the work output is shared** — the
git model applied to agent work.

Conflicts are prevented by resource lock: two people cannot hold the same files at
once, and the second is told who has them.

---

## Personality

Cortex is opinionated. It pushes back, notices when you are stuck, remembers how
you work, and recommends rather than enumerating options.

Personality is per-user and constant across model tiers — the system prompt
carries it, so the voice does not change when the router does. That matters more
under outcome pricing than it did under BYOK: if the user cannot see which model
ran, the experience must not visibly change when it switches.

---

## What makes it defensible

1. **Verification nobody else does.** Gateways forward; Cortex checks. The
   evidence — diffs, test output, exit codes — is the product surface.
2. **Outcome pricing.** Routing savings accrue to Cortex rather than to a token
   passthrough. This is the only unit where efficiency is worth building.
3. **Provider neutrality.** No incentive to favour a model — which disappears the
   day a lab invests, and is a reason to be careful who funds this.
4. **Eval data.** Which model actually resolves which class of task, measured on
   real work. That compounds, and cannot be copied in a quarter.

1 and 4 are durable. 2 and 3 are structural but contingent.

Explicitly **not** a moat: unified API, provider failover, model breadth, per-token
price, observability dashboards. All commoditised, several free.

---

## The risks that shape the design

**Provider terms.** Anthropic §D.4 bars reselling the Services except as approved;
§A.1 permits powering your own product. This is precisely why credits are
denominated in verified work rather than tokens — the schema is the argument.
Written clarification from Anthropic is worth having before scaling.

**Rate limits arrive before revenue.** Anthropic's monthly spend caps run Start
$500 / Build $1,000 / Scale $200,000. A $1,000 cap supports roughly 4–8 active
customers, and acceleration limits mean a successful launch day looks like abuse.
Start that conversation before there are customers.

**Margin is real but not SaaS-shaped.** Roughly 45–55% gross before infrastructure
and 35–45% after, compressing as model prices fall. Competing bundles already sell
at $10–18/month, so Cortex must be worth more than a bundle — which returns to
verification.

**Untrusted code on our infrastructure.** Operator-funded execution moves sandboxes
from the user's machine to ours, running model-authored code beside our own
provider keys. That is a genuine cost centre and the strictest isolation
requirement in the system.

---

## Sequencing

Not here. [PLAN-2026-08.md](PLAN-2026-08.md) holds the phased plan, verified
current state, and per-task effort levels. [CREDITS.md](CREDITS.md) holds the
credit unit decision and metering schema.

Splitting vision, plan, and money across three documents is deliberate. The
previous single doc mixed all three, went stale in the status section first, and
then could not be trusted anywhere.
