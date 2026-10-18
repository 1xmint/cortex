# Slice 2: plain words in, proven work out

Status: draft for the owner's approval
Date: 2026-10-18
Builds on: [the brain design](2026-10-18-cortex-brain-design.md) (approved),
slice 1 ([plan](../plans/2026-10-18-brain-slice-1.md), proven 10 of 10 on a
real engine).

## 1. Why this slice, and why now

Slice 1 proved the engine room: given a request **and checks a person wrote**,
Cortex gets real work out of Claude Code safely and accepts it only when its
own checks pass. But the people Cortex is for (AGENTS.md: "anyone ... from
developers who want to learn every decision to people who never want to see a
repository") cannot write checks. Until Cortex writes its own, it is not
usable by them at all.

So the owner reordered the design's slices (2026-10-18): this slice brings
forward the planner-written, second-family-reviewed checks (design slice 4)
and pairs them with a first, simple chat page (design slice 2). Memory and the
full working agreement (design slice 3) come after.

The test of success: **a person types a coding request into a web page, in
plain words, and gets back working code that Cortex has proven, without ever
writing or reading a check.**

### 1.1 Corrections after the owner's review (2026-10-18)

The owner restated the product: Cortex runs on whichever frontier models'
APIs give the best result for the money, and later on open models on
servers he owns. The Claude Code and Codex command lines are only how the
builder tests during development. Three changes follow, all the builder's
calls:

1. **Roles, not vendors.** Planner, reviewer, worker and judge are roles.
   Each role names a provider and a model in one setting, behind one
   provider-neutral model interface (5.6). Slice 1's API engine is
   Anthropic-only with the model written into the code; this slice fixes
   that. Recorded in a new ADR 0006 (ADR 0005 stays as accepted).
2. **Measure value, not just passes.** Every session records tokens and
   cost per role, and bench results show cost per prompt. "Best bang for
   the buck" cannot be chosen without it.
3. **Brain before face.** The slice is built in two parts. Part A is the
   brain: planner, reviewer and the new bench mode, run from the terminal.
   Part B, the chat page and its protocol, starts only after Part A's bench
   passes (section 6). A page on top of a brain that writes bad checks
   would only show the bad checks faster.

**Why no "whole team" of specialist roles yet.** The research behind the
brain design (its section 2) found that what wins is short sessions with
fresh context and independent checks, not many personas talking to each
other. A team of roles is how the product should feel from outside. Inside,
a role (researcher, prompter, artist) is added when a bench prompt shows
the brain failing without it, and kept only if the bench shows it helps.

## 2. Goals

1. **Cortex writes its own checks.** From a plain request (and the project,
   if there is one), a planner session writes the acceptance checks and a
   one-line plain-language description of each.
2. **A different model family reviews them** (design 3.3): Codex reviews the
   checks Claude Code wrote, against the request. It can approve or send them
   back with reasons.
3. **The checks still have to be able to fail** (invariant 2): every check is
   run on the starting project and must fail before work starts, exactly as in
   slice 1.
4. **Two ways to work, chosen per request:**
   - **"Build it with me":** Cortex shows the checks in plain words and waits
     for the person's yes (or their changes) before building, and shows each
     session's progress as it happens.
   - **"Just do it":** Cortex goes straight from reviewed checks to building
     and tells the person when it is done or stuck.
   This is the first, fixed piece of the working agreement; the full rule
   language stays in the later memory slice.
5. **A chat web page** on the design's protocol (3.10): type a request, pick
   the way to work, watch the mission live, steer it with a message, stop it,
   and get the result.
6. **Scoreboard keeps judging.** The same ten bench prompts run in a new mode
   where Cortex gets only the plain request. Its own checks decide when the
   work is accepted; the bench's hidden checks then judge whether it really
   did what was asked.

## 3. Non-goals

- Memory, the project bible, the full working agreement and notifications
  outside the chat page (next slice).
- Media pieces, creation graphs with more than one piece, the headless
  browser checker.
- Mobile or desktop apps, accounts, hosting, more than one person.
- The brain deciding steer versus interrupt from a message (design 3.1).
  Here a chat message during a mission is always a steer; the page has a
  separate Stop button. The judgement comes with the conversation work later.

## 4. Anti-goals: it would be a failure if…

- Cortex declares work done that the person's request did not ask for, or
  that the hidden bench checks show is wrong, more than 3 times in 10.
- A person has to read code or a check file to use it.
- The planner can weaken the checks after seeing the work (checks are frozen
  before the work starts, as in slice 1).
- Any website the person visits could talk to Cortex's local page and start
  missions.

## 5. How it works

### 5.1 The mission, end to end

```
request (plain words) + way of working
  → planner session writes draft checks + plain descriptions
  → Cortex validates them (they load, there is at least one, each says
    what it checks)
  → Codex reviews them against the request ── "revise" → planner again
                                              (at most 2 rounds, then stuck)
  → "with me": person sees the checks in plain words → yes / change this
  → checks frozen and fingerprinted; each must fail on the start → else
    the planner is asked once to replace the ones that already pass
  → slice 1's loop: worker sessions until every check passes (unchanged)
  → done: result shown in the chat with how to get it
```

Every step is an event in the mission's log, so a crash at any point resumes
as in slice 1, and the page can always rebuild what it shows from the log.

### 5.2 The planner

- Runs as an engine session (Claude Code, same jail as slice 1) with a
  different brief: read the request and the project; write checks into a
  **draft checks folder** (not the worktree); do not write the solution.
- A check is the slice 1 format plus one required field, `says`: one plain
  sentence a non-programmer can read ("Adding a to-do and listing them shows
  the new to-do").
- The planner may use Cortex's `run` tool (sandboxed) to try its checks
  against the starting project.
- The draft is copied out and frozen by Cortex; the planner never touches the
  frozen copy, and workers never see the draft folder.
- For an empty project the planner also decides the file and command names
  the checks expect, and states them in the brief workers receive.

### 5.3 The reviewer (Codex)

- The reviewer is a role (5.6) and must be a different model family from
  the planner's. In the product it is a model API. During development it
  is `codex exec --json` on the owner's Codex subscription, in its own
  bubblewrap jail like the Claude engine, with no worktree access: the
  request and the draft checks are passed in as quoted data.
- ADR 0005 admits Codex only after its must-fail test passes (asked to run
  `id` with `shell_tool` and `unified_exec` disabled, it must not). That
  test is the first real run this slice needs. **If Codex cannot be
  restricted, the slice stops and comes back to the owner**; it does not
  fall back to Claude reviewing Claude.
- It answers through the judgement interface (design 3.12): a fixed
  question ("Do these checks prove the request was done, without demanding
  things it did not ask for?"), answers `approve` or `revise`, reasons per
  check. The verdict and reasons are logged and shown on the page.

### 5.4 Projects and results

- A request either names an existing folder (a git repository) or starts a
  **new project**, which Cortex creates as a fresh repository under
  `~/cortex/projects/<short-name>`.
- For a new project, the accepted work is merged into its main branch when
  the mission is done; nothing of the person's is overwritten.
- For an existing repository, the result stays on the `cortex/<mission>`
  branch as in slice 1, and the page offers a Merge button that asks first
  (the fixed minimal agreement: ask before anything irreversible).

### 5.5 The page and the protocol

- `cortex serve` starts the core's protocol server and serves the page, on
  `127.0.0.1` only. The address it prints carries a random token; the
  WebSocket refuses connections without it and refuses foreign origins.
  WSL2 forwards it to the browser on Windows.
- JSON-RPC 2.0 over WebSocket (design 3.10). The message types are defined
  once in Rust and generated for TypeScript; CI fails if the generated types
  drift.
- The page (TypeScript + React, design 4): a chat column, a mission panel
  that streams events (the checks in plain words, the reviewer's verdict,
  each session starting and ending, checks passing, parked for a usage
  limit and until when), Approve / Change buttons in "with me" mode, a Stop
  button, and the result at the end.
- The terminal keeps working: `cortex run "<request>"` without `--checks`
  now runs the same planner path; with `--checks` it behaves as in slice 1.

### 5.6 Roles, models and cost

- **One model interface for every provider.** Three kinds of provider sit
  behind it:
  - the Anthropic API;
  - any OpenAI-compatible API, which covers OpenAI and the common servers
    for running open models locally (to be confirmed in ADR 0006);
  - the Claude Code and Codex command lines, for development only.
  Cortex's own tool loop (slice 1's API engine) works over the first two,
  so no vendor's agent program sits between the brain and its tools in the
  product.
- **Each role picks provider and model in one setting.** Development
  defaults: the planner and workers use Claude Code with Sonnet, not the
  CLI's default (Opus), because the owner's quota is the scarce thing; the
  reviewer uses Codex with its default.
- **Cost is recorded per session**: provider, model, input and output
  tokens, and money cost where the provider reports it (subscription runs
  record tokens only). The bench results add cost per prompt and per role.
  Choosing models by measured value per role is a later judgement (design
  3.12); this slice only makes the numbers exist.
- No live API run happens in this slice. Running the product path for
  real needs API billing, which is the owner's call when Part A is proven.

## 6. When this slice is done

**Part A, the brain (built first):**
1. ADR 0006 accepted; roles and providers are settings; sessions record
   tokens and cost.
2. In CI, with fake engines and a fake reviewer: the whole path (plan,
   review, revise, approve, freeze, see-fail, work, done) and its failure
   paths (planner writes no checks, a check already passes, review says
   revise twice, person changes a check, crash at every step) are proven,
   driven from `cortex run "<request>"`.
3. On the owner's PC, two real runs only, because each decides something:
   - Codex's must-fail test (decides whether Codex can be the reviewer).
   - The bench in the new mode (decides whether the planner's checks are
     good enough): **at least 7 of 10 pass the hidden checks**, and the
     results file, with cost per prompt, is committed.

**Part B, the face (only after Part A's bench passes):**
4. The page's protocol is tested against the core in CI.
5. The owner opens the page, types one request of his own in each way of
   working, and gets proven work back.

If Part A's bench falls short, Part B waits and the planner's brief is
fixed first.

## 7. Settled by the builder

Settled here as the author's calls, which the owner can overturn:
- A message during a mission is always a steer; Stop is a button.
- At most 2 review rounds, and 1 re-plan for checks that already pass, before
  the mission is stuck.
- New projects live under `~/cortex/projects/`.
- Sonnet for planner and workers by default.
- **"Change this"** is a free-text reply. It goes to the planner as a change
  request; the changed checks go through review and the see-fail run again.
  Person-asked changes do not count against the 2 review rounds.
- **A message before the work starts** (planning, review, waiting for yes)
  is a change request to the planner, as above. Once workers start, it is a
  steer, as in slice 1.
- **Stuck is not the end.** A stuck mission says why in plain words and waits.
  The person can reply (a change request, which re-plans) or press Stop. A
  check that already passes can never be approved through (invariant 2).
- **Existing projects** are chosen by typing a path on the page. It must be a
  git repository inside Ubuntu with no uncommitted changes. Otherwise Cortex
  refuses and says why. The page does not browse folders.
- **Many missions.** The page lists missions from the log and reattaches
  after a reload or a `cortex serve` restart. Several missions can run, one
  per project at a time (slice 1's lock).
- **Bench, new mode:** each prompt starts from its existing starting
  project, in "just do it" mode. It passes only if the hidden checks pass on
  the accepted result. Stuck, or planner checks so strict they never pass,
  counts as a fail. The results file also records the reviewer rounds and
  how many checks the planner wrote.
- **Order:** the Codex must-fail test is the plan's first real step. The
  rest of the reviewer is built against a fake reviewer meanwhile, so the
  result changes only the last wiring step.

Left to the plan: crate and folder layout, the exact event and message
names, the page's look, how types are generated, the fake reviewer for CI.

## 8. What the owner must do

- Sign in to Codex in Ubuntu (`codex login`) with his subscription, as he did
  for Claude. Cortex never handles the login.

## 9. Risks

- **Codex may not be restrictable.** Then the review design needs another
  family that can be, and the owner decides.
- **The planner may write checks that are too loose** (pass with half the
  work) or too strict (demand things not asked). The reviewer and the hidden
  bench checks are how we find out; the bench result decides whether the
  planner's brief needs work before the next slice.
- **Cost:** a mission now has at least two more sessions (planner, reviewer)
  before the work starts.
