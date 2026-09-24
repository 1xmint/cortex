#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# deploy-receive.sh — receives a release artifact and swaps it into place.
#
# Runs on the deploy host over Tailscale SSH. deploy.yml connects as guardian
# and invokes this script by its fixed path directly:
#   ssh ... guardian@clawguard.tail618cfc.ts.net '~/cortex-next/deploy-receive.sh' < release.tar.gz
# Tailscale SSH (RunSSH) authenticates the connection via the tailnet ACL —
# see docs/DEPLOY.md — so there is no authorized_keys forced command and no
# deploy private key to leak; anyone who can reach this script can already
# run arbitrary code as guardian over the same SSH session, so this script's
# job is safety (verify, back up, roll back), not access control.
#
# Input: a gzip'd tar of a build-release.yml artifact, on stdin. Contains:
#   COMMIT, SHA256SUMS, bin/{cortex-server,cortex-worker,cortex-worker-key}, www/
#
# Install root is wherever this script actually lives (readlink -f "$0"'s
# directory), so the same script works if the install root is ever renamed,
# without a second place to update. In production that resolves to
# /home/guardian/cortex-next.
#
# Order of operations:
#   1. take the install-root lock (flock -n; refuse if another deploy holds it)
#   2. extract stdin into a fresh incoming/<ts>/ dir, with a size cap and a
#      check that nothing in the tar escapes that directory
#   3. verify SHA256SUMS and that COMMIT is a 40-hex-char sha
#   4. back up the DB and the current bin/, COMMIT, SHA256SUMS, www/
#   5. swap the new bin/, COMMIT, SHA256SUMS, www/ into place
#   6. restart cortex-next, poll /api/health, then restart cortex-next-worker and
#      confirm it is active
#   7. confirm /api/deploy-info reports the new commit
#
# On any failure at or after step 5, the previous bin/, COMMIT, SHA256SUMS and
# www/ are restored from the backup taken in step 4 and the service is
# restarted on the old build, then this script exits non-zero. The database
# is deliberately NOT rolled back on failure: the new binary may already have
# migrated it, and un-migrating a schema is not something this script can do
# safely on its own — that is a call for a human, made with
# backups/<ts>/cortex.db in hand.
# ---------------------------------------------------------------------------
set -euo pipefail
trap '' PIPE HUP

INSTALL_ROOT="$(cd -- "$(dirname -- "$(readlink -f -- "$0")")" && pwd)"
cd "$INSTALL_ROOT"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"

log() {
  printf '[deploy-receive] %s\n' "$1" >>"$INSTALL_ROOT/deploy.log" 2>/dev/null || true
  printf '[deploy-receive] %s\n' "$1" >&2 || true
}

fail() {
  log "ERROR: $1"
  exit 1
}

TS="$(date -u +%Y%m%dT%H%M%SZ)"
LOCK_FILE="$INSTALL_ROOT/.deploy.lock"
INCOMING_DIR="$INSTALL_ROOT/incoming/$TS"
BACKUP_DIR="$INSTALL_ROOT/backups/$TS"
MAX_ARTIFACT_BYTES=$((1024 * 1024 * 1024)) # 1 GiB — generous headroom over a real release
HEALTH_URL="http://localhost:3001/api/health"
DEPLOY_INFO_URL="http://localhost:3001/api/deploy-info"
HEALTH_TIMEOUT_SECS=60

log "invoked"

exec 9>"$LOCK_FILE"
if ! flock -n 9; then
  fail "install root is locked by another deploy; aborting"
fi
log "acquired lock on $LOCK_FILE"

RESTORE_NEEDED=0
cleanup() {
  set +e
  local status="$1"
  if [ "$status" -ne 0 ] && [ "$RESTORE_NEEDED" -eq 1 ]; then
    log "deploy failed after swap; restoring previous release from $BACKUP_DIR"
    local restore_failed=0
    for item in bin COMMIT SHA256SUMS www; do
      [ -e "$BACKUP_DIR/$item" ] || continue
      rm -rf -- "$INSTALL_ROOT/$item.restore" "$INSTALL_ROOT/$item.failed"
      cp -a -- "$BACKUP_DIR/$item" "$INSTALL_ROOT/$item.restore" || { log "ERROR: cannot copy $item from backup"; restore_failed=1; continue; }
      [ -e "$INSTALL_ROOT/$item" ] && mv -T -- "$INSTALL_ROOT/$item" "$INSTALL_ROOT/$item.failed"
      if mv -T -- "$INSTALL_ROOT/$item.restore" "$INSTALL_ROOT/$item"; then
        rm -rf -- "$INSTALL_ROOT/$item.failed"
      else
        [ -e "$INSTALL_ROOT/$item.failed" ] && mv -T -- "$INSTALL_ROOT/$item.failed" "$INSTALL_ROOT/$item" 2>/dev/null
        log "ERROR: restore of $item failed"; restore_failed=1
      fi
    done
    if [ -d "$INSTALL_ROOT/bin" ]; then
      chmod +x "$INSTALL_ROOT"/bin/* 2>/dev/null || true
    fi
    log "restarting cortex-next on restored release"
    systemctl --user restart cortex-next || true
    wait_for_health || log "WARNING: restored release did not become healthy either"
    systemctl --user restart cortex-next-worker || true
    if [ "$restore_failed" -eq 0 ]; then
      log "restore complete; exiting non-zero"
    else
      log "restore was PARTIAL (one or more items failed to restore); exiting non-zero"
    fi
  fi
  rm -rf -- "$INCOMING_DIR" 2>/dev/null || true
  exit "$status"
}
trap 'cleanup $?' EXIT

wait_for_health() {
  local waited=0
  while [ "$waited" -lt "$HEALTH_TIMEOUT_SECS" ]; do
    if curl -fsS --max-time 10 -o /dev/null -w '%{http_code}' "$HEALTH_URL" 2>/dev/null | grep -q '^200$'; then
      return 0
    fi
    sleep 2
    waited=$((waited + 2))
  done
  return 1
}

# --- 1. extract stdin into a fresh, size-capped, path-safe directory --------
mkdir -p "$INCOMING_DIR"
ARTIFACT_TAR="$INCOMING_DIR/artifact.tar.gz"

log "reading artifact from stdin (cap: $MAX_ARTIFACT_BYTES bytes)"
head -c "$MAX_ARTIFACT_BYTES" >"$ARTIFACT_TAR"
if [ ! -s "$ARTIFACT_TAR" ]; then
  fail "no artifact data received on stdin"
fi
# If stdin still had bytes left after the cap, the artifact was truncated (or
# is simply too large); either way, refuse it rather than deploy a partial
# build.
if IFS= read -r -n 1 _extra 2>/dev/null <&0; then
  fail "artifact exceeds the $MAX_ARTIFACT_BYTES byte cap"
fi

log "checking artifact for unsafe paths"
if tar tzf "$ARTIFACT_TAR" | grep -E '(^|/)\.\.(/|$)|^/'; then
  fail "artifact contains an absolute path or a '..' path segment"
fi

log "extracting artifact to $INCOMING_DIR"
# No -P/--absolute-names: this is GNU tar's default, and it's what strips any
# leading '/' from an entry rather than honoring it. Combined with the
# '..'-segment and leading-'/' check above, nothing in the archive can land
# outside $INCOMING_DIR.
tar xzf "$ARTIFACT_TAR" -C "$INCOMING_DIR"
rm -f "$ARTIFACT_TAR"

for required in COMMIT SHA256SUMS bin www; do
  [ -e "$INCOMING_DIR/$required" ] || fail "artifact is missing $required"
done

# --- 2. verify integrity ----------------------------------------------------
log "verifying SHA256SUMS"
(cd "$INCOMING_DIR" && sha256sum -c SHA256SUMS) || fail "SHA256SUMS verification failed"

NEW_COMMIT="$(tr -d '[:space:]' <"$INCOMING_DIR/COMMIT")"
if ! printf '%s' "$NEW_COMMIT" | grep -Eq '^[0-9a-f]{40}$'; then
  fail "COMMIT is not a 40-character hex sha: '$NEW_COMMIT'"
fi
log "artifact verified for commit $NEW_COMMIT"

# --- 3. back up the current release and the database ------------------------
mkdir -p "$BACKUP_DIR"
log "backing up current release to $BACKUP_DIR"
for item in bin COMMIT SHA256SUMS www; do
  if [ -e "$INSTALL_ROOT/$item" ]; then
    cp -a -- "$INSTALL_ROOT/$item" "$BACKUP_DIR/$item"
  fi
done

if [ -f "$INSTALL_ROOT/data/cortex.db" ]; then
  log "backing up database with sqlite3 .backup"
  sqlite3 "$INSTALL_ROOT/data/cortex.db" ".backup '$BACKUP_DIR/cortex.db'" \
    || fail "database backup failed; refusing to deploy over an unbacked-up database"
else
  log "no existing database at data/cortex.db; skipping database backup"
fi

# --- 4. swap the new release into place -------------------------------------
chmod +x "$INCOMING_DIR"/bin/* 2>/dev/null || true

log "swapping in new release"
RESTORE_NEEDED=1
for item in bin COMMIT SHA256SUMS www; do
  rm -rf -- "${INSTALL_ROOT:?}/$item"
  mv -- "$INCOMING_DIR/$item" "$INSTALL_ROOT/$item"
done

# --- 5. restart, health-check, restart worker -------------------------------
log "restarting cortex-next"
systemctl --user restart cortex-next
CORTEX_NEXT_N0="$(systemctl --user show -p NRestarts --value cortex-next)"
log "waiting up to ${HEALTH_TIMEOUT_SECS}s for $HEALTH_URL"
if ! wait_for_health; then
  fail "cortex-next did not become healthy within ${HEALTH_TIMEOUT_SECS}s"
fi
# Health answered once; wait past the unit's restart delay so a crash right
# after the first 200 shows up as a restart or a non-active state.
sleep 12
CORTEX_NEXT_N1="$(systemctl --user show -p NRestarts --value cortex-next)"
CORTEX_NEXT_ACTIVE="$(systemctl --user show -p ActiveState --value cortex-next)"
if [ "$CORTEX_NEXT_ACTIVE" != "active" ] || [ "$CORTEX_NEXT_N1" != "$CORTEX_NEXT_N0" ]; then
  fail "cortex-next is not stable after restart (ActiveState=$CORTEX_NEXT_ACTIVE, NRestarts $CORTEX_NEXT_N0 -> $CORTEX_NEXT_N1)"
fi
log "cortex-next is healthy and stable"

log "restarting cortex-next-worker"
systemctl --user restart cortex-next-worker
n0="$(systemctl --user show -p NRestarts --value cortex-next-worker)"
sleep 12
n1="$(systemctl --user show -p NRestarts --value cortex-next-worker)"
worker_active="$(systemctl --user show -p ActiveState --value cortex-next-worker)"
if [ "$worker_active" != "active" ] || [ "$n1" != "$n0" ]; then
  fail "cortex-next-worker is not stable after restart (ActiveState=$worker_active, NRestarts $n0 -> $n1)"
fi
log "cortex-next-worker is stable"

# --- 6. confirm the deployed commit -----------------------------------------
DEPLOY_INFO="$(curl -fsS --max-time 10 "$DEPLOY_INFO_URL")" || fail "could not reach $DEPLOY_INFO_URL after restart"
LIVE_COMMIT="$(printf '%s' "$DEPLOY_INFO" | grep -o '"commit"[[:space:]]*:[[:space:]]*"[0-9a-f]\{40\}"' | grep -o '[0-9a-f]\{40\}' || true)"
if [ "$LIVE_COMMIT" != "$NEW_COMMIT" ]; then
  fail "deploy-info commit '$LIVE_COMMIT' does not match deployed commit '$NEW_COMMIT'"
fi
log "confirmed deploy-info reports commit $NEW_COMMIT"

# Success: nothing to restore.
RESTORE_NEEDED=0

# --- 7. prune old backups, keeping the newest 10 (and never LATEST's target) -
ln -sfn "$BACKUP_DIR" "$INSTALL_ROOT/backups/LATEST"
LATEST_TARGET="$(readlink -f -- "$INSTALL_ROOT/backups/LATEST" || true)"

log "pruning backups, keeping the newest 10"
mapfile -t OLD_BACKUPS < <(find "$INSTALL_ROOT/backups" -mindepth 1 -maxdepth 1 -type d | sort -r | tail -n +11)
for dir in "${OLD_BACKUPS[@]:-}"; do
  [ -n "$dir" ] || continue
  if [ -n "$LATEST_TARGET" ] && [ "$(readlink -f -- "$dir")" = "$LATEST_TARGET" ]; then
    log "keeping $dir (LATEST target)"
    continue
  fi
  log "removing old backup $dir"
  rm -rf -- "$dir"
done

log "deploy of commit $NEW_COMMIT complete"
