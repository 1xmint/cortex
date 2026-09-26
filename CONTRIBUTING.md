# Contributing

This describes how work actually lands in this repository, including the parts
that are easy to get wrong. It is not aspirational — where the process has a
sharp edge, the edge is written down.

## The shape of a change

**Work is brief-driven.** A substantial change starts as a brief in
`cortex/plan/briefs/PR-*.md` that states what the change is, what it
deliberately does *not* do, and how it will be verified. An implementer reads
only the brief. The running checkpoint for what has landed and what is still
open is `cortex/plan/EXECUTION-STATE.md` — **read that first**; it will tell you
which of the other plan documents still matter.

`cortex/plan/HARNESS-EXCELLENCE-PLAN-2026-08.md` is several thousand lines.
Consult it by section. Never read it whole.

**One PR, one concern.** A formatting sweep does not ride along with a
behaviour change; a migration does not ride along with a refactor. If you find
a second problem while fixing the first, write it up rather than widening the
diff.

## Branches and PRs

Branch off `main`. Name the branch for what it does: `fix/…`, `feat/…`,
`chore/…`, `docs/…`, `test/…`.

The PR body should say what changed, why, what it deliberately leaves alone, and
how it was verified. If a claim is untested, say so in the body rather than
letting a reviewer assume coverage that does not exist.

### Auto-merge is armed on every PR

`.github/workflows/automerge.yml` enables GitHub auto-merge on **every**
non-draft PR. Consequences, both of which have bitten before:

- **A PR merges the moment its required checks go green.** There is no separate
  "and now I press merge" step. Open it in draft if it is not ready.
- **Any CI job that is not on the required list is advisory** and cannot stop a
  merge no matter how red it is.

The required checks on `main` are:

`rust`, `cortex`, `npm-audit (cortex)`, `cargo-deny`, `sandbox`,
`no-default-features`

Read the live list rather than trusting this paragraph:

```bash
gh api repos/:owner/:repo/branches/main/protection -q '.required_status_checks.contexts'
```

`clippy` is deliberately **advisory** (`continue-on-error: true`) inside the
`rust` job and must not be made required until someone reads its first full
run — see the comment in `.github/workflows/ci.yml`.

### Branch protection is strict, and auto-merge will not rescue you

`required_status_checks.strict` is `true`, and auto-merge does **not** update a
stale branch. When two PRs are open against the same base and the first merges,
the second goes to `BEHIND` — every check green, auto-merge armed — and stays
there until somebody rebases it by hand:

```bash
git fetch origin && git rebase origin/main && git push --force-with-lease
```

Plan for this: merge one at a time, or expect one manual rebase per queued PR.

If you ever change branch protection, note that `gh api -X PUT` **replaces the
whole object**. Re-read the current settings immediately before building the
payload, resend every field, and diff field by field afterwards — otherwise you
will silently drop `required_conversation_resolution` and the force-push ban.

## Running the checks locally

### Rust: `--all-targets`, never `--lib`

```bash
cargo test --workspace --all-targets --locked
```

**`cargo test --lib` does not compile `tests/`.** It runs every unit test in
`src/` and reports a confident green while the entire integration suite remains
uncompiled. This is not a hypothetical: a branch landed with "396 lib tests
green" and three integration tests that did not build, and CI caught what the
local run could not. Use `--all-targets`.

Also worth running before you push:

```bash
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --check
```

### Formatting

`rustfmt.toml` pins the format explicitly so `cargo fmt` is reproducible across
toolchains. **The `fmt` CI job runs `cargo fmt --all --check`** — if it fails,
run `cargo fmt --all` and commit the result. There is nothing to interpret; the
fix is the command.

Run `cargo fmt` as its **own commit**, never mixed with a change to behaviour —
a reviewer cannot see a logic change inside a thousand-line whitespace diff.
Formatting-only commits belong in `.git-blame-ignore-revs`, and only commits
that changed no behaviour at all may go in that file: a commit listed there is
invisible in blame.

Turn the ignore file on for your checkout — a commit cannot do it for you:

```bash
git config blame.ignoreRevsFile .git-blame-ignore-revs
```

GitHub's blame view reads the file automatically and needs no setup.

`.editorconfig` covers everything rustfmt does not.

### Frontend

`cortex/` is an npm project with its own type check and build; see the `cortex`
CI job for the exact commands. `vitest` does not typecheck — `npm run build` is
the gate that does.

## The migration counter must be re-checked at rebase

`schema_version` in `crates/api/src/db.rs` is a single counter, and every branch
that adds a migration competes for the next number on it.

**Read the current maximum at rebase time, not at design time.** If two branches
both pick the next number and one merges first, the second branch's migration is
silently skipped on every database that already ran the first — no error, no
warning, a schema that does not match the code. This has happened three times in
a single wave.

```bash
git fetch origin
git show origin/main:crates/api/src/db.rs | grep -oE 'fn migrate_v[0-9]+' | sort -V | tail -1
```

If your number is taken, renumber before merging. Say in the PR body which
maximum you checked against and when.

When you add a migration, also bump `SCHEMA_VERSION` in
`crates/api/src/db/mod.rs` to match — deploy tooling reads that const (via
`out/SCHEMA` in the release artifact) to refuse an automatic deploy that would
migrate the production schema, and a unit test in that file fails the build if
it drifts from what the migration chain actually produces.

## Deployment notes a contributor will trip over

- **`CORTEX_SINGLE_NODE=1` is required.** The server exits without it. The
  SQLite store is a `Mutex<Connection>` that two processes do not share, so a
  second dispatcher grades every delivery twice. Anything that deploys Cortex
  outside `deploy/cortex-api.service`, the compose files, and `.env.example`
  needs to set it.
- Cloudflare Pages deploys `cortex/` from `main` continuously through its own
  Git integration, with no workflow in this repository involved. The web app
  can run ahead of the backend.

## Commits

Conventional-commit prefixes (`feat`, `fix`, `chore`, `docs`, `test`,
`refactor`), scoped where it helps: `fix(auth):`, `docs(cortex):`. The subject
says what changed; the body says why, and what it does not do.

If the work was done with an AI assistant, keep the trailer:

```
Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
```

## Security

Do not report vulnerabilities through issues or pull requests. See
[SECURITY.md](SECURITY.md).
