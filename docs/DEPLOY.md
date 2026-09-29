# Automatic deploy to the production host

Status: canonical

When a release build finishes on `main` (`.github/workflows/build-release.yml`),
`.github/workflows/deploy.yml` ships the resulting artifact to the production
host over Tailscale and restarts it. Nothing runs until the two secrets below
are set — until then the workflow exits quietly with a `::notice::`.

This document is the one-time owner setup for those secrets, plus how to
trigger a deploy by hand and how to roll back.

## How it works

- `deploy.yml` only runs for a push-built `main` release: the job's `if:`
  checks that the triggering `workflow_run` came from `build-release.yml`,
  from a `push`, on `main`, in this repository, and the job's
  `environment: production` (setup step 4) is separately restricted to the
  `main` branch.
- It downloads the `cortex-release-<sha>` artifact, verifies its checksums,
  joins the tailnet as an ephemeral, tagged (`tag:ci`) node, and pipes a
  `tar.gz` of the artifact over Tailscale SSH straight into
  `~/cortex-next/deploy-receive.sh` on `guardian@clawguard.tail618cfc.ts.net`.
- There is no deploy private key and no `authorized_keys` forced command.
  Tailscale SSH is on for this host, so SSH sessions over the tailnet are
  served by `tailscaled`, not `sshd`, and authenticated against the tailnet
  ACL from setup step 1 instead of a key file. Whoever can reach `guardian@`
  this way can run arbitrary code on the host — same as with the old forced
  command, since a leaked deploy key could always pipe anything it wanted
  into that one script. The `production` environment's branch rule is the
  access control that matters, not the SSH layer.
- `deploy-receive.sh` verifies the artifact, backs up the current release and
  database, swaps in the new binaries and web app, restarts `cortex-next` then
  `cortex-next-worker` (confirming each comes up before continuing), and
  rolls the binaries back automatically if the new release doesn't come up
  healthy.

## One-time setup

### 1. Tailscale ACL: let `tag:ci` reach `guardian` on the deploy host over SSH

The deploy host already carries `tag:deploy`. In the tailnet's ACL policy
(https://login.tailscale.com/admin/acls):

Add `tag:ci` to `tagOwners` (owned by whoever administers the tailnet):

```json
{
  "tagOwners": {
    "tag:ci": ["autogroup:admin"]
  }
}
```

If `tag:ci` already exists in `tagOwners` — `.github/workflows/host-db-migration.yml`
joins the tailnet as `tag:ci` too, using the same `TS_OAUTH_CLIENT_ID` /
`TS_OAUTH_SECRET` pair — merge into that existing entry instead of adding a
second `"tag:ci"` key.

Grant `tag:ci` reach to the deploy host on port 22 — with `grants`:

```json
{
  "grants": [
    { "src": ["tag:ci"], "dst": ["tag:deploy"], "ip": ["tcp:22"] }
  ]
}
```

or with the older `acls` form, if this tailnet doesn't use grants yet:

```json
{
  "acls": [
    { "action": "accept", "src": ["tag:ci"], "dst": ["tag:deploy:22"] }
  ]
}
```

And add an `ssh` rule — this is the one that actually authorizes the
connection, since the host serves SSH over the tailnet through `tailscaled`
(Tailscale SSH is on), not `sshd`:

```json
{
  "ssh": [
    {
      "action": "accept",
      "src": ["tag:ci"],
      "dst": ["tag:deploy"],
      "users": ["guardian"]
    }
  ]
}
```

Use `action: "accept"`, not `"check"` — `check` demands an interactive
browser re-auth that a CI runner can't perform. Use the `tag:deploy` tag in
`dst`, not the host's MagicDNS name: a name resolves to whichever device
holds it today, while the tag follows the device even if that changes.

If the policy has a catch-all such as
`{"action":"accept","src":["*"],"dst":["*:*"]}` (grants form:
`{"src":["*"],"dst":["*"],"ip":["*"]}`), do **not** just delete it: it is
usually the only rule that lets your own devices reach the server, and
Tailscale SSH still needs network access to port 22. Replace it with a rule
that covers people but not tagged machines:
`{"action":"accept","src":["autogroup:member"],"dst":["*:*"]}` (grants form:
`{"src":["autogroup:member"],"dst":["*"],"ip":["*"]}`). Add explicit rules for
any other tagged device that needs to reach something. Then confirm
`ssh guardian@clawguard.tail618cfc.ts.net` still works from your own machine
before going on to the next step.

`host-db-migration.yml` uses the same `tag:ci` credentials but connects as
`vars.DEPLOY_USER || 'deploy'`. The `ssh` rule above only allows `guardian`,
so if that workflow is still used it needs its own `ssh` rule for its user.

Confirm only the deploy host carries `tag:deploy` (Settings -> Machines,
filter by tag). `tag:ci`'s `ssh`/grant reach is scoped to `dst: ["tag:deploy"]`,
not to a specific device, so if a second machine ever picked up that tag,
`tag:ci` would gain SSH into it too without anyone touching the ACL.

### 2. Create a Tailscale OAuth client scoped to `tag:ci`

In the admin console under Settings -> OAuth clients, create a client with:

- Scopes: `auth_keys` (write) only. This client only needs to mint the
  ephemeral auth key the CI runner uses to join as `tag:ci` — it does not
  manage devices, so it does not need `devices:core`.
- Tags: `tag:ci`

Save the client ID and secret; they become `TS_OAUTH_CLIENT_ID` and
`TS_OAUTH_SECRET` below.

#### If this OAuth client ID/secret ever leaks

1. In the admin console under Settings -> OAuth clients, revoke the client
   immediately.
2. Under Settings -> Machines, filter by tag `tag:ci` and delete every device
   in that list — a leaked secret can mint new ephemeral `tag:ci` nodes for as
   long as any of them still exist, and revoking the client alone doesn't
   remove nodes it already created.
3. Re-check the ACL policy for any *other* `ssh` or grant rule whose `src`
   includes `tag:ci`, `autogroup:tagged`, or `*` — the rules in step 1 above
   are meant to be the only path in, but a leak is exactly the moment to
   confirm nothing broader was added later that would let a re-minted
   `tag:ci` node (or any tagged node) reach further than `guardian` on the
   deploy host.
4. Confirm only the deploy host carries `tag:deploy` (see above) — a
   `tag:ci` node's reach is bounded by that tag, so a stray device holding it
   would matter here too.
5. Create a new OAuth client (step 2 above) and update the
   `TS_OAUTH_CLIENT_ID` / `TS_OAUTH_SECRET` environment secrets (step 4
   below) with the new values.

### 3. Install `deploy-receive.sh` on the deploy host

On the deploy host, as `guardian`:

```bash
mkdir -p /home/guardian/cortex-next
```

From Git Bash in this repo, copy the script to the host:

```bash
scp scripts/deploy/deploy-receive.sh guardian@clawguard.tail618cfc.ts.net:cortex-next/
```

Then, on the host:

```bash
sed -i 's/\r$//' ~/cortex-next/deploy-receive.sh && chmod 755 ~/cortex-next/deploy-receive.sh
```

scp from a Windows machine drops the executable bit and (depending on Git's
`core.autocrlf` setting) can leave CRLF line endings, which make the script
fail to run on the host — `chmod +x` (or `755` as above) alone isn't enough;
always run both the `sed` and the `chmod` after copying the script over,
before the first deploy tries to run it. (`.gitattributes` forces
`scripts/deploy/*.sh` to LF in the repo itself, but that only controls what
`git checkout` writes — it doesn't touch a file `scp` already copied out.)

CI does not ship this script to the host — `deploy.yml` only invokes
`~/cortex-next/deploy-receive.sh` by its fixed path, it never uploads it.
Whenever `scripts/deploy/deploy-receive.sh` changes in the repo, repeat the
`scp` + `sed` + `chmod` steps above to reinstall it, or the host keeps running
the old version.

`deploy-receive.sh` restarts `cortex-next` and `cortex-next-worker` with
`systemctl --user`, which needs `guardian`'s user manager to be running even
outside an interactive login (Tailscale SSH sessions don't count as one).
That requires `loginctl enable-linger guardian` on the host — it is already
set, but if this is ever set up on a new host, do that first or the restarts
will fail with no `XDG_RUNTIME_DIR`.

There is no `authorized_keys` entry to add and no deploy key to generate.
Tailscale SSH authenticates the connection using the ACL from step 1, not a
key installed on this host. Whoever can reach `guardian@` this host over
Tailscale SSH can already run arbitrary code as `guardian` — a forced command
here would not meaningfully add to that, since a leaked, unrestricted deploy
key could always have piped anything into `deploy-receive.sh` anyway. The
`production` environment's branch rule (step 4) is the actual access control:
it limits who can even get a workflow run in a position to reach the host.

### 4. Create or edit the `production` environment and set secrets

In the repo's Settings -> Environments, create or edit an environment named
`production` with a deployment branch rule restricting it to `main`. Create it
explicitly rather than letting the first workflow run do it implicitly: a
job's first reference to an environment that doesn't exist yet auto-creates it
with no deployment branch rules at all, which would let `deploy.yml`'s
`environment: production` gate run for any branch until someone notices and
adds the rule by hand. The `deploy` job in `deploy.yml` declares
`environment: production`, so a run can only reach the deploy steps if its ref
is `main`.

Store the two secrets as environment secrets (not repository secrets), so
they're only available to jobs running under `production`. Run each of these
from a shell, reading the value from stdin so it never appears in shell
history:

```bash
gh secret set TS_OAUTH_CLIENT_ID --env production -R 1xmint/cortex
gh secret set TS_OAUTH_SECRET --env production -R 1xmint/cortex
```

`gh secret set NAME` with no value and no redirect prompts for the value
interactively (or reads stdin if it's piped) — use that so the values aren't
left in a file.

Once both are set, delete any local copy of the OAuth secret.

## The API refuses to start without a production auth config

None of this workflow's own steps write the API's `.env`/`EnvironmentFile` on
the deploy host — that file is set up once, by hand, alongside whichever
`cortex-server` unit is running there. Whatever sets it up must include four
variables, or the API silently starts in development mode (see below) instead
of the production auth mode a deploy host needs:

- **`CORTEX_ENV`** — set to `production` to tell the API it is running in
  production. `is_production_env` (`crates/api/src/lib.rs:331-343`) treats any
  of `HEYVERA_ENV`, `CORTEX_ENV`, `APP_ENV`, `RUST_ENV`, or `ENVIRONMENT` set to
  `production` or `prod` (case-insensitive) as production — `CORTEX_ENV` is
  only one of five names it checks (`HEYVERA_REQUIRE_AUTH=1` also forces
  production auth requirements, independently of this list). **If none of
  these are set, the API does not exit — it starts in development/local-auth
  mode**, treating every request as user `"local"`
  (`crates/api/src/main.rs:192-197`; `HeyVeraAuthMode::LocalDevelopment`).
  Nothing in this repository's production templates sets any of these
  implicitly: a deploy that forgets them runs as an unauthenticated local/dev
  instance without warning, not as a service that refuses to start.
- **`CLERK_SECRET_KEY`** — the Clerk backend API secret. Required once
  production is detected (any of the five vars above); startup exits non-zero
  if it is missing or blank while production is on.
- **`CLERK_ISSUER`** — the Clerk instance's issuer URL (a non-empty `https://`
  URL). Required in production; used to validate incoming JWTs.
- **`CLERK_AUTHORIZED_PARTY`** — the `https://` origin (no path, query, or
  fragment) that Clerk-issued tokens must have been authorized for. Required
  in production.

`deploy/deploy-production.sh` and `scripts/cortex-install-service.sh` both
write out a `.env` file with `CORTEX_ENV=production` and empty `CLERK_*`
lines already present — fill in the three Clerk values before starting the
service, or the server exits immediately with a message naming the missing
variable. See `deploy/cortex-api.service` for the systemd unit that sets
`CORTEX_ENV=production` for that install path.

**`CORTEX_AUTH_DISABLED` and `CORTEX_ALLOW_ANONYMOUS_WORKER` are refused in
production, not merely ignored.** `validate_auth_config_values`
(`crates/api/src/clerk.rs:74-92`) returns `Err("CORTEX_AUTH_DISABLED is
forbidden in HeyVera production")` when production is detected and
`CORTEX_AUTH_DISABLED` is set (`clerk.rs:74-76`), and
`Err("CORTEX_ALLOW_ANONYMOUS_WORKER is forbidden in HeyVera production")` when
production is detected and anonymous-worker access is requested
(`clerk.rs:78-82`). Either error propagates out of `load_heyvera_auth_config`
(`clerk.rs:119-136`) and `main.rs` exits the process with status 1 on it
(`crates/api/src/main.rs:184-190`) — so setting either flag on a production
deploy host does not quietly re-open the hole, it stops the server from
starting.

## Triggering a deploy manually

Deploys normally happen automatically after a successful `Build release` run
on `main`. To deploy a specific past build instead:

```bash
gh workflow run deploy.yml -R 1xmint/cortex -f build_run_id=<run-id-of-a-build-release-run>
```

Find the run ID with `gh run list -R 1xmint/cortex --workflow=build-release.yml`.

## A migrating deploy must be a human's choice

Migrations run in-process at server boot (`crates/api/src/db/mod.rs`), and
`deploy-receive.sh` can roll back the binaries and web app on a failed health
check but cannot roll back the schema. If an automatic deploy migrated the
database and then failed its health check, the database would be left ahead
of the rolled-back binary — a state nothing here can safely repair on its
own.

The guard is two independent checks, both of which have to pass for an
automatic deploy to proceed. Either one refusing requires `--allow-migration`
(or `deploy.yml`'s `allow_migration` input) to override.

### Check 1: `SCHEMA_VERSION` vs. the live database's version

The build carries the schema version its migration chain ends at
(`SCHEMA_VERSION` in `crates/api/src/db/mod.rs`, written to `out/SCHEMA` by
`build-release.yml`), and `deploy-receive.sh` compares that single number
against the live database's `schema_version` before swapping anything in.
This is a version-number comparison, not a diff of the schema itself, and by
itself it only catches a schema change if whoever made it also remembered to
bump `SCHEMA_VERSION`.

- **Artifact SCHEMA equal to the live version** — this check passes.
- **Artifact SCHEMA behind the live version** — refused outright, always, with
  or without `--allow-migration`: that binary would run against a newer
  schema than it knows.
- **Artifact SCHEMA ahead of the live version** — refused unless the script
  was invoked with `--allow-migration`. The automatic `workflow_run` deploy
  path never passes this.

### Check 2: schema fingerprint vs. the last deployed fingerprint

`SCHEMA_VERSION` is a number a migration author has to remember to move.
`crates/api/src/db/mod.rs` also runs unconditional bootstrap DDL
(`ensure_social_tables`, `ensure_social_posts_fts`, called from
`apply_migrations` on every boot, outside the numbered `migrate_vN` chain)
that can add or change a table or index without touching `SCHEMA_VERSION` at
all — check 1 above would wave that straight through. So a second, independent
check compares a fingerprint of the *entire* schema instead of a hand-maintained
number:

- `SCHEMA_FINGERPRINT` in `crates/api/src/db/mod.rs` and
  `ROUTING_SCHEMA_FINGERPRINT` in `crates/engine/src/store.rs` each hash every
  DDL statement `sqlite_master` reports for a fresh boot of `cortex.db` /
  `routing.db` respectively. `schema_fingerprint_matches_pinned_value` in each
  file fails the build if the live schema no longer matches the pinned
  constant — see that test's failure message for what to do (update the
  constant; this section is what to expect from the deploy guard once you do).
- `build-release.yml` reads both constants and writes them to
  `out/SCHEMA_FINGERPRINT` / `out/ROUTING_SCHEMA_FINGERPRINT` in the release
  artifact, the same way it writes `out/SCHEMA`.
- On the deploy host, `deploy-receive.sh` records the fingerprints of the last
  successfully deployed build in `$INSTALL_ROOT/SCHEMA_DEPLOYED` (written only
  after health and `/api/deploy-info` both confirm the new release is live —
  a deploy that fails and rolls back never touches this file). Before
  swapping anything in, it compares the artifact's fingerprints against that
  record:
  - **Fingerprints match the record** — this check passes, regardless of
    whether `SCHEMA_VERSION` moved.
  - **Fingerprints differ from the record** — refused unless invoked with
    `--allow-migration`, even if `SCHEMA_VERSION` (check 1) did not change.
    This is the case check 1 alone would miss.
  - **No `SCHEMA_DEPLOYED` record exists yet** — refused unless invoked with
    `--allow-migration`. This is fail-closed by design: the very first deploy
    of `deploy-receive.sh` after this check was added has nothing to compare
    against, so it needs one deliberate `--allow-migration` deploy to record
    a baseline. Every deploy after that compares normally.

Because check 2 is a whole-schema hash, not a version number, there is no
partial credit for "the version matched" — a fingerprint mismatch refuses the
deploy even when check 1 passed.

To deploy a build that migrates the schema (or whose fingerprint changed),
trigger `deploy.yml` by hand with `allow_migration` set:

```bash
gh workflow run deploy.yml -R 1xmint/cortex \
  -f build_run_id=<run-id-of-a-build-release-run> \
  -f allow_migration=true
```

Only do this once you've confirmed the migration is safe to run against
production — there's still no automatic schema rollback if the deploy fails
after migrating.

### What this guard does not cover

- **Boot-time statements that change data but not table shape are not seen.**
  The fingerprint hashes a freshly built database, which never contains old
  tables or rows. A `DROP TABLE IF EXISTS legacy_x`, or a `DELETE`/`UPDATE`
  added to `ensure_social_tables`, leaves the fingerprint unchanged and would
  auto-deploy. Review any boot-time SQL that is not `CREATE ... IF NOT EXISTS`
  by hand.
- **`routing.db`'s schema is fingerprinted (`ROUTING_SCHEMA_FINGERPRINT`,
  above) but has no version-number check**: it has no `SCHEMA_VERSION`-style
  counter of its own, and unlike `cortex.db` it is not backed up by
  `deploy-receive.sh`. A fingerprint change there is refused by check 2 the
  same as a `cortex.db` change would be.
- **`crates/context/src/index.rs`'s DDL is not fingerprinted or checked at
  all.** That index lives in its own SQLite file outside `cortex.db` and
  `routing.db`; a schema change there ships through this guard completely
  unguarded. Fingerprinting it would need its own pinned constant and test,
  the same way `SCHEMA_FINGERPRINT` and `ROUTING_SCHEMA_FINGERPRINT` work
  today — that has not been done.
- **The guard reads `cortex.db` at `$INSTALL_ROOT/data/cortex.db`** (`DB_PATH`
  in `deploy-receive.sh`). This has to be the same file the running
  `cortex-next` service actually opens (`cortex_db_path()` in
  `crates/api/src/state.rs`, which honors `$CORTEX_DB_PATH` with a fallback
  under `$CORTEX_WORKSPACE`). If the unit that starts `cortex-next` on the
  deploy host ever sets `CORTEX_DB_PATH` to something other than
  `$INSTALL_ROOT/data/cortex.db`, the guard would be checking a different
  database than the one the server migrates, defeating it silently — update
  `DB_PATH` in `deploy-receive.sh` to match if that ever happens. (The
  `deploy/cortex-api.service` example unit in this repo is not what runs on
  the Tailscale-deployed host and does not set `CORTEX_DB_PATH`; the actual
  `cortex-next`/`cortex-next-worker` user units live only on the deploy host,
  outside this repo — confirm their environment matches `DB_PATH` when
  setting up or auditing that host.)

A few more things worth knowing about this guard before you rely on it:

- **The `cortex-next` workspace is not defined in this repo, and the server
  refuses to boot without one that is a git repository.** `cortex-api` and
  `cortex-server` exit at startup unless `$CORTEX_WORKSPACE` (the working
  directory when unset) contains a `.git` (`require_workspace_repository` in
  `crates/api/src/state.rs`), because verification reads the delivered commit
  out of it. `deploy-production.sh`, `vps-deploy-cortex.sh`,
  `cortex-install-service.sh` and the Docker image create it themselves, but
  the `cortex-next` user unit and its environment live only on the deploy
  host, so nothing here does it for that unit. Once, on the deploy host, as
  the service user: `git -C "$CORTEX_WORKSPACE" init -q` (idempotent), using
  the path from that unit's environment.
- **Builds made before `SCHEMA_VERSION` checking landed have no `SCHEMA` file
  at all**, and builds made before this fingerprint check landed have no
  `SCHEMA_FINGERPRINT` / `ROUTING_SCHEMA_FINGERPRINT` files — `deploy-receive.sh`
  refuses any artifact missing any of them. To bring an older build back, use
  "Rolling back by hand" below instead.
- **A build whose `SCHEMA` is behind the live database's schema can never go
  through `deploy.yml`, with or without `allow_migration`.** That refusal is
  unconditional: an artifact behind the live schema is refused outright
  regardless of the flag, since that binary would run against a newer schema
  than it knows.
- **This check lives entirely in the deploy host's own copy of
  `deploy-receive.sh`**, which CI does not ship — `deploy.yml` invokes the
  copy already installed at `~/cortex-next/deploy-receive.sh` over SSH, not
  anything from the workflow run. If you haven't reinstalled it since this
  change landed (setup step 3, above), the host is still running an older
  script without the fingerprint check (or without any check at all) and this
  protection does not exist yet. Reinstall it before relying on this check in
  production — and expect the first deploy after reinstalling to need
  `--allow-migration` once, to record a `SCHEMA_DEPLOYED` baseline.

## Rolling back by hand

`deploy-receive.sh` already rolls back automatically if a new release fails
its health check. To roll back a release that came up healthy but is
otherwise bad:

1. SSH to the host over Tailscale SSH as `guardian` (any tailnet identity the
   ACL from setup step 1 allows to reach `tag:deploy` as `guardian` can do
   this — there is no separate restricted deploy key anymore):
   ```bash
   ssh guardian@clawguard.tail618cfc.ts.net
   ```
2. Pick a backup directory under `/home/guardian/cortex-next/backups/`.
   `backups/LATEST` already holds the release that was live before the most
   recent deploy, so it's what you want to undo that deploy. To go back
   further than that, use an older timestamped directory instead.
3. Stop the services, restore `bin/`, `COMMIT`, `SHA256SUMS`, and `www/` from
   that backup directory, then restart:
   ```bash
   cd /home/guardian/cortex-next
   systemctl --user stop cortex-next cortex-next-worker
   for item in bin COMMIT SHA256SUMS www; do
     rm -rf "$item"
     cp -a "backups/<timestamp>/$item" "$item"
   done
   chmod +x bin/*
   systemctl --user start cortex-next
   curl -sf http://localhost:3001/api/health
   systemctl --user start cortex-next-worker
   ```
4. The database is intentionally never restored automatically. Only restore
   `backups/<timestamp>/cortex.db` over `data/cortex.db` if you have confirmed
   the rolled-back binary expects that schema — restoring the wrong schema
   version can be worse than leaving the current database in place.
