#!/usr/bin/env bash
set -euo pipefail

# Install the Cortex systemd service on the VPS.
# Run once after first deploy. Requires: cargo, claude CLI, codex CLI authenticated.
#
# Usage: sudo bash scripts/cortex-install-service.sh

CORTEX_USER="${CORTEX_USER:-deploy}"
CORTEX_PORT="${CORTEX_PORT:-3001}"
CORTEX_LEDGER="${CORTEX_LEDGER:-/var/lib/cortex/ledger.jsonl}"

CORTEX_ENV="${CORTEX_ENV:-/etc/cortex/cortex.env}"

sudo mkdir -p /var/lib/cortex
sudo chown "$CORTEX_USER:$CORTEX_USER" /var/lib/cortex
sudo mkdir -p /var/www/cortex
sudo mkdir -p "$(dirname "$CORTEX_ENV")"

if [ ! -f "$CORTEX_ENV" ]; then
  cat <<ENVEOF | sudo tee "$CORTEX_ENV"
CORTEX_PORT=$CORTEX_PORT
CORTEX_WORKSPACE=/home/$CORTEX_USER/claw-net
CORTEX_ALLOWED_ORIGINS=https://cortex.heyvera.org
RUST_LOG=info
# Auth — set these to enable real user accounts
# CLERK_SECRET_KEY=
# VITE_CLERK_PUBLISHABLE_KEY=
# Billing — set these to enable subscriptions
# STRIPE_SECRET_KEY=
# STRIPE_WEBHOOK_SECRET=
ENVEOF
  sudo chmod 600 "$CORTEX_ENV"
  sudo chown root:root "$CORTEX_ENV"
  echo "[cortex] Created env file at $CORTEX_ENV — fill in keys to go live"
fi

cat <<EOF | sudo tee /etc/systemd/system/cortex.service
[Unit]
Description=Cortex Orchestration Engine
After=network.target

[Service]
Type=simple
User=$CORTEX_USER
ExecStart=/usr/local/bin/cortex-server
EnvironmentFile=$CORTEX_ENV
WorkingDirectory=/home/$CORTEX_USER/claw-net
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable cortex
sudo systemctl start cortex

echo "[cortex] Service installed and started on port $CORTEX_PORT"
echo "[cortex] Check status: sudo systemctl status cortex"
echo "[cortex] View logs: sudo journalctl -u cortex -f"
echo ""
echo "IMPORTANT: Set up CLI authentication for BYOS (Bring Your Own Subscription) mode:"
echo ""
echo "Option 1 - Automated setup (recommended):"
echo "  1. Get API keys from https://console.anthropic.com/account/keys"
echo "  2. Get API keys from https://platform.openai.com/account/api-keys"
echo "  3. sudo nano /etc/cortex/cli-auth.env"
echo "     Add: ANTHROPIC_API_KEY=sk-ant-..."
echo "     Add: OPENAI_API_KEY=sk-proj-..."
echo "  4. sudo chmod 600 /etc/cortex/cli-auth.env"
echo "  5. sudo systemctl edit cortex"
echo "     Add: [Service]"
echo "     Add: EnvironmentFile=/etc/cortex/cli-auth.env"
echo "  6. sudo systemctl daemon-reload && sudo systemctl restart cortex"
echo ""
echo "Option 2 - Manual CLI authentication:"
echo "  1. SSH in as $CORTEX_USER and run:"
echo "     claude auth login"
echo "     codex login --device-auth"
echo ""
echo "Verify setup: sudo -u $CORTEX_USER bash scripts/verify-cli-auth.sh"
echo ""
echo "See docs/operations/cli-auth-setup.md for detailed instructions."
