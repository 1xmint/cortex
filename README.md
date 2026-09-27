# Cortex

An AI coding platform: you describe a task, a worker does it in a sandbox, and
a verifier decides whether the result is actually correct before you are
charged for it.

## Start here

- **[AGENTS.md](AGENTS.md)** — what to know before changing anything. Short.
- **[STATE.md](STATE.md)** — what is true now, and who each open thing waits on.
- **[CONTRIBUTING.md](CONTRIBUTING.md)** — how a change actually lands. Auto-merge,
  the required checks, `--all-targets`.

## Structure

```
cortex/            cortex.heyvera.org — the frontend
  plan/VISION.md   what Cortex is and who for (scope, not status)
  plan/CREDITS.md  what a credit is — the metering decision
  src/             React app

crates/            the Rust backend
  api/             HTTP API server (port 3001), the crate `cortex-api`
  core/            domain types, protocol, verification contract
  engine/          routing, scoring, and the verifier's verdict arithmetic
  worker/          sandboxed execution: worktree, container, egress
  context/         repo map, symbol index, retrieval
  egress/          the mediator a sandbox's only route out goes through
  cortex-server/   the binary
  tui/             terminal client
  soma*/           fenced behind the `soma` feature, OFF by default

deploy/            deployment scripts and service files
scripts/           build, setup, deploy scripts
docs/              architecture specs and proposals
```

This repository used to hold a second product — HeyVera Socials, a social
network — sharing the same backend. Socials moved to its own repository in
September 2026. If you find a reference to `heyvera-server`, `build_heyvera_router`,
a `/v1/social/` or `/v1/pulse/` route, or a `heyvera/` frontend directory, it is
a leftover and should be deleted rather than restored.

## Quick Start

```bash
# Backend
cargo build --release
# CORTEX_SINGLE_NODE=1 is required — the verification dispatcher refuses to
# start without it, because the SQLite store is a Mutex<Connection> and two
# dispatchers grade every delivery twice.
CORTEX_SINGLE_NODE=1 CORTEX_ADMIN_EMAILS="you@email.com" ./target/release/cortex-server

# Frontend
cd cortex && npm install && npm run dev
```

## Deploy

See [docs/DEPLOY.md](docs/DEPLOY.md) for the VPS backend deploy.

Cloudflare Pages deploys `cortex/` from `main` on its own, independently of
the backend release process, so the web app can run ahead of the backend.

## Key Docs

- Product vision: [cortex/plan/VISION.md](cortex/plan/VISION.md) — scope and positioning
- Credit unit & metering: [cortex/plan/CREDITS.md](cortex/plan/CREDITS.md) — read before touching billing
- Architecture: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — **see the 2026-08-02 amendment at the top before relying on any section**
- Operations Room: [docs/reference/cortex-operations-room.md](docs/reference/cortex-operations-room.md)
- CLI auth setup: [docs/operations/cli-auth-setup.md](docs/operations/cli-auth-setup.md)
