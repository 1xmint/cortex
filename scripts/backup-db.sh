#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# backup-db.sh — SQLite backup for Cortex API database
#
# Creates a consistent, gzip-compressed backup of the Cortex SQLite database
# using the SQLite .backup command (safe for concurrent reads/writes).
# Intended to run daily via cron.
#
# Usage:
#   ./scripts/backup-db.sh
#
# Environment variables (all optional):
#   DB_PATH            Path to the SQLite database (default: /home/deploy/cortex-data/cortex.db)
#   BACKUP_DIR         Directory to store backups (default: /home/deploy/cortex-data/backups)
#   RETENTION_DAYS     Number of days to keep backups (default: 30)
#   BACKUP_S3_BUCKET   S3 (or R2) bucket name for offsite backup upload.
#                      If set, the compressed backup is synced to this bucket after
#                      the local backup completes. Requires either:
#                        - `aws` CLI configured with appropriate credentials/role, OR
#                        - `rclone` configured with a remote named to match your provider
#                      Example: BACKUP_S3_BUCKET=my-heyvera-backups
#   BACKUP_S3_PREFIX   Key prefix inside the bucket (default: heyvera-backups).
#                      The backup is uploaded to s3://<BUCKET>/<PREFIX>/<filename>.
#                      Example: BACKUP_S3_PREFIX=prod/cortex
#
# Cron example (daily at 3 AM):
#   0 3 * * * /home/deploy/claw-net/scripts/backup-db.sh >> /var/log/cortex-backup.log 2>&1
#
# Restore procedure:
#   See scripts/restore-db.sh or use manually:
#     1. Stop the cortex-server service
#     2. gunzip /path/to/backup/cortex-YYYYMMDD-HHMMSS.db.gz
#     3. cp /path/to/backup/cortex-YYYYMMDD-HHMMSS.db /home/deploy/cortex-data/cortex.db
#     4. Start the cortex-server service
#     5. Verify with: curl http://localhost:3402/v1/health
# ---------------------------------------------------------------------------
set -euo pipefail

DB_PATH="${DB_PATH:-/home/deploy/cortex-data/cortex.db}"
BACKUP_DIR="${BACKUP_DIR:-/home/deploy/cortex-data/backups}"
RETENTION_DAYS="${RETENTION_DAYS:-30}"
BACKUP_S3_BUCKET="${BACKUP_S3_BUCKET:-}"
BACKUP_S3_PREFIX="${BACKUP_S3_PREFIX:-heyvera-backups}"

TIMESTAMP="$(date -u +"%Y%m%d-%H%M%S")"
DB_NAME="$(basename "$DB_PATH" .db)"
BACKUP_FILE="${BACKUP_DIR}/${DB_NAME}-${TIMESTAMP}.db"

# --- Preflight checks ---

if [ ! -f "$DB_PATH" ]; then
  echo "[backup] ERROR: Database not found at $DB_PATH" >&2
  exit 1
fi

if ! command -v sqlite3 >/dev/null 2>&1; then
  echo "[backup] ERROR: sqlite3 not found in PATH" >&2
  exit 1
fi

mkdir -p "$BACKUP_DIR"

# --- Create backup using SQLite .backup (safe for WAL mode) ---

echo "[backup] Starting backup of $DB_PATH"
sqlite3 "$DB_PATH" ".backup '${BACKUP_FILE}'"

if [ ! -f "$BACKUP_FILE" ]; then
  echo "[backup] ERROR: Backup file was not created" >&2
  exit 1
fi

# --- Compress with gzip ---

gzip "$BACKUP_FILE"
BACKUP_GZ="${BACKUP_FILE}.gz"

SIZE="$(du -h "$BACKUP_GZ" | cut -f1)"
echo "[backup] Created ${BACKUP_GZ} (${SIZE})"

# --- Generate checksum ---

sha256sum "$BACKUP_GZ" > "${BACKUP_GZ}.sha256"

# --- Prune old backups beyond retention window ---

if [ "$RETENTION_DAYS" -gt 0 ] 2>/dev/null; then
  PRUNED="$(find "$BACKUP_DIR" -maxdepth 1 -name "${DB_NAME}-*.db.gz" -type f -mtime "+${RETENTION_DAYS}" | wc -l)"
  if [ "$PRUNED" -gt 0 ]; then
    find "$BACKUP_DIR" -maxdepth 1 -name "${DB_NAME}-*.db.gz" -type f -mtime "+${RETENTION_DAYS}" -exec rm -f {} +
    find "$BACKUP_DIR" -maxdepth 1 -name "${DB_NAME}-*.db.gz.sha256" -type f -mtime "+${RETENTION_DAYS}" -exec rm -f {} +
    echo "[backup] Pruned ${PRUNED} backup(s) older than ${RETENTION_DAYS} days"
  fi
fi

echo "[backup] Done. Backups in ${BACKUP_DIR}:"
ls -lht "$BACKUP_DIR"/${DB_NAME}-*.db.gz 2>/dev/null | head -5

# --- Offsite sync (optional) ---

if [ -n "$BACKUP_S3_BUCKET" ]; then
  S3_DEST="s3://${BACKUP_S3_BUCKET}/${BACKUP_S3_PREFIX}/$(basename "$BACKUP_GZ")"

  if command -v rclone >/dev/null 2>&1; then
    echo "[backup] Uploading to ${S3_DEST} via rclone..."
    rclone copy "$BACKUP_GZ" "s3:${BACKUP_S3_BUCKET}/${BACKUP_S3_PREFIX}/"
    rclone copy "${BACKUP_GZ}.sha256" "s3:${BACKUP_S3_BUCKET}/${BACKUP_S3_PREFIX}/"
    echo "[backup] Offsite upload complete (rclone)"
  elif command -v aws >/dev/null 2>&1; then
    echo "[backup] Uploading to ${S3_DEST} via aws s3 cp..."
    aws s3 cp "$BACKUP_GZ" "$S3_DEST"
    aws s3 cp "${BACKUP_GZ}.sha256" "${S3_DEST}.sha256"
    echo "[backup] Offsite upload complete (aws cli)"
  else
    echo "[backup] WARNING: BACKUP_S3_BUCKET is set but neither 'rclone' nor 'aws' found in PATH — skipping offsite upload" >&2
  fi
else
  echo "[backup] NOTE: No offsite backup configured. Set BACKUP_S3_BUCKET to enable S3/R2 sync."
fi
