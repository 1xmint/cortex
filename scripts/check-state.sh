#!/usr/bin/env bash
# Keeps STATE.md short and current.
#   check-state.sh                     line cap only
#   check-state.sh BASE_SHA PR_BODY    also: a PR that changes more than docs
#                                      must touch STATE.md or give a reason
set -euo pipefail
cd "$(dirname "$0")/.."

cap=150
lines=$(wc -l < STATE.md)
if [ "$lines" -gt "$cap" ]; then
  echo "STATE.md is $lines lines; the cap is $cap. Move history out (decisions belong in ADRs)."
  exit 1
fi
grep -Eq '^Updated: [0-9]{4}-[0-9]{2}-[0-9]{2}$' STATE.md || {
  echo "STATE.md needs a line 'Updated: YYYY-MM-DD'."
  exit 1
}

[ $# -ge 1 ] || { echo "STATE.md OK ($lines/$cap lines)."; exit 0; }
base="$1"
body="${2:-}"
changed=$(git diff --name-only "$base"...HEAD)
non_doc=$(echo "$changed" | grep -Ev '^docs/|\.md$' || true)
if [ -n "$non_doc" ] && ! echo "$changed" | grep -qx 'STATE.md'; then
  if ! printf '%s\n' "$body" | grep -q '^STATE-unchanged:'; then
    echo "This PR changes more than docs but does not update STATE.md:"
    echo "$non_doc"
    echo "Update STATE.md, or add a line starting 'STATE-unchanged:' to the PR description saying why."
    exit 1
  fi
fi
echo "STATE.md OK ($lines/$cap lines)."
