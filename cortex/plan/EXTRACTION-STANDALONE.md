# Cortex standalone — the resume trail

**Acceptance target, set 2026-09-20:** a Cortex checkout that builds, tests and
runs with no Socials (HeyVera) source and no HeyVera runtime state. HeyVera's
own dependency on `cortex-api` is explicitly **not** a gate; Josh took it off
the list. Socials keeps the `1xmint/heyvera` repository and can depend on
whatever it likes until it is repointed later.

Where this sits: `docs/proposals/cortex-socials-split-inventory.md` is the
ownership survey everything below rests on, and
`crates/api/route-manifest.csv` (merged PR #650) is the machine-readable form
of it. Steps 35–38 of `EXECUTION-STATE.md` are the PR-by-PR record.

## What is already done

| | |
|---|---|
| PR [#649](https://github.com/1xmint/heyvera/pull/649) | Socials no longer charges the Cortex credit ledger. `deduct_credits` and `record_usage` are Cortex-only callers now. |
| PR [#650](https://github.com/1xmint/heyvera/pull/650) | Route ownership is executable: a literal 205-route manifest, separate routers, negative cross-product probes. 92 cortex / 15 duplicate / 98 socials. |
| PR [#651](https://github.com/1xmint/heyvera/pull/651) | `heyvera-db` exists and can export a checksummed 43-table Socials v1 schema out of the mixed v68 database. |

All three merged green on 2026-09-20. PR 4 (an independent Socials API crate)
is parked on branch `refactor/socials-application-boundary` in the
`heyvera-social-audit` worktree. **It is not on the critical path any more** —
it is Socials' side of the split, and the standalone-Cortex target does not
wait on it.

## The work left, in order

Branch: `extract/cortex-standalone` in `C:\Users\Josh\Desktop\GitHub\cortex-extraction-work`
(a clone of `1xmint/heyvera`; nothing here is ever pushed to that origin).

1. ~~**Amputate Socials from the Rust workspace.**~~ **Done** — four commits,
   merged into this branch on 2026-09-20:

   | | |
   |---|---|
   | `6ede9643` | The Socials HTTP modules and `build_heyvera_router`. |
   | `5422da3b` | `crates/heyvera-server`, `crates/heyvera-db`, `crates/shared`. |
   | `201c5664` | Two admin routes that only ever touched Socials. |
   | `e8985a9c` | The Socials query layer inside `crates/api/src/db/mod.rs`. |

   The database module went from 28,032 lines to 17,636: 154 public
   `social_*`/`pulse_*` query methods, 15 private helpers, 41 in-file tests and
   13 Socials-only types. Also deleted in that pass: `clerk_webhooks.rs` (the
   webhook endpoint was a Socials route; Cortex authenticates *against* Clerk
   but never received its webhooks), the account-suspension admin handlers, an
   orphaned test module in `state.rs` that built a type from the deleted
   messaging module, and `scripts/reconcile-counters.sh`, whose endpoint went
   in `201c5664`. `route-manifest.csv` is 106 routes — 91 cortex, 15
   duplicate, 0 socials — and `tests/route_ownership.rs` asserts that count.

   Proof, all three run before the merge and all three silent:

   ```
   cargo check --workspace --all-targets
   cargo clippy -p cortex-api --all-targets -- -D warnings
   cargo clippy -p cortex-api --no-default-features --all-targets -- -D warnings
   ```

   The third one is the Soma fence still holding with the Socials code gone.

   **Two things nearly went out with the rubbish**, both caught and restored.
   `crates/api/tests/vera_surface.rs` was deleted alongside the three genuine
   Socials test files; Vera is Cortex's own trust arithmetic (`pub mod vera` in
   `lib.rs`) and that file asserts the legacy Vera HTTP surface returns 404,
   which is a guarantee worth keeping. And a test proving a panic under the
   database lock does not poison the mutex for every later caller was cut
   because the write it used happened to be a Socials one; it is back, with a
   Cortex write instead. The lesson both times: match on what a thing *is*, not
   on whether a Socials name appears anywhere inside it.

   **Soma stays.** An earlier draft of this list said to remove it. That was
   wrong: the inventory puts `soma.rs`, `soma_bridge.rs` and `soma_fence.rs` in
   their own category, not in Socials, and `vera.rs` is the Soma trust
   arithmetic. It is fenced behind a cargo feature that is off by default and
   guarded by the `no-default-features` CI job. Removing it would be scope
   creep that breaks a working gate.

   **Eleven modules, not fifteen.** The inventory's 2026-09-06 prose calls
   `/api/integrations/*`, `/api/conversations*`, `/api/deploy*` and
   `/api/projects*` Socials. `route-manifest.csv` calls all four Cortex, and
   the manifest is the one a test enforces. **When the prose and the manifest
   disagree, the manifest wins.**
2. ~~**Amputate the repository shell.**~~ **Done** — commits `8e1410a1`
   (1,213 files, 284,679 lines deleted: `heyvera/`, `archive/`, the four
   Socials workflows, the install and deploy scripts, `deploy/heyvera-api.service`),
   `b331315c` (the CI matrices, dependabot, the audit allowlist) and `d6a9e86b`
   (comments citing deleted files). Two of those were real breaks, not tidying:
   the SPA's production API base pointed at `api.heyvera.org`, a host whose
   Caddy block had just been deleted, and `scripts/deploy-cortex.sh` fell back
   to stopping a systemd service called `heyvera`. The `Caddyfile` now names
   `/v1/health` and `/v1/ready` instead of proxying all of `/v1/*`.
3. ~~**Rename what is left.**~~ **Done** — commit `1f293dcb`. `crates/shared`
   turned out to be unused by Cortex and was dropped with the other Socials
   crates rather than renamed. `README.md`, `AGENTS.md`, `CONTRIBUTING.md`,
   `SECURITY.md`, `STATE.md`, `docs/ARCHITECTURE.md`, `replit.md` and the bug
   report template now describe one product. `HEYVERA-VISION.md` is deleted.
4. **New repository, private, fresh history.** The history scrub gate in the
   inventory is satisfied by not carrying HeyVera's history at all: a single
   initial commit. Private first — going public is a separate decision, and
   reversing a public push is not possible.

   **The name `cortex` is taken.** `hey-vera/Cortex` exists, archived since
   2026-05-19, 284 KB, three commits. Its own description says it is superseded
   and kept for one thing: a `Cargo.toml` showing the right way to depend on
   Soma. The recommendation is to rename that relic (GitHub leaves a redirect,
   so nothing that links to it breaks) and give the live repository the plain
   name. The alternative is an awkward second-choice name forever.
5. **CI green in the new repository.** The test suite runs there, never on this
   PC. `stub-provider-e2e` is cheap and safe; `live-model` spends real money and
   stays `workflow_dispatch`.

## Deliberately deferred, and why it is safe to defer

**The fresh Cortex v1 schema.** The inventory's recommendation is that Cortex's
half of the 106-table database restart at schema v1 rather than being carved
out of the shared one, which production permits because Cortex has executed
zero steps. Step 1 above deletes the Socials *methods* but leaves the Socials
`CREATE TABLE` statements and the shared `schema_version` counter alone.

The consequence of leaving it: a standalone Cortex creates ~38 tables it never
reads. Nothing breaks, nothing leaks, and the migration chain stays provably
the same one production is on. Doing it properly is a rewrite of the DDL with
its own verification, and it should not ride along inside a deletion pass.

**Socials' own independence.** `heyvera-server` still depends on `cortex-api`
in the `1xmint/heyvera` repository. Out of scope by decision.
