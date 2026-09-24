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

## Triggering a deploy manually

Deploys normally happen automatically after a successful `Build release` run
on `main`. To deploy a specific past build instead:

```bash
gh workflow run deploy.yml -R 1xmint/cortex -f build_run_id=<run-id-of-a-build-release-run>
```

Find the run ID with `gh run list -R 1xmint/cortex --workflow=build-release.yml`.

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
