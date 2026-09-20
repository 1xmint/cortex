# The product surface

> Decision record. Written 2026-08-05. Companion to [VERIFIER.md](VERIFIER.md)
> and [CONTEXT.md](CONTEXT.md) — those define what Cortex does; this defines
> where a customer meets it. Phase 4 of [PLAN-2026-08.md](PLAN-2026-08.md),
> given its scope at last.

---

## The decision

**Cortex is bought at the repository, not at a chat box. The primary surface
is a forge integration (GitHub first) whose unit of delivery is a pull
request carrying a verification receipt. The web app is mission control, not
the product. Everything else — CLI, Slack, IDE — is a doorway into the same
run/receipt machinery.**

The one-sentence adoption story this buys, which is the story Josh asked
for: *install the GitHub App on your org, pick repositories, and the next
issue you assign to Cortex comes back as a PR with a "Cortex Verified —
14/14 checks passed" status in the merge box.* No editor changes, no new
workflow, no training. Team drops in; superpowers appear where the team
already lives.

## Why the forge is the primary surface

1. **The verifier maps 1:1 onto forge-native UI.** GitHub Check Runs render
   pass/fail with detail pages — exactly the shape of
   `verification_runs`/`verification_checks` (VERIFIER.md). The receipt
   becomes a commit status. Cortex's core differentiator displays inside
   GitHub's own merge box with zero UI invention. GitLab's commit statuses
   are the same shape — hence a `Forge` trait from day one, GitHub
   implementation first.
2. **Enterprise adoption physics.** A 200-developer org will not migrate
   editors (the market data is unambiguous — the lowest-friction tools win
   adoption regardless of capability). An org-level app install is one
   decision by one admin; an editor plugin is 200 individual decisions.
3. **Coherence lives at the forge.** Semantic leases (CONTEXT.md C4) surface
   as PR annotations: "held: `validateToken` via run #412 (checkout-service →
   auth-lib), releases ~14:30." The 200-developer story is *visible at the
   place merges happen*, which is where collisions hurt.

## Surface ranking and roles

| Rank | Surface | Role | Status today |
|---|---|---|---|
| 1 | **GitHub App** (→ `Forge` trait → GitLab later) | delivery: issues/PRs in, verified PRs + check-run receipts out | does not exist |
| 2 | **Web app** (cortex.heyvera.org) | mission control: runs, receipts, ledger, leases, org admin, launcher | exists as BYOK-era chat shell; needs triage |
| 3 | **CLI + MCP** | power users and other tools; MCP context surface is C5 | CLI paths exist server-side |
| 4 | **Slack** | notifications + kickoff (`/cortex fix #1234`) | does not exist |
| 5 | IDE plugins | **deliberately deferred** — MCP serves IDEs for free; a first-party plugin is a Phase 5 luxury | — |

## The web app, honestly triaged

What exists (`cortex/src`, ~24.7k LOC, unaudited by PLAN §7): a 1,090-line
`App.tsx` chat shell with rooms/groups, plus components from the superseded
BYOK era — `ReplitProjects.tsx`, marketing panels, cross-project badges.
It predates every decision now on main.

Its future job — mission control — has exactly five surfaces, each backed by
an API that exists or is on the build plan:

1. **Runs** — DAG view of steps, live state (SSE, per PLAN §6), each
   completed step showing its receipt (verdict, check tails).
2. **Receipts** — the trust page: every verification, replayable evidence.
   One receipt schema rendered here, in the PR check, and in Slack —
   same object, three renderings, never diverging.
3. **Ledger** — credits, spend, refunds, straight from `credit_transactions`
   (#437). No invented numbers: the balance shown IS the ledger sum.
4. **Leases** — who holds what surface, why (graph path), until when. The
   org's coherence dashboard; this page is the enterprise demo.
5. **Admin** — members (WorkOS SSO at org creation), roles
   (admin/member/billing), policy (model allowlist, per-team spend caps —
   backend 2.6), API keys.

The chat/launcher remains as the sixth pane — a *doorway*, not the identity.
Everything BYOK-era that serves none of these six gets amputated in the
audit, not redecorated.

## The drop-in path (the onboarding contract)

The metric that governs all surface work: **time from org install to first
verified PR — target under 10 minutes.**

1. Admin installs GitHub App on the org, selects repositories (2 min)
2. Cortex indexes selected repos (CONTEXT.md C2; repo map available in
   seconds, full graph in background)
3. Any developer assigns an issue to Cortex, comments `@cortex`, or uses the
   web launcher
4. Cortex plans (visible as a checklist on the issue), executes, opens a PR
   with the verification check attached
5. Billing: the org bought credits once at install; each verified task
   draws down per CREDITS.md — the receipt and the charge reference each
   other

## Cost confidence (a surface requirement, not a nicety)

Developers' strongest objection to agentic tools is not capability, it is
**bill anxiety** — the fear of a runaway loop turning into a surprise
invoice. Cortex's pricing model already removes the cause: prices attach to
task classes, so token variance is Cortex's COGS problem and never reaches
the customer, a run that fails verification refunds, and infra failures
never charge (CREDITS.md). But *structural* safety that a user cannot see
does not calm anyone. Every surface must therefore show it:

- **Pre-run confirmation.** Before a run starts: task class, credits it will
  cost, balance after. Rendered in the web launcher, as an issue comment
  from the GitHub App, and in the CLI. No run begins on an unseen price.
- **A per-run ceiling.** A run that would exceed its quoted class stops and
  asks rather than continuing — the user's worst case is bounded by the
  number they already saw.
- **Org spend caps, visible.** Per-team caps exist in the backend (2.6);
  Admin must expose them and the Ledger must show consumption against them.
- **No estimate the ledger cannot back.** The quoted price and the charged
  price are the same number, or it is a bug.

Applies across F2 (launcher), F3 (GitHub App comments), and F6 (Slack) —
the same quoted-price object, three renderings, like the receipt.

Enterprise gates that are business work, not code — tracked so nobody
mistakes them for engineering backlog: SOC 2 / ISO certification track,
IP-indemnity posture, data-handling page for the index (CONTEXT.md's
embeddings caveat). **Josh owns starting these conversations; they have
months of lead time and gate six-figure deals.**

## What not to do

- Do not restyle the chat shell before the audit amputates the BYOK-era
  surface — polish on the wrong information architecture is negative work.
- Do not build the GitHub App's delivery loop before V1–V4 exist; a check
  run with no real verdict behind it is the hollow-gate disease (PLAN
  standing rule 2) shipped to customers.
- Do not build a first-party IDE plugin in 2026.
- Do not let any surface show a number the ledger cannot back.
- WebSockets stay out of the frontend (SSE per PLAN §6); the WS
  infrastructure is for workers.

## Build plan (hand-off — Lane C)

| # | Task | Effort | Depends on |
|---|---|---|---|
| F1 | Frontend audit + amputation: map `cortex/src` against the six panes, delete BYOK-era surface, land the IA skeleton | Opus 5 · high | nothing — start any time |
| F2 | Mission control on real APIs: Runs (SSE), Ledger (from #437 tables), Receipts | Opus 5 · high | V3–V4 for receipts |
| F3 | GitHub App: org install flow, webhook security, issue/PR ops, check-run receipts, `Forge` trait boundary | Opus 5 · xhigh | V1–V4 |
| F4 | Onboarding: install → index → first verified PR, instrumented against the 10-minute target | Opus 5 · high | F3, C2 |
| F5 | Leases board + PR lease annotations | Opus 5 · high | C4 |
| F6 | Slack app (kickoff + receipt notifications) | Sonnet 5 · medium | F3 |
| F7 | GitLab forge implementation | Opus 5 · high | F3 stable |

Sequencing: F1 immediately (it is also the frontend audit PLAN §7 still
owes). F2 when receipts exist. F3 is the product launch surface — it gets
the most careful session of the entire plan after the verifier itself.
