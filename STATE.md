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
  secret is unreadable and the network unreachable. Also proven in WSL2
  Ubuntu on the owner's PC (2026-10-18).
- Mission log (ADR 0003): append-only events per mission in SQLite (WAL),
  refused updates and deletes, project-scoped missions, and leases so one
  process drives a mission at a time.
- Git: each worker session gets a disposable worktree; git run there
  ignores planted hooks, fsmonitor, `.gitattributes` filters, a swapped
  `.git` file and any repository the worker makes inside it (removed
  before commit, so its files are kept as plain files). Accepted work moves a `cortex/...` branch, never the
  person's checked-out one.
- Checks: frozen read-only outside the worktree with a SHA-256
  fingerprint, run in the sandbox, seen failing before work starts (a
  check that already passes is blocked and the mission cannot finish).
  They run on a clean checkout of the session's commit, so only committed
  work counts. A check set with no checks is refused.
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
  subscription inside a bubblewrap jail that shows only system files, the
  sign-in file, a throwaway copy of `~/.claude.json` (nothing else in
  `~/.claude`, and nothing the session writes there survives), the
  worktree and the read-only checks. It has no shell: its only commands go
  through `cortex mcp` (`run`, `test`), each in the gVisor sandbox; `test`
  works on a throwaway copy, so a check cannot leave answers behind. A
  session that would bill an API key, or does not say how it bills, is
  refused. A usage limit parks the mission until the reset time: the park
  is recorded (a restart keeps waiting), `stop` ends it, and parked time
  does not count against the 2 h. Proven in CI with stand-in binaries, and
  on 2026-10-18 with the real signed-in CLI (2.1.302) in WSL Ubuntu:
  `cargo test -- --ignored claude_cannot_run_shell` passed.
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
  ten with the fake engine. First real run, 2026-10-18, Claude Code on the
  owner's subscription: **10 of 10 passed**, each in one session, 10–20 s
  each ([bench/results/2026-10-18-claude.json](bench/results/2026-10-18-claude.json)).
  These prompts are now too easy to tell engines apart; harder ones are next.
- Brain design approved: version 2,
  [docs/superpowers/specs/2026-10-18-cortex-brain-design-v2.md](docs/superpowers/specs/2026-10-18-cortex-brain-design-v2.md),
  approved by the owner on 2026-10-18 after three Codex reviews. It
  replaces [version 1](docs/superpowers/specs/2026-10-18-cortex-brain-design.md),
  kept for history.
- Foundational ADRs accepted: Rust core ([0002](docs/adr/0002-rust-core.md)),
  event log in SQLite ([0003](docs/adr/0003-event-log-in-sqlite.md)), sandbox
  ([0004](docs/adr/0004-sandbox-bubblewrap-gvisor.md)), engines
  ([0005](docs/adr/0005-engines.md)).
- Slice 1 ([plan](docs/superpowers/plans/2026-10-18-brain-slice-1.md)) is
  built and proven on a real engine (2026-10-18).

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
- 2026-10-18 — Brain design version 2 replaces version 1: layers with a
  strategy layer for lasting intent, a spec plus checks planned
  progressively, context assembly, a provider-neutral model layer, and
  browser games and small web apps as the first target. Why: a skeptical
  second look plus Codex reviews found version 1 weak on lasting intent,
  context and evaluation. The slice-2 design (PR 26) closed as superseded.

## Known problems

- The Claude engine's jail must show Claude Code its own sign-in files, so
  keeping them from the model rests on Claude Code's `--restricted` mode and
  deny rules, not on the jail. Only the sign-in file is shared; if Claude
  Code refreshes it by replacing the file, the refresh is lost (unproven on
  the real CLI). A `claude` installed under nvm or npm outside
  `/usr` and the home folder may not run in the jail.
- Accepted work can include build leftovers: the real bench run committed
  Python's `__pycache__` files alongside the answers.
- The Claude engine runs whatever model the CLI defaults to (Opus on the
  owner's plan), which spends the five-hour usage limit fastest.

## Next slices

1. Brain architecture study → design document → owner approval (done).
2. ADRs for the core, storage, sandbox and engines (done); later components
   get theirs when their slice starts. The first code PR adds build, lint and
   test checks to CI.
3. First brain slice: done 2026-10-18 (doctor, jail test and a 10 of 10
   bench run in WSL Ubuntu).
4. Brain design version 2 approved (done 2026-10-18).
5. Next: slice 2 of version 2 (section 6): a bench of browser games and
   small web apps that can tell brains apart, cost per accepted result,
   and full model-call records. Starts with its plan; needs the owner's
   private holdout repository (design section 8).
