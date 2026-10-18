# 0004. Sandbox: Linux side, bubblewrap and gVisor

Status: Accepted
Date: 2026-10-18

## Context

Invariant 3: model-written code never gets host credentials, provider keys
or the wider machine, and if isolation cannot be set up the step is
blocked. Untrusted work includes the project's code and tests, headless
browser runs and local tool servers (design section 3.9).

The owner's PC runs Windows 10 Home. WSL2 is switched on there (used by
Docker Desktop) but has no general Linux system yet. Windows 10 Home does
not give WSL2 nested virtualisation, so Firecracker cannot run there. Hosted
sandboxes (E2B, Daytona, Modal) bill separately, which the owner's rules
exclude. GitHub's Ubuntu runners can run both bubblewrap and gVisor, though
Ubuntu 24.04 restricts unprivileged user namespaces through AppArmor until
`kernel.apparmor_restrict_unprivileged_userns` is set to 0.

## Options

| Criterion | Windows-native (Job objects, AppContainer) | Process sandbox only (bubblewrap) | bubblewrap + gVisor | bubblewrap + Firecracker |
|---|---|---|---|---|
| Security | 2: weak isolation, little tooling | 3: shares the host kernel | 5: second layer with its own kernel surface | 5 |
| Runs on the owner's PC | 5 | 4 (inside WSL2) | 4 (inside WSL2; to be proven there) | 1: no nested virtualisation |
| Runs on hosted Linux later | 1 | 5 | 5 | 5 |
| Reliability and maintainability | 2 | 5 | 4 | 3 |
| Cost | 0 | 0 | 0 | 0 |

## Decision

- **The core and everything it starts run on Linux**: inside WSL2 (Ubuntu)
  on the owner's PC, on Ubuntu runners in CI, on Linux hosts later. No
  worktree is shared across the Windows/Linux file boundary.
- **Commands from models** (`run` and `test` tools) execute in two layers,
  both required:
  1. **gVisor** (`runsc`, rootless, `--network=none`) runs the command in a
     container whose root holds only system directories (read-only) and the
     session's worktree (read-write).
  2. **bubblewrap** wraps the `runsc` process itself with fresh namespaces,
     a temporary home and no view of the user's home, keys or engine logins.
- **Engine processes** (coding agent command lines) run under bubblewrap
  with only the worktree, their own config folder and network access. They
  are not given a shell (ADR 0005), so they never run model-chosen
  commands themselves.
- If `bwrap` or `runsc` is missing or a self-test fails, every step that
  needs them is blocked with a clear message.
- **Proven by checks that must fail** (run in CI and by `cortex doctor`):
  sandboxed code tries to read a planted key file outside the worktree and
  to open a network connection; both must be refused, and the test fails
  if either succeeds.
- Firecracker replaces gVisor as layer 2 on hosted Linux when hosting
  starts (a new ADR).

## Consequences

- The owner must install a Linux system in WSL2 and log the coding agents
  in there once before running the bench on his PC.
- gVisor under WSL2 is not yet proven on this PC; the first slice 1 task
  proves the layers in CI, and `cortex doctor` proves them on the PC.
- Revisit if gVisor cannot run nested inside bubblewrap; the fallback is
  bubblewrap applied inside gVisor's own namespace configuration, recorded
  in a new ADR, never one layer alone.
