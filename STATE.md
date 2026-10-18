# State

Updated: 2026-10-18

What is true now and what comes next. Kept under 150 lines by CI.

## Where things stand

- Foundation: this repository, its rules ([AGENTS.md](AGENTS.md)) and the
  checks that keep the docs honest. No product code yet.
- Brain design approved:
  [docs/superpowers/specs/2026-10-18-cortex-brain-design.md](docs/superpowers/specs/2026-10-18-cortex-brain-design.md).
  Approved by the owner on 2026-10-18 after an independent review and a
  builder check.
- Foundational ADRs accepted: Rust core ([0002](docs/adr/0002-rust-core.md)),
  event log in SQLite ([0003](docs/adr/0003-event-log-in-sqlite.md)), sandbox
  ([0004](docs/adr/0004-sandbox-bubblewrap-gvisor.md)), engines
  ([0005](docs/adr/0005-engines.md)).
- Slice 1 plan written:
  [docs/superpowers/plans/2026-10-18-brain-slice-1.md](docs/superpowers/plans/2026-10-18-brain-slice-1.md).
  Being built task by task.

## Decisions

- 2026-10-18 — Start fresh, brain first. The previous Cortex (now archived at
  [1xmint/cortex-v1](https://github.com/1xmint/cortex-v1)) was a hosted
  verified-coding service that never completed a real task; its planning was a
  sentence split. Ideas carried over, not code: independent proof as the only
  truth, and strict isolation of untrusted work.
- 2026-10-18 — Public repository with no license for now (all rights reserved,
  readable by anyone). Keeps the business option open; an open license can be
  added later but not taken back.
- 2026-10-18 — The product runs on API billing. During development, tests run
  through the owner's existing Claude Code and Codex subscriptions, with no
  API key.
- 2026-10-18 — The first face is a chat web page; mobile and desktop apps come
  later; a terminal command and an HTTP API are available from the start.
- 2026-10-18 — Involvement is personal: each person has a working agreement
  (what to show, when to ask, when and how to notify), not a fixed set of
  levels.
- 2026-10-18 — The gold rule (AGENTS.md invariant 5): technology is chosen per
  component for the best long-term result, recorded in an ADR.

## Known problems

- None yet.

## Next slices

1. Brain architecture study → design document → owner approval (done).
2. ADRs for the core, storage, sandbox and engines (done); later components
   get theirs when their slice starts. The first code PR adds build, lint and
   test checks to CI.
3. First brain slice, planned with superpowers:writing-plans and built
   test-first.
