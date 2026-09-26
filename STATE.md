# State

What is true now, and who each open thing is waiting on. Updated 2026-09-25.

For *how* to work here, read [AGENTS.md](AGENTS.md) then
[CONTRIBUTING.md](CONTRIBUTING.md). For why a thing is the way it is, read
`cortex/plan/EXECUTION-STATE.md` — this file is the summary, that one is the
record.

## The one that matters

**Cortex grades both directions of a task, end to end.** As of 2026-09-04 the
stubbed chain reaches:

```
verdict=Verified  required_passed=2 required_total=2   sealed "succeeded"
verdict=Failed    required_passed=1 required_total=2   sealed "failed"
```

`Verdict::Failed` had never been reachable before. Eight findings stood between
delivery and a failing verdict, and every one of them made the refund the
product is sold on undeliverable. They are written up as F13–F20 in
`EXECUTION-STATE.md`.

**What that does not mean.** No model has been consulted. The provider was a
scripted stub, at zero API cost. Nothing here is evidence that Cortex has
completed a task; that claim belongs to `live-model.yml` and to nothing else.

**Reproduced in the standalone repository on 2026-09-20.** The same two
verdicts, out of a tree with no Socials in it:
[run 35536911093](https://github.com/1xmint/cortex/actions/runs/35536911093).
The sandbox, egress and runner images built from `1xmint/cortex` as well, which
is the part the extraction put at risk and nothing had checked until now.

## Blocked on Josh

1. **`ANTHROPIC_API_KEY` as a repository secret**, scoped per
   `docs/adr/ADR-0004-provider-credential.md` — shortest-lived key, narrowest
   scope, hard spend cap. Then run the `live model` workflow from the Actions
   tab. That run would be the first time Cortex completes a real task. Nothing
   else blocks it. As of 2026-09-25, `live-model.yml` has **zero runs** and no
   `ANTHROPIC_API_KEY` repository secret exists — Cortex has never completed a
   real task.
2. **A worker key on the host.** [PR #54](https://github.com/1xmint/cortex/pull/54)
   (merged 2026-09-23) ships `cortex-worker-key` to mint it and optional worker
   images; `/etc/cortex/worker.env` holds the minted key as `CORTEX_TOKEN`. The
   worker key must still be installed on the host — production is at v66,
   healthy, and cannot execute without it.
3. **Auto-deploy exists but has not moved production.** PR #67 added
   `build-release.yml` → `deploy.yml`. The first Deploy run, 2026-09-24, is
   [run 36072223296](https://github.com/1xmint/cortex/actions/runs/36072223296)
   and completed **"success"**, but it only printed a `::notice::` and skipped
   the actual deploy, because `TS_OAUTH_CLIENT_ID`/`TS_OAUTH_SECRET` are not
   set in the GitHub `production` environment. Production has not moved by
   auto-deploy yet.
4. ~~**Where the new repository lives, and whether it is public.**~~
   **Answered 2026-09-20:** public, at
   [`1xmint/cortex`](https://github.com/1xmint/cortex). The split is finished
   and this repository is the result — one commit, no HeyVera history, 459
   files. `cortex/plan/EXTRACTION-STANDALONE.md` is the record of how it was
   done and what was deliberately left behind.

## Blocked on nobody, not yet started

- **F18's sibling risk is closed but the npm gap is not.**
  `ecosystem:npm-ci` installs into `node_modules/` inside the graded tree, so
  the scratch mount does not rescue it. Recorded in F15 rather than papered
  over.
- ~~**F16 has no end-to-end coverage.**~~ Done — a third `NOOP` scenario in
  `testing/stub-provider/claude` drives the direction that delivers nothing, and
  asserts `NothingDelivered` with no receipt.
- **Phases 27–30** — the capability mechanism, and the falsification test that
  has to exist before any "better than a single model" claim does. Nothing is
  built.
- **`bollard` is pinned at 0.18 and cannot move without work.** 0.21 relocates
  the container and network option types and makes `container::Config`
  private, so `crates/worker/src/sandbox/container.rs` and `sandbox/egress.rs`
  stop compiling. The bump merged unverified in the minutes before branch
  protection existed and was reverted. Dependabot will keep re-offering it, and
  each attempt now fails its own checks and sits blocked, which is the right
  outcome but not a quiet one. Porting the sandbox to the 0.21 API is a change
  to the code that isolates untrusted work and wants its own review.
- ~~**`aes-gcm` is pinned at 0.10.3 for the same reason.**~~ **Done.**
  Dependabot's [#7](https://github.com/1xmint/cortex/pull/7) (merged
  2026-09-21) bumped `aes-gcm` to 0.11.1, moving nonce randomness off the
  `OsRng` re-export that 0.11 dropped from `aes_gcm::aead`. `Cargo.lock` shows
  `aes-gcm 0.11.1` on `main` as of 2026-09-25.

## The split, and what it unblocked

Socials — the social network this backend also used to serve — left in
September 2026. Three merged pull requests did the separating: #649 stopped
Socials charging the Cortex credit ledger, #650 made route ownership a
machine-checked list, #651 gave Socials its own database export. This checkout
then deleted the Socials half outright.

What that unblocks: `deduct_credits` now has one caller shape, a verified step.
Phase 34's "ledger behind a narrow interface" was stuck because a `ChargeKey`
derived from a verification id could not compile against `pulse.rs`, a Socials
action with no verification behind it. `pulse.rs` is gone, so the interface can
be as narrow as it should have been.

What it does not mean: the database still creates the ~38 tables Socials used,
because the migration chain was left byte-identical to the one production is
running. A fresh Cortex-only schema is deliberately deferred; the reasoning is
in `cortex/plan/EXTRACTION-STANDALONE.md`.

## Gates

Every required check is unconditional and cannot pass vacuously. As of
2026-09-04 the two that could not fail now can:

- `clippy` runs with `-D warnings`. The allow list is in
  `[workspace.lints.clippy]` in the root `Cargo.toml`, with a reason per entry,
  so a local run sees what CI sees.
- `eslint` blocks on errors. Warnings are still permitted.
- The frontend has a **test floor** — the count is asserted and may only go up,
  the same shape as the `sandbox` job.

`main` requires a branch to be up to date before it merges, and `ci.yml` runs
on pull requests only. Those two go together: with no run on `main`, the only
thing making the merged tree the tested tree is the up-to-date requirement.
Turning it off means a pull request can be tested against a `main` that has
moved, and nothing ever tests the result — which is how a red commit reached
`main` unnoticed on 2026-09-20. The repository auto-updates branches, so a
stale pull request with auto-merge armed updates itself rather than stalling.

`stub-provider-e2e` and `live-model` are `workflow_dispatch` only. The first is
cheap and safe to run on a branch; the second spends money and is the only
thing that may be cited as task completion.

## Traps that have bitten more than once

- **Auto-merge is armed on every PR** and re-armed on every push. A stacked PR
  merges into its base the moment that base goes green, which collapses stacks
  without asking. Open a draft if a PR must wait — disabling auto-merge does not
  survive the next push.
- **Read the migration counter at *rebase* time, not design time.** Two
  branches that both claim the next number merge cleanly and one of them then
  never runs, silently.
- **`cargo clippy --fix` is not feature-aware.** It fixes what it compiled. A
  variable unused by default and read under `#[cfg(feature = "soma")]` gets
  renamed, and the soma build breaks.
- **`vitest` does not typecheck.** A green frontend test says nothing about
  whether it compiles; `npm run build` is the typecheck gate.
