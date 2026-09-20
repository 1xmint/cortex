# Splitting Cortex out: what the seam actually looks like

Status: **inventory, not a plan.** Read-only survey taken 2026-09-04 against
`main`. It exists to make the extraction PR's description writable, and to
correct one assumption that the extraction was going to be planned around.

Every count below is reproducible from the commands quoted with it.

## The headline: the routers are not a seam

The extraction was expected to start from an existing boundary — two router
functions, `build_cortex_router` (`crates/api/src/lib.rs:417`) and
`build_heyvera_router` (`:754`) — with the crate, database, CI and migration
counter as the parts still to separate.

**That boundary does not exist.** The Cortex router's routes are a strict
subset of the Socials router's:

```sh
awk 'NR>=417 && NR<=753' crates/api/src/lib.rs | grep -oE '\.route\("[^"]+"' | sort -u   # 164
awk 'NR>=754 && NR<=1314' crates/api/src/lib.rs | grep -oE '\.route\("[^"]+"' | sort -u  # 198
```

| | count |
|---|---|
| routes in `build_cortex_router` | 164 |
| routes in `build_heyvera_router` | 198 |
| routes **only** in the Cortex router | **0** |
| routes only in the Socials router | 34 |
| routes in both | 164 |

And of the Cortex router's own 164 routes, **70 are `/v1/social/*`** — 43% of
the "Cortex" router is the Socials API. The remaining 91 are `/api/*` (a mix of
both products) plus three health/metrics endpoints.

So `build_cortex_router` is not "Cortex's router". It is the shared
application, and `build_heyvera_router` is that plus 34 more Socials routes
(Pulse, Vera, projects, extra admin). Nothing today serves Cortex alone.

**Consequence for the extraction:** there is no router to lift. The Cortex
surface has to be *identified* first — route by route — and that identification
is the real work, not the file moves. Budget for it accordingly.

The 34 Socials-only routes are: 11 `/v1/pulse/*`, 2 `/api/vera/*`, 3
`/api/projects*`, 8 `/api/admin/*`, and the rest context/github/keys/social
odds and ends.

## Modules

`crates/api/src` is 76,736 lines across 61 modules. By size and ownership:

**Cortex-owned** — `scheduler.rs`, `ws.rs`, `verification_driver.rs`,
`verification_dispatcher.rs`, `check_runner.rs`, `run_payload.rs`,
`run_stream.rs`, `context_flow.rs`, `context_api.rs`, `worker_key.rs`,
`docker.rs`, `mission_control.rs`, `ecosystem_probe.rs`, `llm_client.rs`,
`cost_estimator.rs`, `budget_enforcer.rs`, `pricing.rs`, `sse.rs`.

**Socials-owned** — `social.rs` (3,268), `integrations.rs` (2,804),
`pulse.rs` (2,280), `messaging.rs` (1,813), `deploy_status.rs` (1,396),
`media.rs` (1,355), `social_policy.rs`, `vera.rs`, `replit.rs`,
`moderation.rs`, `agent_auth.rs`, `conversations.rs`, `notifications.rs`,
`x402.rs`.

**Shared, and the reason this is not a clean cut** — `db.rs` (29,593),
`auth.rs`, `clerk.rs`, `clerk_webhooks.rs`, `user.rs`, `billing.rs`,
`stripe_client.rs`, `ratelimit.rs`, `state.rs`, `routes.rs`, `admin.rs`,
`storage.rs`, `github.rs`, `github_repos.rs`, `credentials.rs`,
`api_keys.rs`, `usage_api.rs`, `metrics.rs`, `validate.rs`, `api_error.rs`,
`crypto.rs`, `key_material.rs`, `lock.rs`, `token_refresh.rs`.

**Soma, fenced** — `soma.rs`, `soma_bridge.rs`, `soma_fence.rs`. Behind the
`soma` feature, off by default, guarded by the `no-default-features` CI job.
Stays in `heyvera`.

## Database

```sh
grep -oE "CREATE TABLE(( IF NOT EXISTS)?) [a-z_]+" crates/api/src/db.rs | awk '{print $NF}' | sort -u | wc -l   # 106
grep -oE "fn migrate_v[0-9]+" crates/api/src/db.rs | sort -uV | tail -1                                          # migrate_v67
```

106 tables, 67 migrations, one shared `schema_version` counter.

- **38 are unambiguously Socials**: everything `social_*` (31 tables), the four
  `pulse_*`, plus `conversations` and `messages`.
- **26 are unambiguously Cortex**: runs, steps, attempts, workers, leases,
  verification, receipts, checks, jobs, routing decisions, context, artifacts,
  bundles, credits, ledger, quotes, prices, catalog, delegations, scopes,
  containers.
- **42 are shared or need a decision.** The ones that matter: `accounts`,
  `user_profiles`, `user_credentials`, `user_api_keys`, `user_budgets`,
  `subscriptions`, `billing_history`, `usage_events`, `promo_codes`,
  `referral_codes`, `code_redemptions`, `audit_log`, `idempotency_keys`,
  `webhook_events`, `integration_*` (4 tables), `github_imports`,
  `project_workspaces`.

Note the `cortex_*` prefixed tables (`cortex_groups`, `cortex_tasks`,
`cortex_task_chats`, `cortex_authority_*`, `cortex_approval_requests`) are
Cortex by name and were counted in the shared bucket by the crude prefix match
above; they belong with Cortex.

Cortex has 0 executed steps and 5 pending steps from May in production, so its
half of this schema can restart at v1 rather than being migrated.

## CI and deploy

| Workflow | Owner |
|---|---|
| `live-model.yml`, `stub-provider-e2e.yml`, `host-db-migration.yml` | **Cortex** |
| `deploy-frontend.yml` | **Socials** — triggers on `heyvera/**` only |
| `heyvera-launch-smoke.yml` | **Socials** |
| `deploy-production.yml` | **Socials** — see below |
| `ci.yml`, `supply-chain.yml`, `automerge.yml`, `add-to-project.yml` | both; trimmed rather than moved |

**`deploy-production.yml` was the open question, and the answer is Socials.**
Its step is still called "Deploy ClawNet", it deploys out of
`/home/<user>/claw-net`, it runs `scripts/deploy.sh` (which builds and rsyncs
the HeyVera SPA), and it health-checks `/v1/social/longform` on port 3001. It
does not deploy Cortex. Cortex deploys through `deploy/cortex-deploy.sh` and
`scripts/deploy-cortex.sh`.

The `claw-net` naming is stale throughout that path and is worth a separate
cleanup; it is not load-bearing for the split.

Cortex-owned Dockerfiles: `Dockerfile`, `Dockerfile.worker`,
`Dockerfile.sandbox`, `Dockerfile.sandbox-provider`, `Dockerfile.sandbox-stub`,
`Dockerfile.runner`, `Dockerfile.egress`.

## What this changes about the extraction

1. **There is no router to lift.** The Cortex HTTP surface must be enumerated
   route by route first. That is the long pole, and it is not a mechanical step.
2. **The shared middle is real.** Auth, Clerk, users, billing, Stripe, rate
   limiting and `AppState` are genuinely used by both. Each needs a decision:
   duplicate into Cortex, or extract to a third crate both depend on.
   Duplicating is faster and defensible while Cortex has no customers.
3. **`db.rs` cannot be split by moving functions.** At 29,593 lines with a
   shared `schema_version` and shared user/billing tables, the Cortex half is
   better rewritten against a fresh v1 schema than carved out — which the
   production state permits, since Cortex has executed nothing.
4. **The history scrub still stands** as described in the tranche 2 plan, and
   is independent of everything above.

## What was not surveyed

Frontend (`cortex/` 25,708 lines of TS/TSX, `heyvera/` separate) beyond
confirming they are separate npm projects with separate CI jobs. `crates/shared`
ownership. Which of the 42 shared tables carry rows that matter. None of these
block writing the extraction PR description; all of them block executing it.

## Follow-up survey, 2026-09-05: the seam is two functions and one ledger call

The section above says the shared middle is real and each shared module needs a
decision. That is true of the *modules*. It is not true of the **running
coupling**, and the difference decides how the extraction is sequenced.

Measured on `main` at `5708f1b7`: for every `Database` method, which product's
modules call it.

```sh
# Socials-owned modules, per the classification above
socials="social.rs integrations.rs pulse.rs messaging.rs deploy_status.rs \
media.rs social_policy.rs vera.rs replit.rs moderation.rs conversations.rs \
notifications.rs x402.rs agent_auth.rs"
# Cortex-owned modules
cortex="scheduler.rs ws.rs verification_driver.rs verification_dispatcher.rs \
check_runner.rs run_payload.rs run_stream.rs context_flow.rs context_api.rs \
worker_key.rs mission_control.rs cost_estimator.rs budget_enforcer.rs pricing.rs"
```

Of the 429 `pub fn` on `Database`, the methods reached from **both** sides are:

| method | called from Socials by | why |
|---|---|---|
| `deduct_credits` | `pulse.rs` | the shared credit ledger |
| `record_usage` | `pulse.rs` | the same charge's usage row |
| `lease_step` | `integrations.rs` | — |
| `deliver_step` | `integrations.rs` | — |
| `begin_verifying_step` | `integrations.rs` | — |
| `register_worker` | `integrations.rs` | — |
| `update_run_status` | `integrations.rs` | — |
| `record_verification_outcome` | `integrations.rs` | — |

That is the whole list. Two callers, and the second one is a misclassification.

### `integrations.rs` is not a Socials module

The classification above put its 2,808 lines under Socials because of the Slack
and Replit OAuth. Splitting its functions by what they touch:

**Cortex** (15 functions, `cortex_groups`, `cortex_tasks`,
`cortex_authority_scopes`, `cortex_authority_delegations`,
`cortex_approval_requests`): `get_group_tasks`, `get_group_task_projection`,
`attach_group_task_chat`, `create_group_task`, `apply_group_task_actions`,
`patch_group_task`, `update_group_tasks`, `list_authority_scopes`,
`create_authority_scope`, `update_authority_scope`, `delegate_authority`,
`revoke_authority_delegation`, `list_authority_delegations`,
`list_group_approval_requests`, `create_group_approval_request`,
`resolve_group_approval_request`.

**Socials** (7 functions): `integration_status`, `slack_oauth_start`,
`slack_oauth_callback`, `slack_channels`, `import_slack_channels`,
`slack_events`, `replit_workspaces`.

**Both** (2 functions): `slack_command` and `import_replit_workspace`. These are
where a Socials integration creates Cortex work — a Slack command that opens a
task, a Replit import that seeds one.

Those two functions are the seam. Not a boundary to be negotiated across 42
tables: two call sites where one product asks the other to do something.

### What this changes

1. **Split `integrations.rs` before extracting anything.** It is the only file
   that genuinely straddles, and once its Cortex half moves to a Cortex-owned
   module, the Cortex step-lifecycle methods have no Socials caller at all.
2. **After that split, the shared runtime surface is `deduct_credits` and
   `record_usage`, from `pulse.rs`, and nothing else.** That is the coupling
   already documented against `ChargeKey` — Socials charges the same ledger for
   a Pulse draft, with no verification behind it. It is one decision, not a
   category of them.
3. **`slack_command` and `import_replit_workspace` become the inter-product
   API.** They are already the only place the products speak, so they define the
   interface rather than needing one designed for them.
4. **The 42 shared *tables* are still 42 decisions, and they are no longer on
   the critical path.** Cortex has executed nothing in production, so its half
   restarts at schema v1 (see above) and the shared tables stay with Socials
   until Cortex has rows worth migrating. Deciding them now is optional work.

### What still blocks execution

- Enumerating the Cortex HTTP surface route by route. Unchanged, and still the
  long pole: **0 of the Cortex router's 162 routes are Cortex-only, and 70 are
  `/v1/social/*`.** Re-measured 2026-09-05; the count moved from 164 to 162 with
  intervening changes and the ratio did not.
- The `pulse.rs` ledger decision: does Socials keep charging Cortex's ledger, or
  get its own? This is a product decision and it gates narrowing the ledger to a
  Cortex-only caller set.

Neither is a refactor. Both are decisions, which is why this stayed an inventory
rather than becoming a plan.
## The Cortex HTTP surface, route by route — a proposal, 2026-09-06

The survey above says the route-by-route identification is the long pole and
that it is not a mechanical step. It is not, but it is also not a research
project: it took an afternoon, and what it needs from a human is **approval, not
authorship**. This section is the proposal.

Measured against `main`: 162 distinct routes in `build_cortex_router`
(`crates/api/src/lib.rs:417-760`), plus 34 more in `build_heyvera_router`. Every
route below is classified by what its handler actually reads and writes, checked
against the `Database` methods it calls — not by its path prefix, which is
misleading in both directions.

| Owner | Routes |
|---|---|
| **Socials** | 88 |
| **Cortex** | 53 |
| **Shared — duplicate into both** | 21 |

### Cortex — 53

| Group | Count | Why |
|---|---|---|
| `/api/runs*` | 6 | the harness itself: create, estimate, get, events, PR, stream |
| `/api/groups/*` | 9 | `cortex_groups`, `cortex_tasks` — moved out of `integrations.rs` |
| `/api/authority/*` | 4 | `cortex_authority_scopes`, `cortex_authority_delegations` |
| `/api/admin/{runs,runs/{},workers,containers,containers/stats,decisions,pressure}` | 7 | operator views of harness state |
| `/api/auth/*` | 6 | provider credential auth — Claude, OpenAI, Codex subscriptions |
| `/api/credentials/*` | 3 | assigning those credentials to runs |
| `/api/chat*` | 3 | calls `list_active_runs`, `touch_container_activity` |
| `/api/context/*` | 3 | the comprehension layer |
| `/api/github/*` | 3 | repo import; calls `touch_container_activity` |
| `/api/user/{github/status,repos/select,routing}` | 3 | which repo a run targets, and how it routes |
| `/api/mc*` | 2 | mission control |
| `/api/ws` | 1 | the worker websocket |
| `/api/ledger` | 1 | the credit ledger Cortex's guarantee rests on |
| `/api/operations/summary` | 1 | personal operations view |
| `/api/providers` | 1 | provider catalogue |

### Socials — 88

`/v1/social/*` (70), `/api/integrations/slack/*` (6), `/api/integrations/replit/*`
(2), `/api/integrations/status`, `/api/projects/import`, `/api/conversations*`
(2), `/api/deploy-*` and `/api/deployment/*` (6).

Two of these are the seam identified above: `/api/integrations/slack/command` and
`/api/integrations/replit/import` create Cortex work. They stay Socials-owned and
call across the boundary.

### Shared — 21, and the recommendation is to duplicate

| Group | Count | Recommendation |
|---|---|---|
| `/api/billing/*` | 6 | Socials keeps Stripe today; Cortex gets its own when it sells |
| `/api/admin/{stats,usage,usage/users,audit-log,redemptions}` | 5 | Socials keeps; Cortex grows its own operator surface |
| `/api/health`, `/v1/health`, `/v1/ready`, `/metrics` | 4 | duplicate — every service needs its own |
| `/api/usage*` | 2 | duplicate |
| `/api/stripe/webhook`, `/api/clerk/webhooks` | 2 | Socials keeps both |
| `/api/user/profile` | 1 | identity — duplicate against a shared Clerk |
| `/api/keys` | 1 | duplicate |

**Duplicating rather than extracting a third crate is the recommendation**, for
the reason the section above gives: it is faster and defensible while Cortex has
no customers, and it avoids designing a shared-platform interface around a
product whose shape is not settled. Revisit when Cortex has paying users.

### What this does not settle

Whether Socials keeps charging Cortex's ledger. That is the one genuine product
decision left in the extraction, and it is unaffected by any of the above:
`pulse.rs` calls `deduct_credits` for a Pulse draft, with no verification behind
it. After the `integrations.rs` split it is the **only** cross-product runtime
coupling remaining.

### How to check this

Every classification above is re-derivable:

```sh
# every route in the Cortex router, with its handler
awk 'NR>=417 && NR<=760' crates/api/src/lib.rs \
  | grep -oE '\.route\("[^"]+", *[a-z]+\([a-z_]+::[a-z_]+\)'

# what a given handler module actually touches
grep -oE 'db\.[a-z_]+\(' crates/api/src/<module>.rs | sort -u
```

The second command is what decides the disagreements. `/api/chat` looks like a
Socials feature and reads Cortex run state; `/api/auth/*` looks like user login
and is provider-credential plumbing. Path prefixes are not evidence.
