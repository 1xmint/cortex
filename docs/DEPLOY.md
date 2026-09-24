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

Grant `tag:ci` reach to the deploy host on port 22 — with `grants`:

```json
{
  "grants": [
    { "src": ["tag:ci"], "dst": ["tag:deploy"], "ip": ["22"] }
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

If this tailnet has a catch-all rule such as
`{"action":"accept","src":["*"],"dst":["*:*"]}` (or an equivalent
default-allow grant), remove it — otherwise `tag:ci` already has access to
everything and the rules above restrict nothing.

### 2. Create a Tailscale OAuth client scoped to `tag:ci`

In the admin console under Settings -> OAuth clients, create a client with:

- Scopes: `auth_keys` (write) only. This client only needs to mint the
  ephemeral auth key the CI runner uses to join as `tag:ci` — it does not
  manage devices, so it does not need `devices:core`.
- Tags: `tag:ci`

Save the client ID and secret; they become `TS_OAUTH_CLIENT_ID` and
`TS_OAUTH_SECRET` below.

### 3. Install `deploy-receive.sh` on the deploy host

On the deploy host, as `guardian`:

```bash
mkdir -p /home/guardian/cortex-next
# Copy scripts/deploy/deploy-receive.sh from this repo to that path, then:
chmod 755 /home/guardian/cortex-next/deploy-receive.sh
```

scp from a Windows machine drops the executable bit — always `chmod +x` (or
`755` as above) after copying the script over, before the first deploy tries
to run it.

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

### 4. Create the `production` environment and set secrets

In the repo's Settings -> Environments, create an environment named
`production` with a deployment branch rule restricting it to `main`. The
`deploy` job in `deploy.yml` declares `environment: production`, so a run can
only reach the deploy steps if its ref is `main`.

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
