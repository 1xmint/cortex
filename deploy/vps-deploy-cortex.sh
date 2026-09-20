#!/bin/bash

# VPS-side deployment script
# Install this as /usr/local/bin/deploy-cortex on your VPS
# Usage: deploy-cortex

set -euo pipefail

CORTEX_HOME="/opt/cortex"
SERVICE_NAME="cortex-api"

# Colors
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m'

log_info() {
    echo -e "${GREEN}[DEPLOY]${NC} $1"
}

log_step() {
    echo -e "${BLUE}[STEP]${NC} $1"
}

# Check if running as root
if [[ $EUID -ne 0 ]]; then
   echo "This script must be run as root (for service restart)"
   exit 1
fi

cd "$CORTEX_HOME"

log_info "🚀 Deploying Cortex conversational orchestration system..."

# Pull latest code (assumes git remote is set up)
log_step "📥 Pulling latest code..."
git pull origin main

# Build backend
log_step "🔨 Building Rust backend..."
cargo build --release --bin cortex-server

# Build frontend
log_step "📦 Building frontend..."
cd cortex
npm ci
npm run build
cd ..

# Copy built files to production locations
log_step "📋 Installing built files..."
cp target/release/cortex-server /opt/cortex/bin/
chown cortex:cortex /opt/cortex/bin/cortex-server
chmod +x /opt/cortex/bin/cortex-server

rm -rf /opt/cortex/frontend/dist
cp -r cortex/dist /opt/cortex/frontend/
chown -R cortex:cortex /opt/cortex/frontend

# Database migrations (if any)
log_step "🗃️ Running database migrations..."
# Add DB migration logic here if needed

# Restart service
log_step "🔄 Restarting service..."
systemctl restart "$SERVICE_NAME"

# Wait for service to come up
log_step "⏳ Waiting for service..."
sleep 5

# Health check
log_step "🔍 Health check..."
if curl -f http://localhost:3001/api/health &>/dev/null; then
    log_info "✅ Health check passed!"
else
    echo -e "${YELLOW}[WARN]${NC} Health check failed - service may still be starting"
fi

# Service status
echo
systemctl status "$SERVICE_NAME" --no-pager -l

log_info "🎉 Cortex deployment complete!"
echo
echo "🌐 Frontend: https://cortex.heyvera.org"
echo "🔧 API: http://localhost:3001/api/health"
echo "📊 Logs: journalctl -fu $SERVICE_NAME"
echo
echo "✨ Ready for conversational AI orchestration!"
