# Frontend audit — `cortex/src`

> Task F1 ([SURFACE.md](SURFACE.md)) and the audit [PLAN-2026-08.md](PLAN-2026-08.md)
> §7 recorded as owed after a session limit killed it. Written 2026-08-05
> against `origin/main`, with a green `tsc -b && vite build` before and after
> every change described here.

---

## Headline

One finding outranks everything else in this document, and it is not a
cosmetic one.

**The shipped settings UI offered ordinary users a provider *subscription*
credential flow, and that was the default.** `CredentialsTab` initialised
`addType` to `'subscription'`; the toggle that could switch it to `api_key`
was wrapped in `isAdmin &&`. So a non-admin clicking "+ Claude (Anthropic)"
started a subscription OAuth connection.

That is the exact path Anthropic's February 2026 terms update prohibits —
subscription (Free/Pro/Max) tokens used inside third-party tools, enforced
from 2026-04-04 — and it is the practice PLAN §3 cites as having ended a
whole product category on six weeks' notice. It also contradicts the model
every plan document is built on: Cortex holds operator-funded API keys and
sells credits.

Fixed in this pass: the subscription path is gone from the UI, `addType` is
a constant `'api_key'`, and the admin-only section says plainly that
subscription credentials are not supported. **The backend `startAuth`
subscription flow still exists and should be removed in the Rust workstream**
— this pass could not verify backend behaviour without a compile, which this
machine cannot do (handoff standing rule: CI is the only verifier).

## Inventory

| | Before | After |
|---|---|---|
| Files (`.ts` / `.tsx`) | 101 | 79 |
| Lines | 26,764 | 23,793 |
| Client bundle (gzip) | 75.42 kB | 72.60 kB |
| CSS (gzip) | 15.12 kB | 14.11 kB |

PLAN §1.1 recorded `cortex/` at ~24.7k LOC. That figure counted only source;
the difference above is measurement scope, not code appearing from nowhere.

## What was removed, and why each

**Dead by proof, not by opinion.** Every file below was checked for inbound
`import` and dynamic `import()` references across the whole tree; each had
zero. They were not reachable from any route, lazy boundary, or barrel.

| File | Lines | Note |
|---|---|---|
| `components/marketing/HomePage.tsx` | 344 | marketing relic, named in SURFACE.md |
| `components/work-surface/WorkSurface.tsx` | 378 | App.tsx already carried the comment *"WorkSurface hidden for clean interface"* |
| `components/session/SessionControls.tsx` | 231 | BYOK-era session controls |
| `components/chat/ProviderSetup.tsx` | 212 | user-supplied provider setup — BYOK by definition |
| `components/onboarding/GitHubStep.tsx` | 162 | superseded by the GitHub App install flow (F3) |
| `components/sovereignty/SovereigntyLoopPanel.tsx` | 143 | Soma-era concept, no Cortex meaning |
| `components/billing/ReferralInput.tsx` | 115 | |
| `lib/mockChat.ts` | 100 | mock data in the production bundle |
| `components/shell/StatusBar.tsx` | 95 | |
| `components/RoutingIntel.tsx`, `ProviderHealth.tsx`, `PerformanceChart.tsx`, `LiveFeed.tsx` | 268 | pre-decision dashboards; the Runs pane rebuilds against real APIs in F2 |
| `components/SomaIdentityBadge.tsx` | 67 | Soma-era |
| `components/ActiveRooms.tsx`, `chat/ChatHeader.tsx` | 92 | superseded chat shell parts |
| `components/billing/PaymentFailed.tsx` | 53 | |
| `lib/state.ts`, `components/cost/index.ts`, `components/settings/index.ts` | 45 | empty or unused barrels |

**Routed, and removed deliberately:** `components/ReplitProjects.tsx` (399)
and `lib/replitProjectApi.ts`, plus the `/replit-projects` route and its
import in `App.tsx`. SURFACE.md names this one explicitly.

## What was deliberately *not* removed

Recorded so the next session does not have to re-derive the reasoning:

- **The Replit *connector*** inside `components/integrations/IntegrationSetup.tsx`
  and its API types in `lib/cortexApi.ts` (`getReplitWorkspaces`,
  `importReplitWorkspace`, `ReplitWorkspace`). Removing these means removing
  backend endpoints too — a Rust change, not a frontend one. Same for the
  "GitHub, Slack, and Replit integrations" copy in `BillingPage.tsx` and
  `PricingCards.tsx`, which should die with the pricing page rewrite, not
  before it.
- **`SettingsPanel.tsx` (986 lines) as a whole.** Its `providers`,
  `apikeys`, and `credentials` tabs are all BYOK-shaped, but under
  operator-funded keys an *operator* still needs somewhere to add keys. What
  it should become is an Admin-pane concern (SURFACE.md pane 5), which is IA
  work, not deletion work.
- **`components/authority/`, `components/personal/`, `components/groups/`.**
  These belong to the chat-shell doorway (pane 6) and cannot be judged until
  the IA skeleton exists.

## The six panes, mapped against what exists

SURFACE.md defines mission control as six panes. This is the honest state of
each after the amputation:

| Pane | Existing code | Verdict |
|---|---|---|
| 1. **Runs** | `runs/RunPanel.tsx` (739), `tasks/OperationsGraphPanel.tsx` (494), `operations/OperationsRoom.tsx` (503) | **Real material.** A DAG panel and an operations room already exist; they need rewiring to SSE and receipts (F2), not rewriting. |
| 2. **Receipts** | *nothing* | **Absent by necessity** — the verifier does not exist yet (V1–V5). This is the trust surface and it is empty. |
| 3. **Ledger** | `ledger/LedgerView.tsx`, `usage/UsageView.tsx`, `spend/SpendDashboard.tsx`, `billing/*`, `cost/*` | **Five overlapping surfaces for one concept.** They predate the credit decision and none reads `credit_transactions`. Consolidate to one in F2. |
| 4. **Leases** | `resources/ConflictViewer.tsx` | **A real seed.** Conflict rendering exists; C4's semantic leases give it something worth showing. |
| 5. **Admin** | `admin/AdminView.tsx`, `admin/PromoCodeManager.tsx` (536), `SettingsPanel.tsx` (986), `settings/BudgetSettings.tsx` | **Exists, misfiled.** Admin concerns are spread across a settings modal and an admin panel; SURFACE wants one pane. `BudgetSettings` is where per-team spend caps (backend 2.6) will surface. |
| 6. **Chat / launcher** | `App.tsx` (1,089), `chat/*`, `tasks/*`, `groups/*`, `shell/*` | **The current identity of the app, and it should stop being that.** SURFACE is explicit: the chat shell is a doorway, not the product. |

## Honest read on what is left

The app is **not** a hollow BYOK shell — that framing, carried since the
BYOK-era assessment, is too harsh. There is real, non-trivial machinery here:
a DAG operations view, a task board with an inspector, a conflict viewer, a
command palette, resizable panels, an admin panel, and a 2,300-line typed API
client. What is wrong is **arrangement and wiring**, not substance: five
different ledger-ish views none of which read the ledger, an admin surface
split across two entry points, and a chat shell occupying the position the
run/receipt machinery should hold.

That changes the F2 estimate in a useful direction — mission control is more
a re-composition than a green-field build.

## The IA skeleton (built, and checked in a browser)

`components/mission/MissionControl.tsx` is a layout route wrapping six panes
at `/runs`, `/receipts`, `/ledger`, `/leases`, `/admin`, and `/` (chat).
`Ledger` and `Admin` mount the components that already exist; `Runs`,
`Receipts`, and `Leases` render an honest stub naming the task that fills
them — an empty pane that explains itself beats a screen of numbers the
ledger cannot back.

Verified against a running dev server, not just the compiler: every pane
routes, the nav renders all six links, and the chat shell still loads inside
the layout. (`/api` calls 502 locally — there is no Rust backend on this
machine — which is expected and unrelated.)

Two things the live check surfaced that the file tree did not:

- **Two sidebars.** `CortexShell` brings its own full-height sidebar
  (GROUPS / Task managers / Projects), so the chat doorway now renders a nav
  inside a nav. This is the visible form of the problem SURFACE.md describes:
  chat still behaves like the identity of the app. F2 resolves it by moving
  chat's own navigation into the pane nav.
- **Subscription-era copy is still live.** The shell renders *"You're in
  Preview mode. Subscribe to unlock full AI agent capabilities."* — a
  BYOK/subscription-tier string that contradicts the credit model. It should
  die with the pricing surface rewrite, and it is a good example of why Josh
  reading the real screens catches what a file audit does not.

## What F1 leaves for the next session

1. **Splitting `App.tsx`.** It is still ~1,100 lines holding routing, the
   shell, and every modal. The pane routes are extracted; the shell is not.
2. **Ledger consolidation** — one pane reading `credit_transactions` (the
   real table name — see CREDITS.md's reconciliation note), replacing five
   views that invent numbers.
3. **Backend subscription-credential removal** — the UI path is closed; the
   `startAuth` subscription flow underneath it is not.
4. **Lint debt.** `npm run lint` reports 9 problems (6 errors) after this
   pass, down from 15 (12 errors) before it. The `cortex` CI job runs
   `build` only, so lint is not gating — worth making it gate once the count
   reaches zero.
