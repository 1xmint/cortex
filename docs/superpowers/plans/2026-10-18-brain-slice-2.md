# Brain slice 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A bench that can tell brains apart: eight multi-feature browser
games and small web apps, graded by a real headless browser, with graders
proven both ways. Every model call is recorded with its tokens and cost.
Cost per accepted result is the headline, and a results page tells each
mission's story. It ends with one real Sonnet run that shows whether the new
tier discriminates and what a run costs.

**Architecture:** Same single Rust crate (`core/`). The new parts:
- **Schema-versioned events:** every event row carries a schema version, the
  Cortex build and the prompt version.
- **Payload store:** verbatim model requests and responses go to a separate,
  deletable table. The append-only log keeps only a `ModelCall` event that
  points at them.
- **Smallest model layer:** model profiles in TOML. A `worker` role chooses
  the profile, so the Claude CLI runs Sonnet under Cortex's own loop.
- **Multi-feature bench tier:** visible checks (the worker's `test` tool runs
  these) plus hidden checks (only the bench runs them, after acceptance).
  Browser checks are Node scripts that drive Playwright's Chromium inside the
  existing two-layer sandbox.
- **Results v2:** arms, tries, each mission's story, cost per accepted
  result, a paired comparison, and a static HTML page.

**Tech Stack:**
- Rust: unchanged dependencies.
- Browser checks: Node.js plus Playwright plus Chromium headless shell,
  pinned and installed under `/usr/local/lib/cortex-browser` (ADR 0006).

**Spec:** [docs/superpowers/specs/2026-10-18-cortex-brain-design-v2.md](../specs/2026-10-18-cortex-brain-design-v2.md)
sections 3.2, 3.5, 3.11 and 6 ("Slice 2"). It also builds on ADRs
[0003](../../adr/0003-event-log-in-sqlite.md), [0004](../../adr/0004-sandbox-bubblewrap-gvisor.md)
and [0005](../../adr/0005-engines.md).

## Global Constraints

**Where tests and runs happen:**
- Tests run on GitHub CI (`ubuntu-24.04`), never on the owner's PC.
- Locally, run only these, in `core/`:
  - `cargo fmt --check`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo check --all-targets`
- RED is a local compile failure or a CI failure. GREEN is CI.
- No real model call happens anywhere except Task 11, and Task 11 waits for
  the owner's go-ahead.
- The API engine is never run for real. It is tested on recorded responses
  only.

**Sandbox and checks:**
- Linux-only code stays behind `#[cfg(target_os = "linux")]`, as in slice 1.
- Model-written code, including the pages a browser check loads, only ever
  runs inside the sandbox (invariant 3).
- The sandbox keeps `--network=none`. Browser checks use loopback only.
- Every check, visible or hidden, must be seen failing on `start/` (invariant 2).
- A task is **accepted** only when two things are true:
  - its mission ended `Done`;
  - every hidden check passed on the accepted commit.

**The log and payloads:**
- Events are never updated or deleted.
- Verbatim model payloads never go into `events`. They go into the
  separate `payloads` table, which is deletable (design 3.2 and 3.6).
- New fields on existing `Event` variants are `#[serde(default)]`, so
  slice-1 logs still read.

**Cost:**
- Cost is never invented.
- A price that is not in a profile, or a cost no provider reported, is
  `Unknown`, never 0.
- Command-line runs on a subscription record:
  - the CLI's dollar figure as an *estimated API-equivalent* cost;
  - actual spend as `Subscription`.
- Every bench result made through a command-line engine is labelled
  `provisional: true` (design 3.5).

**The holdout:**
- Holdout task text, file names, solutions and stories never appear in this
  public repository, in its PRs or in commit messages.
- Holdout results are written only under the holdout checkout.

**Defaults:**
- Bench budgets for the multi tier: 4 sessions, 20 minutes per session, and
  a **30-minute mission cap**.
- The small tier keeps the slice-1 defaults.

**How work lands:**
- Each task lands as its own PR, merged when CI is green, with STATE.md
  updated.
- Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **Unknown cost silently becoming zero.**
   - If any attempt in a batch has unknown cost, the batch's cost per
     accepted result is `Unknown`.
   - With zero accepted, it is `Unbounded`, never a number.
   - Task 9 tests: `unknown_cost_is_never_zero`, `no_accepted_is_unbounded`.
2. **Model-written text on the results page.** Stories quote check output,
   file names and stuck reasons that a model wrote, and all of it must be
   HTML-escaped. Task 10 test: `page_escapes_model_text`, which uses a file
   named `<script>alert(1)</script>.js`.
3. **A slice-1 database opened by the new build.**
   - Old rows have no version column. They must read as version 1 with
     their events intact.
   - New appends must work on the same file.
   - Task 2 test: `slice1_database_upgrades_in_place`.
4. **Holdout leaking into the public repository.**
   - When the bench `--dir` holds a `bench.toml` with `holdout = true`,
     results and pages go only to that directory's `results/`.
   - The bench refuses `--results` outside that directory.
   - Task 7 test: `holdout_results_stay_in_holdout_dir`.
5. **A page that hangs or spins the browser.**
   - A check whose page never settles is killed at the check timeout.
   - The browser process does not outlive the sandbox.
   - An uncaught page error fails the check even if the assertions pass.
   - Task 1 tests: `hung_page_is_killed_at_timeout`, `page_error_fails_check`.

---

### Task 1: A headless browser inside the sandbox (riskiest first)

This proves the assumption that most likely sinks the slice: that Chromium
runs under bubblewrap plus rootless gVisor with no network, and can load a
page from a loopback server.

**If it cannot be made to work, stop.** Write the finding, and propose a
superseding ADR to the owner. Never run browser checks with one layer or
outside the sandbox.

**Files:**
- Create:
  - `docs/adr/0006-browser-checks.md`
  - `scripts/install-browser-tools.sh`
  - `bench/browser/cortex-check.mjs`
  - `bench/browser/package.json` (pins exact versions)
  - `bench/browser/package-lock.json`
  - `core/tests/browser.rs`
  - `core/tests/fixtures/browser/` (tiny pages: `ok/`, `error/`, `hang/`)
- Modify:
  - `core/src/sandbox.rs` (add `--ro-bind-try` for `/etc/fonts`; nothing
    else is widened)
  - `.github/workflows/rust.yml` (run the install script before
    `cargo test`)
  - `docs/adr/README.md` (via `scripts/adr-index.sh`)
  - `STATE.md`

**Interfaces:**
- **ADR 0006** chooses Playwright (Node) with its pinned Chromium headless
  shell. It scores the gold-rule criteria against Puppeteer and raw
  DevTools Protocol. The reasons are auto-waiting, which cuts flaky checks,
  and a maintained cross-browser path.
- **`scripts/install-browser-tools.sh`** installs the toolchain at
  `/usr/local/lib/cortex-browser/`:
  - a pinned Node.js tarball, verified by its SHA-256, into `node/`;
  - `npm ci` of `bench/browser/` into `node_modules/`;
  - the browsers, with `PLAYWRIGHT_BROWSERS_PATH=/usr/local/lib/cortex-browser/browsers`
    and `playwright install --with-deps chromium-headless-shell`.

  It is idempotent and needs sudo. The same script sets up WSL for Task 11.
  The tree sits under `/usr`, so the sandbox's existing read-only `/usr`
  bind already shows it.
- **`bench/browser/cortex-check.mjs`** is copied into each frozen check set
  as `files/lib/cortex-check.mjs` (Task 6). It exports:
  - `export async function withPage(fn, { entry = "index.html", timeoutMs = 60000 } = {})`

  The function:
  - serves `/work` read-only over HTTP on `127.0.0.1:0`;
  - launches Chromium headless with `--no-sandbox` (the sandbox is the
    boundary) and `--disable-dev-shm-usage`;
  - opens `entry`, records page errors and `console.error`, and runs
    `fn(page)`;
  - fails (throws) on any page error, even after `fn` succeeds;
  - always closes the browser.

  It imports Playwright by absolute path,
  `/usr/local/lib/cortex-browser/node_modules/playwright/index.mjs`, and sets
  the browsers path itself.
- **`export function check(name, cond, detail)`** prints `PASS name` or
  `FAIL name: detail` and sets the process exit code to 1 on failure. A
  check script runs all its assertions, then exits.

- [ ] **Step 1: Write the failing tests** in `core/tests/browser.rs`. All of
  them are `#[cfg(target_os = "linux")]` and run through `sandbox::run`,
  with `checks_dir` holding `lib/cortex-check.mjs` plus a test script.
  - `browser_loads_page_and_clicks`: the `ok/` page has a button that sets
    text to `clicked`. The script clicks it and checks the text. Assert
    exit 0 and that stdout contains `PASS clicked`.
  - `page_error_fails_check`: the `error/` page throws on load, and the
    script's own assertion passes. Assert the exit code is not 0.
  - `hung_page_is_killed_at_timeout`: the `hang/` page runs
    `while(true){}`, with a 20 s `RunSpec.timeout`.
    - Assert `timed_out`.
    - Assert the call returns within 40 s.
    - Assert no `chrome` process is left on the host
      (`pgrep -f cortex-browser` is empty).
  - `browser_has_no_network`: the script runs `page.goto("http://1.1.1.1")`.
    Assert it fails, and `PASS offline` is printed when the script catches
    that.
  - `sandbox_self_test_still_passes`: `sandbox::self_test()` is still `Ok`
    after the `/etc/fonts` bind.
- [ ] **Step 2: Local compile shows RED.** `cargo check --all-targets`
  passes, because these tests only need existing APIs. RED is CI instead:
  push the tests with the install step but without `cortex-check.mjs`, and
  confirm `browser_loads_page_and_clicks` fails.
- [ ] **Step 3: Implement.** Write the ADR, the install script, the helper
  and the sandbox bind. If Chromium crashes under gVisor, try these
  Chromium flags first, record each as a ruling, and go no further:
  - `--disable-gpu`
  - `--single-process`
  - `--no-zygote`
- [ ] **Step 4: Open the PR; CI green.** All five tests ran. The PR body
  quotes the CI lines for each. Merge.

### Task 2: Schema-versioned events naming their build

**Files:**
- Modify: `core/src/store.rs`, `core/src/event.rs`, `core/tests/store.rs`
- Create: `core/build.rs`, `core/tests/fixtures/slice1.db` (a database
  written by the slice-1 build, with one mission of five events)

**Interfaces:**
- **Constants:**
  - `pub const SCHEMA_VERSION: u32 = 2;` in `event.rs`. Version 1 is
    slice 1's format.
  - `pub const PROMPT_VERSION: &str` in `engine/mod.rs`, a date-tag string
    bumped whenever any brief template changes.
- **Build commit:** `core/build.rs` sets `CORTEX_BUILD` from
  `git rev-parse --short=12 HEAD`, or `unknown`.
  `pub const BUILD: &str = env!("CORTEX_BUILD");` in `lib.rs`.
- **Schema:**
  - `events` gains `schema_version INTEGER NOT NULL DEFAULT 1`,
    `build TEXT NOT NULL DEFAULT 'unknown'` and
    `prompt_version TEXT NOT NULL DEFAULT ''`.
  - `Store::open` adds missing columns with `ALTER TABLE … ADD COLUMN`.
    This runs once, inside a transaction, and is detected through
    `PRAGMA table_info`.
  - `append` writes `SCHEMA_VERSION`, `BUILD` and `PROMPT_VERSION`.
- **Reading old events:** `fn upgrade(version: u32, body: serde_json::Value) -> Result<Event>`
  converts old bodies as they are read, and old rows are never rewritten.
  For 1 to 2 it is the identity, because new fields default. A version
  newer than `SCHEMA_VERSION` is `Err(Invalid("event from a newer Cortex"))`.
- **Produces:**
  - `pub struct StoredEvent { pub seq: u64, pub schema_version: u32, pub build: String, pub prompt_version: String, pub event: Event }`
  - `Store::stored_events(&MissionId) -> Result<Vec<StoredEvent>>`
  - `events()` keeps its signature.

- [ ] **Step 1: Write the failing tests.**
  - `appended_event_records_version_and_build`: `stored_events` returns
    `schema_version == 2`, `build == cortex::BUILD` and a non-empty
    `prompt_version`.
  - `slice1_database_upgrades_in_place`:
    - Copy the fixture to a temp dir and open it.
    - Expect the five old events back, each with `schema_version == 1`.
    - Append one more event; it gets `schema_version == 2`.
    - Reopen; there are now six events.
  - `newer_event_is_refused_not_guessed`: insert a row with
    `schema_version = 99` through raw SQL. `events()` returns `Err` and
    names the version.
  - `columns_cannot_be_updated`: the existing append-only triggers still
    reject an `UPDATE events SET build = 'x'`.
- [ ] **Step 2: Local compile shows RED** (`StoredEvent` is undefined).
- [ ] **Step 3: Implement.** Build the fixture with the slice-1 binary at
  the current `main`, using `cortex run --engine fake` on CI. Commit it as
  a binary fixture, with a one-line note in the test naming the commit it
  came from.
- [ ] **Step 4: Open the PR; CI green; merge.**

### Task 3: Payload store and model-call records

**Files:**
- Create: `core/src/payload.rs`, `core/tests/payload.rs`
- Modify:
  - `core/src/event.rs` (new variant)
  - `core/src/store.rs` (new table)
  - `core/src/lib.rs`

**Interfaces:**
- **Table:** `payloads (id TEXT PRIMARY KEY, mission_id TEXT NOT NULL, project_id TEXT NOT NULL, sha256 TEXT NOT NULL, body BLOB NOT NULL, at INTEGER NOT NULL)`.
  It has no append-only triggers, because deletion is its purpose.
- **Payload calls:**
  - `Store::put_payload(&MissionId, &[u8]) -> Result<PayloadRef>`
  - `Store::payload(&PayloadRef) -> Result<Option<Vec<u8>>>` (`None` once
    deleted)
  - `Store::delete_payload(&PayloadRef) -> Result<()>`
- **Payload types:**
  - `pub struct PayloadRef { pub id: String, pub sha256: String, pub bytes: u64 }`
    (serde)
  - `pub enum Spend { Usd { micros: u64 }, Subscription, Unknown }`
  - `pub struct Tokens { pub input: Option<u64>, pub output: Option<u64>, pub cache_read: Option<u64>, pub cache_write: Option<u64> }`
- **New event:** `Event::ModelCall`, with these fields:
  - `session: u32`
  - `role: String` (`"worker"` for now)
  - `provider: String` (`anthropic-api`, `claude-cli` or `fake`)
  - `profile: String`
  - `model: String`
  - `params: serde_json::Value` (temperature, max tokens and the like, as
    sent)
  - `tokens: Tokens`
  - `estimated_usd_micros: Option<u64>`
  - `spent: Spend`
  - `millis: u64`
  - `request: Option<PayloadRef>` (`None` when the provider does not show
    it, as with the CLI)
  - `response: Option<PayloadRef>`
- **Engine hook:** `SessionInput` gains `record: &mut dyn FnMut(ModelCallRecord)`.
  This is plumbed as a second callback beside `on_line` in `Engine::run`.
  The mission loop turns each record into payloads plus one `ModelCall`
  event. `pub struct ModelCallRecord` holds the event's fields, with raw
  `request` and `response` bytes in place of the refs.

- [ ] **Step 1: Write the failing tests.**
  - `model_call_points_at_payloads`:
    - Record a call with a 10 KB response.
    - The event body in `events` is under 2 KB.
    - `payload(ref)` returns the exact bytes, and the SHA-256 matches.
  - `deleted_payload_leaves_audit_event`: after `delete_payload`,
    `payload(ref)` is `None`, and the `ModelCall` event still reads with
    its tokens.
  - `old_model_call_without_fields_reads`: a `ModelCall` JSON missing
    `cache_read` and `params` still deserialises, with `None` and null.
  - `fake_engine_records_one_call_per_session`: a two-session fake mission
    has two `ModelCall` events, with `provider == "fake"` and
    `spent == Unknown`.
- [ ] **Step 2: Local compile shows RED.**
- [ ] **Step 3: Implement.** Make the matching changes in the fake engine
  (one record per session, tokens all `None`), the API engine and the
  Claude engine:
  - **API engine:** one record per HTTP exchange. Record the verbatim
    request and response bodies, `usage.*` tokens, and `Spend::Usd` from
    the profile price (Task 4). Until Task 4 lands, cost is `Unknown`.
  - **Claude engine:** one record per `assistant` stream line, with
    `message.model` and `message.usage` tokens and that line as the
    response.
    - There is no request (the CLI hides it).
    - Spend is `Subscription`.
    - `estimated_usd_micros` is set from the profile price.
    - The `result` line's `total_cost_usd` goes on `SessionEnded`
      (`reported_estimate_usd_micros: Option<u64>`, a defaulted field).
  - Update the existing recorded-response tests in `api_engine.rs` and
    `claude_engine.rs`: one `ModelCall` per exchange or assistant line,
    with the recorded token counts.
- [ ] **Step 4: Open the PR; CI green; merge.**

### Task 4: The smallest model layer: profiles and the worker role

**Files:**
- Create: `core/src/model.rs`, `core/profiles.toml`, `core/tests/model.rs`
- Modify:
  - `core/src/engine/claude.rs` (`--model`)
  - `core/src/engine/api.rs` (model from the profile instead of the
    `MODEL` constant)
  - `core/src/main.rs` (`--worker <profile>` on `run`, `resume` and
    `bench`)
  - `core/src/lib.rs`

**Interfaces:**
- **Profile type:** `pub struct Profile`, with these fields:
  - `id: String`
  - `engine: EngineKind` (`fake`, `claude-cli` or `api`)
  - `model: String`
  - `price_in_per_mtok_usd: Option<f64>`
  - `price_out_per_mtok_usd: Option<f64>`
  - `price_cache_read_per_mtok_usd: Option<f64>`
  - `price_cache_write_per_mtok_usd: Option<f64>`
  - `context_tokens: Option<u64>`
  - `knowledge_cutoff: Option<String>`
  - `tags: Vec<String>`
  - `price_source: Option<String>` (URL and date)
- **Loading and cost:**
  - `pub fn load_profiles(cortex_home: &Path) -> Result<Vec<Profile>>`
    reads the embedded `core/profiles.toml` (`include_str!`), then
    `$CORTEX_HOME/profiles.toml` entries override by `id`.
  - `pub fn estimate_usd_micros(p: &Profile, t: &Tokens) -> Option<u64>` is
    `None` when any needed price or token count is missing.
  - `pub struct Roles { pub worker: String }` defaults to `claude-cli-sonnet`.
- **Shipped profiles:**
  - `fake`
  - `claude-cli-sonnet` (`claude-sonnet-5-5`)
  - `claude-cli-opus` (`claude-opus-5-5`)
  - `api-sonnet` (`claude-sonnet-5-5`)

  Prices are copied from Anthropic's pricing page at implementation time,
  with `price_source` set. If a price cannot be confirmed, leave it out, so
  the cost is `Unknown`.
- **CLI behaviour:**
  - `ClaudeEngine::build_args` adds `--model <profile.model>`. The built-in
    tools stay off, as today.
  - A profile whose engine is `claude-cli` marks the run `provisional`.
- **Deferred to later slices** (named here so nobody builds them now): the
  internal message type, and the OpenAI-compatible adapter. Slice 2 has one
  provider family, and the design's adapter rules start to matter only
  when a second family arrives.

- [ ] **Step 1: Write the failing tests.**
  - `embedded_profiles_parse_and_ids_are_unique`.
  - `home_profile_overrides_by_id`: a home file changes `claude-cli-sonnet`
    pricing; the loaded profile shows the new price, and the others are
    unchanged.
  - `missing_price_estimates_none`: a profile without `price_out` gives
    `estimate_usd_micros == None`, and never `Some(0)`.
  - `estimate_is_exact_for_known_prices`: with 1,000,000 input tokens at
    $3 and 0 output at $15, the estimate is 3,000,000 micros.
  - `claude_args_carry_profile_model`: `build_args` contains `--model` then
    `claude-sonnet-5-5`, and still contains the slice-1 tool restrictions.
    Reuse the existing assertion from `claude_engine.rs`.
  - `api_request_uses_profile_model`: the recorded request body has
    `"model": "claude-sonnet-5-5"`.
- [ ] **Step 2: Local compile shows RED.**
- [ ] **Step 3: Implement.** Set costs in the engines from Task 3.
- [ ] **Step 4: Open the PR; CI green; merge.**

### Task 5: Multi-feature task format, graders proven both ways

**Files:**
- Create:
  - `core/src/bench/task.rs` (move `bench.rs` to `core/src/bench/mod.rs`)
  - `core/tests/graders.rs`
  - `core/tests/fixtures/multi/t00-counter/`, a deliberately tiny browser
    task that exercises the format only
- Modify:
  - `core/src/bench/mod.rs`
  - `core/src/engine/fake.rs`
  - `core/src/mission.rs` (budgets passed in)
  - `bench/README.md`

**Interfaces:**
- **Layout:** a multi task lives at `bench/multi/<id>/`. It holds:
  - `prompt.md`;
  - `task.toml`, with `tier = "multi"`, `kind = "fresh" | "change" | "foreign"`
    and `features = [..]`, each feature a short name;
  - `start/`;
  - `visible/checks.toml` plus `visible/files/`;
  - `hidden/checks.toml` plus `hidden/files/`;
  - `solutions/reference/`, a file overlay applied on top of `start/`;
  - `solutions/bad-<name>/`, overlays, each with a `WHY.md` saying what is
    wrong. At least two per task: `bad-gamer`, which passes every visible
    check and fails at least one hidden check, and one other that breaks a
    feature.
- **Staging:** `pub fn stage_checks(dir: &Path, out: &Path) -> Result<PathBuf>`
  copies a checks dir and adds `files/lib/cortex-check.mjs` from
  `bench/browser/`. It returns the staged `checks.toml` path, which is then
  frozen as in slice 1.
- **Task loading:**
  - `pub struct Task { pub id: String, pub dir: PathBuf, pub tier: Tier, pub kind: Kind, pub features: Vec<String> }`
  - `pub enum Tier { Small, Multi }`
  - `pub fn tasks(bench: &Path, tier: Option<Tier>) -> Result<Vec<Task>>`
    reads `prompts/` as `Small` (slice-1 layout unchanged) and `multi/` as
    `Multi`.
- **Fake engine:** `FakeEngine::from_overlay(dir: &Path) -> Result<FakeEngine>`
  is one session that writes every file under `dir` (text only; it rejects
  non-UTF-8).
- **Grading:** `pub fn grade(task: &Task, overlay: &Path, home: &Path) -> Result<Grade>`
  builds `start/`, applies the overlay, then runs the visible and hidden
  checks in the sandbox. It returns
  `pub struct Grade { pub visible_failed: Vec<String>, pub hidden_failed: Vec<String> }`.
- **Missions:** `mission::drive` takes `&Budgets`, as it does now. The
  bench passes `Budgets { mission_timeout: 30 min, ..Default }` for `Multi`.

- [ ] **Step 1: Write the failing tests** in `core/tests/graders.rs`. They
  loop over every task in `bench/multi/` and the fixture, so later tasks
  are covered automatically.
  - `start_fails_every_check`: `red_run` on `start/` blocks no visible or
    hidden check.
  - `reference_passes_everything`: `grade(reference)` has both lists empty.
  - `every_bad_solution_fails`: for each `bad-*`, `hidden_failed` or
    `visible_failed` is non-empty.
  - `gamer_passes_visible_fails_hidden`: for `bad-gamer`, `visible_failed`
    is empty and `hidden_failed` is not.
  - `task_files_are_complete`: each task has the layout above, a
    `WHY.md` for each bad, and 5 to 10 features. `foreign` tasks have
    `start/SOURCE.md` naming the upstream URL, commit and licence.
- [ ] **Step 2: Local compile shows RED** (`grade` is undefined).
- [ ] **Step 3: Implement** the format and the `t00-counter` fixture (one
  button and a counter: 2 visible checks, 1 hidden, a reference, a gamer
  and one broken bad).
- [ ] **Step 4: Open the PR; CI green; merge.**

### Task 6: Four fresh multi-feature tasks

**Files:**
- Create: `bench/multi/m01-snake/`, `m02-kanban/`, `m03-breakout/` and
  `m04-quiz/`, each in the Task 5 layout.
- Modify: `STATE.md`

**Interfaces:**
- **Feature lists.** Each prompt states every feature in plain words.
  - `m01-snake` (canvas): start screen, arrow-key steering, growth on
    food, game over on wall or self, score, best score kept in
    `localStorage`, pause on Space, restart.
  - `m02-kanban`: three columns; add a card; edit and delete; move by
    buttons and by drag; text filter; per-column counts; survives reload
    (`localStorage`).
  - `m03-breakout`: paddle by keys and mouse, ball and bricks, score,
    three lives, level 2 with a new layout, win and lose screens.
  - `m04-quiz`: questions from `questions.json`, a per-question timer,
    score, a results screen, review of wrong answers, full keyboard play,
    and restart with shuffled order.
- **Determinism for games.** Each prompt for a real-time game requires a
  documented test hook, so checks do not race the clock:
  - `window.game.step(ms)` advances the simulation, and the game does not
    advance on its own when `?test=1`;
  - `window.game.state()` returns a JSON snapshot whose fields are listed
    in the prompt.

  Checks drive input with Playwright and assert on visible text and the
  DOM, using `state()` only where pixels would be the alternative.
- **Visible versus hidden checks.** Visible checks cover the happy path of
  about half the features. Hidden checks cover:
  - every feature;
  - the edge cases the prompt states (empty input, reload persistence,
    the second level);
  - "no page errors over a full game".
- **Content source.** Checks and solutions are written for this repository
  and taken from no existing project.

- [ ] **Step 1:** Add `m01` to `m04` with their visible and hidden checks
  and the bad solutions, but no `solutions/reference/` yet. CI RED:
  `task_files_are_complete` and `reference_passes_everything` fail.
- [ ] **Step 2:** Add the references. CI GREEN: all Task 5 tests pass for
  the four tasks.
- [ ] **Step 3: Open the PR; merge.** The PR body lists, per task, its
  features and what each bad solution gets wrong.

### Task 7: Two change tasks, one foreign, one fresh; the holdout guard

**Files:**
- Create: `bench/multi/m05-memory-match/`, `m06-expenses/`,
  `m07-2048-foreign/` and `m08-paint/`, plus `core/tests/holdout.rs`.
- Modify: `core/src/bench/mod.rs`, `core/src/main.rs`, `STATE.md`

**Interfaces:**
- **`m05-memory-match`** (`change`):
  - `start/` is a working card-matching game, written for this repository.
  - The prompt adds a move counter, a timer, three difficulty sizes, the
    best score per size, and restart.
  - Hidden checks include **preservation checks**: the original matching
    and win flow still work.
- **`m06-expenses`** (`change`):
  - `start/` is an add-and-list expense app.
  - The prompt adds categories, edit and delete, monthly totals, a
    category chart (canvas, checked through a
    `data-total-<category>` attribute the prompt names), CSV export
    (checked through the download Playwright captures), and persistence.
  - It has preservation checks.
- **`m07-2048-foreign`** (`foreign`):
  - `start/` is a vendored copy of `gabrielecirulli/2048` (MIT) at a
    pinned commit, with `SOURCE.md` and its `LICENSE`. This is the
    "repository Cortex did not write" (design 3.6a).
  - The prompt adds undo (one level), the best score surviving reload,
    an animations toggle, a "keep playing after 2048" button, and a seeded
    mode `?seed=N` for deterministic tile spawns.
  - Checks use the seed.
- **`m08-paint`** (`fresh`): pen, eraser, five colours, three brush sizes,
  undo and redo, clear with confirm, and PNG export (the check verifies
  the downloaded file's PNG signature and its size).
- **Holdout guard:** `bench.toml` at a bench root may say `holdout = true`.
  When it does:
  - results go to `<dir>/results/` only;
  - an explicit `--results` path outside `<dir>` is `Err(Invalid)`;
  - the run prints `holdout run: results stay in <dir>`.

  `cortex bench` gains `--tier small|multi|all` (default `all`).

- [ ] **Step 1: Write the failing test** in `core/tests/holdout.rs`.
  - `holdout_results_stay_in_holdout_dir`:
    - Make a temp holdout bench with `bench.toml` (`holdout = true`) and
      the `t00` fixture copied in.
    - A fake run writes only under its `results/`.
    - `--results <public temp dir>` is refused.
- [ ] **Step 2:** Add the four tasks without references (CI RED), then add
  the references (CI GREEN). This is the same rhythm as Task 6.
- [ ] **Step 3: Implement the guard and `--tier`. Open the PR; CI green;
  merge.** After this there are eight multi tasks: three not fresh
  (`m05`, `m06`, `m07`) and one foreign (`m07`).

### Task 8: The holdout set (private repository)

**Files** (in `1xmint/cortex-holdout`, never in this repo):
- `bench.toml` (`holdout = true`)
- `multi/h01-…/` to `multi/h04-…/`, in the Task 5 layout
- `results/.gitkeep`
- `.github/workflows/graders.yml`
- `README.md`, which says what this is and that nothing here is copied to
  the public repo

**Interfaces:**
- **Tasks:** four multi tasks at the same difficulty as `m01` to `m08`:
  two fresh, one change, one foreign. Their themes are chosen at writing
  time and named only in the private repo.
- **`graders.yml`:**
  - It runs on PR and `workflow_dispatch`.
  - It checks out `1xmint/cortex` at a pinned commit (the merge commit of
    Task 7), runs the install script, and runs
    `CORTEX_BENCH_DIR=$GITHUB_WORKSPACE/holdout cargo test --test graders`.
  - It uses the private repo's free Actions minutes, about 10 minutes a
    run, and needs no paid features.
- **Graders tests:** `core/tests/graders.rs` reads an optional
  `CORTEX_BENCH_DIR` environment variable. Unset, it means `bench/`.

- [ ] **Step 1:** In this repo, have `graders.rs` honour `CORTEX_BENCH_DIR`
  (a test, `graders_follow_bench_dir`, points it at a temp copy of `t00`).
  Open the PR; CI green; merge.
- [ ] **Step 2:** In `cortex-holdout`, add the tasks without references,
  run the workflow (RED), add the references, and get it GREEN. Open the
  PR there and merge it. The public PR body says only "holdout graders
  green at <private commit>", never task names.

### Task 9: Results v2: arms, tries, stories, cost per accepted result

**Files:**
- Create: `core/src/bench/score.rs`, `core/tests/score.rs`
- Modify:
  - `core/src/bench/mod.rs`
  - `core/src/main.rs` (`--arm <name>` defaults to `baseline`;
    `--tries <k>` defaults to 1)
  - `bench/README.md`
  - `STATE.md`

**Interfaces:**
- **The baseline arm.** Today's loop *is* the design's "one capable agent,
  same tools, no planner". So the baseline arm is a label on the run, not a
  second agent. Slice 3's planner becomes the second arm.
- **Results file (schema 2):**

  ```
  { schema: 2, arm, tier, worker_profile, engine, engine_version,
    provisional, cortex_commit, started,
    attempts: [ { task, kind, try, mission_id, mission_done, accepted,
                  hidden_failed, sessions, seconds, tokens,
                  estimated_usd_micros, spent, story } ],
    summary: { accepted, total, pass_rate, pass_hat_k: {task: f64},
               tokens, seconds, cost_per_accepted } }
  ```

  The slice-1 results file stays readable as schema 1, and it is not
  migrated.
- **Story:** `pub struct Story { pub request: String, pub commit: Option<String>, pub files_changed: Vec<String>, pub failures: Vec<Failure>, pub stuck_reason: Option<String> }`.
  `pub struct Failure { pub check_id: String, pub hidden: bool, pub output_tail: String }`.
  All of it is built from the event log and the hidden-check run.
- **Cost per accepted result:**
  - `pub enum PerAccepted { Usd { micros: u64 }, Unbounded, Unknown }`
  - `pub fn cost_per_accepted(attempts: &[Attempt]) -> PerAccepted`
  - The rule:
    - no accepted attempt gives `Unbounded`;
    - otherwise any attempt with `estimated_usd_micros == None` gives
      `Unknown`;
    - otherwise the result is the sum over **all** attempts divided by the
      number accepted.
- **pass^k:** `pub fn pass_hat_k(successes: u32, tries: u32, k: u32) -> f64`
  is the unbiased estimator C(c,k)/C(n,k), and is 0 when c < k.

- [ ] **Step 1: Write the failing tests** in `core/tests/score.rs`.
  - `no_accepted_is_unbounded`: three failed attempts with known cost give
    `Unbounded`.
  - `unknown_cost_is_never_zero`: two accepted attempts plus one failed
    attempt with unknown cost give `Unknown`.
  - `failed_attempts_count_in_cost`: costs 4, 2 and 6 (in micros), with
    only one accepted, give `Usd { micros: 12 }`.
  - `pass_hat_k_matches_formula`: (3,4,2) gives 0.5, (4,4,3) gives 1.0,
    and (1,4,2) gives 0.0.
  - `fake_multi_run_writes_schema2`: a fake run with `--tier multi --tries 2`
    over the fixture tasks (references as fake solutions) writes:
    - `schema == 2`;
    - `arm == "baseline"`;
    - `accepted == total`;
    - `provisional == false`;
    - each story's `files_changed` is non-empty.
  - `gamer_mission_done_but_not_accepted`: a fake run whose fake solution
    is `bad-gamer` gives `mission_done == true`, `accepted == false`, and a
    hidden failure in the story.
- [ ] **Step 2: Local compile shows RED.**
- [ ] **Step 3: Implement.** The bench runs the hidden checks on the
  mission's accepted commit in a fresh worktree, inside the sandbox.
- [ ] **Step 4: Open the PR; CI green; merge.**

### Task 10: Paired comparison and the results page

**Files:**
- Create:
  - `core/src/bench/compare.rs`
  - `core/src/bench/page.rs`
  - `core/tests/compare.rs`
  - `core/tests/page.rs`
  - `core/tests/fixtures/results/` (two synthetic schema-2 files)
- Modify: `core/src/main.rs`, `bench/README.md`, `STATE.md`

**Interfaces:**
- **Comparison:**
  - `cortex bench compare <a.json> <b.json> [--win-margin <n>] [--max-cost-ratio <r>]`
  - `pub fn compare(a: &Report, b: &Report, rule: &Rule) -> Result<Comparison>`
  - It refuses (`Err`) unless both reports cover the same task set with
    the same tries.
  - `Comparison` holds:
    - accepted counts per arm;
    - `discordant: (a_only, b_only)` over task-and-try pairs;
    - `p_value`, the exact two-sided sign test on the discordant pairs;
    - the cost ratio as `PerAccepted` b over a (it carries through
      `Unknown` and `Unbounded`);
    - `verdict: Win | NoWin | Undecided`, where `Undecided` means a cost
      was unknown.

  The margin and the cost cap are the "decided in advance" numbers a
  comparison PR states (design 3.11).
- **Page:**
  - `cortex bench page <results.json> [-o <out.html>]`
  - `pub fn render(report: &Report) -> String` returns one self-contained
    HTML file (inline CSS, no scripts, light and dark).
  - The page shows a header with arm, worker, provisional, pass rate, time,
    tokens, estimated cost and cost per accepted result, plus each
    attempt's story. A story is the request, the files changed, the commit,
    the failures (hidden marked "hidden") and the stuck reason.
  - All report text passes through one `escape()` function.
  - `bench::run` also writes the `.html` next to each `.json`.

- [ ] **Step 1: Write the failing tests.**
  - `compare_refuses_different_task_sets`.
  - `sign_test_known_values`: (0,6) gives p of about 0.03125, and (3,3)
    gives 1.0.
  - `unknown_cost_makes_verdict_undecided`.
  - `win_needs_margin_and_cost_cap`:
    - B is 2 tasks better at 1.4× the cost, with margin 2 and cap 1.5:
      `Win`.
    - At 1.6× the cost: `NoWin`.
  - `page_escapes_model_text`: a story with the file
    `<script>alert(1)</script>.js` and output `<img src=x onerror=1>`
    renders with neither raw tag present and the escaped forms present.
  - `page_shows_unbounded_and_unknown_in_words`: these render as
    "unbounded (nothing accepted)" and "unknown", never "$0".
- [ ] **Step 2: Local compile shows RED.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Open the PR; CI green; merge.**

### Task 11: The one real run (spends the owner's quota)

**Files:**
- Create: `bench/results/<date>-claude-cli-sonnet.json` and `.html`,
  written by the command only.
- Modify: `STATE.md`, and the plan's status line (landed)

**Decided in advance** (set here, before the run):
- **Shape:** one try per task, the multi tier only, worker
  `claude-cli-sonnet`, baseline arm, and a 30-minute cap per mission.
- **Accepted 7 or 8 of 8:** the tier does not discriminate. Before slice
  3, the tasks get harder: more features, or more hidden edge cases.
- **Accepted 0 of 8:** read every transcript before blaming the brain. A
  task the reference passes but no model could follow from its prompt gets
  its prompt fixed.
- **Accepted 1 to 6:** the tier discriminates, so slice 3 is sized from
  this run's tokens and time.

- [ ] **Step 1: Owner's go-ahead.** Ask in chat. Give the expected quota:
  eight missions, up to 30 minutes each, on Sonnet through the owner's
  Claude Code subscription. Do nothing further without a clear yes.
- [ ] **Step 2: WSL setup.** In WSL Ubuntu, run
  `scripts/install-browser-tools.sh`, then `cortex doctor`.
  `cargo test --test browser` must pass there; `doctor` must show both
  sandbox layers.
- [ ] **Step 3: Run.** In WSL:
  `cortex bench --tier multi --worker claude-cli-sonnet --arm baseline --tries 1`.
- [ ] **Step 4: Record.**
  - Commit the generated JSON and HTML as they came out.
  - Publish the HTML page as a private artifact for the owner.
  - Update STATE.md with the result, the rule it triggered, and the next
    step.
  - Mark this plan landed.
  - Open the PR; CI green; merge.

## Out of scope for slice 2

- The planner, the strategy layer and plain-request mode (slice 3).
- The internal message type and the OpenAI-compatible adapter (when a
  second provider family is needed).
- Replay mode, snapshots and OpenTelemetry export.
- A second arm, and holdout runs against any arm. The first holdout run
  comes with slice 3's first comparison.
