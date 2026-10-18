# State

Updated: 2026-10-18

What is true now and what comes next. Kept under 150 lines by CI.

## Where things stand

- Foundation: this repository, its rules ([AGENTS.md](AGENTS.md)) and the
  checks that keep the docs honest.
- Code: one Rust crate in `core/` (binary `cortex`), checked on every PR by
  the `rust` job (format, lint, tests on Ubuntu 24.04). It only prints its
  version so far.
- Sandbox (ADR 0004) works in CI: bubblewrap wraps gVisor (`runsc do
  -force-overlay=false`, no network). `cortex doctor` proves a planted
  secret is unreadable and the network unreachable. Not yet proven inside
  WSL2 on the owner's PC.
- Mission log (ADR 0003): append-only events per mission in SQLite (WAL),
  refused updates and deletes, project-scoped missions, and leases so one
  process drives a mission at a time.
- Git: each worker session gets a disposable worktree; git run there
  ignores planted hooks, fsmonitor, `.gitattributes` filters and a swapped
  `.git` file. Accepted work moves a `cortex/...` branch, never the
  person's checked-out one.
- Checks: frozen read-only outside the worktree with a SHA-256
  fingerprint, run in the sandbox, seen failing before work starts (a
  check that already passes is blocked and the mission cannot finish).
- Engines: one interface for whatever does a session's work, with a
  cancel switch and a time limit. A scripted fake engine stands in for
  real models in tests; it cannot write outside its worktree.
- Mission loop: `cortex run --repo --checks "<request>"` freezes the
  checks, sees them fail, then gives an engine up to 4 sessions (20 min
  each, 2 h total). Work is accepted only when every check passes, onto
  branch `cortex/<mission>`; `cortex status <mission>` shows how to merge
  it. `--engine claude` (the default), `--engine api` and the fake engine
  are wired.
- Claude Code engine: runs `claude -p --restricted` on the person's
  subscription inside a bubblewrap jail that shows only system files, its
  own config (personal CLAUDE.md, plugins, skills and history hidden), the
  worktree and the read-only checks. It has no shell: its only commands go
  through `cortex mcp` (`run`, `test`), each in the gVisor sandbox. A session
  that would bill an API key is refused; a usage limit parks the mission
  until the reset time. Proven in CI with stand-in binaries only; a real
  signed-in session (`cargo test -- --ignored claude_cannot_run_shell`) has
  not run yet.
- API engine: `--engine api` talks to the Claude API directly
  (`claude-sonnet-5-5`, key from `ANTHROPIC_API_KEY`) with four tools: read,
  write and list files inside the worktree only, and `run`, which goes
  through the gVisor sandbox. The key is sent in a header and scrubbed from
  every transcript line and failure note. Stops after 50 turns. Proven in CI
  against recorded replies and a local stand-in server; it has never called
  the real API.
- Crash recovery: if the process dies at any point, `cortex resume
  <mission>` finishes the mission as if nothing happened (kill-tested after
  every kind of event). A dead process's lock is taken over at once; a
  live one's is respected.
- Steering: `cortex steer <mission> "<text>"` adds an instruction that every
  later brief carries. `cortex stop <mission> ["<text>"]` cancels the running
  session within about a second; with text the mission re-plans with it
  (limited by the re-plan budget), without text it pauses until
  `cortex resume`.
- Bench: ten small prompts in [bench/](bench/README.md), each with hidden
  checks that fail on its starting project. `cortex bench --engine claude`
  runs them all as missions and writes `bench/results/<date>-<engine>.json`.
  CI proves the checks fail on every start and that known answers pass all
  ten with the fake engine. No real engine has run it yet.
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

- The Claude engine's jail must show Claude Code its own sign-in files, so
  keeping them from the model rests on Claude Code's `--restricted` mode and
  deny rules, not on the jail. A `claude` installed under nvm or npm outside
  `/usr` and the home folder may not run in the jail.

## Next slices

1. Brain architecture study → design document → owner approval (done).
2. ADRs for the core, storage, sandbox and engines (done); later components
   get theirs when their slice starts. The first code PR adds build, lint and
   test checks to CI.
3. First brain slice: built test-first, **unproven on a real engine**. It
   is done when the owner, in WSL Ubuntu, runs `cortex doctor`,
   `cargo test -- --ignored claude_cannot_run_shell` and
   `cortex bench --engine claude`, at least 7 of 10 pass, and that results
   file is committed.
