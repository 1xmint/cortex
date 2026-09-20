# Opus 5 handoff — the one path

> Written 2026-08-05 by the Fable 5 session that closed the architecture
> phase. This is the map, not a spec — every spec it points to is merged and
> cited. If you are an Opus 5 session and you are lost, come back here.

## Read first, in this order

1. [PLAN-2026-08.md](PLAN-2026-08.md) — ground truth, contradictions resolved, phase plan
2. [CREDITS.md](CREDITS.md) — what a credit is; the two-ledger rule
3. [VERIFIER.md](VERIFIER.md) — what "verified" means; build plan V1–V7
4. [CONTEXT.md](CONTEXT.md) — the context engine; build plan C1–C5
5. [SURFACE.md](SURFACE.md) — where customers meet Cortex; build plan F1–F7
6. [PACKAGING.md](PACKAGING.md) — how Cortex is bought; credits pooled, seats never metered
7. `docs/ARCHITECTURE.md` — the implementation spec. **Read its
   "Amendment 2026-08-02" table first**: it marks section by section what
   survives operator-funded keys, what is revised, and what the code
   refuted. Trust the amendment over any section below it.
8. This file — sequencing, standing rules, stop conditions

Two names that will not match your memory of the docs: the authoritative
ledger table is **`credit_transactions`** (rebuilt as integers by v60), not
`credit_ledger` as CREDITS.md's draft SQL called it, and its columns are
`amount` / `balance_type` / `clerk_user_id`. CREDITS.md now carries the
reconciliation table; the decisions are unchanged, only the names.

## Standing rules (violating any of these is how sessions get lost)

- **The migration counter is a single integer.** v53–v59 belong to
  `fix/socials-message-integrity` (PR #438), v60 to credits (PR #437), v61 is
  reserved for the verifier tables (V3). Never mint a migration number
  without checking `apply_migrations` on main AND every open PR that touches
  `crates/api/src/db.rs`. Merge order: #438 before #437 before any v61 work.
- **CI is the only verifier for Rust — re-tested 2026-08-05, still true, and
  here is exactly why so nobody re-tests it a third time.** `rustc`/`cargo`
  1.97.1 *are* installed and work; what fails is linking:
  `lld: error: unable to find library -lgcc_eh` / `-lgcc` (MinGW GNU
  toolchain present, its GCC support libraries are not). `cargo check` does
  not save you — the workspace's proc-macro and build-script crates
  (`proc-macro2`, `getrandom`) must be *linked and executed* before checking
  can proceed, so `cargo check -p cortex-engine` dies in the same place.
  Fixing this means installing MSVC Build Tools and switching to the
  `x86_64-pc-windows-msvc` target. Until someone does, push a branch, open a
  PR, read the `rust` job. The three advisory jobs (cargo-deny, npm-audit ×2)
  fail on every PR until their triage lands — they are not your failure.
- **The frontend, by contrast, verifies locally and should.** `npm ci` and
  `npm run build` (which is `tsc -b && vite build`) work in `cortex/`, and
  the dev server runs — F1 was checked in a real browser. Never push
  frontend work you have not built.
- **Commit early, push WIP.** Task #7 survived a quota cutoff only because
  its scratchpad happened to survive (backup:
  `C:\Users\Josh\Desktop\GitHub\task7-credit-idempotency-base2bfa7a9.patch`).
  Never hold >1 hour of work uncommitted.
- **Nothing grades its own homework** (PLAN standing rule). Applies to you:
  claim done only what a check confirms.
- **Branch per task, PR to `main`, non-draft only when mergeable.**
  Auto-merge arms on non-draft PRs with green required checks
  (`cortex`, `heyvera`, `rust`).
- **This repo holds two products; you own one.** Cortex sessions work
  `cortex/**` and the Rust backend. HeyVera Socials (PR #438, branch
  `fix/socials-message-integrity`) is handled in a HeyVera conversation —
  never review, merge, or rebase it from a Cortex session. The only thing
  that crosses the line is the shared migration counter, and it crosses as a
  wait, not as work.
- **Pricing and customer-facing decisions belong to Josh.** Flag, don't decide.

## The one thing blocking the most — read before picking up work

**#438 is `CONFLICTING`, and that is why everything downstream is stuck.**

It is not a checks problem, so **auto-merge cannot help**: auto-merge waits
for checks, it cannot resolve a conflict, and GitHub's own server-side
updater refuses too (`gh pr update-branch 438` → *"Cannot update PR branch
due to conflicts"*). Someone has to merge `main` into that branch locally and
push. The conflict itself is trivial — one `use crate::social_policy::{…}`
import that #438 has and `main` does not; keep it.

The chain it holds up, in the order it must happen:

    #438 → #437 (v60) → host migration → #411 → Deploy Production

Two of those steps are ordering traps rather than preferences:

- **#437 after #438**, or the shared migration counter silently skips
  v53–v59.
- **The host migration before #411 merges and before any deploy.** Deploys
  run `git reset --hard` on the production checkout, and the live database
  is still tracked inside it.

## Definition of done — "ready for external users"

The lanes below are not the goal; this is. Every task exists to move a row
here from no to yes, and a lane that is "finished" while a row is still no
is not finished.

| # | A stranger can… | Gated on |
|---|---|---|
| 1 | reach a Cortex that is running current code | A0 (deploy unfrozen and firing) |
| 2 | sign up and land somewhere coherent | F1, F4, WorkOS (3.6) |
| 3 | connect a repo and start a task | F3 (GitHub App), C1–C2 |
| 4 | see what it will cost before it runs | SURFACE cost-confidence section |
| 5 | get work back with a receipt they believe | V1–V5 |
| 6 | be charged correctly, and refunded when it fails | #437 live, V4, **V7 before refund copy ships** |
| 7 | pay Cortex at all | JOSH-ACTIONS §5 — entity, ToS, Stripe |
| 8 | trust it with a real repo | 3.4 sandboxes, 2.7 secret scanning, red-team pass |

Row 7 is a Josh gate with multi-day external lead times, so it starts in
parallel with the build, not after it. Row 6's V7 dependency is the one
place where shipping early is actively harmful — a refund promise on an
unhardened verifier either leaks money or breaks trust.

**Josh is the acceptance test for rows 2–5** ([JOSH-ACTIONS.md](JOSH-ACTIONS.md)
§6). Ask for his pass before calling those done; an agent cannot judge whether
a first-time user is confused, because it already knows the answer.

## The queue

Work strictly in order inside each lane. Lane B may run parallel to Lane A
in cheap sessions.

**Lane A — the product**

| # | Task | Done when |
|---|---|---|
| A0 | Ship the **backend**. The three-click runbook is under "Josh's own list" below. **Correction to PLAN §5: the frontend was never frozen.** `deploy-frontend.yml` has indeed not fired since 2026-07-23, but Cloudflare Pages has its own Git integration and has been deploying `cortex/` from `main` continuously — verified 2026-08-05 by finding this session's strings in the live bundle at cortex.heyvera.org. The freeze is backend-only, which is why the web app has drifted ahead of a VPS still running the June 3 binary | `/v1/health` on the VPS reports a build from this week; Josh can log in and the panes talk to a backend that knows the new endpoints |
| A1 | Land #437 (credits, v60). **#438 is HeyVera Socials — not this lane's work**; it is reviewed and merged in a HeyVera session. Your job is only to wait for it, because the migration counter forces #438 → #437. When #438 is on main: mark #437 ready, merge | `main` has migrations through v60 |
| A2 | Task 1.4: instrument real token cost on 3–5 representative tasks (~$300 budget) | measured $/task table committed as `cortex/plan/COSTS-MEASURED.md`; pricing handed to Josh |
| A3 | VERIFIER.md V1→V7. **The pure logic of V1, V2, V4, V5, V6 and V7 is all on main** (#453–#456, #471, #473, #478, #479). **What is missing is not more logic — it is V3, and therefore every caller.** Verify this rather than trusting it: `grep -c verification_runs crates/api/src/db.rs` → 0, and `grep -rn billing_binding crates/api/src/` → nothing. The verdict machinery is correct, tested, and **not in the billing path**, because it has no tables to write to. V3 needs v61 → v60 (#437) → v53–v59 (#438, HeyVera lane). One merge unblocks the rest | verdicts computed from Cortex-run checks *and actually reached from the scheduler*; refund copy still withheld until V7's findings are closed |
| ~~A4~~ | CONTEXT.md C1→C4 | **Done** — #457 (repo map), #458 (into every dispatched step), #459 (persistent index), #460 (hybrid retrieval), #461 (impact sets), #468 (impact API + Leases pane). C5 (MCP surface) still later |
| A5 | Consult checkpoint: before Phase 3 cutover (Postgres/Temporal), request a Fable/xhigh review of the cutover plan | reviewed plan exists before any data moves |

**Lane C — the surface (per SURFACE.md; F1 may start immediately, the rest
gate on Lane A as SURFACE.md's dependency column specifies)**

| # | Task | Done when |
|---|---|---|
| ~~C-F1~~ | ~~Frontend audit + BYOK-era amputation; mission-control IA skeleton~~ | **Done** — #445 (audit + amputation, 22 files/~3k lines) and #448 (six-pane IA). Findings in [FRONTEND-AUDIT.md](FRONTEND-AUDIT.md); the one that matters is that the settings UI defaulted users to a *subscription* credential flow, closed in the UI, still live in the backend `startAuth` |
| C-F2 | Runs pane **done** (#464) — live on the real run API over SSE. Ledger and Receipts panes still wait on #437 and V3–V4 respectively | per-row criteria in SURFACE.md |
| C-F5 | Leases pane **done** (#468) — a query tool over C4's impact sets, not a board of held leases (that needs an endpoint which does not exist) | |
| C-F3/F4/F6/F7 | Follow SURFACE.md's table and dependencies exactly. F3 (GitHub App) additionally needs Josh to create the app | per-row criteria in SURFACE.md |

**Lane B — hygiene (cheap sessions)**

| # | Task | Done when |
|---|---|---|
| ~~B1~~ | Dependabot triage | **Done for this lane** (#449). #435 was the pinned Rust 1.88.0 predating `cfg_select!` in libsqlite3-sys's build script — toolchain bumped to 1.97.1, CI green including `clippy -D warnings`. #427 was `typescript@7` against typescript-eslint's `<6.1.0` peer range — ignored until that widens. #425 is a HeyVera-lane `vite.config.ts` TS2769; diagnosis posted on the PR, fix belongs to that session |
| B2 | Advisory triage: cargo-deny findings + npm audit for both apps, one PR with justified fixes/ignores | advisory jobs green on a fresh PR |
| ~~B3~~ | Stale PR sweep | **Done** — #397, #408, #409, #410 closed (they target `archive/dashboard`, removed in Phase 0.6). **#105 deliberately left open**: it is Socials, so closing it is the HeyVera session's call, not this lane's |

**Josh's own list — see [JOSH-ACTIONS.md](JOSH-ACTIONS.md).** His role is two
things: **using the product as a real user**, and holding credentials. Agents
do the rest. Surface these when relevant; never work around them.

- **#411 — now three clicks in the Actions tab, not a terminal session.**
  `.github/workflows/host-db-migration.yml` runs the host steps over the
  same Tailscale credentials the deploy already uses. A dry run on
  2026-08-05 confirmed passwordless sudo works, so nothing needs a shell.
  Order: **run the migration (dry run first) → merge #411 → Deploy
  Production, branch `main`.**
  Two corrections that runbook encodes, both found by inspecting the live
  host rather than by reading:
  1. **The service is left stopped after migrating, deliberately.** The
     June 3 binary does not read `CORTEX_DB_PATH` — that code arrives with
     #411 — so restarting it after the move points it at a path that no
     longer exists and SQLite creates a fresh empty database. Production
     would look wiped while the real data sat safe in `/var/lib/cortex`.
     The deploy is what brings it back up, on a binary that knows the new
     paths. Zero users, so the gap costs nothing.
  2. **`routing.db` moves too.** #411's original command list missed it. It
     is *untracked*, so `git clean -fd` deletes rather than reverts it, and
     losing it discards every UCB statistic the router learned in
     production — nothing breaks, the router just quietly gets worse.
     `CORTEX_ROUTING_DB_PATH` was added to #411's branch for this.
- Anthropic **rate-tier / spend-cap request** — the one that matters
  operationally (low tiers cap monthly spend and would strangle launch week).
  The §A.1 commercial comfort check is *optional insurance*, not a blocker:
  operator API keys powering a product is the API's intended use; §D.4
  covers reselling access, which Cortex does not do. Its only real value is
  as a saved answer for a future enterprise security review.
- UNVERIFIED pricing row: approve/reject (recommendation: approve)
- SOC 2 clock: trigger condition + concrete path are in the doc
- Legal entity + Terms of Service + privacy policy before Stripe goes live
- `OPENCODE_ZEN_API_KEY` on the VPS for live Zen testing
- GHAS billing decision (blocks dependency-review restoration)

## State as of this handoff

Merged: #439 (Zen provider — three providers wired, OpenAI-compatible seam
ready for the next gateway), #440 (VERIFIER.md), #442 (this plan set).

Open PRs, verified against GitHub at handoff time — the whole list, so
nothing looks like a surprise later:

| PR | State | Whose |
|---|---|---|
| #437 credits (v60) | draft, mergeable, waits on #438 | **yours (A1)** |
| #438 Socials (v53–v59) | draft, mergeable, CI-green | HeyVera session |
| #411 db out of git tree | draft, mergeable | host step, then merge |
| #435 cargo group | open | should pass now that #449 bumped the toolchain — rerun it |
| #427 cortex npm | open | dependabot will recreate it without the TS 7 bump |
| #425 heyvera npm | open | HeyVera session (diagnosis on the PR) |
| #105 Socials P09 | open, superseded | HeyVera session's call |

Merged this session: #443 (reground), #444 (consistency + launch definition),
#445 + #448 (F1), #449 (toolchain + dependabot ignores). #397/#408/#409/#410
closed.

Production: zero users, June 3 binary, decisions still free — that stops
being true at the first payment, which is why Lane A runs in the order it
does.
