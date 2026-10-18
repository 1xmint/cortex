# Cortex brain: architecture design

Status: draft for owner approval (revised after independent review) ·
Date: 2026-10-18

This is the design the ADRs and the first brain slice are built from. It
answers one question: what shape must the brain have so that one person can
describe a creation in plain words and get it designed, built, proven and
remembered, whether that creation is an app or a game with art and sound?

## 1. What the brain must do

Taken from [AGENTS.md](../../../AGENTS.md) and the owner's direction:

1. **Talk like Jarvis.** A conversation that is always open. The person can
   ask, steer or stop at any moment, including while work is running.
2. **Work in the background for hours** and come back the way the person's
   working agreement says.
3. **React, don't run a script.** Failed checks, new facts and the person's
   words change the plan while it runs.
4. **Build creations that mix media** as one coherent thing, not as separate
   departments.
5. **Never trust a claim of done.** Only Cortex's own checks, which can fail,
   decide that something works (invariants 1 and 2).
6. **Remember like a person**: what this mission is doing, what this project
   has decided, how this person likes to work, and which approaches have
   actually won.
7. **Survive crashes.** A restart resumes the mission where it stopped.
8. **Keep model-written work isolated** from the person's machine and keys
   (invariant 3).
9. **Run on API billing in the product, and on the owner's Claude Code and
   Codex subscriptions during development, with no API key.**

## 2. What the evidence says

The three research reports (2026-10-18) are mostly secondary sources: vendor
blogs and third-party summaries. Where a point below decides the design,
several sources agree on it. The ADRs re-check any specific claim they rely
on.

- **Every leading agent runs the same core loop**: the model reasons, calls a
  tool, reads the result, repeats. The differences are in what surrounds it.
- **Steering is two separate verbs.** Codex's app-server has `steer` (add a
  message to the running turn) and `interrupt` (stop the current step, keep
  the thread). The Claude Agent SDK works the same way.
- **Long runs fail in known ways.** Success falls sharply as tasks get longer
  (METR); quality drops when a task sits inside a long history; small per-step
  errors compound; agents declare victory early. What works: short worker
  sessions with fresh context, a written plan, a progress file, small commits,
  and a feature list that only independent checks can flip to passing
  (Anthropic's long-running harness).
- **An append-only event log works as the source of truth for an agent.**
  OpenHands' SDK does this in production with replay, pause, resume and fork.
- **Durable-execution engines don't remove the hard parts.** Temporal,
  Restate, Inngest and DBOS all record steps and replay. None makes a
  non-repeatable side effect safe on its own.
- **Memory benchmarks are unreliable.** LoCoMo has a wrong answer key in
  6.4% of cases; vendors dispute each other's scores. The real failures are
  stale facts that look as relevant as their replacements, values never
  marked as replaced, and poisoned memories written from untrusted content.
- **Skill memory helps when it only stores verified wins** (Voyager; Agent
  Workflow Memory), but skills are brittle outside their original setting.
- **MCP is the universal tool plug.** Its real risks are tool poisoning,
  servers that change after approval, and prompt injection through fetched
  content.
- **Judging media automatically works best as an itemised checklist**: a
  yes/no with evidence per atomic check, not one overall score. Any model
  judge must be validated against human labels first.
- **Codex moved its CLI from TypeScript to Rust** for a zero-dependency
  install, native sandbox control and a wire protocol other languages can
  drive. Claude Code stayed on TypeScript, so a TypeScript loop is viable.
- **Process sandboxes protect a developer's own machine; hostile code needs a
  VM-grade layer.** Hosted sandboxes (E2B, Daytona, Modal) are billed
  separately, which the owner's rules exclude.

## 3. The shape

```
 person ──chat / terminal / API──▶  Cortex core (one process)
                                     │
            ┌────────────────────────┼─────────────────────────┐
            ▼                        ▼                         ▼
     Conversation             Missions (each an           Memory
     (always open;            event log): planner,        (bible, agreement,
      steer / interrupt)      workers, checker            facts, skills)
                                     │
                     ┌───────────────┼────────────────┐
                     ▼               ▼                ▼
                 Engines         Tools (MCP)       Sandbox
             (models & coding   (media APIs,      (where all untrusted
              agents)            browser, git)     work runs)
```

### 3.1 Conversation and missions

The person always talks to one **conversation**. When a request needs real
work, the brain starts a **mission** and keeps talking. A person can have
several missions running. Every message the person sends during a mission is
recorded on it as a `steer` (take this into account at the next step) or an
`interrupt` (stop the current step and re-plan). The brain decides which and
says which it chose.

### 3.2 A mission is an event log

Each mission is an append-only log of events: the request, every plan
version, every worker session and its transcript, every check result, every
message from the person, every notification sent. The log is the truth.
Tables that speed up reading (current plan, open checks) are allowed only as
caches that can be rebuilt from the log at any time.

**The unit of recovery is the worker session.** A coding agent run headless
is a black box with many internal steps, so Cortex does not try to replay
inside it. Each session works in its own disposable copy of the project (a
git worktree) that starts from the last accepted commit. If the process dies
mid-session, the copy is thrown away and the session is run again; the log
shows the abandoned attempt. The session's full transcript (the engine's
streamed output) is stored as events, so "why did you do that?" always has
an answer.

Steps with effects outside the copy (merging a piece, publishing, a paid
media call, sending a notification) carry an idempotency key. After a crash
Cortex checks whether the effect already happened before trying again.

The scoreboard reads the same logs: time, cost and pass/fail come from real
runs, not from anyone's summary.

### 3.3 The loop: plan, work, check, repeat

Inside a mission three roles take turns, each recorded as events.

1. **Planner.** Turns the request into a **creation graph** (3.4) and an
   acceptance list of concrete checks. It re-plans when a check fails twice,
   when a worker reports something new, or when the person steers or
   interrupts. A plan is a version in the log, never edited in place.
2. **Workers.** Each takes one piece and one failing check, in a short
   session with fresh context. It reads the bible and the piece's notes,
   works only inside its copy of the project, commits small steps and writes
   a progress note. A worker never marks anything passing and cannot touch
   the checks.
3. **Checker.** Runs the acceptance checks itself, inside the sandbox
   (tests, builds, headless-browser play-throughs, itemised media checks).
   It is the only thing that can flip a check to passing (invariant 1).

What keeps the checks honest:
- **Checks live outside the workers' copy**, frozen and fingerprinted with
  the plan version. Changing a check is a new plan version the person can
  see.
- **A check must be seen failing** against the project before the work
  starts. A check that cannot fail is blocked, not counted (invariant 2).
- **Checks for a piece are reviewed by a different engine family** from the
  one doing the work (for example, Codex reviews checks for work Claude Code
  does).
- **A passed check records what it depended on** (the bible version and the
  pieces it read) and is re-run when those change.
- **Integration re-runs every check**, not just the last piece's.
- **Checks that can be flaky** (browser play-throughs, model judges) run more
  than once; flakiness is tracked, and a flaky check is fixed or replaced.

What keeps the loop from running away:
- A budget per mission (tries, time, money) and a cap on re-plans per
  mission, not only per check.
- A **parked** state: when a subscription rate limit, a budget or a slow
  media job forces a wait, the mission parks with a durable timer and
  resumes on its own.
- Waiting for the person blocks only the pieces that need their answer;
  other ready pieces keep going.
- One copy of the project per piece being worked on; finished pieces are
  merged one at a time.

The mission is done when every acceptance check passes on the merged
result. If it is stuck within its budget, it stops and asks, as the working
agreement allows.

### 3.4 Creations are graphs of pieces

A creation is a graph of **pieces**: game code, a sprite set, music, sound
effects, an animation, the build, the deploy. Edges say what each piece needs
from another. **Integration is its own piece** with its own checks (the game
loads, the sprites appear, the music plays on level start).

Every piece shares one **project bible**: the style, rules, names, palette,
tone and decisions for this creation. Workers read it; checks are written
against it. That is what keeps art, code and sound coherent without
departments.

Nothing in the loop knows what kind a piece is; the piece says which
engines, tools and checks it uses. Coding is the first kind Cortex proves
itself on.

### 3.5 Engines: who does the work

An **engine** is anything that can do a worker's job: a coding agent (Claude
Code, Codex) driven headless, or a model called through its API. One
interface, two families:

- **Subscription engines** (development): the `claude` and `codex` command
  lines in non-interactive mode, on the owner's own subscriptions, for the
  owner's own development testing.
- **API engines** (product): the providers' APIs with Cortex's own tool
  loop. Proven early (slice 1 includes a minimal one behind the same
  interface) so the product is never built around one CLI's behaviour.

**Engines use Cortex's tools, not their own shell.** Each engine is started
with its built-in shell and command tools turned off and with Cortex's tool
server attached. Cortex's `run` and `test` tools execute inside the sandbox;
file edits are allowed only inside the session's copy of the project. The
engine process holds its own login, so it must never run model-chosen
commands next to it. If an engine cannot be restricted this way, it is not
used (checked in the engine ADR).

The brain chooses the engine per piece; skill memory learns which engine
wins at which kind of piece. Engines' own reports of success are notes,
never results.

### 3.6 Memory: four kinds, two forms

| Kind | What it holds | Form |
|---|---|---|
| Working | This mission: its plan, notes, open checks | The mission's event log |
| Project | The bible and past decisions for one creation | A versioned document the project's members can read and edit |
| Personal | How this person likes to work, including the working agreement | A versioned document only that person can read and edit |
| Skill | Approaches that won: steps, preconditions, the runs that proved them | Records, admitted only after a verified pass |

Plus a store of **facts** learned along the way. Every fact has a source (the
person said it / a check showed it / inferred / read from outside content), a
confidence and a status. A newer fact never overwrites an older one; the old
one is marked replaced and linked. Search returns only current facts. A fact
about code records the commit it was true at and is re-checked when the code
changes.

**Who can see what.** Data is stored per project (with its members, so
teams work) and per person (for personal memory). No query crosses those
lines. Skills shared across people are learned only from scoreboard runs or
runs whose owner agreed, are stored as method without project content, and
always record where they came from.

**Against poisoning.** Facts that are inferred or come from outside content
are shown to models only as quoted data with their source, never as part of
their instructions. Only the person or a passing check can raise their
confidence. A model's change to the bible is a plan event that goes through
the working agreement.

The bible and the agreement are loaded whole, never found by similarity
search, because they are short and must not be missed. Skills track wins
and losses and retire when they stop winning. Memory is tested with Cortex's
own suite (updated facts, contradictions, "I don't know", no leaking between
people or projects), not vendor scores.

### 3.7 The working agreement

A person writes their agreement in plain words. Cortex translates each rule
into a small, fixed **rule language** and shows the translation back for the
person's yes:

- **When** (fixed hook points): plan made, design choice made, piece done,
  problem found, check passed, about to do something irreversible, mission
  done, mission stuck.
- **If** (fixed conditions): kind of problem, severity, cost so far, which
  piece, whether a fix's checks have passed.
- **Then** (fixed actions): show the reasoning, ask before continuing,
  notify by chat / email / phone, wait until a condition holds, then notify.

Examples: *"Explain every decision so I can learn"* → when a plan or design
choice is made, show the reasoning. *"Email me if you find a dangerous bug,
but only once the fix is tested and proven"* → when a security problem is
found, wait until its fix's checks pass, then email with the evidence.

Rules:
- The core evaluates the rules itself, the same way every time. No model
  reads the plain words when enforcing them.
- A wish that the rule language cannot express is refused with an
  explanation, never approximated.
- Some asks always win over the agreement: spending above budget,
  publishing, anything destructive or irreversible. Between the person's own
  rules, the more cautious one wins.
- Judgements inside conditions ("is this bug dangerous?") are made by a model
  and logged as judgements (3.12), so the person can see and correct them.
- Cortex may suggest changes it learned ("you approve every plan without
  edits; stop asking?") but never changes the agreement without a yes.
- Until the full agreement arrives (slice 3), a fixed minimal agreement is
  enforced at the same hook points: ask before anything irreversible, report
  when done or stuck.

### 3.8 Tools

Every outside capability is a tool behind MCP: image, music, voice, 3D and
video generation; the headless browser; git; deploy targets. The core is the
MCP host. Per-piece allow-lists; tool definitions pinned by fingerprint and
re-reviewed when they change; tool output always treated as data, never as
instructions; no tool output can change permissions. Media outputs are files
stored by content fingerprint in the project, with the provider and its
usage terms recorded, never pasted into a model's context. Each paid call
carries an idempotency key and counts against the mission's budget.

### 3.9 Sandbox

All untrusted work (model-written code, the project's tests, headless-browser
play-throughs, local tool servers) runs on a Linux side, never directly on
the person's machine:

1. **Layer 1, process sandbox (bubblewrap):** no access to keys, home folder
   or the network except through an allow-listed proxy.
2. **Layer 2, VM-grade isolation:** gVisor on the owner's PC; Firecracker
   microVMs on hosted Linux later.

On the owner's PC (Windows 10 Home) the Linux side is **WSL2**. Firecracker
needs nested virtualisation, which Windows 10 Home does not give WSL2, so
gVisor is layer 2 there. The sandbox is proven by checks that must fail:
sandboxed code tries to read a key file and reach the network, and both
attempts must be refused.

If the required layers cannot be set up, the step is blocked, not run
unsandboxed (invariant 3). Both layers are self-hosted; no billed sandbox
service.

### 3.10 Faces and the protocol

One protocol for every face: JSON-RPC 2.0, over standard input/output for the
terminal and over WebSocket for the web page, modelled on Codex's
app-server, including the core asking the face for approvals. Message types
are defined once in the core and generated for TypeScript; CI fails if the
generated types drift. The chat web page comes first; desktop and mobile
later use the same protocol.

### 3.11 Scoreboard

A fixed set of prompts, each with its checks written in advance, from "a
to-do app" to "a one-screen platformer with art and music". Each run records
time, cost, which checks passed, and a recording of the result. Results are
generated files in `bench/results/`, never written by hand. This is the
"how good is it" home in AGENTS.md, the source of skill memory, and the
measure every slice is judged by, starting with slice 1.

### 3.12 Judgements

The brain makes many small calls: was that message a steer or an interrupt,
is this bug dangerous, which engine should build this piece, does this
request need a mission at all. All of them go through one **judgement**
interface in the core: a fixed question, the allowed answers, the answer
chosen and a confidence, each logged as an event. The person can see and
correct any judgement, and corrections are logged too.

Judgements never replace the places where the design forbids guessing:
checks decide "done" (invariant 1), the working agreement's rules are
evaluated by the core, and the mandatory asks in 3.7 are fixed.

The first judge is the cheapest frontier model available through the same
engines. Logged judgements with the person's corrections become a labelled
set; a fast specialised model (for example a small local classifier such as
GLiNER2) replaces the model judge for a kind of question only if the
scoreboard shows it at least as accurate on that set. Hosted classifier
services that need their own key or bill separately (for example Jev) are
excluded by the owner's rules.

## 4. Technology per component (gold rule)

Scored on the AGENTS.md criteria. One ADR per row, each re-checking the
claims it relies on.

| Component | Choice | Why it wins | Main cost |
|---|---|---|---|
| Core (loop, missions, agreement, sandbox control, protocol server) | **Rust** | The core controls sandboxes and processes directly; the type system enforces the event model and the rule language; a single binary with nothing to install. The ADR must score TypeScript honestly: shared types with the web page and the more mature agent and MCP SDKs are real advantages | Slower to build (a tie-breaker only); thinner agent SDKs |
| Event log and memory store | **SQLite** (WAL, full-text search) inside the core | Transactional, nothing to operate, no bill; schema kept portable to Postgres for when hosting needs it | Vector search added later |
| Durable execution | **Own event-sourced log**, worker session as the unit of recovery, proven by kill-at-every-step tests | The mission already is an event log; an engine would add a server and workflow-code rules | We own leases, retries, timers and idempotency. Fallback if outgrown: Restate, then Temporal |
| Tools | **MCP**, with the core as host | The standard plug; providers ship MCP servers | The security rules in 3.8 are mandatory |
| Sandbox | **WSL2 + bubblewrap + gVisor** on the owner's PC; Firecracker on hosted Linux | Strongest isolation available on this PC without a billed service | Requires WSL2 (decision 1) |
| Web chat (first face) | **TypeScript + React**, types generated from the core | Best ecosystem for the web | Second language |
| Desktop / mobile (later) | **Tauri** for desktop; mobile decided when it starts | Reuses the Rust core and web UI | Tauri mobile maturity unknown |
| Media tools | **Each provider in its own process**, Python where its SDK is best | Media libraries live in Python; the process boundary keeps it out of the core | Third language, kept behind MCP |

Languages are held to Rust and TypeScript, with Python allowed only inside
tool processes. Every boundary is a process boundary with a schema.

The ADRs for the core language, storage and event log, sandbox, and engines
are accepted before slice 1 code starts.

## 5. Order of building

Every slice is judged by the scoreboard, starting with the first. Each ends
with something the owner can run and see.

**Slice 1: one small coding task, proven, end to end.**
- A bench of 5 to 10 small coding prompts, each with checks written in
  advance (this is the start of the scoreboard). Done means: at least 7 of
  10 pass Cortex's own checker on the owner's PC, with the results files
  generated by the run.
- Built in this order, each step judged against the same bench:
  1. The sandbox (WSL2, bubblewrap, gVisor) with its must-fail checks.
  2. The simplest loop that can pass the bench: mission log, one piece, plan
     → one worker session (Claude Code headless, shell off, Cortex tools on)
     → checker. Plus a minimal API engine behind the same interface.
  3. Kill-at-every-step tests and resume.
  4. `steer` and `interrupt`, as explicit terminal commands (`cortex run`,
     `cortex steer`, `cortex stop`, `cortex resume`).
- Fixed defaults stand in for budgets and the minimal agreement (3.7).
- No creation graph yet (one piece), no protocol server yet (an in-process
  library plus the terminal command).
- GitHub CI proves the loop with a scripted fake engine (no subscription
  can log in on GitHub's machines); the real-engine bench runs on the
  owner's PC and its generated results are committed.

**Slice 2:** the web chat page on the JSON-RPC protocol, showing missions
live; the brain decides steer vs interrupt from plain messages.
**Slice 3:** memory and the full working agreement, including notifications.
**Slice 4:** the headless-browser checker and planner-written checks
reviewed by a second engine family.
**Slice 5:** creation graphs with media pieces, model judges validated
against labelled examples, and bench prompts with art and sound.

## 6. Decisions that are the owner's

1. **Turn on WSL2 on this PC.** All model-written code runs on a Linux side,
   and on Windows that is WSL2, a Windows feature you switch on once.
   Without it slice 1 cannot run anything, because unisolated runs are never
   the fallback. Recommendation: yes.
2. **Who Cortex serves first.** Recommendation: you, on your own machine,
   with data kept per project and per person from day one, so hosting for
   others and teams later is a move, not a rewrite.

Email notifications (slice 3) will need a way to send mail, which is a
credential decision for then.

## 7. Open risks

- The research is mostly secondary; the ADRs re-check the claims they rely
  on (Codex app-server protocol, current MCP spec, gVisor under WSL2,
  whether each CLI's shell can be turned off).
- Hours-long autonomy is unreliable for every model today. The design leans
  on short sessions and independent checks; the scoreboard shows how far
  that gets.
- Driving the subscription CLIs headless must stay within each product's
  terms of use; it is for the owner's own development testing only, and the
  API engine is kept working so the product never depends on it.
