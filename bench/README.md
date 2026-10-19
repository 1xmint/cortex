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

`--tier small|multi|all` picks the tasks (default `all`), `--only 03-roman`
runs one task, `--tries 3` runs each task three times as separate missions,
and `--arm baseline` names the brain design being measured. `--worker
api-sonnet` uses the API engine (the profiles are in
[core/profiles.toml](../core/profiles.toml)). Run it from the repository
root, or pass `--dir`.

Each run writes `results/<UTC date>-<arm>-<engine>.json`, schema 2 (format:
[core/src/bench/mod.rs](../core/src/bench/mod.rs)). Per attempt: whether the
mission finished, whether the hidden checks then accepted it, sessions,
seconds, tokens, estimated cost, and a short story (the request, the commit,
files changed, failing checks, why it got stuck). The summary gives
accepted of total, pass^k per task (passed every one of k tries), and cost
per accepted result: failed attempts count toward the cost, nothing accepted
means unbounded, and any unpriced attempt makes it unknown, never 0. The
slice-1 file `results/2026-10-18-claude.json` is schema 1 and is kept as it
is. Results files are written only by this command, never by hand; commit
them as they come out.

Next to each `.json` the run writes a `.html` page: the header (arm, worker,
pass rate, time, tokens, cost) and every attempt's story, with model-written
text escaped and unknown costs said in words. `cortex bench page <file.json>`
rebuilds it.

## Comparing two brains

```bash
cortex bench compare results/a.json results/b.json --win-margin 2 --max-cost-ratio 1.5
```

Both files must cover the same tasks and tries. B wins only when it accepts
at least `--win-margin` more results than A and its cost per accepted result
is at most `--max-cost-ratio` times A's. A comparison PR states both numbers
before the run. If either cost is unknown the verdict is undecided. The sign
test p-value on the pairs only one arm passed is reported, not used as a
gate.
