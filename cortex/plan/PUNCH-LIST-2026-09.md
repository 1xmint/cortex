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

## Done in this change

- CREDITS.md §2 (retry double-charge) and the "refunds make the verifier
  load-bearing" note, plus VISION.md's refund warning, described code that no longer
  exists. Updated to the current mechanism.
