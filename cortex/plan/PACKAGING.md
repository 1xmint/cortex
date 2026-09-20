# How Cortex is bought

> Decision record. Written 2026-08-05. Companion to [CREDITS.md](CREDITS.md)
> (which defines the unit) and [SURFACE.md](SURFACE.md) (which defines where
> buyers meet the product). This document decides the *structure* of buying —
> tiers, wallets, seats. Price *points* still wait on task 1.4's measured
> costs and Josh's sign-off, exactly as CREDITS.md ordered.

---

## The decision

**One metering unit everywhere — the credit. Seats are identity and access
control, never metering. Money enters as a platform fee (flat, per tier) plus
a credit wallet (pooled, prepaid). The same ledger serves a solo vibe coder
and a 5,000-developer corporation; only the wallet's owner and the controls
around it change.**

## Why seats are the wrong meter

Per-seat pricing for agentic tools failed publicly and repeatedly in
2025–26 (CREDITS.md cites the arc: Cursor's June 2025 repricing and refunds,
Copilot's multiplier jumps). The structural reason: **agentic COGS is
per-task, not per-person**, and usage variance across developers on one team
exceeds 100× — one power user can out-consume their whole org. Seat-flat
pricing therefore either bankrupts the vendor on power users or overcharges
the quiet ones; either way it ends in an emergency repricing that burns
trust. Meanwhile non-pooled per-seat allocations (Cursor Teams' model) make
enterprises pay for unused allowances — the single most-cited procurement
complaint in the category.

Cortex's unit — the verified task — has a property none of the incumbents
can match: **every charge has a receipt.** A CFO can reconcile the invoice
against verification receipts line by line. Pricing delivered work, pooled
across the org, is both the honest model and the audit-friendly one.

The platform fee exists because some costs really are seat-shaped or
org-shaped (SSO, audit-log retention, index storage, support) and because a
flat floor keeps revenue predictable while credits float with usage.

## The three shapes of buyer

| | Solo / vibe coders | Teams (roughly 5–50) | Enterprise (50+) |
|---|---|---|---|
| Who | individuals, hobby groups | startups, mid-size product teams | corporations, many teams, compliance gates |
| What they buy | monthly subscription including a credit allotment; credit packs on top | flat platform fee (seats uncapped within tier), **pooled org wallet**, self-serve | annual committed credit contract at volume discount + enterprise platform fee |
| Wallet | personal (`subscription_remaining` + `pack_remaining` — **already live in the v60 schema**) | org wallet; per-team budget caps (backend task 2.6) | org wallet; per-team budgets, policy controls, invoicing, custom terms |
| Access control | account | roles: admin / member / billing | + SSO/SCIM (WorkOS), audit log, model allowlists |
| Procurement | credit card, self-serve | credit card, self-serve, auto-top-up | sales conversation, invoice, security review (see business track) |
| Overage | buy a pack | auto-top-up with threshold alerts | pay-as-you-go at contract rate |

Continuity is the point: a solo user who starts a company upgrades their
personal wallet into an org wallet; a team that grows into an enterprise
adds SSO and a committed contract. **Nobody ever migrates metering systems**,
because there is only one — the #437 ledger.

## Wallet mechanics

- **Draw order** stays as CREDITS.md fixed it: allotment before packs;
  org-context runs draw the org wallet, personal runs draw the personal one.
- **Team budgets** are spend caps against the org wallet (task 2.6), not
  sub-wallets — one pool, many limits. Sub-wallets fragment liquidity and
  recreate the per-seat waste problem internally.
- **Auto-top-up** with a threshold and an alert is what keeps a 200-dev org
  from stalling mid-sprint; it ships with the Team tier, not after.
- **Expiry/breakage** stays deferred to an accountant (CREDITS.md's
  unclaimed-property warning is still the controlling caution).

## The one schema consequence (flagged now, built later)

`credit_balances` and `credit_transactions` key on `clerk_user_id`
(v60 schema — verified). Org wallets need a **polymorphic wallet owner**
(`user | org`). Decision: introduce `wallet_id` at the Postgres move
(Phase 3.1) where the ledger is being re-homed anyway — do **not** retrofit
SQLite with it. Until then, Team-tier development can prototype against a
designated org-owner user id. This is recorded so 3.1's schema design treats
it as a requirement, not a surprise.

## What not to do

- No per-seat metering, ever, at any tier. If a future partner insists on
  seat-shaped invoices, convert: committed credits ÷ headcount is a line on
  their invoice, not a change to the meter.
- No unlimited tiers. "Unlimited" plus agentic variance is the repricing
  trap with extra steps.
- No pricing page before task 1.4's measured costs and Josh's sign-off on
  points — structure (this doc) and numbers (later) are separate decisions.
- No tier that hides the receipt. Every charge stays reconcilable at every
  tier; that is the brand.
