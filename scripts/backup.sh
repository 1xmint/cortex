#!/usr/bin/env bash
set -euo pipefail

REPO_DIR="${REPO_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
DATA_DIR="${DATA_DIR:-$REPO_DIR/data}"
BACKUP_DIR="${BACKUP_DIR:-$DATA_DIR/backups}"
DB_BASENAME="${DB_BASENAME:-orchestrator.db}"
RETENTION_DAYS="${RETENTION_DAYS:-14}"

DB_PATH="$DATA_DIR/$DB_BASENAME"
WAL_PATH="${DB_PATH}-wal"
SHM_PATH="${DB_PATH}-shm"

if [ ! -f "$DB_PATH" ]; then
  echo "[backup] No database found at $DB_PATH - skipping"
  exit 0
fi

timestamp="$(date -u +"%Y%m%dT%H%M%SZ")"
target_dir="$BACKUP_DIR/$timestamp"

mkdir -p "$target_dir"

cp -p "$DB_PATH" "$target_dir/"
if [ -f "$WAL_PATH" ]; then
  cp -p "$WAL_PATH" "$target_dir/"
fi
if [ -f "$SHM_PATH" ]; then
  cp -p "$SHM_PATH" "$target_dir/"
fi

(
  cd "$target_dir"
  sha256sum ./* > SHA256SUMS
)

echo "[backup] Database snapshot written to $target_dir"

if [[ "$RETENTION_DAYS" =~ ^[0-9]+$ ]]; then
  find "$BACKUP_DIR" -mindepth 1 -maxdepth 1 -type d -mtime "+$RETENTION_DAYS" -exec rm -rf {} +
fi
