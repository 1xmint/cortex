#!/bin/bash

# Cortex Service Control Script
# Simple interface for managing the Cortex conversational orchestration system

set -euo pipefail

# Configuration
SERVICE_NAME="cortex-api"
HEALTH_URL="http://localhost:3001/api/health"
CORTEX_HOME="/opt/cortex"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

show_usage() {
    cat << EOF
Cortex Service Control

Usage: $0 <command>

Commands:
    start       Start the Cortex service
    stop        Stop the Cortex service
    restart     Restart the Cortex service
    status      Show service status
    health      Run health check
    logs        Show service logs (follows)
    deploy      Deploy latest version
    backup      Create data backup
    restore     Restore from backup
    help        Show this help message

Examples:
    $0 start
    $0 health
    $0 logs
EOF
}

wait_for_service() {
    local max_attempts=30
    local attempt=1

    log_info "Waiting for service to be ready..."

    while [[ $attempt -le $max_attempts ]]; do
        if curl -f "$HEALTH_URL" &>/dev/null; then
            log_success "Service is ready!"
            return 0
        fi

        echo -n "."
        sleep 2
        ((attempt++))
    done

    log_error "Service failed to become ready after $((max_attempts * 2)) seconds"
    return 1
}

cmd_start() {
    log_info "Starting Cortex service..."

    if systemctl is-active --quiet "$SERVICE_NAME"; then
        log_warning "Service is already running"
        return 0
    fi

    systemctl start "$SERVICE_NAME"
    wait_for_service
}

cmd_stop() {
    log_info "Stopping Cortex service..."

    if ! systemctl is-active --quiet "$SERVICE_NAME"; then
        log_warning "Service is not running"
        return 0
    fi

    systemctl stop "$SERVICE_NAME"
    log_success "Service stopped"
}

cmd_restart() {
    log_info "Restarting Cortex service..."
    systemctl restart "$SERVICE_NAME"
    wait_for_service
}

cmd_status() {
    echo "=== Service Status ==="
    systemctl status "$SERVICE_NAME" --no-pager -l

    echo
    echo "=== Health Check ==="
    if curl -f "$HEALTH_URL" 2>/dev/null; then
        log_success "API health check passed"
    else
        log_error "API health check failed"
    fi

    echo
    echo "=== Resource Usage ==="
    if pid=$(pgrep -f "cortex-server" 2>/dev/null); then
        memory=$(ps -o rss= -p "$pid" | tr -d ' ')
        memory_mb=$((memory / 1024))
        cpu=$(ps -o %cpu= -p "$pid" | tr -d ' ')
        echo "Memory: ${memory_mb}MB"
        echo "CPU: ${cpu}%"
    else
        echo "Process not found"
    fi
}

cmd_health() {
    log_info "Running comprehensive health check..."

    # Check if health-check script exists
    if [[ -f "$(dirname "$0")/health-check.sh" ]]; then
        bash "$(dirname "$0")/health-check.sh"
    else
        # Basic health check
        echo "Running basic health check..."

        # Service check
        if systemctl is-active --quiet "$SERVICE_NAME"; then
            log_success "Service is active"
        else
            log_error "Service is not active"
            return 1
        fi

        # API check
        if curl -f "$HEALTH_URL" &>/dev/null; then
            log_success "API is responding"
        else
            log_error "API is not responding"
            return 1
        fi

        # Port check
        if ss -tlnp | grep -q ":3001"; then
            log_success "Port 3001 is listening"
        else
            log_error "Port 3001 is not listening"
            return 1
        fi

        log_success "Basic health check passed"
    fi
}

cmd_logs() {
    log_info "Following service logs (Ctrl+C to exit)..."
    journalctl -fu "$SERVICE_NAME"
}

cmd_deploy() {
    log_info "Deploying latest version..."

    if [[ -f "$(dirname "$0")/deploy-production.sh" ]]; then
        bash "$(dirname "$0")/deploy-production.sh"
    else
        log_error "Deployment script not found"
        return 1
    fi
}

cmd_backup() {
    local backup_dir="$CORTEX_HOME/backups"
    local timestamp=$(date +%Y%m%d_%H%M%S)
    local backup_file="$backup_dir/cortex_backup_$timestamp.tar.gz"

    log_info "Creating backup..."

    mkdir -p "$backup_dir"

    # Create backup
    tar -czf "$backup_file" \
        -C "$CORTEX_HOME" \
        --exclude="backups" \
        --exclude="workspace/.git" \
        data workspace .env 2>/dev/null || true

    if [[ -f "$backup_file" ]]; then
        log_success "Backup created: $backup_file"

        # Keep only last 10 backups
        cd "$backup_dir"
        ls -t cortex_backup_*.tar.gz 2>/dev/null | tail -n +11 | xargs rm -f 2>/dev/null || true
    else
        log_error "Backup failed"
        return 1
    fi
}

cmd_restore() {
    local backup_dir="$CORTEX_HOME/backups"

    echo "Available backups:"
    ls -la "$backup_dir"/cortex_backup_*.tar.gz 2>/dev/null || {
        log_error "No backups found"
        return 1
    }

    echo
    read -p "Enter backup file name: " backup_file

    if [[ ! -f "$backup_dir/$backup_file" ]]; then
        log_error "Backup file not found"
        return 1
    fi

    log_warning "This will overwrite current data. Continue? (y/N)"
    read -p "> " confirm

    if [[ "$confirm" != "y" && "$confirm" != "Y" ]]; then
        log_info "Restore cancelled"
        return 0
    fi

    log_info "Stopping service..."
    systemctl stop "$SERVICE_NAME"

    log_info "Restoring from backup..."
    tar -xzf "$backup_dir/$backup_file" -C "$CORTEX_HOME"

    log_info "Starting service..."
    systemctl start "$SERVICE_NAME"
    wait_for_service

    log_success "Restore completed"
}

# Main command handling
case "${1:-}" in
    start)
        cmd_start
        ;;
    stop)
        cmd_stop
        ;;
    restart)
        cmd_restart
        ;;
    status)
        cmd_status
        ;;
    health)
        cmd_health
        ;;
    logs)
        cmd_logs
        ;;
    deploy)
        cmd_deploy
        ;;
    backup)
        cmd_backup
        ;;
    restore)
        cmd_restore
        ;;
    help|--help|-h)
        show_usage
        ;;
    "")
        log_error "No command specified"
        echo
        show_usage
        exit 1
        ;;
    *)
        log_error "Unknown command: $1"
        echo
        show_usage
        exit 1
        ;;
esac