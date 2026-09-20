#!/usr/bin/env bash
set -euo pipefail

# Install the Cortex Worker systemd service on the VPS.
# The worker connects to the brain and executes tasks via claude/codex CLIs.
#
# Usage: sudo bash scripts/cortex-install-worker.sh
#
# The worker needs a credential. It is a headless daemon: it has no browser and
# can never obtain a Clerk user JWT, so it authenticates with a `cwk_` worker
# service key. Mint one on the host, next to the database:
#
#   cortex-worker-key issue --owner <clerk_user_id>
#
# and pass it in as CORTEX_TOKEN, either in the environment when running this
# script or by editing $WORKER_ENV afterwards. The secret is shown once.
#
# Until CORTEX_ALLOW_ANONYMOUS_WORKER=1 is set on the *brain* — which is a
# development-only escape hatch and is off by default — a worker with no
# CORTEX_TOKEN will be refused at connect. That refusal is the point: an unset
# CLERK_SECRET_KEY used to be enough to authenticate any client as `local`.

CORTEX_USER="${CORTEX_USER:-deploy}"
CORTEX_BRAIN_URL="${CORTEX_BRAIN_URL:-ws://localhost:3001/api/ws}"
CORTEX_TOKEN="${CORTEX_TOKEN:-}"

WORKER_ENV="${WORKER_ENV:-/etc/cortex/worker.env}"

if [ -z "$CORTEX_TOKEN" ] && [ ! -f "$WORKER_ENV" ]; then
  echo "[worker] WARNING: no CORTEX_TOKEN given and no existing $WORKER_ENV."
  echo "[worker] The worker will start but the brain will refuse its connection."
  echo "[worker] Mint a key with:  cortex-worker-key issue --owner <clerk_user_id>"
  echo "[worker] then add it to $WORKER_ENV as CORTEX_TOKEN=cwk_... and restart."
fi

sudo mkdir -p "$(dirname "$WORKER_ENV")"

if [ ! -f "$WORKER_ENV" ]; then
  # 0600 root:root below is why this needs sudo: the file holds a bearer
  # credential and must not be readable by the service user's shell, only by
  # systemd reading EnvironmentFile as root before the drop to $CORTEX_USER.
  cat <<ENVEOF | sudo tee "$WORKER_ENV" >/dev/null
CORTEX_BRAIN_URL=$CORTEX_BRAIN_URL
CORTEX_TOKEN=$CORTEX_TOKEN
SOMA_ENFORCE_DELEGATION=false
RUST_LOG=info
ENVEOF
  sudo chmod 600 "$WORKER_ENV"
  sudo chown root:root "$WORKER_ENV"
  echo "[worker] Created env file at $WORKER_ENV"
elif [ -n "$CORTEX_TOKEN" ]; then
  # An existing file is updated in place rather than recreated, so an operator
  # rotating a key does not lose the rest of the worker's configuration.
  if sudo grep -q '^CORTEX_TOKEN=' "$WORKER_ENV"; then
    sudo sed -i "s|^CORTEX_TOKEN=.*|CORTEX_TOKEN=$CORTEX_TOKEN|" "$WORKER_ENV"
    echo "[worker] Rotated CORTEX_TOKEN in $WORKER_ENV"
  else
    echo "CORTEX_TOKEN=$CORTEX_TOKEN" | sudo tee -a "$WORKER_ENV" >/dev/null
    echo "[worker] Added CORTEX_TOKEN to $WORKER_ENV"
  fi
  sudo chmod 600 "$WORKER_ENV"
  sudo chown root:root "$WORKER_ENV"
fi

cat <<EOF | sudo tee /etc/systemd/system/cortex-worker.service
[Unit]
Description=Cortex Worker (executes tasks via claude/codex CLI)
After=cortex.service network.target
Wants=cortex.service

[Service]
Type=simple
User=$CORTEX_USER
ExecStart=/usr/local/bin/cortex-worker
EnvironmentFile=$WORKER_ENV
WorkingDirectory=/home/$CORTEX_USER/claw-net
Restart=on-failure
RestartSec=5
Environment=HOME=/home/$CORTEX_USER
Environment=PATH=/home/$CORTEX_USER/.local/bin:/home/$CORTEX_USER/.cargo/bin:/usr/local/bin:/usr/bin:/bin

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable cortex-worker
sudo systemctl start cortex-worker

echo "[worker] Service installed and started"
echo "[worker] Check status: sudo systemctl status cortex-worker"
echo "[worker] View logs: sudo journalctl -u cortex-worker -f"
echo ""
echo "If the brain logs 'invalid worker key' or 'empty auth token', the worker"
echo "has no valid CORTEX_TOKEN. Mint one and rotate it in:"
echo "  cortex-worker-key issue --owner <clerk_user_id>"
echo "  sudo CORTEX_TOKEN=cwk_... bash scripts/cortex-install-worker.sh"
echo ""
echo "IMPORTANT: The worker uses 'claude' and 'codex' CLI authentication for the"
echo "providers it executes with. That is separate from CORTEX_TOKEN, which is"
echo "how the worker proves its identity to the brain."
echo ""
echo "To verify worker authentication:"
echo "  sudo -u $CORTEX_USER bash scripts/verify-cli-auth.sh"
echo ""
echo "If authentication fails, ensure CLI tools are installed:"
echo "  sudo -u $CORTEX_USER npm install -g @anthropic-ai/claude-cli @openai/codex"
echo ""
echo "For headless auth setup, see: docs/operations/cli-auth-setup.md"
