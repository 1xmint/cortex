# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Nothing has been released. Everything below is unreleased work on `main`, seeded
from the merged pull requests of waves 1–6 (#500–#544, 2026-08-09 to
2026-08-17). Earlier history predates this file and is not reconstructed here;
`git log` is the record for it.

For *why* a change was made, and for what each change deliberately did **not**
do, see `cortex/plan/EXECUTION-STATE.md`.

## [Unreleased]

### Added

- Execution sandbox and the `ExecutionJob` interface (PR C) (#502)
- Independent verification: no successful completion without it (PR A) (#505)
- Scoped egress — a per-task host allowlist that actually holds (PR C2) (#507)
- Verification survives the process that accepted it (PR B) (#513)
- Soma fenced behind a cargo feature, off by default (#515)
- Egress allowlist derived at plan time from the repository's ecosystem (#518)
- Routing signal restored from the independent verdict (#519)
- Provenance-typed context — only the contract may instruct (PR U) (#523)
- Plan-derived write sets (PR R part 1) (#524)
- Step-scoped path leases, and queue on conflict (PR R part 2) (#525)
- Hotspot classification and scheduler-allocated sequences (PR R part 3) (#526)
- Provider egress derived from the routing decision, so the sandbox can reach
  its model (#527)
- Longform-by-handle endpoint behind the count on every Socials profile (#529)
- The end-to-end test that will prove Cortex can complete a task — built, and
  gated behind a provider credential it has never been given (#531)
- The price catalog as versioned data, rather than an invented number (#532)
- A real worker service credential (`cwk_` keys, migration v67) (#543)

### Changed

- The `sandbox` check can block a merge (#504)
- Dependency majors get their own PR instead of riding a group (#508)
- `CODEOWNERS` points at paths that exist (#520)

### Fixed

- Cortex owns the trust arithmetic; the half-open Soma fence is closed (#517)
- The scheduler could never lease a step — and the model finally sees the
  assembled context (F8) (#528)
- No check could ever execute: the verifier had nowhere to write (F9) (#530)
- The sandbox scratch paths did not exist at runtime (F10, F12) (#533)

### Security

- **Worker authentication.** Before #543 the only worker token the server
  accepted in practice was the empty string, and it accepted that only because
  `CLERK_SECRET_KEY` was unset — so no legitimate worker could authenticate and
  any client at all could. Worker keys are now real credentials, and both
  anonymous paths sit behind `CORTEX_ALLOW_ANONYMOUS_WORKER`, default off.
  **Deploy consequence, intended:** an unauthenticated worker stops connecting.
  (#543)
- Soma is off by default and its fence is closed on both sides (#515, #517).
- Scoped per-task egress limits what customer code can reach (#507, #518, #527).

### Documentation

- The harness excellence plan, and the first execution briefs (#500, #501, #506,
  #522)
- Execution checkpoints for PR C, wave 2, PR C2 and the #499 review (#503, #509,
  #514)
- Wave 3 governance record, PR #105, and two recommendations (#521)
- F9, F10 and F12 proven against a real container runtime (#534)
- Round 6 — the evidence Phases 27–31 lean on, graded; the two Phase 35 drafts
  reconciled into one (#544)

### Dependencies

- Grouped Cargo, npm and GitHub Actions bumps (#510, #511, #512, #516, #535)

## Known gaps

Recorded here because a changelog that lists only what landed overstates the
state of the product.

- **Cortex has never completed a task.** The end-to-end live test (#531) exists
  and has never run — there is no provider API key in CI or on the host.
- **Production cannot execute.** It runs current code, but until a worker key
  is minted and installed there are no authenticated workers (#543).
- **The verifier cannot charge.** A provisional price class never reaches the
  ledger by construction, and what a credit is worth commercially is undecided
  (#532).
