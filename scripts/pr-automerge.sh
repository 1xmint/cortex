#!/usr/bin/env bash
# Open (or reuse) a PR for the current branch and enable squash automerge.
# Usage: scripts/pr-automerge.sh ["PR title"] ["PR body"]
set -euo pipefail

if [ -z "${GH_TOKEN:-}" ]; then
  GH_TOKEN=$(printf 'protocol=https\nhost=github.com\n\n' | git credential fill 2>/dev/null | awk -F= '/^password=/{print $2}' || true)
  export GH_TOKEN
fi

if [ -z "${GH_TOKEN:-}" ]; then
  echo "GH_TOKEN missing; run: gh auth login" >&2
  exit 1
fi

BRANCH=$(git branch --show-current)
if [ "$BRANCH" = "main" ] || [ "$BRANCH" = "master" ]; then
  echo "Refusing to open PR from $BRANCH" >&2
  exit 1
fi

git push -u origin "$BRANCH"

TITLE=${1:-}
BODY=${2:-}

if gh pr view --json number -q .number >/dev/null 2>&1; then
  NUM=$(gh pr view --json number -q .number)
  echo "PR already exists: #$NUM"
else
  if [ -n "$TITLE" ]; then
    gh pr create --base main --title "$TITLE" --body "${BODY:-See commits.}"
  else
    gh pr create --base main --fill
  fi
  NUM=$(gh pr view --json number -q .number)
  echo "Opened PR #$NUM"
fi

# Squash automerge when required checks pass
gh pr merge "$NUM" --auto --squash
echo "Automerge (squash) enabled for #$NUM"
echo "Watch: gh pr checks $NUM --watch"
gh pr view "$NUM" --json url,state,autoMergeRequest -q .
