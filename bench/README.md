# Bench

Ten small programming prompts that measure the whole of Cortex, not one
model. Each runs as an ordinary mission: Cortex freezes the prompt's hidden
checks, proves they fail on the starting project, then works until they pass
or it runs out of sessions.

## A prompt

Each folder in `prompts/` holds:

- `prompt.md`: the request, exactly as a person would type it.
- `checks.toml` and `files/`: the hidden checks. The worker never sees them
  before they run; they test behaviour, not file contents.
- `start/`: the starting project, committed as the first commit of a fresh
  repository.
- `fake-solution.toml`: a known answer for the scripted fake engine, so CI
  proves every prompt can pass and that its checks are right.

A prompt whose check already passes on `start/` is blocked and counts as a
failure (invariant 2).

## A multi-feature task

Bigger tasks live in `multi/<id>/` (format: [core/src/bench/task.rs](../core/src/bench/task.rs)):

- `prompt.md` and `task.toml`: the request, its tier (`multi`), its kind
  (`fresh`, `change` or `foreign`) and its 5 to 10 features.
- `start/`: the starting project. A `foreign` task's `start/SOURCE.md` names
  the upstream URL, commit and licence.
- `visible/` checks, which the worker may run, and `hidden/` checks, which
  only the bench runs. Each is a `checks.toml` plus `files/`.
- `solutions/reference/` passes everything; each `solutions/bad-<name>/`
  fails something and says why in its `WHY.md`. `bad-gamer` passes the
  visible checks and fails the hidden ones.

CI proves all of this for every task (`core/tests/graders.rs`).

## Running it

```bash
cortex bench --worker claude-cli-sonnet
```

`--only 03-roman` runs one prompt; `--worker api-sonnet` uses the API engine
(the profiles are in [core/profiles.toml](../core/profiles.toml)). Run it
from the repository root, or pass `--dir`. Each run writes
`results/<UTC date>-<engine>.json`: the engine and its version, the Cortex
commit, and per prompt whether it passed, how many sessions and seconds it
took, and any blocked checks. Results files are written only by this
command, never by hand; commit them as they come out.
