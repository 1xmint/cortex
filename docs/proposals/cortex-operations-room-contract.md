# Cortex Operations Room Contract

Status: proposal

## Title

Cortex Operations Room product and state contract.

## Problem

Cortex has the right direction: Project Chat, Task Manager, Live Map, runs, workers, verifier reports, and evidence are all present in partial form. The risk is that each surface grows as a separate product: chat as one state machine, Task Manager as local JSON, Live Map as visual state, and runs as backend state.

That would make Cortex feel impressive but unreliable. Users would see activity without knowing what is true, what is blocked, what is safe to approve, or what actually changed.

## Why Now

The product is moving from "chat with coding agents" toward "operations room for AI software work." Before adding premium visual polish, Cortex needs a shared contract for:

- what each surface owns
- what state is canonical
- what "done" means
- how Project Chat attaches to work
- how Task Manager controls work
- how Live Map visualizes work
- what evidence is required before completion

This is the batter-first step that lets later UI polish become trustworthy instead of decorative.

## Broad Idea

Cortex should have one Operations Room engine with multiple views:

- **Project Chat** is the workbench for intent, collaboration, debugging, review, and detailed implementation conversation.
- **Task Manager** is the control room for intake, queueing, prioritization, dispatch, pause/resume, retry, cancellation, approvals, dependencies, blockers, budgets, and evidence inspection.
- **Live Map** is the truth/debugging lens over active projects, tasks, runs, steps, workers, dependencies, blockers, leases, evidence, and cost/time hotspots.
- **Personal Operations** is an aggregate inbox over the user's authorized personal and org scopes.
- **Org Operations** is the shared authority root for a team/company/GitHub organization.

All of these must project the same canonical graph:

```text
Scope -> Project -> Task -> Run -> Step -> Attempt -> Evidence -> VerifierReport
```

## What 10/10 Looks Like

A customer can open Cortex and immediately understand:

- what needs their attention now
- what work is queued, active, blocked, done, or failed
- which chat thread belongs to which project/task
- which worker/agent is doing what
- whether work is looping or making progress
- what changed in code or external systems
- why Cortex thinks a task is done
- what evidence supports that claim
- what action is safe to take next

The interface feels premium because it is coherent. The luxury comes from confidence: the user can trust every task card, map edge, chat status, and completion badge.

## Surface Contract

### Project Chat

Project Chat owns human/agent collaboration.

It may:

- capture intent and clarifying decisions
- attach messages to a project or task
- start or attach to a task/run when the user asks for work
- show run and evidence summaries inline
- raise or resolve human decisions
- explain why Cortex split work, retried work, or needs approval

It must not:

- be the canonical source for run state
- create hidden machine state that exists only in scrollback
- spawn a new chat for every run, step, retry, or worker
- mark work done without verifier/evidence state

### Task Manager

Task Manager owns operational control.

It may:

- intake work without opening chat
- queue, prioritize, assign, pause, resume, retry, cancel, and approve work
- show dependencies and blocked frontiers
- open a task inspector with budget, autonomy, authority, run state, evidence, and next action
- attach or open Project Chat only when collaboration is needed

It must not:

- be local-only state for production work
- show fake seeded activity as if it were real
- treat "task complete" as a manual visual flag
- hide worker, verifier, or evidence failures behind generic status text

### Live Map

Live Map owns topology and debugging.

It may:

- render scopes, projects, tasks, runs, steps, workers, dependencies, resources, blockers, evidence, and cost/time signals
- support pan, zoom, click-to-inspect, and later replay
- visualize loops, stale leases, blocked edges, expensive branches, and verifier failures
- expose safe actions when authority permits

It must not:

- be the primary queue
- show decorative graph nodes not backed by canonical state
- imply parallel autonomy before leases and conflict controls exist

### Personal Operations

Personal Operations owns the user-level inbox.

It may:

- aggregate authorized personal and org work
- show "what needs me now"
- link into org operations, project chat, task inspectors, and approvals
- show spend and active-run rollups scoped to what the user may see

It must not:

- bypass org policy
- mutate org work without explicit org-context handoff
- reveal org secrets or unauthorized work

### Org Operations

Org Operations owns shared team/company execution.

It may:

- represent a GitHub org/team/company authority boundary
- hold org tasks, repos, projects, runs, approvals, audit events, budgets, and policies
- coordinate parallel work under leases and merge gates

It must not:

- mix private user scratch work into org truth without promotion
- silently change authority, membership, secrets, deploy policy, or trust policy

## Canonical State

### Entities

| Entity | Meaning | Canonical owner |
|---|---|---|
| Scope | personal or org authority boundary | backend |
| Project | durable work container, usually tied to repo/org context | backend |
| Task | customer-visible work item with objective, priority, status, owner, budget, and approval policy | backend |
| ChatThread | human/agent conversation attached to project or task | backend |
| Run | concrete execution attempt for a task | backend |
| Step | planned unit of work inside a run | backend |
| Attempt | leased execution of a step by a worker | backend |
| Evidence | diff, files, logs, command results, artifacts, receipts | backend/object storage |
| VerifierReport | verdict and diagnostic over evidence | backend |
| Ask | human decision request | backend |
| Lease | exclusive claim over resource/path/branch/deploy surface | backend |

### Status Model

Task statuses:

```text
inbox -> ready -> active -> blocked -> review -> done
                           -> failed
                           -> cancelled
```

Run statuses:

```text
planned -> queued -> running -> verifying -> completed
                              -> blocked
                              -> failed
                              -> cancelled
```

Step statuses:

```text
pending -> ready -> leased -> running -> verifying -> completed
                                      -> blocked
                                      -> failed
                                      -> orphaned
                                      -> cancelled
```

Attempt statuses:

```text
leased -> started -> heartbeat -> completed
                         -> failed
                         -> stale
```

Completion rule:

```text
Task done requires:
  at least one completed run
  all required steps terminal
  required evidence captured
  verifier verdict accepted or explicit human override
  no open blocking Ask
```

## Event Contract

Operations Room state should be rebuilt from append-only events, then served as projections to the UI.

Minimum event families:

- `task.created`
- `task.updated`
- `task.prioritized`
- `task.blocked`
- `task.unblocked`
- `task.cancelled`
- `task.completed`
- `chat.attached`
- `run.created`
- `run.queued`
- `run.started`
- `run.completed`
- `run.failed`
- `step.planned`
- `step.leased`
- `step.started`
- `step.heartbeat`
- `step.completed`
- `step.failed`
- `step.orphaned`
- `evidence.recorded`
- `verifier.reported`
- `ask.created`
- `ask.resolved`
- `lease.created`
- `lease.released`

The first implementation may keep existing tables and add event recording incrementally. The product contract is that views read projections, not independent local truth.

## Fitness Check

- vision fit: strong; this is the core product shape behind Cortex as a premium AI operations room.
- real user/operator need: strong; without this contract users cannot trust AI work across chats, tasks, workers, and repos.
- security exposure: high; authority, org state, GitHub writes, approvals, evidence, and deploy controls are in scope.
- evidence this is needed now: live Cortex already has Project Chat, Task Manager, run graph, workers, verifier, and Cloudflare/VPS deploy complexity; boundaries are needed before more feature growth.
- keep / reshape / pause / remove: keep and reshape into small implementation slices.

## Evidence Ledger

- current status: promoted from backlog discovery into proposal.
- upstream dependencies: Soma delegation semantics, GitHub App trust model, org membership policy, verifier/evidence format, resource lease model.
- missing evidence: exact customer workflow telemetry, graph size expectations, org permission matrix, GitHub write rollout plan.
- blocks current work: yes for deeper Task Manager/Live Map foundations; no for isolated bug fixes.
- next gate: implement v1 slices below and create ADRs when event log, authority model, or GitHub trust model changes.
- terminal condition: accepted by implementation through shipped docs, tests, and production projections, or superseded by a newer operations architecture.

## Repo Ownership

- protocol truth: Soma owns Heart/delegation/Pulse semantics.
- platform/runtime truth: this repo owns Cortex runs, steps, workers, events, verifier reports, task projections, and deploy behavior.
- product/integration truth: `docs/` owns accepted Cortex product contracts; `internal/` keeps raw brainstorms.
- internal-only material: speculative multi-company UX, launch positioning, unvalidated premium packaging.

## First Consumer

Cortex dashboard and Rust API in this repo.

The first implementation consumer should be the existing Task Manager and run inspector, not a new visual map. The UI should first show true task/run/evidence state in a task inspector.

## Security / Reliability Requirements

- threat model: unauthorized cross-org reads/writes, hidden local state, stale worker completions, fake done states, lease conflicts, verifier bypass, GitHub write drift.
- rollback or recovery: append-only events must allow projection rebuilds; existing tables remain source during migration until event projections are verified.
- auditability: org-affecting writes require scope, actor, authority, target, event id, and before/after state where applicable.
- failure modes: no worker, lost worker, stale lease, verifier rejection, auth expired, billing blocked, GitHub unavailable, deploy lock held, evidence missing.

## Delivery Shape

1. Add canonical docs/reference contract for Cortex Operations Room.
2. Add backend event table and write first events for task/run/step lifecycle.
3. Add task-to-run binding and expose a task inspector projection.
4. Add Project Chat attachment rules and show attached task/run state.
5. Add evidence/verifier gate to task completion projection.
6. Add read-only Live Map over real projection data.
7. Add leases for branch/path/deploy resources before enabling parallel write autonomy.

## ADR Needed?

- yes, when the append-only event log becomes production source of truth.
- yes, when org authority and GitHub write boundaries are implemented.
- no for this proposal doc alone.

## Open Questions

- Which v1 customer workflow should be the first proof: solo repo task, team GitHub org task, or personal multi-project inbox?
- Should personal scope be backed by the same org-style policy engine from day one, or start as a simplified policy row?
- Which verifier verdicts require human override versus automatic retry?
- What is the first resource lease set: branch, path, issue, PR, deployment environment, or migration?
- What event retention and privacy policy is needed before org beta?

## Links

- backlog: `internal/backlog/cortex-operations-room-foundation-checklist.md`
- live map proposal: `docs/proposals/live-orchestration-mapping-view.md`
- vision: `internal/vision/cortex.md`
