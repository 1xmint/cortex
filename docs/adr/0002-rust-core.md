# 0002. Rust for the core

Status: Accepted
Date: 2026-10-18

## Context

The core is one long-running process that owns missions, the event log,
the working agreement's rule engine, the judgement interface, sandbox
control, engine processes and (from slice 2) the protocol server. See the
[brain design](../superpowers/specs/2026-10-18-cortex-brain-design.md),
sections 3 and 4.

It must run for hours without leaking or crashing, start and supervise
child processes (engines, `bwrap`, `runsc`, MCP tool servers), and never
let a model-written string become an instruction. Faces (web, desktop,
mobile) talk to it over JSON-RPC, so their language does not bind it.

Facts relied on (checked 2026-10-18):
- Codex's CLI moved from TypeScript to Rust for a zero-dependency install
  and native sandbox control; Claude Code stayed on TypeScript, so either
  can host an agent loop.
- The official MCP Rust SDK is published as `rmcp` (crates.io, version 3.x).
  The TypeScript MCP SDK is older and more widely used.
- `rusqlite` is a mature SQLite binding (crates.io 0.40).

## Options

Scored on the gold-rule criteria (1 weak – 5 strong).

| Criterion | Rust | TypeScript (Node) | Go | Python |
|---|---|---|---|---|
| Reliability over hours-long runs | 5: no GC pauses, no null crashes, exhaustive matches over event kinds | 3: one unhandled promise rejection ends the process; types are erased at run time | 4: simple, robust runtime; nil errors possible | 2: dynamic types, weakest at long-lived concurrency |
| Security | 5: memory safe; direct control of namespaces and file descriptors for sandboxes | 3: sandbox control only through child processes and native add-ons | 4: memory safe, good syscall access | 3: same as TypeScript |
| Long-term maintainability | 4: the compiler enforces the event model and rule language; steep learning curve | 4: familiar to most contributors; refactors less safe | 4 | 3 |
| Ecosystem fit | 4: `tokio`, `rusqlite`, `rmcp`, `serde`; thinner agent SDKs | 5: richest agent and MCP SDKs; shares types with the web face without generation | 3: thin agent and MCP ecosystem | 5 for models and media; weak for a supervisor process |
| Performance where it matters | 5: small, fast, low memory | 3 | 4 | 2 |
| Cost of another language boundary | 4: types generated for the web face (one boundary, already needed for desktop and mobile) | 5: none for the web face | 4 | 3: still needs TypeScript for faces |

TypeScript's real advantages: one language with the web face, and the
most mature agent and MCP SDKs. They lose here because the core's hardest
jobs are reliability over hours and process and sandbox control, where
Rust is strongest, and because the faces cross a protocol boundary anyway.

## Decision

The core is written in Rust (stable toolchain, 2024 edition), as one
binary. Types shared with faces are defined in Rust and generated for
TypeScript; CI fails if the generated files drift (slice 2).

## Consequences

- Slower to build than TypeScript (a tie-breaker only, per invariant 5).
- Agent-loop helpers that TypeScript SDKs provide (for the API engine) are
  written by us, against the providers' HTTP APIs.
- Revisit if `rmcp` falls behind the MCP specification in a way that
  blocks a needed feature, or if a core contributor pool matters more than
  the criteria above.
