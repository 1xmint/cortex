# Punch list — road to "done" (opened 2026-09-26)

What stands between Cortex and Josh's real run, ranked. Source: backend audit of
`cortex/main` at 1cebff1c on 2026-09-26; the frontend audit (Stage 2 of the plan)
is still to run and will append its items here. Each item names where the fix lives
and what proves it. Strike an item only when its PR has merged.

## Must fix before the real run

1. **A failed, refunded task could still hand over its work.** `validate_run_for_pr`
   (`crates/api/src/routes.rs`) ignored the verdict, so a customer could be refunded
   and still open a PR with the changes. Fix: refuse PR creation (409) when a step
   failed verification and was refunded; hide the action in the Runs pane. The PR is
   the only delivery path (`WorktreeManager::push_branch` is never called).
   → PR #76, branch `fix/withhold-failed-deliverable`. Money review required.
2. **Any signed-in user could read another customer's run context.**
   `list_artifacts_for_run` (`crates/api/src/context_api.rs`) ignored the caller and
   `get_context_artifacts_for_run` has no user filter; sibling routes likewise.
   Fix: owner check (404) on per-run routes, admin-only on cross-customer ones.
   → branch `fix/context-routes-ownership`. Tenancy review required.
3. **No price list means free work.** `freeze_step_quote` (`crates/api/src/scheduler.rs`)
   and `verification_dispatcher.rs` dispatch unquoted and uncharged with only a
   warning when no price list covers the task class. Recommendation: refuse dispatch
   in production when no price list covers the class. Josh decides at Stage 5
   together with the first price list.

## Should fix

4. **npm installs can leak into the graded tree (F15).** The worker's auto-commit runs
   `git add -A` (`crates/worker/src/worktree.rs` ~77-138), so when `ecosystem:npm-ci`
   runs worker-side, `node_modules/` and lockfile drift get committed. The sandbox
   policy (`crates/worker/src/sandbox/policy.rs` ~161-169) only redirects the npm
   cache. `check_runner.rs` records npm-ci as not executed in the read-only verifier.
   EXECUTION-STATE ~2458-2473 calls this resolved; it is not. Fix: exclude
   `node_modules/` before `commit_changes`, or run npm-ci only in the verifier.
   Sandbox review required.

## Tech debt

5. **`bollard` 0.18.1 → 0.21.1.** `cargo deny` advisories are clean, so not urgent.
   Touches `container.rs`, `egress.rs`, `check_runner.rs`; about half a day to a
   day. Sandbox review required.
6. **Orphan BYOK tables.** `user_provider_device_keys` / `user_provider_keys` are
   unused since PR #72; a migration can drop them, and the comments at
   `crates/api/src/db/mod.rs` ~685-744 are stale. Needs a deliberate deploy (the
   auto-deploy refuses migrating builds once PR #71 lands).

## Not actually done — corrected 2026-09-27 (M-D-0022 docs PR)

- **This section's claim was false.** CREDITS.md §2, the "refunds make the
  verifier load-bearing" note, and VISION.md's refund warning were *not*
  updated by whatever change first wrote this line — they still described the
  original fixed-price/refund-on-failure model verbatim until the M-D-0022 docs
  pass (this PR) marked them superseded and rewrote the surrounding text. Do
  not trust a "done in this change" line without checking the diff it claims.

## New items (M-D-0022 docs pass, 2026-09-27)

7. **Seed price list is wrong in both directions vs. Anthropic's list prices,
   and blocks pass-through billing.** `pricing.rs:377+` `seed_models` charges
   claude-opus-4-6 and claude-opus-5 at $15/$75 per MTok in/out against a list
   price of $5/$25 (customer overcharged 3x); claude-sonnet-5 at $3/$15 against
   $2/$10 (overcharged 1.5x); claude-haiku-4-5(-20251001) at $0.8/$4 against
   $1/$5 (**undercharged 20%, Cortex loses money on every call**); an Opus 5.5
   row is missing entirely. claude-sonnet-4-6 at $3/$15 matches list price.
   Verified against https://platform.claude.com/docs/en/about-claude/pricing,
   fetched 2026-09-27. Also: `pricing.rs`'s `margin_bp = 4_000` applies a 40%
   margin on top of these rates for `quoted_credits`, which must not carry into
   anything customer-charged under pass-through, and the seed table only bills
   cache reads — cache-write tokens (`cache_creation_input_tokens`) are not
   billed at all. Which price list is live in production (this seed, or a
   later one published through `publish_price_list`, `db/ledger.rs:631`) is
   not verified. Not a code change here — a PR 2 (M-D-0023) blocker.
8. **Buying a credit pack grants no credits.** `add_pack_credits`
   (`crates/api/src/db/ledger.rs:1134`) is called only from tests
   (`db/ledger.rs:1230,1246,1268,1377,1428,1450`; `voice_session.rs:3858`, also
   a test) — never from the Stripe webhook (`billing.rs`). The webhook's
   `checkout.session.completed` handler calls `should_init_credits_on_checkout`
   (`billing.rs:996-998`, "Only subscription mode (not one-time payment
   packs)") and only inits a *subscription* balance
   (`db.init_credit_balance`, `billing.rs:1311`) — there is no handling of a
   one-time pack purchase anywhere in `stripe_webhook` that credits
   `pack_remaining`. A customer who buys a one-time credit pack today pays and
   receives nothing.
9. **Chat "unavailable" dead end.** `chat.rs:318` returns a plain "unavailable"
   message when `PaidReplyError::Unavailable` fires (gateway off or no rate),
   with no retry path or explanation for the customer.
10. **TrialBanner / BillingPage "blank plan name" — could not confirm on
    current `cortex/main`.** The brief for this punch item cited
    `TrialBanner.tsx:75` and `BillingPage.tsx` ~140-160 for a blank
    interpolated plan name. Read on `cortex/main`: `TrialBanner.tsx:75` renders
    a static "Preview mode" string with no plan-name interpolation, and
    `BillingPage.tsx:166` builds `planLabel` as `` `Cortex Pro ${plan_type ===
    'annual' ? 'Annual' : 'Monthly'}` `` — deterministic, not blank-prone.
    Neither file has a `plan.name` field or similar. Leaving this open rather
    than asserting a bug that does not reproduce; needs a fresh look at
    whatever state (e.g. a specific `access_state`) the original report meant.
11. **Estimate shows tokens.** `routes.rs:1319-1420`
    (`/api/runs/estimate`) and `agent_tools.rs:171,332` (`run_estimate`) return
    token counts to the customer. Under M-D-0022 the customer sees cost, not
    tokens: the frontend shows dollars and the backend keeps credits. Keep the
    dollar figure, drop the token count, and derive the dollar figure from
    the corrected list-price rates (item 7), not the old seed table.
12. **RunsPane SHIPPABLE status mismatch.** `RunsPane.tsx:69` treats
    `verified | manual_override | completed` as shippable, but the engine's
    actual run statuses are `succeeded | failed | …` (`captain.rs:27-32`) — a
    `succeeded` run does not match any of the three strings `RunsPane` checks
    for, so it is not recognized as shippable. Taken from the brief, not
    independently re-read here; a red test is the way to confirm it.
13. **Stuck verification after a crashed claim.** `reclaim_expired_verification_jobs`
    (`db/verification_queue.rs:148-165`) re-queues an expired `claimed`
    verification job, but `claim_verification(...)?`
    (`verification_driver.rs:152-158`) returns `None` when the crashed
    attempt's `verifications` row already exists, and the dispatcher
    (`verification_dispatcher.rs` ~276) treats a `None` claim as "declined, job
    done" — likely leaving the run with no verdict forever. Strongly
    suspected from reading the code; not yet confirmed by a red test.
14. **Impact `SKIP_DIRS` hang.** `crates/context/src/extract.rs:24-33,199`
    walks directories against a hardcoded `SKIP_DIRS` list instead of
    respecting `.gitignore`, so a large ignored directory (e.g. a shared
    `target/` or `node_modules/`) not in that hardcoded list can hang or
    slow the walk on a large repo.
15. **Impact re-index has no rate limit.** `context_api.rs:318-377` lets
    repeated impact requests each trigger a re-sync with no debounce or rate
    limit inside the request window.
16. **Settings panel crashes without Clerk configured.** `SettingsPanel.tsx`
    imports `useUser` from `@clerk/clerk-react` and calls it unconditionally
    inside `AccountTab` (`SettingsPanel.tsx:3,139`). `main.tsx:19-46` renders
    the whole app without a `ClerkProvider` in the tree whenever
    `VITE_CLERK_PUBLISHABLE_KEY` is unset (the local-dev path). `useUser`
    throws outside a `ClerkProvider`, so opening the Settings panel's Account
    tab in local dev — no publishable key set — crashes instead of showing a
    local-user placeholder.
