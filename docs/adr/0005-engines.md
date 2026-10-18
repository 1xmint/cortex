# 0005. Engines: coding-agent command lines without a shell, plus an API engine

Status: Accepted
Date: 2026-10-18

## Context

Workers need an engine: something that can read a piece's brief and change
files to make a failing check pass (design section 3.5). During development
the owner's Claude Code and Codex subscriptions are used, with no API key;
the product will run on API billing. An engine must not run model-chosen
commands next to its own login (ADR 0004).

Facts checked on the owner's PC on 2026-10-18 (`claude --help`,
`codex exec --help`, `codex features list`):
- Claude Code 2.1.302: `-p` with `--output-format stream-json` streams the
  session; `--tools "Read,Edit,Write,Glob,Grep"` limits the built-in tools
  (no `Bash`); `--strict-mcp-config --mcp-config <file>` loads only the MCP
  servers Cortex names. `--bare` would also confine file tools but requires
  an API key, so it is not used with a subscription.
- Codex CLI 0.163.0: `codex exec --json` streams events; the features
  `shell_tool` and `unified_exec` can be turned off with `--disable`.
  Whether that removes every way to run a command is not yet proven.

## Options

1. **Subscription command lines as engines**, restricted as above, with
   Cortex's MCP server providing `run` and `test` inside the sandbox.
2. **API engines only.** Needs an API key now, which the owner's rules
   exclude during development.
3. **Both behind one interface**, with a scripted fake engine for CI
   (subscriptions cannot log in on GitHub's machines).

## Decision

Option 3.
- One `Engine` interface: start a session with a brief, a worktree and the
  Cortex MCP server; stream transcript events; end with an outcome. The
  engine's own claim of success is stored as a note only.
- **Claude Code** is the slice 1 engine: `claude -p --output-format
  stream-json --tools "Read,Edit,Write,Glob,Grep" --strict-mcp-config
  --mcp-config <cortex>`, plus permission rules denying file access outside
  the worktree. Run under bubblewrap (ADR 0004).
- **Codex** is admitted only after a must-fail test shows a session asked
  to run a shell command cannot do so with `shell_tool` and `unified_exec`
  disabled.
- **Before any engine is used**, the same must-fail test runs for it: the
  engine is asked to run `id` without Cortex's tool, and must not.
- **A minimal API engine** (Anthropic Messages API, our own tool loop over
  the same MCP tools) is built in slice 1 and tested in CI against recorded
  responses. It is not run against the live API until the owner provides
  API billing.
- **A scripted fake engine** replays fixed file edits; CI uses it to prove
  the loop, the checker and recovery.

## Consequences

- Driving subscription command lines headless is for the owner's own
  development testing only and must stay within each product's terms.
- The engine's file tools can still see its own config folder, which holds
  its login. Permission rules deny reading it, and the sandboxed commands
  have no network, but a model that bypasses those rules could copy the
  login into the worktree. Accepted for development only; the API engine,
  whose key stays in the core, removes this risk.
- Command-line flags change between versions; the engine records the CLI
  version on every session and the must-fail test runs on each new version.
- Revisit when the API engine is proven live: it may become the default.
