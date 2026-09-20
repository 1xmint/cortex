#!/bin/bash

# Cortex Production Deployment Script
# Builds and deploys the full conversational orchestration system

set -euo pipefail

# Configuration
CORTEX_USER="cortex"
CORTEX_HOME="/opt/cortex"
WORKSPACE_DIR="$CORTEX_HOME/workspace"
DATA_DIR="$CORTEX_HOME/data"
BIN_DIR="$CORTEX_HOME/bin"
FRONTEND_DIR="$CORTEX_HOME/frontend"
SERVICE_NAME="cortex-api"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check if running as root
if [[ $EUID -ne 0 ]]; then
   log_error "This script must be run as root for system service installation"
   exit 1
fi

log_info "Starting Cortex production deployment..."

# Create cortex user if it doesn't exist
if ! id "$CORTEX_USER" &>/dev/null; then
    log_info "Creating cortex user..."
    useradd --system --home-dir "$CORTEX_HOME" --shell /bin/bash "$CORTEX_USER"
fi

# Create directories
log_info "Creating directory structure..."
mkdir -p "$CORTEX_HOME" "$WORKSPACE_DIR" "$DATA_DIR" "$BIN_DIR" "$FRONTEND_DIR"

# Build Rust backend
log_info "Building Rust backend..."
cd "$(dirname "$0")/.."
cargo build --release --bin cortex-server

# Copy binary
log_info "Installing backend binary..."
cp target/release/cortex-server "$BIN_DIR/"
chown "$CORTEX_USER:$CORTEX_USER" "$BIN_DIR/cortex-server"
chmod +x "$BIN_DIR/cortex-server"

# Build frontend
log_info "Building frontend..."
cd cortex
npm ci
npm run build

# Copy frontend build
log_info "Installing frontend..."
rm -rf "$FRONTEND_DIR/dist"
cp -r dist "$FRONTEND_DIR/"
chown -R "$CORTEX_USER:$CORTEX_USER" "$FRONTEND_DIR"

# Set up directory permissions
log_info "Setting up permissions..."
chown -R "$CORTEX_USER:$CORTEX_USER" "$CORTEX_HOME"
chmod 755 "$CORTEX_HOME" "$WORKSPACE_DIR" "$DATA_DIR" "$BIN_DIR" "$FRONTEND_DIR"
chmod 644 "$FRONTEND_DIR/dist"/*

# Install systemd service
log_info "Installing systemd service..."
cp deploy/cortex-api.service /etc/systemd/system/
systemctl daemon-reload

# Install nginx configuration if nginx is available
if command -v nginx &> /dev/null; then
    log_info "Installing nginx configuration..."
    cp deploy/nginx-cortex.conf /etc/nginx/sites-available/cortex

    # Enable site if not already enabled
    if [[ ! -e /etc/nginx/sites-enabled/cortex ]]; then
        ln -s /etc/nginx/sites-available/cortex /etc/nginx/sites-enabled/
    fi

    # Test nginx config
    if nginx -t; then
        log_info "Nginx configuration valid, reloading..."
        systemctl reload nginx
    else
        log_error "Nginx configuration test failed!"
        exit 1
    fi
else
    log_warn "Nginx not found, skipping web server configuration"
fi

# Create environment file
log_info "Creating environment configuration..."
cat > "$CORTEX_HOME/.env" << EOF
# Cortex Production Environment
CORTEX_PORT=3001
CORTEX_WORKSPACE=$WORKSPACE_DIR
CORTEX_LEDGER_PATH=$DATA_DIR/ledger.jsonl
CORTEX_ALLOWED_ORIGINS=https://cortex.heyvera.org
# VITE_CORTEX_API is left unset on purpose: the SPA and the API are served
# from the same host, so relative /api paths reach the backend directly.
RUST_LOG=info

# Database
# CORTEX_DATABASE_URL=postgresql://user:pass@localhost/cortex

# Authentication (configure as needed)
# CLERK_SECRET_KEY=your_clerk_secret_key

# GitHub integration (optional)
# GITHUB_TOKEN=your_github_token

# Billing (optional)
# STRIPE_SECRET_KEY=your_stripe_secret_key
# STRIPE_WEBHOOK_SECRET=your_webhook_secret
EOF

chown "$CORTEX_USER:$CORTEX_USER" "$CORTEX_HOME/.env"
chmod 600 "$CORTEX_HOME/.env"

# Enable and start services
log_info "Enabling and starting services..."
systemctl enable "$SERVICE_NAME"

# Start the service
if systemctl is-active --quiet "$SERVICE_NAME"; then
    log_info "Restarting existing service..."
    systemctl restart "$SERVICE_NAME"
else
    log_info "Starting service for the first time..."
    systemctl start "$SERVICE_NAME"
fi

# Wait for service to be ready
log_info "Waiting for service to be ready..."
sleep 5

# Health check
if systemctl is-active --quiet "$SERVICE_NAME"; then
    log_info "✅ Service is running!"

    # Test API endpoint
    if curl -f http://localhost:3001/api/health &>/dev/null; then
        log_info "✅ API health check passed!"
    else
        log_warn "⚠️  API health check failed - service may still be starting"
    fi
else
    log_error "❌ Service failed to start!"
    systemctl status "$SERVICE_NAME"
    exit 1
fi

# Setup log rotation
log_info "Setting up log rotation..."
cat > /etc/logrotate.d/cortex << EOF
$DATA_DIR/*.log {
    daily
    missingok
    rotate 30
    compress
    delaycompress
    notifempty
    create 644 $CORTEX_USER $CORTEX_USER
    postrotate
        systemctl reload $SERVICE_NAME
    endscript
}
EOF

# Create monitoring script
log_info "Creating monitoring script..."
cat > "$CORTEX_HOME/monitor.sh" << 'EOF'
#!/bin/bash

# Cortex Health Monitor
# Checks service health and restarts if needed

SERVICE_NAME="cortex-api"
HEALTH_URL="http://localhost:3001/api/health"
LOG_FILE="/var/log/cortex-monitor.log"

log_message() {
    echo "$(date '+%Y-%m-%d %H:%M:%S') - $1" >> "$LOG_FILE"
}

# Check if service is running
if ! systemctl is-active --quiet "$SERVICE_NAME"; then
    log_message "ERROR: Service not running, attempting restart..."
    systemctl start "$SERVICE_NAME"
    sleep 10
fi

# Check API health
if ! curl -f "$HEALTH_URL" &>/dev/null; then
    log_message "ERROR: Health check failed, restarting service..."
    systemctl restart "$SERVICE_NAME"
    sleep 10

    # Verify restart worked
    if curl -f "$HEALTH_URL" &>/dev/null; then
        log_message "INFO: Service restart successful"
    else
        log_message "CRITICAL: Service restart failed, manual intervention required"
    fi
else
    log_message "INFO: Health check passed"
fi
EOF

chmod +x "$CORTEX_HOME/monitor.sh"
chown "$CORTEX_USER:$CORTEX_USER" "$CORTEX_HOME/monitor.sh"

# Add monitoring cron job
log_info "Setting up monitoring cron job..."
cat > /etc/cron.d/cortex-monitor << EOF
# Cortex health monitoring - runs every 2 minutes
*/2 * * * * $CORTEX_USER $CORTEX_HOME/monitor.sh
EOF

log_info "🚀 Cortex production deployment complete!"
echo
echo "Service Status:"
systemctl status "$SERVICE_NAME" --no-pager -l
echo
echo "Next steps:"
echo "1. Configure environment variables in $CORTEX_HOME/.env"
echo "2. Set up SSL certificates for HTTPS (certbot recommended)"
echo "3. Configure authentication (Clerk) and GitHub integration"
echo "4. Test the conversational orchestration at https://cortex.heyvera.org"
echo
echo "Monitoring:"
echo "- Service logs: journalctl -fu $SERVICE_NAME"
echo "- Health check: curl http://localhost:3001/api/health"
echo "- Monitoring logs: tail -f /var/log/cortex-monitor.log"
