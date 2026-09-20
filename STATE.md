# State

What is true now, and who each open thing is waiting on. Updated 2026-09-20.

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

## Blocked on Josh

1. **`ANTHROPIC_API_KEY` as a repository secret**, scoped per
   `docs/adr/ADR-0004-provider-credential.md` — shortest-lived key, narrowest
   scope, hard spend cap. Then run the `live model` workflow from the Actions
   tab. That run would be the first time Cortex completes a real task. Nothing
   else blocks it.
2. **A worker key on the host.** `cortex-worker-key` mints it;
   `/etc/cortex/worker.env` holds it as `CORTEX_TOKEN`. Production is at v66,
   healthy, and cannot execute without it.
3. **Where the new repository lives, and whether it is public.** The split is
   finished in this checkout — the Rust workspace, the repository shell and the
   naming all done, `cargo check --workspace --all-targets` and both clippy
   gates silent — and the destination is the only thing left. The
   recommendation is private, with a single fresh commit and no HeyVera
   history: that satisfies the history-scrub gate by having no history to
   scrub, and it is the only version that can be undone. Going public is a
   separate decision.

   Two answers are needed, and neither can be given from here. **The name:**
   `hey-vera/Cortex` already holds the plain one — archived since 2026-05-19,
   284 KB, three commits, kept only for a `Cargo.toml` that shows how to depend
   on Soma. Renaming it frees the name and GitHub leaves a redirect, so nothing
   linking to it breaks; the alternative is a second-choice name forever. That
   rename is a change to a shared repository, so it needs Josh's hand or his
   say-so. **The visibility:** private unless he says otherwise.
   `cortex/plan/EXTRACTION-STANDALONE.md` is the resume trail.

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
