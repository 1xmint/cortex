# Working in this repository

What an agent needs before it acts. Everything mechanical lives in
[CONTRIBUTING.md](CONTRIBUTING.md); this file exists because that one is not
loaded automatically, and it deliberately does not restate it.

## 1. Read these first, in this order

1. **[CONTRIBUTING.md](CONTRIBUTING.md)** — how a change actually lands. Auto-merge,
   the required checks, `--all-targets`, formatting, the shared migration
   counter. Read it before your first commit, not after your first failure.
2. **`cortex/plan/EXECUTION-STATE.md`** — the running checkpoint: what has
   landed, what is open, and which of the other plan documents still matter.
   It also tells you which are superseded.

`cortex/plan/HARNESS-EXCELLENCE-PLAN-2026-08.md` is over eight thousand lines.
**Consult it by section; never read it whole.** Its "Non-negotiable invariants"
list (§ near the top) is the part most changes need, and invariants are cited
by number throughout the code and the ADRs.

`docs/adr/` holds decisions that are settled. `docs/ARCHITECTURE.md` opens with
an amendment headed *"read before relying on any section below"*, and one of its
sections is retained only for diff context. Read the amendment first; parts of
the document below it are no longer true.

## 2. One product lives here

- **Cortex** — the agentic coding harness. `crates/{core,engine,api,worker,context,egress,cortex-server}`
  and the `cortex/` frontend. That is the whole repository.
- **Soma** — fenced behind the `soma` feature, **off by default**. See
  `docs/adr/ADR-0003-soma-feature-fence.md`. The `no-default-features` CI job
  exists to keep it out of the default graph; do not defeat it.

Until September 2026 a second product, HeyVera Socials, shared this backend.
It has its own repository now. `crates/api/route-manifest.csv` is the list of
routes this API serves, and `crates/api/tests/route_ownership.rs` fails the
build if the code and the list disagree — so a Socials route cannot quietly
come back.

## 3. The rules that are easy to violate without noticing

- **`--all-targets`, never `--lib`.** `cargo test --lib` does not compile
  `tests/` and reports a confident green while the integration suite has not
  built. CONTRIBUTING explains what this cost once.
- **Read the migration counter at *rebase* time, not design time.** Two
  branches that both add migration N merge cleanly and then one of them never
  runs. Nothing warns you.
- **`CORTEX_SINGLE_NODE=1` is required.** The server exits without it, and a
  second dispatcher grades every delivery twice.
- **The sandbox is the only execution boundary.** A code-writing agent never
  gets host credentials, a Docker socket, the production database, or arbitrary
  host filesystem access — invariant 1. If isolation cannot be established the
  step is `Blocked`; running on the host instead is never the fallback.
- **The independent verdict is the sole truth** — invariant 6. A worker's own
  report of its checks is a diagnostic, not a gate. Gating on it once made
  `Verdict::Failed` unreachable and the refund promise undeliverable (F14).

## 4. Auto-merge is armed on every pull request

It merges the moment the required checks go green — there is no separate step
where somebody presses the button. **Open a PR as draft if it is not ready**, and
be aware that a stacked PR merges into its base branch as soon as *that* base's
checks pass, which collapses stacks without asking.

**Always true:** A ready pull request merges itself when checks pass; a draft
does not. Payment, sign-in and data-deleting changes stay drafts until an
independent review passes.

Anything not on the required list is advisory and cannot stop a merge, however
red it is. CONTRIBUTING has the live list and the command to read it.

## 5. Evidence, not assertion

This repository's findings are numbered (F0–F18 in `EXECUTION-STATE.md`) and
almost all of them were found by *running* something rather than reading it. Two
habits follow:

- **Do not claim a thing works because it compiles.** Say which command you ran
  and what it printed. If a claim is untested, write that in the PR body rather
  than letting a reviewer assume coverage that is not there.
- **A test that cannot fail is worse than no test.** Several checks here were
  green for years because the red case could not occur. If you write an
  assertion, know what would make it fail.

## 6. Communication

- Lead with the result or the conclusion. Do not bury it under preamble.
- Explain decisions that matter; do not narrate every step.
- Surface discoveries, blockers, risks and changes of direction early rather
  than at the end.
- Correct a material mistake plainly and briefly, then continue.
- Write for a tired reader: small words, short paragraphs, exact paths, and a
  recommendation with every decision that is the owner's to make.
