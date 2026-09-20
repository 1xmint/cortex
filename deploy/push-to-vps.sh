#!/bin/bash

# Push from Replit container to VPS and trigger deploy
# Usage: ./deploy/push-to-vps.sh

set -euo pipefail

# Configuration - update these for your VPS
VPS_HOST="${VPS_HOST:-your-vps-ip}"
VPS_USER="${VPS_USER:-root}"
VPS_PATH="${VPS_PATH:-/opt/cortex}"
DEPLOY_KEY="${HOME}/.ssh/vps_deploy_key"

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log_info() {
    echo -e "${GREEN}[DEPLOY]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

# Check if we're in the right directory
if [[ ! -f "Cargo.toml" || ! -d "crates" ]]; then
    echo "Error: Must run from Cortex project root"
    exit 1
fi

log_info "🚀 Pushing Cortex conversational orchestration to VPS..."

# Create deployment package
log_info "Creating deployment package..."
tar -czf /tmp/cortex-deploy.tar.gz \
    --exclude='.git' \
    --exclude='target' \
    --exclude='node_modules' \
    --exclude='cortex/dist' \
    .

# Copy to VPS
log_info "Uploading to VPS..."
scp -i "$DEPLOY_KEY" /tmp/cortex-deploy.tar.gz "$VPS_USER@$VPS_HOST:/tmp/"

# Extract and deploy on VPS
log_info "Deploying on VPS..."
ssh -i "$DEPLOY_KEY" "$VPS_USER@$VPS_HOST" << 'EOF'
set -euo pipefail

cd /tmp
tar -xzf cortex-deploy.tar.gz -C /opt/cortex --strip-components=1

cd /opt/cortex
echo "🔨 Building Rust backend..."
cargo build --release --bin cortex-server

echo "📦 Building frontend..."
cd cortex
npm ci --only=production
npm run build
cd ..

echo "🔄 Restarting services..."
sudo systemctl restart cortex-api

echo "✅ Deployment complete!"
echo "🔍 Testing health..."
sleep 3
curl -f http://localhost:3001/api/health || echo "❌ Health check failed"

echo "📊 Service status:"
sudo systemctl status cortex-api --no-pager -l
EOF

log_info "🎉 Cortex deployed! Check https://cortex.heyvera.org"

# Cleanup
rm -f /tmp/cortex-deploy.tar.gz

log_info "✨ Deploy complete. Run 'deploy-cortex' on your VPS for future updates."