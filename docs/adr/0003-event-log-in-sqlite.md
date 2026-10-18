# 0003. Missions as an event log in SQLite

Status: Accepted
Date: 2026-10-18

## Context

A mission can run for hours and must resume after a crash, a restart or a
rate-limit wait (design sections 3.2 and 3.3). Its history must also answer
"why did you do that?" and feed the scoreboard. Worker sessions driven
through a coding agent's command line are black boxes with many internal
steps that cannot be replayed one by one.

Facts relied on: Temporal, Restate, Inngest and DBOS all record steps and
replay them, and none of them makes a non-repeatable side effect safe on its
own. OpenHands' agent SDK runs an append-only event log as its source of
truth, with replay, pause, resume and fork.

## Options

1. **Durable-execution server (Temporal, then Restate).** Mature replay and
   timers. Adds a server to run and back up, and workflow code must be
   deterministic, a rule a model-driven planner breaks easily. Rust support
   is thinner than Go, Java or TypeScript. Its main strength, step replay,
   does not reach inside a worker session anyway.
2. **Our own event log in an embedded database.** Nothing to operate, and
   the mission already is a log. We own leases, retries, timers and
   idempotency, and must prove them with tests.
3. **Plain tables updated in place.** Simplest, but loses history and
   makes "why?" and crash recovery guesswork.

Storage for option 2: **SQLite** (WAL mode, full-text search) is
transactional, embedded and needs no server or bill; **Postgres** needs a
server, which serving one person first does not justify. The schema stays
portable to Postgres for hosting.

## Decision

- Each mission is an append-only sequence of events in SQLite, written in
  one transaction per event with a per-mission sequence number. Events are
  never updated or deleted. Read-side tables are caches rebuilt from events.
- **The unit of recovery is the worker session.** A session runs in its
  own disposable git worktree cut from the last accepted commit. On
  restart, a session with a `session_started` event and no matching end is
  marked abandoned, its worktree is deleted, and the session is run again.
- Effects outside the worktree carry an idempotency key recorded before the
  effect; on restart Cortex checks for the effect before retrying.
- Waits (rate limits, budgets, slow jobs) are `parked` events with a wake
  time; the core re-arms them on start.
- Every write path has a kill test: the process is killed after each event
  kind and must resume to the same end state.
- Data is keyed by project and person from the first table.

## Consequences

- One more thing we own and must test; the kill tests are required, not
  optional.
- Revisit, with Restate first and Temporal second, if missions need to run
  across several machines, or if the kill tests keep finding recovery bugs
  we cannot close.
