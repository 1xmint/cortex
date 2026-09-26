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
#   e. artifact SCHEMA (71) behind the fixture DB's version (72), no flag ->
#      refused.
#   f. same as (e), with --allow-migration -> still refused (behind-live is
#      never allowed).
#   g. no existing DB file, no flag -> refused as a 0->N migration.
#   h. DB file present but the sqlite3 stub exits non-zero -> refused with a
#      "could not read" message, both with and without --allow-migration.
#   i. sqlite3 stub prints a header line before the version number (as a real
#      sqlite3 does when ~/.sqliterc sets headers on) -> refused, not treated
#      as schema 0.
#   j. artifact SCHEMA file missing, or containing non-numeric text -> refused.
#
# Each refusal scenario asserts COMMIT and bin/ are left untouched.
#
# Set SCRIPT to point the harness at a different deploy-receive.sh (e.g. to
# confirm an old version of the script fails scenario (i)):
#   SCRIPT=/path/to/old/deploy-receive.sh bash scripts/deploy/test-deploy-receive.sh
#
# Usage: bash scripts/deploy/test-deploy-receive.sh
# ---------------------------------------------------------------------------
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "$(readlink -f -- "$0")")/../.." && pwd)"
SCRIPT_SRC="${SCRIPT:-$REPO_ROOT/scripts/deploy/deploy-receive.sh}"

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
#
# SQLITE_MODE switches behaviour per scenario:
#   normal (default) - behaves as described above.
#   fail             - exits non-zero, simulating a read failure (permission
#                       denied, corrupt db, sqlite3 missing, etc).
#   header           - prints a header line before the version number, as a
#                       real sqlite3 does when ~/.sqliterc turns on headers.
mode="${SQLITE_MODE:-normal}"
if [ "$mode" = "fail" ]; then
  exit 1
fi
# deploy-receive.sh's read-only query passes flags (-batch -noheader
# -readonly -init /dev/null) before the db path; its .backup call doesn't.
# Either way, the db path and the SQL are the last two positional args; skip
# known flags and -init's value argument to find them.
args=()
skip_value_for_next=0
for a in "$@"; do
  if [ "$skip_value_for_next" -eq 1 ]; then
    skip_value_for_next=0
    continue
  fi
  case "$a" in
    -init) skip_value_for_next=1 ;;
    -*) ;;
    *) args+=("$a") ;;
  esac
done
db="${args[0]:-}"
sql="${args[1]:-}"
case "$sql" in
  *".backup"*)
    target="$(printf '%s' "$sql" | sed -n "s/.*\.backup '\(.*\)'.*/\1/p")"
    [ -n "$target" ] && cp -- "$db" "$target"
    ;;
  *"schema_version"*)
    [ "$mode" = "header" ] && echo "COALESCE(MAX(version), 0)"
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

# Like make_artifact, but the tar has no SCHEMA file at all (scenario j).
make_artifact_no_schema() {
  local commit="$1" outfile="$2"
  local work
  work="$(mktemp -d)"
  mkdir -p "$work/bin" "$work/www"
  echo "new binary $commit" >"$work/bin/cortex-server"
  echo "new binary $commit" >"$work/bin/cortex-worker"
  echo "new binary $commit" >"$work/bin/cortex-worker-key"
  echo "new index" >"$work/www/index.html"
  printf '%s' "$commit" >"$work/COMMIT"
  (cd "$work" && sha256sum bin/* COMMIT >SHA256SUMS)
  tar czf "$outfile" -C "$work" COMMIT SHA256SUMS bin www
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
    SQLITE_MODE="${SQLITE_MODE:-normal}" \
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

# --- scenario e: artifact SCHEMA behind live, no flag -> refused ------------
note "e. artifact SCHEMA (71) behind live (72), no flag -> refusal"
setup_install_root 72
ARTIFACT_E="$TMPROOT/e.tgz"
make_artifact 71 "666666666666666666666666666666666666666e" "$ARTIFACT_E"
printf '%s' "666666666666666666666666666666666666666e" >"$FIXTURE_DIR/expected_commit"
RC_E="$(run_deploy "$ARTIFACT_E")"
assert_eq "$RC_E" "1" "e. exit code is non-zero"
assert_contains "is behind the live database's schema" "e. behind-live refusal message printed"
assert_eq "$(cat "$INSTALL_ROOT/COMMIT")" "$OLD_COMMIT" "e. COMMIT untouched"
assert_eq "$(cat "$INSTALL_ROOT/bin/cortex-server")" "old binary $OLD_COMMIT" "e. bin/ untouched"

# --- scenario f: same, with --allow-migration -> still refused --------------
note "f. same as (e), with --allow-migration -> still refused"
setup_install_root 72
ARTIFACT_F="$TMPROOT/f.tgz"
make_artifact 71 "777777777777777777777777777777777777777f" "$ARTIFACT_F"
printf '%s' "777777777777777777777777777777777777777f" >"$FIXTURE_DIR/expected_commit"
RC_F="$(run_deploy "$ARTIFACT_F" --allow-migration)"
assert_eq "$RC_F" "1" "f. exit code is non-zero"
assert_contains "is behind the live database's schema" "f. behind-live refusal message printed even with --allow-migration"
assert_eq "$(cat "$INSTALL_ROOT/COMMIT")" "$OLD_COMMIT" "f. COMMIT untouched"
assert_eq "$(cat "$INSTALL_ROOT/bin/cortex-server")" "old binary $OLD_COMMIT" "f. bin/ untouched"

# --- scenario g: no existing DB, no flag -> refused as a 0->N migration -----
note "g. DB absent, no flag -> refused as 0->N"
setup_install_root 71
rm -f "$INSTALL_ROOT/data/cortex.db"
ARTIFACT_G="$TMPROOT/g.tgz"
make_artifact 5 "8888888888888888888888888888888888888888" "$ARTIFACT_G"
printf '%s' "8888888888888888888888888888888888888888" >"$FIXTURE_DIR/expected_commit"
RC_G="$(run_deploy "$ARTIFACT_G")"
assert_eq "$RC_G" "1" "g. exit code is non-zero"
assert_contains "migrate schema 0->5" "g. refusal reports 0->5 migration"
assert_eq "$(cat "$INSTALL_ROOT/COMMIT")" "$OLD_COMMIT" "g. COMMIT untouched"
assert_eq "$(cat "$INSTALL_ROOT/bin/cortex-server")" "old binary $OLD_COMMIT" "g. bin/ untouched"

# --- scenario h: DB present but sqlite3 stub exits 1 -> refused -------------
note "h. sqlite3 read fails (exit 1), no flag -> refused"
setup_install_root 71
ARTIFACT_H="$TMPROOT/h.tgz"
make_artifact 72 "9999999999999999999999999999999999999999" "$ARTIFACT_H"
printf '%s' "9999999999999999999999999999999999999999" >"$FIXTURE_DIR/expected_commit"
RC_H1="$(SQLITE_MODE=fail run_deploy "$ARTIFACT_H")"
assert_eq "$RC_H1" "1" "h. exit code is non-zero (no flag)"
assert_contains "could not read schema_version from" "h. could-not-read message printed (no flag)"
assert_eq "$(cat "$INSTALL_ROOT/COMMIT")" "$OLD_COMMIT" "h. COMMIT untouched (no flag)"
assert_eq "$(cat "$INSTALL_ROOT/bin/cortex-server")" "old binary $OLD_COMMIT" "h. bin/ untouched (no flag)"

note "h. sqlite3 read fails (exit 1), with --allow-migration -> still refused"
setup_install_root 71
RC_H2="$(SQLITE_MODE=fail run_deploy "$ARTIFACT_H" --allow-migration)"
assert_eq "$RC_H2" "1" "h. exit code is non-zero (--allow-migration)"
assert_contains "could not read schema_version from" "h. could-not-read message printed (--allow-migration)"
assert_eq "$(cat "$INSTALL_ROOT/COMMIT")" "$OLD_COMMIT" "h. COMMIT untouched (--allow-migration)"
assert_eq "$(cat "$INSTALL_ROOT/bin/cortex-server")" "old binary $OLD_COMMIT" "h. bin/ untouched (--allow-migration)"

# --- scenario i: sqlite3 prints a header line before the version number -----
note "i. sqlite3 stub prints a header line -> refused, not treated as schema 0"
setup_install_root 71
ARTIFACT_I="$TMPROOT/i.tgz"
make_artifact 72 "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" "$ARTIFACT_I"
printf '%s' "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" >"$FIXTURE_DIR/expected_commit"
RC_I="$(SQLITE_MODE=header run_deploy "$ARTIFACT_I")"
assert_eq "$RC_I" "1" "i. exit code is non-zero"
assert_contains "could not read live schema version from" "i. could-not-read-live-schema message printed"
assert_eq "$(cat "$INSTALL_ROOT/COMMIT")" "$OLD_COMMIT" "i. COMMIT untouched"
assert_eq "$(cat "$INSTALL_ROOT/bin/cortex-server")" "old binary $OLD_COMMIT" "i. bin/ untouched"

# --- scenario j: SCHEMA file missing, or non-numeric -----------------------
note "j. SCHEMA file missing -> refused"
setup_install_root 71
ARTIFACT_J1="$TMPROOT/j1.tgz"
make_artifact_no_schema "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" "$ARTIFACT_J1"
printf '%s' "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" >"$FIXTURE_DIR/expected_commit"
RC_J1="$(run_deploy "$ARTIFACT_J1")"
assert_eq "$RC_J1" "1" "j. exit code is non-zero (SCHEMA missing)"
assert_contains "artifact is missing SCHEMA" "j. missing-SCHEMA message printed"
assert_eq "$(cat "$INSTALL_ROOT/COMMIT")" "$OLD_COMMIT" "j. COMMIT untouched (SCHEMA missing)"
assert_eq "$(cat "$INSTALL_ROOT/bin/cortex-server")" "old binary $OLD_COMMIT" "j. bin/ untouched (SCHEMA missing)"

note "j. SCHEMA file non-numeric -> refused"
setup_install_root 71
ARTIFACT_J2="$TMPROOT/j2.tgz"
make_artifact "abc" "cccccccccccccccccccccccccccccccccccccccc" "$ARTIFACT_J2"
printf '%s' "cccccccccccccccccccccccccccccccccccccccc" >"$FIXTURE_DIR/expected_commit"
RC_J2="$(run_deploy "$ARTIFACT_J2")"
assert_eq "$RC_J2" "1" "j. exit code is non-zero (SCHEMA non-numeric)"
assert_contains "SCHEMA is not a positive integer" "j. non-numeric-SCHEMA message printed"
assert_eq "$(cat "$INSTALL_ROOT/COMMIT")" "$OLD_COMMIT" "j. COMMIT untouched (SCHEMA non-numeric)"
assert_eq "$(cat "$INSTALL_ROOT/bin/cortex-server")" "old binary $OLD_COMMIT" "j. bin/ untouched (SCHEMA non-numeric)"

note "summary"
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
