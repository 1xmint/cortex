#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# restore-db.sh — Restore a Cortex SQLite database from backup
#
# Usage:
#   ./scripts/restore-db.sh /path/to/cortex-YYYYMMDD-HHMMSS.db.gz
#   ./scripts/restore-db.sh --latest
#
# Options:
#   --latest    Restore from the most recent backup in BACKUP_DIR
#   --dry-run   Show what would happen without making changes
#
# Environment variables (all optional):
#   DB_PATH          Target database path (default: /home/deploy/cortex-data/cortex.db)
#   BACKUP_DIR       Where backups are stored (default: /home/deploy/cortex-data/backups)
#   SERVICE_NAME     Systemd service name (default: cortex-server)
#
# Steps performed:
#   1. Verify the backup file exists and checksum matches (if .sha256 present)
#   2. Stop the cortex-server service (if running)
#   3. Move the current database to a .pre-restore backup
#   4. Decompress and place the backup as the active database
#   5. Verify database integrity with PRAGMA integrity_check
#   6. Start the cortex-server service
#   7. Verify the health endpoint responds
# ---------------------------------------------------------------------------
set -euo pipefail

DB_PATH="${DB_PATH:-/home/deploy/cortex-data/cortex.db}"
BACKUP_DIR="${BACKUP_DIR:-/home/deploy/cortex-data/backups}"
SERVICE_NAME="${SERVICE_NAME:-cortex-server}"
DRY_RUN=false
BACKUP_FILE=""

DB_NAME="$(basename "$DB_PATH" .db)"

usage() {
  echo "Usage: $0 [--dry-run] <backup-file.db.gz | --latest>"
  exit 1
}

# --- Parse arguments ---

while [ $# -gt 0 ]; do
  case "$1" in
    --latest)
      BACKUP_FILE="$(ls -t "${BACKUP_DIR}"/${DB_NAME}-*.db.gz 2>/dev/null | head -1)"
      if [ -z "$BACKUP_FILE" ]; then
        echo "[restore] ERROR: No backups found in ${BACKUP_DIR}" >&2
        exit 1
      fi
      shift
      ;;
    --dry-run)
      DRY_RUN=true
      shift
      ;;
    -h|--help)
      usage
      ;;
    *)
      BACKUP_FILE="$1"
      shift
      ;;
  esac
done

if [ -z "$BACKUP_FILE" ]; then
  usage
fi

if [ ! -f "$BACKUP_FILE" ]; then
  echo "[restore] ERROR: Backup file not found: $BACKUP_FILE" >&2
  exit 1
fi

echo "[restore] Backup file: $BACKUP_FILE"
echo "[restore] Target DB:   $DB_PATH"

# --- Verify checksum if available ---

if [ -f "${BACKUP_FILE}.sha256" ]; then
  echo "[restore] Verifying checksum..."
  EXPECTED="$(awk '{print $1}' "${BACKUP_FILE}.sha256")"
  ACTUAL="$(sha256sum "$BACKUP_FILE" | awk '{print $1}')"
  if [ "$EXPECTED" != "$ACTUAL" ]; then
    echo "[restore] ERROR: Checksum mismatch!" >&2
    echo "  Expected: $EXPECTED" >&2
    echo "  Actual:   $ACTUAL" >&2
    exit 1
  fi
  echo "[restore] Checksum OK"
else
  echo "[restore] WARNING: No .sha256 file found, skipping verification"
fi

if $DRY_RUN; then
  echo "[restore] DRY RUN — would stop ${SERVICE_NAME}, replace ${DB_PATH}, restart"
  exit 0
fi

# --- Stop service ---

if systemctl is-active --quiet "$SERVICE_NAME" 2>/dev/null; then
  echo "[restore] Stopping ${SERVICE_NAME}..."
  sudo systemctl stop "$SERVICE_NAME"
  RESTART_SERVICE=true
else
  echo "[restore] Service ${SERVICE_NAME} is not running"
  RESTART_SERVICE=false
fi

# --- Backup current database ---

if [ -f "$DB_PATH" ]; then
  PRE_RESTORE="${DB_PATH}.pre-restore-$(date -u +%Y%m%d-%H%M%S)"
  echo "[restore] Moving current DB to ${PRE_RESTORE}"
  mv "$DB_PATH" "$PRE_RESTORE"
  # Also move WAL/SHM if present
  [ -f "${DB_PATH}-wal" ] && mv "${DB_PATH}-wal" "${PRE_RESTORE}-wal"
  [ -f "${DB_PATH}-shm" ] && mv "${DB_PATH}-shm" "${PRE_RESTORE}-shm"
fi

# --- Decompress backup ---

echo "[restore] Decompressing backup..."
gunzip -c "$BACKUP_FILE" > "$DB_PATH"

# --- Verify integrity ---

echo "[restore] Checking database integrity..."
INTEGRITY="$(sqlite3 "$DB_PATH" "PRAGMA integrity_check;" 2>&1)"
if [ "$INTEGRITY" != "ok" ]; then
  echo "[restore] WARNING: Integrity check returned: $INTEGRITY" >&2
  echo "[restore] The database may be corrupt. Pre-restore backup is at: ${PRE_RESTORE:-none}" >&2
fi

echo "[restore] Database restored: $(du -h "$DB_PATH" | cut -f1)"

# --- Restart service ---

if $RESTART_SERVICE; then
  echo "[restore] Starting ${SERVICE_NAME}..."
  sudo systemctl start "$SERVICE_NAME"
  sleep 2
  echo "[restore] Verifying health endpoint..."
  if curl --fail --silent --max-time 10 http://localhost:3402/v1/health >/dev/null 2>&1; then
    echo "[restore] Health check passed"
  else
    echo "[restore] WARNING: Health check failed — service may need manual inspection" >&2
  fi
fi

echo "[restore] Restore complete"
