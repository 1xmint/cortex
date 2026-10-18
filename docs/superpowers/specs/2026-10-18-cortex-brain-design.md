# Cortex brain: architecture design

Status: draft for owner approval · Date: 2026-10-18

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
blogs and third-party summaries. Where a point below decides the design, it
is one that several sources agree on. Points from a single source are marked.

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
  OpenHands' SDK does this in production with replay, pause, resume and fork
  (MLSys 2026).
- **Durable-execution engines don't remove the hard parts.** Temporal,
  Restate, Inngest and DBOS all record steps and replay. None makes a
  non-repeatable side effect (a push, a payment, a half-written file) safe on
  its own. Each needs idempotency keys regardless.
- **Memory benchmarks are unreliable.** LoCoMo has a wrong answer key in
  6.4% of cases; vendors dispute each other's scores; plain files with search
  beat a popular graph product in one test. The real failures are stale facts
  that look as relevant as their replacements, values never marked as
  replaced, and poisoned memories written from untrusted content.
- **Skill memory helps when it only stores verified wins** (Voyager; Agent
  Workflow Memory), but skills are brittle outside their original setting.
- **MCP is the universal tool plug**, and its real risks are tool poisoning,
  servers that change after approval, and prompt injection through fetched
  content.
- **Judging media automatically works best as an itemised checklist**: break
  the bible into atomic checks and get a yes/no with evidence for each, not
  one overall score. Any model judge must be validated against human labels
  first.
- **Codex moved its CLI from TypeScript to Rust** for a zero-dependency
  install, native sandbox bindings, no garbage-collection pauses over long
  sessions, and a wire protocol other languages can drive. Claude Code stayed
  on TypeScript, so a TypeScript loop is viable. The rewrite's long-term
  outcome is not documented.
- **Process sandboxes (Seatbelt, bubblewrap) protect a developer's own
  machine; hostile code needs a microVM or gVisor.** Hosted sandboxes (E2B,
  Daytona, Modal) are billed separately, which the owner's rules exclude.

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
             (models & coding   (media APIs,      (where untrusted
              agents)            browser, git)     work runs)
```

### 3.1 Conversation and missions

The person always talks to one **conversation**. When a request needs real
work, the brain starts a **mission** and keeps talking. A person can have
several missions running. Every message the person sends during a mission is
recorded on it as a `steer` (take this into account) or an `interrupt` (stop
the current step and re-plan). The brain decides which, and says which it
chose.

### 3.2 A mission is an event log

Each mission is an append-only log of events: the request, every plan
version, every model response, every tool call and result, every check
result, every message from the person, every notification sent. The current
state of a mission is computed from its log, never stored separately.

Consequences:
- **Crash recovery is replay.** Recorded model responses and tool results are
  reused, never re-requested. A step that was running when the crash hit is
  retried only if it is marked repeatable; otherwise it is checked first (did
  the commit land?) using its idempotency key.
- **"Why did you do that?" always has an answer**, read straight from the
  log. This is what lets a learner see every decision.
- **The scoreboard reads the same logs**: time, cost and pass/fail come from
  real runs, not from anyone's summary.

### 3.3 The loop: plan, work, check, repeat

Inside a mission three roles take turns. They are roles of the brain, not
separate products, and each is just events in the log.

1. **Planner.** Turns the request into a **creation graph** (3.4) and an
   acceptance list: concrete checks, each starting as failing. It re-plans
   when a check fails twice, when a worker reports something new, or when the
   person steers or interrupts. A plan is a version in the log, never edited
   in place.
2. **Workers.** Each takes one piece and one failing check, in a short
   session with fresh context. It reads the bible and the piece's notes, works
   in the sandbox, makes small commits and writes a progress note. A worker
   never marks anything passing.
3. **Checker.** Runs the acceptance checks itself (tests, builds, headless
   browser play-throughs, itemised media checks) and is the only thing that
   can flip a check to passing (invariant 1). A check that has never been
   seen to fail is flagged (invariant 2).

The mission is done when every acceptance check passes and the integration
piece passes. If it gets stuck (the same check failing after re-planning
within its budget), it stops and asks, as the working agreement allows.

### 3.4 Creations are graphs of pieces

A creation is a graph of **pieces**: game code, a sprite set, music, sound
effects, an animation, the build, the deploy. Edges say what each piece needs
from another. **Integration is its own piece** with its own checks (the game
loads, the sprites appear, the music plays on level start).

Every piece shares one **project bible**: the style, rules, names, palette,
tone and decisions for this creation. Workers read it; checks are written
against it. That is what keeps art, code and sound coherent without
departments.

Coding is the first kind of piece Cortex proves itself on. Nothing in the
loop knows what kind a piece is; the piece says which engines, tools and
checks it uses.

### 3.5 Engines: who does the work

An **engine** is anything that can do a worker's job: a coding agent (Claude
Code, Codex) driven headless, or a model called through its API. One
interface, two families of implementations:

- **Subscription engines** (development): drive the `claude` and `codex`
  command-line tools in non-interactive mode. This is how the owner tests
  with no API key.
- **API engines** (product): call the providers' APIs directly with Cortex's
  own tool loop.

The brain chooses the engine per piece, and skill memory (3.6) learns which
engine wins at which kind of piece. Engines' own reports of success are
notes, never results.

### 3.6 Memory: four kinds, two forms

| Kind | What it holds | Form |
|---|---|---|
| Working | This mission: its plan, notes, open checks | The mission's event log |
| Project | The bible and past decisions for one creation | A versioned document the person can read and edit |
| Personal | How this person likes to work, including the working agreement | A versioned document the person can read and edit |
| Skill | Approaches that won: steps, preconditions, the runs that proved them | Records, admitted only after a verified pass |

Plus a store of **facts** for everything learned along the way. Every fact
has a source (the person said it / a check showed it / inferred), a
confidence, and a status. A newer fact never overwrites an older one; the old
one is marked replaced and linked. Search returns only current facts.

Rules that come from the evidence:
- The bible and the agreement are loaded whole, never found by similarity
  search, because they are short and must not be missed.
- Nothing is written to memory from web pages, tool output or files without
  a source label; anything from untrusted content stays low confidence until
  the person or a check confirms it. This is the main defence against
  memory poisoning.
- Skills track wins and losses and are retired when they stop winning.
- Memory is tested with Cortex's own suite (updated facts, contradictions,
  "I don't know", no leaking between people or projects), not vendor scores.

### 3.7 The working agreement

Each person's agreement is a set of rules in plain words, kept in personal
memory, which the brain turns into structured rules it can act on and shows
back for confirmation. Every rule has a trigger, a condition and an action:

- *"Explain every decision so I can learn"* → on every plan and design
  choice, show the reasoning.
- *"Show me the code before it merges"* → before a merge, ask.
- *"Just do it"* → ask only when the invariants require it.
- *"Email me if you find a dangerous bug, but only once the fix is tested
  and proven"* → when a check finds a security problem, wait until the fix's
  checks pass, then email with the evidence.

The agreement is checked at fixed points in the loop (plan made, piece done,
problem found, check passed, about to do something irreversible, mission
done). Starting presets exist only as first drafts. Cortex may suggest
changes it learned ("you approve every plan without edits; want me to stop
asking?") but never changes the agreement without the person's yes. Some
things are asked whatever the agreement says: spending above the person's
budget, publishing, and anything destructive or irreversible.

### 3.8 Tools

Every outside capability is a tool behind MCP: image, music, voice, 3D and
video generation; the headless browser; git; deploy targets. The core is the
MCP host. Per-piece allow-lists, tool definitions pinned by hash and
re-reviewed when they change, tool output always treated as data and never
as instructions, and no tool output can change permissions. Media outputs
are files stored by content hash in the project, never pasted into a model's
context. Each paid call carries an idempotency key and counts against the
mission's budget.

### 3.9 Sandbox

Two layers:
1. **Process sandbox** (the kind Codex uses) around every tool and engine
   run on the person's machine: no access to keys, home folder or the
   network except through an allow-listed proxy.
2. **VM sandbox** (Firecracker microVM, or gVisor where KVM is missing) for
   model-written code and for headless-browser play-throughs of it.

If the required layer cannot be set up, the step is blocked, not run
unsandboxed (invariant 3). Both layers are self-hosted; no separately billed
sandbox service.

### 3.10 Faces and the protocol

One protocol for every face: JSON-RPC 2.0, over standard input/output for the
terminal and over WebSocket for the web page, modelled on Codex's
app-server, including the core asking the face for approvals. Its message
types are defined once in the core and generated for the other languages; CI
fails if the generated types drift. The chat web page comes first; desktop
and mobile later use the same protocol.

### 3.11 Scoreboard

A fixed set of prompts, from "a to-do app" to "a one-screen platformer with
art and music", run on a schedule. Each run records time, cost, which checks
passed, and a recording of the result. Results are generated files in
`bench/results/`, never written by hand. This is the "how good is it" home in
AGENTS.md and the source of skill memory.

## 4. Technology per component (gold rule)

Scored on the AGENTS.md criteria. One proposed ADR per row.

| Component | Choice | Why it wins | Main cost |
|---|---|---|---|
| Core (loop, missions, agreement, protocol server) | **Rust** | Single binary with no runtime to install; no garbage-collection pauses or heap growth over hours-long runs; the type system enforces the event model; native control of sandboxes; Codex chose the same for the same reasons | Slower to build than TypeScript (only a tie-breaker under the gold rule) |
| Event log and memory store | **SQLite** (WAL, full-text search) inside the core | Transactional, one file per person, nothing to operate, no bill; schema kept portable to Postgres for when hosting needs it | Vector search added later; its maturity on Windows to be checked then |
| Durable execution | **Own event-sourced log** in the core, proven by kill-at-every-step tests | The mission already is an event log; an engine would force the loop into workflow-code rules and add a server; no extra language | We own leases, retries, timers and idempotency. Fallback if outgrown: Restate, then Temporal |
| Tools | **MCP**, with the core as host | The standard plug; providers already ship MCP servers | Security work above is mandatory |
| Sandbox | **Process sandbox + self-hosted microVM/gVisor**, controlled from the core | Strongest isolation without a billed service | Needs Linux with KVM: WSL2 on Windows (question 1) |
| Web chat (first face) | **TypeScript + React**, types generated from the core | Best ecosystem for the web; no language choice is better there | Second language |
| Desktop / mobile (later) | **Tauri** for desktop, reusing the web UI and core; mobile decided when it starts | Reuses Rust core and web UI | Tauri mobile maturity unknown |
| Media tools | **Each provider in its own process**, Python where its SDK is best | Media libraries live in Python; the process boundary keeps it out of the core | Third language, kept behind MCP |

Languages are held to Rust and TypeScript, with Python allowed only inside
tool processes. Every boundary is a process boundary with a schema, never
linking one language into another.

## 5. Order of building

Each slice ends with something the owner can run and see.

1. **Mission core, in the terminal.** A mission event log, the plan → work →
   check loop on a small coding request, a subscription engine (Claude Code
   headless), the checker running the project's own tests, `steer` and
   `interrupt`, and resume after the process is killed. Proven by tests that
   kill the process at every step and by one end-to-end run.
2. **Web chat page** on the same protocol, showing missions live.
3. **Memory and the working agreement**, with the notify rules.
4. **Sandbox layer 2** and the headless-browser checker.
5. **Creation graphs with media pieces** and the scoreboard.

## 6. Decisions that are the owner's

1. **Sandbox on this PC.** Model-written code needs a Linux VM layer, which
   on Windows means turning on WSL2 (a Windows feature the owner switches on
   once). Recommendation: yes, WSL2. Until then, only the process sandbox
   runs, and steps that need the VM layer are blocked.
2. **Who Cortex serves first.** Recommendation: one person on their own
   machine (the owner), with each person's data kept in its own files so
   hosting for others later is a move, not a rewrite.

## 7. Open risks

- The research is mostly secondary; the ADRs re-check the specific claims
  they rely on (Codex app-server protocol, MCP's current spec, Firecracker
  under WSL2).
- Hours-long autonomy is unreliable for every model today. The design leans
  on short sessions and independent checks; the scoreboard will show how far
  that gets.
- Driving the subscription CLIs headless must stay within each product's
  terms of use; this is for the owner's own development testing only.
