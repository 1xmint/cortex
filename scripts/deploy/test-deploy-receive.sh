#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# test-deploy-receive.sh — fixture harness for scripts/deploy/deploy-receive.sh.
#
# Runs deploy-receive.sh against a throwaway install root, with stub
# `sqlite3`, `flock`, `systemctl` and `curl` on PATH so the script can be
# exercised without a real systemd user session, a real SQLite database, or a
# real HTTP server. No network access, no root, no real service manager
# required — this is meant to run on a developer's own machine (including
# Git Bash on Windows) or in CI.
#
# The fixture "database" at data/cortex.db is not a real SQLite file: the
# stub `sqlite3` just treats its contents as the live schema version, which
# is all deploy-receive.sh's migration-refusal check needs from it.
#
# Scenarios covered (see run at the bottom):
#   a. artifact SCHEMA above the fixture DB's version, no flag -> refused,
#      nothing swapped.
#   b. same, with --allow-migration -> proceeds.
#   c. artifact SCHEMA equal to the fixture DB's version -> proceeds.
#   d. equal SCHEMA, but the health check fails after the swap -> the old
#      bin/ and COMMIT come back and the script exits non-zero.
#
# Usage: bash scripts/deploy/test-deploy-receive.sh
# ---------------------------------------------------------------------------
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "$(readlink -f -- "$0")")/../.." && pwd)"
SCRIPT_SRC="$REPO_ROOT/scripts/deploy/deploy-receive.sh"

TMPROOT="$(mktemp -d)"
trap 'rm -rf -- "$TMPROOT"' EXIT

STUBBIN="$TMPROOT/stubbin"
INSTALL_ROOT="$TMPROOT/install"
FIXTURE_DIR="$TMPROOT/fixture"
mkdir -p "$STUBBIN" "$FIXTURE_DIR"

OLD_COMMIT="111111111111111111111111111111111111111a"
PASS=0
FAIL=0

note() { printf '\n=== %s ===\n' "$1"; }
ok()   { PASS=$((PASS + 1)); printf 'PASS: %s\n' "$1"; }
bad()  { FAIL=$((FAIL + 1)); printf 'FAIL: %s\n' "$1"; }

# --- stub tools --------------------------------------------------------------

cat >"$STUBBIN/flock" <<'EOF'
#!/usr/bin/env bash
# Fixture stub: the harness runs one deploy at a time, so the lock always
# succeeds; real inter-process locking isn't what this harness exercises.
exit 0
EOF

cat >"$STUBBIN/systemctl" <<'EOF'
#!/usr/bin/env bash
# Fixture stub for `systemctl --user ...`. Every unit is reported active and
# with a stable (unchanging) restart count, so deploy-receive.sh's
# post-restart stability check always passes; only the curl stub's health
# response is varied between scenarios.
case "$*" in
  *"show -p NRestarts"*)   echo 0 ;;
  *"show -p ActiveState"*) echo active ;;
  *) exit 0 ;;
esac
EOF

cat >"$STUBBIN/sqlite3" <<'EOF'
#!/usr/bin/env bash
# Fixture stub for `sqlite3 <db> <sql>`. The fixture "database" is a plain
# text file whose content is the live schema version. A `.backup` call just
# copies it to the requested backup path; a schema_version SELECT cats it.
db="$1"
sql="${2:-}"
case "$sql" in
  *".backup"*)
    target="$(printf '%s' "$sql" | sed -n "s/.*\.backup '\(.*\)'.*/\1/p")"
    [ -n "$target" ] && cp -- "$db" "$target"
    ;;
  *"schema_version"*)
    if [ -s "$db" ]; then cat -- "$db"; else echo 0; fi
    ;;
  *)
    echo 0
    ;;
esac
exit 0
EOF

cat >"$STUBBIN/curl" <<EOF
#!/usr/bin/env bash
# Fixture stub for curl. Recognizes deploy-receive.sh's two calls:
#   health poll:        ...-o /dev/null -w '%{http_code}' <.../api/health>
#   deploy-info fetch:  ...-fsS --max-time 10 <.../api/deploy-info>
# FIXTURE_DIR/health_fail present -> health poll always fails (simulates a
# health check that never recovers, forcing deploy-receive.sh's rollback).
# FIXTURE_DIR/expected_commit holds the commit deploy-info should report.
url="\${@: -1}"
case "\$url" in
  *"/api/health")
    if [ -f "$FIXTURE_DIR/health_fail" ]; then
      exit 7
    fi
    echo 200
    exit 0
    ;;
  *"/api/deploy-info")
    commit="\$(cat "$FIXTURE_DIR/expected_commit" 2>/dev/null || true)"
    printf '{"commit":"%s"}' "\$commit"
    exit 0
    ;;
  *)
    exit 7
    ;;
esac
EOF

chmod +x "$STUBBIN"/*

# --- fixture helpers -----------------------------------------------------

setup_install_root() {
  local live_schema="$1"
  rm -rf "$INSTALL_ROOT"
  mkdir -p "$INSTALL_ROOT/bin" "$INSTALL_ROOT/www" "$INSTALL_ROOT/data"
  echo "old binary $OLD_COMMIT" >"$INSTALL_ROOT/bin/cortex-server"
  echo "old binary $OLD_COMMIT" >"$INSTALL_ROOT/bin/cortex-worker"
  echo "old binary $OLD_COMMIT" >"$INSTALL_ROOT/bin/cortex-worker-key"
  echo "old index" >"$INSTALL_ROOT/www/index.html"
  printf '%s' "$OLD_COMMIT" >"$INSTALL_ROOT/COMMIT"
  (cd "$INSTALL_ROOT" && sha256sum bin/* COMMIT >SHA256SUMS)
  printf '%s' "$live_schema" >"$INSTALL_ROOT/data/cortex.db"
  cp -- "$SCRIPT_SRC" "$INSTALL_ROOT/deploy-receive.sh"
  chmod +x "$INSTALL_ROOT/deploy-receive.sh"
  rm -f "$FIXTURE_DIR/health_fail"
}

make_artifact() {
  local schema="$1" commit="$2" outfile="$3"
  local work
  work="$(mktemp -d)"
  mkdir -p "$work/bin" "$work/www"
  echo "new binary $commit" >"$work/bin/cortex-server"
  echo "new binary $commit" >"$work/bin/cortex-worker"
  echo "new binary $commit" >"$work/bin/cortex-worker-key"
  echo "new index" >"$work/www/index.html"
  printf '%s' "$commit" >"$work/COMMIT"
  printf '%s' "$schema" >"$work/SCHEMA"
  (cd "$work" && sha256sum bin/* COMMIT SCHEMA >SHA256SUMS)
  tar czf "$outfile" -C "$work" COMMIT SCHEMA SHA256SUMS bin www
  rm -rf "$work"
}

# Runs deploy-receive.sh with the artifact on stdin; prints its exit code and
# leaves combined stdout+stderr in $TMPROOT/out.log.
run_deploy() {
  local artifact="$1"
  shift
  local rc=0
  PATH="$STUBBIN:$PATH" \
    HEALTH_TIMEOUT_SECS=6 HEALTH_POLL_INTERVAL_SECS=1 POST_RESTART_SETTLE_SECS=1 \
    FIXTURE_DIR="$FIXTURE_DIR" \
    bash "$INSTALL_ROOT/deploy-receive.sh" "$@" <"$artifact" >"$TMPROOT/out.log" 2>&1 || rc=$?
  echo "$rc"
}

assert_contains() {
  local needle="$1" label="$2"
  if grep -qF -- "$needle" "$TMPROOT/out.log"; then
    ok "$label"
  else
    bad "$label (expected output to contain: $needle)"
    sed 's/^/    log> /' "$TMPROOT/out.log"
  fi
}

assert_eq() {
  local actual="$1" expected="$2" label="$3"
  if [ "$actual" = "$expected" ]; then
    ok "$label"
  else
    bad "$label (got '$actual', want '$expected')"
  fi
}

# --- scenario a: migrating deploy, no flag -> refused -----------------------
note "a. artifact SCHEMA (72) > live (71), no flag -> refusal"
setup_install_root 71
ARTIFACT_A="$TMPROOT/a.tgz"
make_artifact 72 "222222222222222222222222222222222222222a" "$ARTIFACT_A"
printf '%s' "222222222222222222222222222222222222222a" >"$FIXTURE_DIR/expected_commit"
RC_A="$(run_deploy "$ARTIFACT_A")"
assert_eq "$RC_A" "1" "a. exit code is non-zero"
assert_contains "automatic deploys refuse migrations" "a. refusal message printed"
assert_eq "$(cat "$INSTALL_ROOT/COMMIT")" "$OLD_COMMIT" "a. COMMIT untouched"
assert_eq "$(cat "$INSTALL_ROOT/bin/cortex-server")" "old binary $OLD_COMMIT" "a. bin/ untouched"

# --- scenario b: migrating deploy, --allow-migration -> proceeds ------------
note "b. same, with --allow-migration -> proceeds"
setup_install_root 71
ARTIFACT_B="$TMPROOT/b.tgz"
NEW_COMMIT_B="333333333333333333333333333333333333333b"
make_artifact 72 "$NEW_COMMIT_B" "$ARTIFACT_B"
printf '%s' "$NEW_COMMIT_B" >"$FIXTURE_DIR/expected_commit"
RC_B="$(run_deploy "$ARTIFACT_B" --allow-migration)"
assert_eq "$RC_B" "0" "b. exit code is 0"
assert_eq "$(cat "$INSTALL_ROOT/COMMIT")" "$NEW_COMMIT_B" "b. COMMIT swapped in"

# --- scenario c: SCHEMA equal -> proceeds without any flag ------------------
note "c. artifact SCHEMA equal to live -> proceeds"
setup_install_root 71
ARTIFACT_C="$TMPROOT/c.tgz"
NEW_COMMIT_C="444444444444444444444444444444444444444c"
make_artifact 71 "$NEW_COMMIT_C" "$ARTIFACT_C"
printf '%s' "$NEW_COMMIT_C" >"$FIXTURE_DIR/expected_commit"
RC_C="$(run_deploy "$ARTIFACT_C")"
assert_eq "$RC_C" "0" "c. exit code is 0"
assert_eq "$(cat "$INSTALL_ROOT/COMMIT")" "$NEW_COMMIT_C" "c. COMMIT swapped in"

# --- scenario d: rollback path — health check fails after the swap ---------
note "d. health check fails after swap -> rollback"
setup_install_root 71
ARTIFACT_D="$TMPROOT/d.tgz"
NEW_COMMIT_D="555555555555555555555555555555555555555d"
make_artifact 71 "$NEW_COMMIT_D" "$ARTIFACT_D"
printf '%s' "$NEW_COMMIT_D" >"$FIXTURE_DIR/expected_commit"
: >"$FIXTURE_DIR/health_fail"
RC_D="$(run_deploy "$ARTIFACT_D")"
assert_eq "$RC_D" "1" "d. exit code is non-zero"
assert_contains "did not become healthy" "d. health-failure message printed"
assert_eq "$(cat "$INSTALL_ROOT/COMMIT")" "$OLD_COMMIT" "d. COMMIT rolled back to old release"
assert_eq "$(cat "$INSTALL_ROOT/bin/cortex-server")" "old binary $OLD_COMMIT" "d. bin/ rolled back to old release"

note "summary"
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
