# Automatic deploy to the production host

Status: canonical

When a release build finishes on `main` (`.github/workflows/build-release.yml`),
`.github/workflows/deploy.yml` ships the resulting artifact to the production
host over Tailscale and restarts it. Nothing runs until the four secrets below
are set — until then the workflow exits quietly with a `::notice::`.

This document is the one-time owner setup for those secrets, plus how to
trigger a deploy by hand and how to roll back.

## How it works

- `deploy.yml` downloads the `cortex-release-<sha>` artifact, joins the
  tailnet as an ephemeral, tagged (`tag:ci`) node, and pipes a `tar.gz` of the
  artifact over SSH to `guardian@clawguard.tail618cfc.ts.net`.
- The only thing the deploy key can do on that host is run
  `scripts/deploy/deploy-receive.sh` — it is installed as an SSH forced
  command, so a leaked deploy key still cannot run arbitrary commands there.
- `deploy-receive.sh` verifies the artifact, backs up the current release and
  database, swaps in the new binaries and web app, restarts `cortex-next` then
  `cortex-next-worker`, and rolls the binaries back automatically if the new
  release doesn't come up healthy.

## One-time setup

### 1. Tailscale ACL: let `tag:ci` reach the host on port 22

In the tailnet's ACL policy (https://login.tailscale.com/admin/acls), add
`tag:ci` to `tagOwners` (owned by whoever administers the tailnet) and grant it
access to the deploy host on port 22 only, for example:

```json
{
  "tagOwners": {
    "tag:ci": ["autogroup:admin"]
  },
  "acls": [
    {
      "action": "accept",
      "src": ["tag:ci"],
      "dst": ["clawguard.tail618cfc.ts.net:22"]
    }
  ]
}
```

Adjust `dst` to the host's actual tailnet IP or hostname if it differs from
the name used in this doc.

### 2. Create a Tailscale OAuth client scoped to `tag:ci`

In the admin console under Settings -> OAuth clients, create a client with:

- Scopes: `devices:core` (write), `auth_keys` (write)
- Tags: `tag:ci`

Save the client ID and secret; they become `TS_OAUTH_CLIENT_ID` and
`TS_OAUTH_SECRET` below.

### 3. Generate a dedicated deploy key

Do not reuse any other key for this. From a machine you trust:

```bash
ssh-keygen -t ed25519 -f ./cortex-deploy-key -N "" -C "cortex-deploy@github-actions"
```

This produces `cortex-deploy-key` (private, becomes `DEPLOY_SSH_KEY`) and
`cortex-deploy-key.pub` (goes on the server).

### 4. Install `deploy-receive.sh` on the server and lock the key to it

On the deploy host, as `guardian`:

```bash
mkdir -p /home/guardian/cortex-next
# Copy scripts/deploy/deploy-receive.sh from this repo to that path, then:
chmod 700 /home/guardian/cortex-next/deploy-receive.sh
```

scp from a Windows machine drops the executable bit — always `chmod +x` (or
`700` as above) after copying the script over, before the first deploy tries
to run it.

Add the public key from step 3 to `/home/guardian/.ssh/authorized_keys` on a
single line, restricted to the deploy script:

```
command="/home/guardian/cortex-next/deploy-receive.sh",restrict ssh-ed25519 AAAA... cortex-deploy@github-actions
```

`restrict` turns off port/agent/X11 forwarding and PTY allocation, so this key
can only pipe a tarball into `deploy-receive.sh` and read its output — nothing
else.

### 5. Capture the host key for `known_hosts`

From a machine already on the tailnet:

```bash
ssh-keyscan -H clawguard.tail618cfc.ts.net > cortex-known-hosts
```

This file's contents become `DEPLOY_KNOWN_HOSTS` below.

### 6. Set the four repository secrets

Run each of these from a shell, reading the value from a file or stdin so it
never appears on the command line or in shell history:

```bash
gh secret set TS_OAUTH_CLIENT_ID -R 1xmint/cortex
gh secret set TS_OAUTH_SECRET -R 1xmint/cortex
gh secret set DEPLOY_SSH_KEY -R 1xmint/cortex < ./cortex-deploy-key
gh secret set DEPLOY_KNOWN_HOSTS -R 1xmint/cortex < ./cortex-known-hosts
```

`gh secret set NAME` with no value and no redirect prompts for the value
interactively (or reads stdin if it's piped) — use that for
`TS_OAUTH_CLIENT_ID` and `TS_OAUTH_SECRET` so they aren't left in a file.

Once all four are set, delete the local copies of the private key and the
OAuth secret.

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

1. SSH to the host (as someone with a normal, non-restricted key — the deploy
   key can only run `deploy-receive.sh`):
   ```bash
   ssh guardian@clawguard.tail618cfc.ts.net
   ```
2. Pick a backup directory under `/home/guardian/cortex-next/backups/`
   (`backups/LATEST` points at the newest one; the one before it is the
   previous release).
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
