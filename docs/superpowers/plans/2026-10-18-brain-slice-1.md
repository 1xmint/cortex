# Brain slice 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `cortex` command that takes a small coding request with checks
written in advance, runs one worker session at a time inside the sandbox
until Cortex's own checker says every check passes, survives being killed
at any point, accepts `steer` and `stop` while running, and is judged by a
10-prompt bench.

**Architecture:** One Rust crate (`core/`, binary `cortex`). A mission is
an append-only event log in SQLite (ADR 0003). The loop is plan (the
request plus its frozen checks) → worker session in a disposable git
worktree → checker in the two-layer sandbox (ADR 0004) → accept or retry.
Engines sit behind one trait: a scripted fake (CI), Claude Code headless
without a shell (ADR 0005), and a minimal Anthropic API engine tested on
recorded responses.

**Tech Stack:** Rust stable (2024 edition), `rusqlite` (bundled), `serde`,
`serde_json`, `toml`, `clap`, `sha2`, `uuid`, `ureq`, `rmcp` + `tokio`
(only in the MCP server subcommand), `tempfile` (tests). Linux tools at run
time: `git`, `bwrap`, `runsc`.

**Spec:** [docs/superpowers/specs/2026-10-18-cortex-brain-design.md](../specs/2026-10-18-cortex-brain-design.md),
with ADRs [0002](../../adr/0002-rust-core.md), [0003](../../adr/0003-event-log-in-sqlite.md),
[0004](../../adr/0004-sandbox-bubblewrap-gvisor.md), [0005](../../adr/0005-engines.md).

## Global Constraints

- Tests run on GitHub CI (`ubuntu-24.04`), never on the owner's PC. Locally
  run only `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`
  and `cargo check --all-targets` in `core/`. RED is shown by a local
  compile failure (the test names a function that does not exist yet) or
  by a CI failure; GREEN is shown by CI.
- Code that needs Linux (`bwrap`, `runsc`, process groups) is behind
  `#[cfg(target_os = "linux")]`; on other systems those functions return
  `Err(Blocked)` so the crate still compiles on Windows.
- Model-written code never runs outside the sandbox; if the sandbox self
  test fails, every step that needs it is blocked (invariant 3).
- A check that was never seen failing is blocked, not counted (invariant 2).
  Only the checker writes `check_passed` (invariant 1).
- Events are never updated or deleted. One SQLite transaction per event.
- Every table row carries `project_id`; personal data carries `person_id`.
  The default person is `owner`.
- Cortex data lives in `$CORTEX_HOME` (default `~/.cortex`): `cortex.db`,
  `worktrees/`, `checks/`.
- Every git command the core runs on a worktree uses
  `git -c core.hooksPath=/dev/null -c core.fsmonitor=false` and refuses to
  run if the worktree's `.git` file differs from what Cortex wrote.
- Default budgets: 4 worker sessions per mission, 20 minutes per session,
  2 hours per mission, 2 re-plans (stops with a message) per mission.
- Each task lands as its own PR, merged when CI is green, with STATE.md
  updated. Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **A model escaping through git.** The worker can write any file in its
   worktree, including `.git`, `.gitattributes` filters and hooks paths;
   the core's own `git add/commit` must not execute anything it wrote
   (Task 4 test `git_ignores_planted_hooks_and_gitdir`).
2. **A check that passes before any work.** A bench prompt whose check
   already passes on the empty project must be blocked, not counted
   (Task 5 test `check_that_never_failed_is_blocked`).
3. **A crash between "session ended" and "commit accepted".** Resume must
   not accept a commit twice or lose one (Task 8 kill test at
   `session_ended`).
4. **A steer sent while no session is running** (between sessions or while
   checking) must still reach the next session's brief (Task 9 test
   `steer_between_sessions_reaches_next_brief`).
5. **A sandboxed command that never ends or floods output.** It must be
   killed at its timeout and its output capped at 1 MiB (Task 2 test
   `runaway_command_is_killed_and_capped`).

---

### Task 1: Crate skeleton and Rust CI

**Files:**
- Create: `core/Cargo.toml`, `core/src/main.rs`, `core/src/lib.rs`,
  `core/src/error.rs`, `.github/workflows/rust.yml`, `core/tests/cli.rs`
- Modify: `STATE.md`, `.gitignore` (add `core/target/`)

**Interfaces:**
- Produces: `cortex::Error` enum with `Blocked(String)`, `Store(String)`,
  `Io(std::io::Error)`, `Invalid(String)`; `pub type Result<T>`. Binary
  `cortex` with `cortex --version` printing `cortex <crate version>`.

- [ ] **Step 1: Write the failing test** `core/tests/cli.rs::version_prints`:
  runs `env!("CARGO_BIN_EXE_cortex")` with `--version`, asserts exit 0 and
  stdout starts with `cortex 0.1.0`.
- [ ] **Step 2: Implement** the clap CLI with only `--version`; `lib.rs`
  exports `error`.
- [ ] **Step 3: CI.** `rust.yml` on pull requests and `main`: job `rust`
  on `ubuntu-24.04`, working directory `core`, runs `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings`, `cargo test` (with
  `Swatinem/rust-cache`). Later tasks add the sandbox setup steps to this
  same job.
- [ ] **Step 4: Local check:** `cargo fmt --check && cargo clippy --all-targets -- -D warnings`
  Expected: no output, exit 0.
- [ ] **Step 5: PR, CI green, merge.** Add `rust` to branch protection's
  required checks (`gh api -X PATCH repos/1xmint/cortex/branches/main/protection/required_status_checks`)
  and read it back.

### Task 2: Sandbox (two layers) with must-fail tests

**Files:**
- Create: `core/src/sandbox.rs`, `core/tests/sandbox.rs`
- Modify: `.github/workflows/rust.yml` (install `bubblewrap`, install
  `runsc` from gVisor's apt repository, `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0`),
  `core/src/main.rs` (`cortex doctor`)

**Interfaces:**
- Produces:
  - `pub struct RunSpec { pub cmd: String, pub workdir: PathBuf, pub checks_dir: Option<PathBuf>, pub timeout: Duration }`
    (`cmd` runs under `/bin/sh -c`; `workdir` mounted read-write at `/work`
    and used as the current directory; `checks_dir` mounted read-only at
    `/checks`).
  - `pub struct RunOutput { pub exit_code: i32, pub stdout: String, pub stderr: String, pub timed_out: bool, pub truncated: bool }`
  - `pub fn self_test() -> Result<()>` (both layers present and working,
    otherwise `Err(Blocked(reason))`)
  - `pub fn run(spec: &RunSpec) -> Result<RunOutput>`
- Layering: `bwrap` builds the filesystem (read-only `/usr`, `/bin`,
  `/lib*`, `/etc/alternatives`, a generated minimal `/etc/passwd`, fresh
  `/tmp` and `/home` as tmpfs, `/work`, `/checks`), `--unshare-all
  --die-with-parent --new-session --clearenv` with `PATH` and `HOME=/home`
  set; inside it `runsc --rootless --network=none --ignore-cgroups do`
  runs the command with writes going straight to `/work` (no overlay).
  Exit code 124 and `timed_out = true` on timeout; the whole process group
  is killed. Output over 1 MiB per stream is cut and `truncated = true`.
  If `runsc` cannot run inside `bwrap` on CI, stop and record a ruling
  plus a superseding ADR; never ship one layer alone (ADR 0004).

- [ ] **Step 1: Write the failing tests** in `core/tests/sandbox.rs`
  (all `#[cfg(target_os = "linux")]`):
  - `self_test_passes` — `self_test()` is `Ok`.
  - `cannot_read_planted_key` — write `$HOME/.cortex-test-key` on the host
    containing `SECRET-123`; run `cat ~/.cortex-test-key; cat /root/.cortex-test-key; grep -r SECRET-123 / 2>/dev/null`;
    assert stdout does not contain `SECRET-123`.
  - `cannot_reach_network` — run `python3 -c "import socket; socket.create_connection(('1.1.1.1', 53), 3)"`;
    assert `exit_code != 0`, and run the same command with plain
    `std::process::Command` on the host first to prove it can succeed
    there (skip with a printed note if the host itself is offline).
  - `writes_land_in_workdir` — `echo hi > out.txt`; host file
    `workdir/out.txt` contains `hi`.
  - `checks_are_read_only` — `touch /checks/x` exits non-zero.
  - `runaway_command_is_killed_and_capped` — `yes` with a 2 s timeout:
    `timed_out` and `truncated` both true, call returns within 10 s.
  - `uses_gvisor_kernel` — `dmesg 2>/dev/null | head -1; cat /proc/version`
    output contains `gVisor` (proves layer 2 is really in the path).
- [ ] **Step 2: Local compile shows RED:** `cargo check --all-targets`
  fails with unresolved `cortex::sandbox`.
- [ ] **Step 3: Implement** `sandbox.rs` and `cortex doctor` (prints each
  layer's status; exit 1 with the reason when `self_test` fails).
- [ ] **Step 4: PR; CI green** (all seven tests ran, none skipped except
  the offline note). Merge.

### Task 3: Event log store

**Files:**
- Create: `core/src/store.rs`, `core/src/event.rs`, `core/tests/store.rs`

**Interfaces:**
- Produces:
  - `pub struct MissionId(pub String)` (UUID v4 text), `pub struct ProjectId(pub String)`.
  - `pub enum Event` (serde, `#[serde(tag = "kind", rename_all = "snake_case")]`):
    `MissionStarted { project_id, person_id, request, repo_path, base_commit }`,
    `PlanVersion { version: u32, checks_fingerprint: String, note: String }`,
    `CheckRed { check_id, exit_code }`, `CheckBlocked { check_id, reason }`,
    `SessionStarted { session: u32, engine: String, engine_version: String, worktree: PathBuf, brief: String }`,
    `Transcript { session: u32, line: String }`,
    `SessionEnded { session: u32, engine_note: String }`,
    `SessionAbandoned { session: u32, reason: String }`,
    `CheckRun { session: u32, check_id, exit_code, passed: bool, output_tail: String }`,
    `CommitAccepted { session: u32, commit: String }`,
    `Steer { text: String }`, `Interrupt { text: Option<String> }`,
    `Parked { until_unix: i64, reason: String }`,
    `MissionDone { commit: String }`, `MissionStuck { reason: String }`,
    `MissionPaused`, `MissionResumed`.
  - `pub struct Store` with `open(path: &Path) -> Result<Store>`,
    `create_mission(project: &ProjectId, person: &str) -> Result<MissionId>`,
    `append(&self, m: &MissionId, e: &Event) -> Result<u64>` (returns seq,
    starts at 1), `events(&self, m: &MissionId) -> Result<Vec<(u64, Event)>>`,
    `events_after(&self, m, seq: u64)`, `missions(&self, project: &ProjectId) -> Result<Vec<MissionId>>`,
    `take_lease(&self, m, holder: &str) -> Result<bool>`, `renew_lease`,
    `release_lease` (lease is free when its heartbeat is older than 15 s).
  - Fault injection: if env `CORTEX_FAULT=after:<kind>` matches the kind
    just appended, `append` commits then calls `std::process::abort()`.
- Schema: `projects(id, path)`, `missions(id, project_id, person_id, created)`,
  `events(mission_id, seq, kind, body_json, at)` with primary key
  `(mission_id, seq)`, `leases(mission_id, holder, heartbeat)`. WAL mode.
  `append` computes `seq` inside the same `BEGIN IMMEDIATE` transaction.

- [ ] **Step 1: Failing tests:** `append_assigns_increasing_seq`,
  `events_round_trip_every_kind` (one of each variant, equal after
  reading), `events_cannot_be_updated` (the store exposes no update; a raw
  `UPDATE events` is rejected by a trigger `RAISE(ABORT, 'append-only')`),
  `missions_are_scoped_to_project` (two projects, each lists only its own),
  `lease_blocks_second_holder_until_stale`, `concurrent_appends_from_two_connections_get_distinct_seqs`.
- [ ] **Step 2: Local compile RED.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: PR; CI green; merge.**

### Task 4: Worktrees and safe git

**Files:**
- Create: `core/src/git.rs`, `core/tests/git.rs`

**Interfaces:**
- Produces:
  - `pub fn head(repo: &Path) -> Result<String>`
  - `pub fn create_worktree(repo: &Path, base: &str, dest: &Path) -> Result<Worktree>`
    (detached at `base`; records the `.git` file's content)
  - `pub struct Worktree { pub path: PathBuf, git_file: String }` with
    `commit_all(&self, message: &str) -> Result<Option<String>>` (`None`
    when nothing changed), `discard(self) -> Result<()>` (`git worktree
    remove --force`, then delete the folder).
  - `pub fn accept(repo: &Path, branch: &str, commit: &str) -> Result<()>`
    (sets `refs/heads/<branch>` with `git update-ref`; never touches the
    person's checked-out branch).
- All git calls go through one private helper adding
  `-c core.hooksPath=/dev/null -c core.fsmonitor=false -c core.attributesFile=/dev/null`,
  `GIT_CONFIG_NOSYSTEM=1`, and `--no-verify` on commit; before each call
  the helper checks the `.git` file is unchanged and returns
  `Err(Blocked("worktree .git was modified"))` otherwise.

- [ ] **Step 1: Failing tests:** `worktree_starts_at_base`,
  `commit_all_returns_none_when_clean`, `discard_removes_worktree`,
  `accept_moves_branch_only`, `git_ignores_planted_hooks_and_gitdir`
  (worker writes `.git` pointing at a gitdir whose `hooks/pre-commit` and
  `config` `core.fsmonitor` touch a marker file; `commit_all` returns
  `Blocked` and the marker never appears; a second case plants
  `.gitattributes` with a `filter=evil` whose driver touches the marker,
  commit succeeds and the marker never appears).
- [ ] **Step 2: Local compile RED.** **Step 3: Implement.** **Step 4: PR; CI green; merge.**

### Task 5: Frozen checks and the checker

**Files:**
- Create: `core/src/checks.rs`, `core/tests/checks.rs`

**Interfaces:**
- Consumes: `sandbox::{run, RunSpec}`, `Store::append`.
- Produces:
  - `pub struct Check { pub id: String, pub command: String, pub expect_exit: i32 }`
  - `pub struct CheckSet { pub checks: Vec<Check>, pub files_dir: PathBuf }`
    loaded from TOML: `[[check]] id = "..." command = "..." expect_exit = 0`
    plus a `files/` folder beside it.
  - `pub fn freeze(set: &CheckSet, cortex_home: &Path, mission: &MissionId) -> Result<Frozen>`
    copies the TOML and files to `checks/<mission>/<fingerprint>/` and
    makes them read-only; `fingerprint` = SHA-256 over the sorted file
    paths and contents.
  - `pub struct Frozen { pub fingerprint: String, pub dir: PathBuf, pub set: CheckSet }`
    with `verify(&self) -> Result<()>` (recomputes the fingerprint).
  - `pub fn red_run(frozen: &Frozen, workdir: &Path, store, mission) -> Result<RedReport>`
    runs each check; a check whose exit equals `expect_exit` is recorded
    `CheckBlocked { reason: "passed before any work" }`; others are
    `CheckRed`. `RedReport { blocked: Vec<String> }`.
  - `pub fn check_all(frozen, workdir, session, store, mission) -> Result<bool>`
    verifies the fingerprint first, runs every non-blocked check, records
    `CheckRun` for each, returns true only if all passed and none are
    blocked. Timeout 5 minutes per check.

- [ ] **Step 1: Failing tests:** `freeze_is_stable_and_read_only`,
  `tampered_frozen_checks_are_refused`,
  `check_that_never_failed_is_blocked`, `failing_then_passing_check`
  (empty workdir → red; write the expected file → `check_all` true),
  `mission_with_blocked_check_is_never_done` (`check_all` false even when
  the other checks pass).
- [ ] **Step 2: Local compile RED.** **Step 3: Implement.** **Step 4: PR; CI green; merge.**

### Task 6: Engine trait and the scripted fake engine

**Files:**
- Create: `core/src/engine/mod.rs`, `core/src/engine/fake.rs`, `core/tests/fake_engine.rs`

**Interfaces:**
- Produces:
  - `pub struct SessionInput { pub brief: String, pub worktree: PathBuf, pub session: u32, pub timeout: Duration }`
  - `pub enum SessionOutcome { Ended { note: String }, TimedOut, Failed { reason: String }, RateLimited { retry_after: Duration } }`
  - `pub trait Engine { fn name(&self) -> &str; fn version(&self) -> Result<String>; fn run(&mut self, input: &SessionInput, on_line: &mut dyn FnMut(&str), cancel: &CancelToken) -> Result<SessionOutcome>; }`
  - `pub struct CancelToken(Arc<AtomicBool>)` with `cancel()`, `is_cancelled()`.
  - `pub struct FakeEngine` from a script TOML: `[[session]] delay_ms = 0`
    and `[[session.write]] path = "..." content = "..."`; session N uses
    entry N (the last entry repeats). It emits one transcript line per
    write plus `brief: <first 200 chars of the brief>`, honours
    `cancel` during `delay_ms`, and refuses paths that leave the worktree.

- [ ] **Step 1: Failing tests:** `fake_applies_session_writes_in_order`,
  `fake_repeats_last_session`, `fake_refuses_paths_outside_worktree`,
  `fake_stops_when_cancelled`, `fake_reports_brief_in_transcript`.
- [ ] **Step 2: Local compile RED.** **Step 3: Implement.** **Step 4: PR; CI green; merge.**

### Task 7: The mission loop and `cortex run`

**Files:**
- Create: `core/src/mission.rs`, `core/tests/mission.rs`
- Modify: `core/src/main.rs` (`cortex run --repo <path> --checks <checks.toml> [--engine fake|claude|api] [--fake-script <file>] "<request>"`, `cortex status <mission>`)

**Interfaces:**
- Consumes: Tasks 2–6.
- Produces: `pub struct Budgets { sessions: u32 = 4, session_timeout = 20 min, mission_timeout = 2 h, replans: u32 = 2 }`;
  `pub fn start(store, home, repo, checks, request, person) -> Result<MissionId>`;
  `pub fn drive(store, home, m: &MissionId, engine: &mut dyn Engine, budgets: &Budgets) -> Result<Outcome>`
  with `pub enum Outcome { Done { commit: String }, Stuck { reason: String }, Paused }`;
  `pub fn brief(request, failing: &[CheckRun], steers: &[String]) -> String`.
- Behaviour of `drive`, all state read back from events:
  1. If no `PlanVersion` yet: freeze checks, append `PlanVersion { version: 1 }`,
     red-run against a worktree at `base_commit` (then discard it).
  2. Loop: take the lease; build the brief from the request, the latest
     failing check outputs and every `Steer` since the last session; create
     a worktree from the latest `CommitAccepted` (or `base_commit`);
     append `SessionStarted`; run the engine (sandbox-wrapped for real
     engines, Task 10); stream `Transcript` lines; append `SessionEnded`;
     `commit_all`; run `check_all`; if all pass append `CommitAccepted`,
     `accept(repo, "cortex/<mission>", commit)` and `MissionDone`;
     otherwise discard and loop.
  3. Stop with `MissionStuck` when any budget runs out; on
     `RateLimited` append `Parked` and sleep until it ends.
  4. The minimal agreement: Cortex never merges into the person's
     branch; `cortex status` prints the result branch and the commands to
     merge it.

- [ ] **Step 1: Failing tests** (fake engine, a temp git repo, real sandbox):
  `solves_on_first_session`, `retries_with_failure_output_in_brief`
  (session 1 wrong, session 2 right; the session-2 brief contains the
  failing check's output), `stops_stuck_after_session_budget`,
  `engine_claim_is_only_a_note` (fake ends with note "all tests pass" but
  writes nothing → no `CommitAccepted`), `result_is_on_cortex_branch_only`
  (the repo's checked-out branch is unchanged).
- [ ] **Step 2: Local compile RED.** **Step 3: Implement.** **Step 4: PR; CI green; merge.**

### Task 8: Crash recovery and `cortex resume`

**Files:**
- Create: `core/tests/recovery.rs`
- Modify: `core/src/mission.rs`, `core/src/main.rs` (`cortex resume <mission>`)

**Interfaces:**
- Produces: on `drive` entry, a `SessionStarted` with no `SessionEnded`
  or `SessionAbandoned` after it gets `SessionAbandoned { reason: "process ended" }`
  and its worktree is discarded; a `SessionEnded` with no following
  `CheckRun` re-runs the checks on that session's worktree if it still
  exists, else abandons it; a `CheckRun` set that all passed with no
  `CommitAccepted` re-commits and accepts (idempotent: `accept` is a ref
  update to the same commit).

- [ ] **Step 1: Failing tests:** for every `kind` in `[mission_started,
  plan_version, check_red, session_started, transcript, session_ended,
  check_run, commit_accepted]`, `kill_after_<kind>_then_resume_matches_clean_run`:
  spawn `cortex run` with `CORTEX_FAULT=after:<kind>` and the fake engine,
  assert it aborted, run `cortex resume`, assert `MissionDone` and the
  branch's tree equals the tree of an uninterrupted run, and that exactly
  one `CommitAccepted` exists. Plus `resume_refuses_while_lease_is_live`.
- [ ] **Step 2: Local compile RED.** **Step 3: Implement.** **Step 4: PR; CI green; merge.**

### Task 9: `steer` and `stop`

**Files:**
- Create: `core/tests/steering.rs`
- Modify: `core/src/mission.rs`, `core/src/main.rs`
  (`cortex steer <mission> "<text>"`, `cortex stop <mission> ["<text>"]`)

**Interfaces:**
- Produces: `steer` appends `Steer`; the next brief includes it.
  `stop` appends `Interrupt`; the running `drive` polls events every
  500 ms, cancels the session's `CancelToken`, appends
  `SessionAbandoned { reason: "interrupted" }`, discards the worktree,
  then: with text, counts a re-plan and starts the next session with the
  text in its brief; without text, appends `MissionPaused` and returns
  `Outcome::Paused`. `cortex resume` on a paused mission appends
  `MissionResumed` and continues.

- [ ] **Step 1: Failing tests:** `steer_during_session_reaches_next_brief`,
  `steer_between_sessions_reaches_next_brief`,
  `stop_with_text_cancels_and_replans` (fake session 1 has `delay_ms =
  30000`; stop within 2 s; session 2 brief contains the text; whole test
  under 15 s), `stop_without_text_pauses_then_resume_finishes`,
  `replan_budget_exhausted_is_stuck`.
- [ ] **Step 2: Local compile RED.** **Step 3: Implement.** **Step 4: PR; CI green; merge.**

### Task 10: Cortex's MCP tool server and the Claude Code engine

**Files:**
- Create: `core/src/mcp.rs`, `core/src/engine/claude.rs`,
  `core/tests/mcp.rs`, `core/tests/claude_engine.rs`,
  `core/tests/fixtures/claude-stream.jsonl`
- Modify: `core/src/main.rs` (`cortex mcp --worktree <path> --checks <dir>`, hidden)

**Interfaces:**
- Produces:
  - MCP server (rmcp, stdio) with tools `run { command: String, timeout_s: Option<u32> }`
    and `test {}` (runs the frozen checks and returns their outputs, but
    records nothing: only the checker's runs count). Both execute through
    `sandbox::run` with the worktree as `/work`.
  - `pub struct ClaudeEngine { pub binary: PathBuf }` implementing
    `Engine`. Command line: `claude -p --output-format stream-json
    --verbose --tools "Read,Edit,Write,Glob,Grep" --strict-mcp-config
    --mcp-config <file> --permission-mode acceptEdits` plus
    `--disallowedTools` rules for reads outside the worktree, run under
    `bwrap` with network, the worktree read-write and only the engine's
    own config folder visible; brief on stdin. Each stream-json line is a
    transcript line; the final `result` message's text is the note; a
    rate-limit error maps to `RateLimited`.
  - `pub fn build_args(&self, input: &SessionInput, mcp_config: &Path) -> Vec<String>` (tested).
  - `#[ignore]` owner-run test `claude_cannot_run_shell`: asks the engine
    to run `id > proof.txt` without Cortex's tools; passes only if
    `proof.txt` is absent.

- [ ] **Step 1: Failing tests:** `mcp_lists_run_and_test`,
  `mcp_run_executes_in_sandbox` (output contains `gVisor` from
  `/proc/version`), `mcp_test_records_no_events`,
  `claude_args_have_no_shell_and_strict_mcp`, `claude_stream_fixture_parses`
  (fixture → transcript lines and the final note),
  `claude_rate_limit_maps_to_parked`.
- [ ] **Step 2: Local compile RED.** **Step 3: Implement.** **Step 4: PR; CI green; merge.**

### Task 11: Minimal API engine on recorded responses

**Files:**
- Create: `core/src/engine/api.rs`, `core/tests/api_engine.rs`,
  `core/tests/fixtures/api-session.json`

**Interfaces:**
- Produces: `pub trait Transport { fn send(&mut self, body: &serde_json::Value) -> Result<serde_json::Value>; }`;
  `HttpTransport` (ureq, `https://api.anthropic.com/v1/messages`, key from
  `ANTHROPIC_API_KEY`, never logged); `ReplayTransport` (fixture of
  request/response pairs). `ApiEngine<T: Transport>` implementing `Engine`
  with its own tool loop over `read_file`, `write_file`, `list_files`
  (confined to the worktree) and `run` (through `sandbox::run`), max 50
  turns per session.
- [ ] **Step 1: Failing tests:** `api_engine_replays_session_and_writes_files`,
  `api_engine_confines_paths`, `api_engine_routes_run_through_sandbox`,
  `api_key_never_in_transcript`.
- [ ] **Step 2: Local compile RED.** **Step 3: Implement.** **Step 4: PR; CI green; merge.**

### Task 12: The bench and `cortex bench`

**Files:**
- Create: `bench/prompts/01-fizzbuzz/` … `bench/prompts/10-<name>/`, each
  with `prompt.md`, `checks.toml`, `files/` (hidden test scripts in
  Python 3 standard library or POSIX shell), `start/` (starting project,
  may be empty) and `fake-solution.toml` (fake-engine script that solves
  it); `core/src/bench.rs`; `core/tests/bench.rs`; `bench/README.md`
- Modify: `core/src/main.rs` (`cortex bench [--engine fake|claude|api] [--only <id>]`),
  `STATE.md`

**Interfaces:**
- Produces: `cortex bench` creates a temp git repo from each `start/`,
  runs a mission, and writes `bench/results/<UTC date>-<engine>.json`:
  `{ engine, engine_version, cortex_commit, started, prompts: [{ id,
  passed, sessions, seconds, blocked_checks, mission_id }], passed, total }`.
  Results are only ever written by this command.
- The ten prompts (small, no dependencies beyond Python 3 and the shell):
  FizzBuzz CLI; word counter; Roman numerals both ways; JSON config
  merger; CSV column statistics; bracket validator; LRU cache class;
  todo-list CLI that persists to a file; Markdown heading table of
  contents; rate limiter with an injected clock.

- [ ] **Step 1: Failing tests:** `every_prompt_check_fails_on_start`
  (red run on each `start/`: no blocked checks), `fake_bench_passes_all_ten`
  (writes a results file to a temp dir with `passed == 10`),
  `results_file_matches_schema`.
- [ ] **Step 2: Local compile RED.** **Step 3: Implement.** **Step 4: PR; CI green; merge.**
- [ ] **Step 5: Owner-run real bench (needs the owner).** In WSL Ubuntu:
  `cortex doctor`, then `cargo test -- --ignored claude_cannot_run_shell`,
  then `cortex bench --engine claude`. Slice 1 is done when at least 7 of
  10 pass and the generated results file is committed. Until then STATE.md
  says slice 1 is built but unproven on a real engine.
