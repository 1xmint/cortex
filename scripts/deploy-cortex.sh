#!/usr/bin/env bash
set -euo pipefail

# One-command Cortex deploy. Run on the VPS with no args.
# Usage: deploy-cortex
#   or:  deploy-cortex feat/some-branch

REPO_DIR="/home/guardian/claw-net"
ENV_FILE="/etc/cortex/cortex.env"
CORTEX_WWW="/var/www/cortex"
MAX_WAIT=60

# The service is called cortex. Older hosts named it heyvera, from when this
# repository held both products; that name is gone and is not looked for here.
SVC_NAME="cortex"

cd "$REPO_DIR"

GIT_REMOTE=$(git remote | head -1)
BRANCH="${1:-main}"

REMOTE_URL=$(git remote get-url "$GIT_REMOTE" 2>/dev/null || true)
if [[ "$REMOTE_URL" == https://github.com/* ]]; then
  SSH_URL="${REMOTE_URL/https:\/\/github.com\//git@github.com:}"
  git remote set-url "$GIT_REMOTE" "$SSH_URL"
  echo "[git] Switched $GIT_REMOTE to SSH: $SSH_URL"
fi

echo ""
echo "  ╔══════════════════════════════════════╗"
echo "  ║         Cortex Deploy                ║"
echo "  ╚══════════════════════════════════════╝"
echo ""
echo "  Remote:  $GIT_REMOTE"
echo "  Branch:  $BRANCH"
echo ""

# ── env ──────────────────────────────────────
if [ -f "$ENV_FILE" ]; then
  set -a
  eval "$(sudo cat "$ENV_FILE")"
  set +a
  echo "[env] Loaded $ENV_FILE"
fi

# ── protect live data before any destructive git op ──
# The database must live outside REPO_DIR (set CORTEX_DB_PATH). If a legacy
# in-tree database is still present, snapshot it — the reset below would
# otherwise revert or delete it, and that snapshot is the only way back.
LEGACY_DB="$REPO_DIR/.cortex/cortex.db"
DATA_BACKUP_DIR="/home/guardian/backups/pre-deploy"
if [ -f "$LEGACY_DB" ]; then
  mkdir -p "$DATA_BACKUP_DIR"
  SNAPSHOT="$DATA_BACKUP_DIR/cortex_$(date +%Y%m%d_%H%M%S).db"
  if command -v sqlite3 >/dev/null 2>&1; then
    sqlite3 "$LEGACY_DB" ".backup '$SNAPSHOT'"
  else
    cp -p "$LEGACY_DB" "$SNAPSHOT"
  fi
  echo "[data] WARNING: in-tree database found at $LEGACY_DB"
  echo "[data] Snapshotted to $SNAPSHOT before reset."
  echo "[data] Move it out of the repo and set CORTEX_DB_PATH — see docs/operations/."
fi

# ── git (bulletproof) ───────────────────────────
echo "[git] Nuclear reset to $BRANCH..."

# Fetch latest remote state
git fetch "$GIT_REMOTE" "$BRANCH" 2>/dev/null || {
    echo "[git] Initial fetch failed, cleaning and retrying..."
    git gc --prune=now 2>/dev/null || true
    git fetch "$GIT_REMOTE" "$BRANCH"
}

# Abort any in-progress operations
git rebase --abort 2>/dev/null || true
git merge --abort 2>/dev/null || true
git cherry-pick --abort 2>/dev/null || true

# Force checkout branch (create if needed)
git checkout "$BRANCH" 2>/dev/null || {
    echo "[git] Creating new branch $BRANCH..."
    git checkout -b "$BRANCH" "$GIT_REMOTE/$BRANCH" 2>/dev/null || {
        # If that fails, force create from remote
        git branch -D "$BRANCH" 2>/dev/null || true
        git checkout -b "$BRANCH" "$GIT_REMOTE/$BRANCH"
    }
}

# Nuclear reset: force exact remote state
git reset --hard "$GIT_REMOTE/$BRANCH"
git clean -fd  # Remove untracked files
git submodule update --init --recursive 2>/dev/null || true

# Ensure tracking is set up correctly
git branch --set-upstream-to="$GIT_REMOTE/$BRANCH" "$BRANCH" 2>/dev/null || true

COMMIT=$(git rev-parse --short HEAD)
echo "[git] ✓ Nuclear reset complete → $COMMIT"

# ── frontend ─────────────────────────────────
# Cloudflare Pages auto-deploys the cortex frontend on push to main.
# Build here only to keep VPS fallback in sync for deploy-drift detection.
echo "[vite] Building frontend (VPS fallback copy)..."
cd "$REPO_DIR/cortex"
npm ci --silent 2>&1 | tail -1
if npm run build 2>&1 | tail -3; then
  cd "$REPO_DIR"
  sudo mkdir -p "$CORTEX_WWW"
  sudo rsync -a --delete "$REPO_DIR/cortex/dist/" "$CORTEX_WWW/"
  echo "[vite] Synced VPS fallback to $CORTEX_WWW"
else
  echo "[vite] ⚠ Frontend build failed — Cloudflare Pages is the live frontend, continuing..."
  cd "$REPO_DIR"
fi

# ── byos docker image ────────────────────────
if command -v docker &>/dev/null; then
  echo "[docker] Building BYOS sandbox image..."
  if docker build -f "$REPO_DIR/Dockerfile.byos" -t cortex-byos:latest "$REPO_DIR" 2>&1 | tail -3; then
    echo "[docker] cortex-byos:latest built"
  else
    echo "[docker] ⚠ BYOS image build failed — container auth will be unavailable"
  fi
else
  echo "[docker] Docker not installed — skipping BYOS image build"
fi

# ── backend ──────────────────────────────────
echo "[rust] Building cortex-api..."
if [ -f "$HOME/.cargo/env" ]; then
  # shellcheck disable=SC1091
  source "$HOME/.cargo/env"
fi
cargo build --release -p cortex-api 2>&1 | tail -5

# Stop the service (and legacy name if still running)
for svc in "$SVC_NAME"; do
  if sudo systemctl is-active --quiet "$svc" 2>/dev/null; then
    sudo systemctl stop "$svc"
    echo "[svc] Stopped $svc"
  fi
done

# Install whichever binary was built
for bin in cortex-server cortex-api; do
  if [ -f "target/release/$bin" ]; then
    sudo cp "target/release/$bin" "/usr/local/bin/$bin"
    echo "[rust] Installed $bin"
    break
  fi
done

if [ -f target/release/cortex-worker ]; then
  sudo cp target/release/cortex-worker /usr/local/bin/cortex-worker
fi

# ── caddy ────────────────────────────────────
if [ -f "$REPO_DIR/Caddyfile" ]; then
  sudo cp "$REPO_DIR/Caddyfile" /etc/caddy/Caddyfile
  sudo systemctl reload caddy 2>/dev/null || true
  echo "[caddy] Reloaded"
fi

# ── start + health ───────────────────────────
echo "[svc] Using service: $SVC_NAME"
sudo systemctl start "$SVC_NAME"
echo "[svc] Started $SVC_NAME"

# Multi-tier health check with automatic fixes
echo "[health] Backend health check..."
HEALTHY=false
for i in $(seq 1 "$MAX_WAIT"); do
  echo -n "."
  if curl -sf http://localhost:3001/api/health >/dev/null 2>&1; then
    HEALTHY=true
    break
  fi
  sleep 1
done
echo ""

if ! $HEALTHY; then
  echo "[fail] Backend health check failed after ${MAX_WAIT}s"
  echo "[fail] Service logs:"
  sudo journalctl -u "$SVC_NAME" --no-pager -n 20
  exit 1
fi

echo "[health] Proxy routing check..."
sudo systemctl reload caddy 2>/dev/null || true
sleep 2

# Test external API routing
PROXY_HEALTHY=false
for i in $(seq 1 15); do
  echo -n "."
  if curl -sf https://cortex.heyvera.org/api/health >/dev/null 2>&1; then
    PROXY_HEALTHY=true
    break
  fi
  sleep 2
done
echo ""

if ! $PROXY_HEALTHY; then
  echo "[fail] Proxy routing failed — API not accessible externally"
  echo "[info] Backend is healthy but Caddy routing is broken"
  echo "[auto] Attempting automatic fix..."

  # Emergency fix: restart caddy and retry
  sudo systemctl restart caddy
  sleep 5

  if curl -sf https://cortex.heyvera.org/api/health >/dev/null 2>&1; then
    echo "[auto] ✓ Proxy fixed by Caddy restart"
  else
    echo "[fail] Automatic fix failed — manual intervention needed"
    echo "[fail] Check: sudo systemctl status caddy"
    exit 1
  fi
fi

echo ""
echo "  ✓ Cortex deployed and verified — $BRANCH @ $COMMIT"
echo "    Backend: https://cortex.heyvera.org/api/health"
echo "    Frontend: https://cortex.heyvera.org"
echo "    Logs: sudo journalctl -u $SVC_NAME -f"
echo ""
echo "  🚀 Deploy drift prevention: backend + proxy verified"
echo ""
