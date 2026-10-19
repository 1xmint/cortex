# Cortex brain: architecture design, version 2

Status: draft for the owner's approval
Date: 2026-10-18
Replaces: [version 1](2026-10-18-cortex-brain-design.md) (approved the same
day). Version 1 stays in the repository for history. Once this version is
approved, version 1 is marked superseded and the open slice 2 design
(PR 26) is closed in favour of the slice order in section 6.

## 0. Why a second version

The owner asked for a skeptical second look at whether the brain will hold
up over years of model changes and professional development (2026-10-18).
Three new research passes looked at three things:

- how long-running agents plan;
- whether one agent or a team of agents works better, and how to choose
  models per job;
- which agent architectures survive model change.

The reports are graded: **P** means a primary source (a vendor's
engineering post, a paper, docs). **S** means a secondary summary.
**U** means unverified.

The short answer: **the core shape holds**. That shape is an event log as
the truth, a harness that can restart from the log, a sandbox for all
untrusted work, and checks that must be able to fail. Anthropic's
Managed Agents (P, Apr 2026) and OpenHands' V1 SDK (P, MLSys 2026) arrived
at the same shape independently.

Six things change:

1. **The plan's centre is a spec plus checks, planned progressively.** It is
   not a full graph built up front (section 3.3).
2. **Evals come first.** Today's bench is too easy to tell one brain design
   from another (sections 3.11 and 6).
3. **Models sit behind Cortex's own provider-neutral layer.** Roles pick
   models by setting, and every call records tokens and cost (section 3.5).
4. **Tools sit behind Cortex's own tool interface.** MCP becomes one
   adapter, not the core's plumbing (section 3.8).
5. **The work runs as one pipeline with one writer at a time.** It is not a
   team of agents with personas. Parallel work is allowed only where pieces
   are truly independent (sections 3.3 and 3.4).
6. **Everything the harness does to work around a model is labelled and
   re-tested** at each model change, so the harness gets thinner as models
   improve (section 3.13).

Section 7 lists every part of version 1 as kept, changed or dropped.

## 1. What the brain must do

Unchanged from version 1. In short:

1. Talk like Jarvis, steerable at any time.
2. Work in the background for hours.
3. React rather than run a script.
4. Build creations that mix media as one coherent thing.
5. Never trust a claim of done.
6. Remember like a person.
7. Survive crashes.
8. Keep untrusted work isolated.

One change to the ninth requirement. The product runs on whichever
providers' APIs give the best result for the money, and later on open
models on the owner's own servers. The Claude Code and Codex command lines
are only how the builder tests during development, on the owner's
subscriptions, with no API key.

## 2. What the evidence says (new since version 1)

**Planning**
- **Long-running systems share one shape.** Anthropic's long-running
  harness, Codex ExecPlans, Kiro, Spec Kit and Manus each keep three files
  on disk (all P):
  - a spec: goals, non-goals, constraints, and what "done" means;
  - a milestone or feature list, with a way to check each item;
  - a status and decision log.
  Each runs short fresh sessions that read those files, and checks gate
  every step. Only Cursor has a separate planner role. It explores
  continuously and starts each cycle fresh (P).
- **The only clean plan-versus-no-plan test** is arXiv 2609.20804 (P,
  2026). Planning took a 30B model from 13.6% to 25.2%. For strong models
  it moved success by about 2 points either way and cut cost by about 30%.
  No study shows that deep, layered plans beat a simple list.
- **Plans should be cut as needed and revised from what actually happens.**
  ADaPT breaks a task down only when the worker fails (P). Plan-and-Act
  gained about 10 points by re-planning after each step (P, web tasks, not
  coding).
- **Checks, not plans, set the ceiling on long work.** In SpecBench (P),
  agents pass the tests they can see and fail hidden tests of how features
  work together. The gap grows by about 27 points for each tenfold growth
  in code size. On long multi-step tasks the best models score only about
  25–39% (SWE-EVO, RoadmapBench, P).
- **Research before building has thin evidence.** Fetching docs helps with
  rare or new APIs and hurts with common ones (Jain et al., P). No study
  shows web research before building improves results.

**One agent or many**
- **Several agents on one sequential job usually do worse.** Cognition says
  parallel agents make conflicting hidden decisions (P, opinion with
  examples). Anthropic says multi-agent suits broad parallel research, not
  "most coding tasks", at about 15 times the tokens (P). A Google/MIT
  scaling study reports 39–70% losses for multi-agent setups on sequential
  tasks (S, abstract only, not verified). The MAST study of 1,600 traces
  found small gains over one agent and named verification failures as a
  main cause (P, 2024-era models).
- **Model routing:** the evidence supports trying a cheaper model first,
  letting checks decide, and escalating when they fail (FrugalGPT, P,
  2023). Use a strong model where few tokens carry a lot of weight:
  planning and review. Anthropic's advisor pattern, where a cheaper model
  asks a stronger one for a short plan, scored 2.7 points higher at 11.9%
  less cost (P, vendor data). We found no independent evidence for
  commercial router services.
- **Review by another model family** reduces shared blind spots but does
  not remove them. Models agree on the same wrong answer 60% of the time,
  most of all within one family, and strong models agree even across
  families (P). No study measures this for code review. Checks that run
  are the stronger defence.

**What survives model change**
- **Harness workarounds go stale.** Anthropic added context resets for one
  model that became "dead weight" on the next (P). A 100-line, bash-only
  agent reaches 74%+ on SWE-bench Verified because the models improved (P).
  What lasted in other projects:
  - sandboxing;
  - a durable log;
  - outcome checks;
  - a small general tool set;
  - control over what goes into the model's context;
  - cost accounting.
  What got thrown away:
  - rigid router-to-specialist pipelines;
  - role personas;
  - prompt chains;
  - model-specific hacks.
- **The harness still decides cost.** Command-line-only setups were 5–28
  times cheaper than MCP-capable ones, and one model's cost varied 139
  times across harnesses (arXiv 2608.08654, P).
- **Evals:** Anthropic's guidance is to start with 20–50 tasks drawn from
  real failures, as follows (P, Jan 2026):
  - test each grader against a reference solution;
  - measure reliability as "passes every one of k tries" (pass^k);
  - read the transcripts.
  OpenAI stopped reporting SWE-bench Verified because of contamination and
  flawed tasks (P, via summary). Any score you have tuned against stops
  being trustworthy.
- **Standards:**
  - AGENTS.md and Agent Skills (plain files) are likely to last.
  - MCP is lasting as a concept but changes its wire format about yearly.
    The 2026-07-28 revision removed sessions (S).
  - Codex split its own client protocol away from MCP (S).
  - The OpenTelemetry conventions for AI are still in "development" status
    (S).

## 3. The shape

```
 person ──chat / terminal / API──▶  Cortex core (one process)
                                     │
            ┌────────────────────────┼─────────────────────────┐
            ▼                        ▼                         ▼
     Conversation             Missions (each an           Memory
     (always open;            event log): planner,        (bible, agreement,
      steer / interrupt)      worker, checker, judge      facts, skills)
                                     │
                     ┌───────────────┼────────────────┐
                     ▼               ▼                ▼
               Model layer       Tool layer         Sandbox
           (roles → profiles;  (Cortex's own      (all untrusted
            any provider)       interface; MCP     work)
                                one adapter)
```

### 3.1 Conversation and missions

Unchanged from version 1. A message during a mission is either a `steer`
or an `interrupt`.

### 3.2 A mission is an event log

Version 1's rules stay:
- the log is append-only;
- tables built from it are caches;
- the worker session is the unit of recovery;
- effects outside the worktree carry idempotency keys.

Additions, so that the log can still be read and replayed years from now:

- **Every event carries a schema version.** Old events are never rewritten.
  When the format changes, an upgrade step converts old events as they are
  read.
- **Every model call is recorded in full:**
  - the provider's request and response, verbatim;
  - the model ID and its parameters;
  - input and output tokens;
  - cost where the provider reports it;
  - how long it took.
  Today the log records the engine's name and version, but no tokens or
  cost.
- **Every event names the Cortex build that wrote it** (git commit) and the
  prompt version.
- **Replay mode:** a recorded mission's tool results can be replayed into a
  newer harness, so two harness versions can be compared on old missions
  without spending model calls on the parts that did not change.
- **OpenTelemetry export** is a view built from the log, never the truth,
  because its conventions still change.

The harness holds no state of its own. `wake(mission)` rebuilds everything
from the log. This is the pattern in Managed Agents (P), and it already
holds in slice 1's `cortex resume`.

### 3.3 The loop: spec, milestone, work, check, judge

**Roles are defined by what they may read and write, not by persona
prompts.** Each role has fixed inputs, outputs and permissions:

| Role | Reads | Writes | Never |
|---|---|---|---|
| Planner | request, project, bible, log, research notes | spec, milestone list, visible checks, held-out checks | the solution, the worktree |
| Worker | spec, current milestone, visible checks, decision record, file pointers | its worktree only | any check, the held-out checks, pass flags |
| Checker (code, not a model) | frozen checks, committed work | pass/fail events | anything else |
| Judge | spec, milestone results, log | continue / re-plan / stop, with reasons | code or checks |
| Reviewer | request, spec, checks (as quoted data) | approve / revise, with reasons | anything else |

The mission runs like this:

1. **Spec.** The planner writes a short spec from the request:
   - goals and non-goals;
   - constraints;
   - what "done" means, as checks;
   - a plain sentence for each check.
   This is the one place a person approves in "build it with me" mode. In
   "just do it" mode it runs straight on. Every vendor studied puts its
   human gate here, and none gate each task.
2. **Two layers of checks, written before any work.**
   - **Visible checks:** what workers build against.
   - **Held-out checks:** tests of how the features work together, error
     cases and whole-project behaviour. Workers never see them. They exist
     because of SpecBench: agents saturate the tests they can see.
   - Both layers are frozen, fingerprinted, and must be seen failing
     before work starts (invariant 2).
3. **Milestones, planned progressively.** The planner lists milestones in
   one line each and details only the next one. After each milestone it
   re-plans from what the code and checks actually show. **Depth adapts:**
   - a milestone is split further only when a worker fails or stalls on
     it (ADaPT);
   - with a strong worker model most milestones are never split;
   - the depth used and its outcome are logged, so skill memory learns the
     right depth per kind of task and model.
4. **Work.** One worker session at a time per project, with fresh context.
   It gets the decision record and file pointers, not a prose summary. It
   commits small steps.
5. **Check.** The checker runs both layers in the sandbox. Only it flips a
   check to passing (invariant 1). **The harness owns pass and fail**, not
   the model. Anthropic found agents declare victory early when they can
   mark their own work.
6. **Judge, at each milestone boundary,** in a fresh context: continue,
   re-plan, or stop and ask. It has a budget cap and stop rules, because
   Anthropic documents runaway search and agent spawning.

The mission is done when every visible and held-out check passes on the
merged result.

**Review by a second model family** is kept as a second layer on the spec
and checks, not as the primary gate. Executable checks are the primary
gate. Whether cross-family review is worth its cost is measured on the
bench, by planting known flaws in checks and comparing catch rates by
family. It is not assumed.

**Research is triggered, not a fixed stage.** The planner (or a worker,
through it) may run a read-only research step when any of these holds:
- a named API or library is unfamiliar or newer than the model;
- a check cannot be written without domain knowledge;
- a worker hits the same failure twice.

Findings go into the spec as a short note that cites its sources. Research
becomes a default only if the bench shows it helps.

Version 1's rules for honest checks and against runaway loops all stay:
- checks live outside the worktree;
- a changed check is a new plan version;
- a passed check records what it depended on;
- integration re-runs every check;
- flaky checks are tracked;
- each mission has a budget and a re-plan cap;
- missions can be parked.

### 3.4 Creations are pieces; one writer at a time

The creation graph and the project bible stay as in version 1. They are now
held as **data the planner rewrites** (milestones and pieces in the log).
The roadmap and intent graphs the person sees are **views generated from
the log**, never a second source of truth to keep in sync.

Parallel work is allowed only where it cannot conflict:
- pieces in different files or different media (a sprite set while the
  code is built);
- read-only work: research, review, checks.

Two workers never write to the same piece at once. Cognition and Cursor
both saw parallel writers make conflicting decisions. Finished pieces merge
one at a time, and integration is its own piece with its own checks.

### 3.5 Model layer: roles, profiles and cost

- **Internal message type.** Cortex has its own message and content type,
  which covers text, tool calls, thinking blocks and cache hints, with one
  adapter per provider:
  - the Anthropic API;
  - an OpenAI-compatible adapter, which also covers local open-model
    servers such as vLLM and llama.cpp.
  The OpenAI format is an adapter, not the internal model, because
  vendor-specific features leak through it.
- **Model profiles are configuration:**
  - endpoint;
  - model ID;
  - price per token;
  - context limit;
  - capability tags;
  - known tool-call quirks.
  Adding a model is a profile, not code. A switch from a local model to a
  paid one never happens silently; it needs a policy setting.
- **Each role picks a profile in one setting.** Defaults follow the
  evidence:
  - the strongest model for planning, judging and review, where tokens are
    few and decisions weigh most;
  - the cheapest model that passes the checks for the worker;
  - when a worker fails twice on a milestone, a **cascade** escalates to a
    stronger profile;
  - a worker may make an **advisor call**: a cheap worker asks the planner
    profile for a short plan mid-task. This is built provider-neutrally,
    not on Anthropic's own advisor tool.
- **Value is measured per role:**
  - first-try pass rate;
  - retries;
  - tokens and cost;
  - defects the held-out checks found later.
  "Best bang for the buck" is chosen from these numbers on the bench, and
  re-measured when a model or price changes.
- **The command-line engines stay development-only.** Claude Code and Codex
  run behind the same engine interface on the owner's subscriptions, so
  the bench can run without API billing. Their behaviour must never shape
  the event schema or the brain's logic.

### 3.6 Memory

Unchanged from version 1: four kinds (working, project, personal, skill)
plus facts with source and status. Two additions:
- the bible and verified skills are kept as plain files in open formats
  (AGENTS.md style, SKILL.md), so they outlive any vendor;
- **a skill carries the eval that proved it** and is re-checked when the
  model changes. A stale skill is the memory version of a stale workaround.

### 3.7 The working agreement

Unchanged from version 1.

### 3.8 Tool layer

Cortex defines its own **tool interface**: a name, an input schema, and a
call that runs in the sandbox and returns data.

- **The worker's base set is small and general:** read, write, edit, list
  and `run`. Small general tool sets are what survived in other harnesses,
  and they are much cheaper.
- **MCP is one adapter behind that interface.** It is used for outside
  services such as media generation and deploy targets. The core does not
  depend on MCP's wire format, which changes about yearly.
- The face protocol (3.10) is separate from both MCP and the model API, as
  Codex learned.
- Version 1's tool security rules all stay:
  - per-piece allow-lists;
  - definitions pinned by fingerprint;
  - output treated as data;
  - idempotency keys on paid calls;
  - budget counting.

### 3.9 Sandbox

Unchanged: bubblewrap plus gVisor on Linux (WSL2 on the owner's PC),
Firecracker on hosted Linux later. **Known limit:** the sandbox is
Linux-only. The product will be hosted on Linux and the owner's PC uses
WSL2, so this is accepted. The sandbox stays behind one `execute`
interface, so a different implementation can replace it.

### 3.10 Faces and the protocol

Unchanged: Cortex's own JSON-RPC protocol, types generated for TypeScript,
the chat page first.

### 3.11 The bench (scoreboard), rebuilt first

Today's ten prompts all passed in one session each. They cannot tell one
planner from another. The new bench has:

- **Tiers:** the existing small tasks (regression, expected near 100%), and
  **multi-feature tasks** (a small app with 5–10 features that must work
  together). The top tier is one that current best models pass under about
  half the time.
- **A hidden holdout set**, never used while tuning prompts or skills. This
  is separate from the planner's held-out checks: the holdout measures
  Cortex, and the planner's held-out checks measure the workers.
- **Graders proven both ways.** A reference solution must pass. Known-bad
  solutions must fail, including one that only games the visible checks.
- **Reliability and value:** pass^k over repeated tries, cost and tokens
  per success, and wall time.
- **Plain-request mode:** Cortex gets only the request and writes its own
  spec and checks. The bench's own checks then judge the result.
- **New tasks every release**, drawn from real failures. Transcripts are
  read, not only scores.

Every architecture choice in this document that is marked "measured, not
assumed" is decided by this bench:
- planning depth;
- research on or off;
- cross-family review;
- which model fills which role.

### 3.12 Judgements

Unchanged from version 1.

### 3.13 Workarounds are labelled and re-tested

Anything the harness does because of a particular model's weakness is
written as a named flag. Examples: forcing a plan step, splitting a
milestone smaller, resetting context. Each flag records:
- the model or models it was added for;
- the date;
- why it was added;
- the bench result that justified it.

When a role's model changes, every flag is re-tested on and off, and flags
that no longer help are removed. Without this the harness turns into the
"dead weight" Anthropic describes.

## 4. Technology per component

Version 1's table stands (Rust core, SQLite log, own durable execution,
WSL2 + bubblewrap + gVisor, TypeScript + React for the page, Tauri later,
Python only inside tool processes), with two changes:

| Component | Version 1 | Version 2 |
|---|---|---|
| Tools | MCP, core as host | Cortex's own tool interface; MCP is an adapter |
| Models | Engines (CLIs and an Anthropic-only API engine) | Provider-neutral model layer with profiles; CLIs dev-only |

The research found no evidence that the language decides agent quality.
Codex (TypeScript to Rust) and Goose (Python to Rust) both moved to Rust
for memory use in long sessions and to embed in other programs (S).
Because the event schema and protocol do not depend on any language,
staying with Rust is low-regret.

## 5. Answers to the owner's own ideas

- **"A planner that makes the plan: roadmap graphs, intent graphs, layers,
  systems, processes."** Yes, the person sees all of these. They are drawn
  from the log, and the truth underneath is the spec and its checks. The
  evidence shows that what helps is the content (intent, constraints,
  checks). It shows nothing for the graph format itself. Keeping one truth
  is what stops the drift that sank version 1 of Cortex.
- **"Progressive planning: the right items at the right moments, building
  on top strategically."** This is exactly what the evidence supports, and
  it is now the design. The planner details only the next milestone,
  re-plans from what really happened, and splits work smaller only where
  it proves hard.
- **"Incredible research about exactly the right things."** Research runs
  when there is a reason: a new API, an unknown domain, or a repeated
  failure. The bench decides whether it should run more often. Research by
  default made common coding tasks worse in the one study that measured
  it.
- **"Performs like a whole coordinated team."** From the outside, yes:
  planning, building, checking, reviewing, researching and, later, art and
  sound. On the inside it is one pipeline with one writer at a time,
  because teams of agents editing one thing do measurably worse. A role is
  added when the bench shows the brain failing without it.

## 6. Order of building (replaces version 1's slices 2–5)

Each slice ends with something the owner can see. Real runs spend the
owner's quota only where the result decides what gets built next.

**Slice 2: a bench that can tell brains apart, and a log that counts
cost.**
- In CI only (no quota):
  - the new bench tiers with reference and known-bad solutions;
  - graders proven both ways;
  - schema-versioned events;
  - full model-call records with tokens and cost;
  - the provider-neutral model layer with profiles;
  - an OpenAI-compatible adapter tested against a local stand-in server.
- One real run (quota): today's harness on the new bench through Claude
  Code. This decides whether the bench discriminates. If the new tiers all
  pass, the tasks are made harder before slice 3.
- **What the owner sees:** a results page listing each task, pass^k, time
  and cost.

**Slice 3: the planner.**
- Spec, visible and held-out checks, progressive milestones, adaptive
  depth and the milestone judge, built test-first against fakes in CI.
- Triggered research and the cross-family reviewer, both behind switches.
- Real runs (quota): the plain-request bench with the planner, compared
  with today's harness on the same tasks. Then the switches for research
  and review are tested on and off. **Done** when the planner beats the
  baseline on the multi-feature tier at an acceptable cost.
- **What the owner sees:** the same results page, with "with planner"
  against "without".

**Slice 4: the chat page.** The face on top of a planner that has been
proven:
- type a request;
- see the spec in plain words;
- approve it or just let it run;
- watch the milestones as a roadmap view drawn from the log;
- steer and stop.

**Slice 5: memory and the working agreement**, as in version 1.

**Slice 6: media pieces**, model judges validated against labelled
examples, and bench tasks with art and sound.

**Not in this order:** a live run on paid APIs. That needs API billing,
which is the owner's decision once slice 3 is proven.

## 7. Version 1, section by section

| Version 1 section | Verdict | Why |
|---|---|---|
| 1 What the brain must do | Keep (point 9 reworded) | Product is provider-neutral APIs |
| 3.1 Conversation and missions | Keep | — |
| 3.2 Event log | Keep, extend | Schema version, full model calls, cost, build ID, replay (P: Managed Agents, OpenHands) |
| 3.3 Planner builds graph and acceptance list up front | Change | Spec plus two check layers, progressive milestones, judge (P: SpecBench, ADaPT, 2609.20804) |
| 3.3 Cross-family check review | Change: second layer, measured | Correlated-error data (P); no code-review study |
| 3.3 Honest-check and anti-runaway rules | Keep | — |
| 3.4 Pieces and bible | Keep; graph becomes a view; one writer | Cognition, Cursor (P); Google/MIT study (S) |
| 3.5 Engines | Change | Provider-neutral layer, profiles, cascade, value per role |
| 3.6 Memory | Keep, two additions | Open file formats; skills carry their eval |
| 3.7 Working agreement | Keep | — |
| 3.8 Tools behind MCP | Change | Own tool interface; MCP an adapter (S: spec churn; P: 2608.08654 cost) |
| 3.9 Sandbox | Keep; Linux-only noted | — |
| 3.10 Protocol | Keep | Codex split its protocol from MCP (S) |
| 3.11 Scoreboard | Change: rebuilt first | Saturated at 10/10; tiers, holdout, pass^k, cost (P: Anthropic evals) |
| 3.12 Judgements | Keep | — |
| — | New: 3.13 workaround flags | Stale scaffolding (P: Managed Agents) |
| 5 Slice order | Change | Bench, then planner, then page |

## 8. Decisions that are the owner's

1. **Approve this version** in place of version 1 and the open slice 2
   design.
2. **Later, not now:** API billing for a live product run, after slice 3.

## 9. Open risks

- **Several key claims rest on secondary sources:** the Google/MIT scaling
  study, MCP's churn, and the Codex protocol history. The ADRs re-check
  any claim they rely on.
- **A harder bench costs more quota per run.** Mitigations: the
  multi-feature tier stays small (a handful of tasks), and real runs
  happen only when they decide something.
- **Planner-written held-out checks may be wrong.** They can be too strict
  and block correct work, or too loose and prove nothing. The bench's own
  holdout is the check on them.
- **Open models' tool-call reliability is unverified.** The adapter is
  tested against a stand-in; real open-model runs wait for the owner's
  servers.
