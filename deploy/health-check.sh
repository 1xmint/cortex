#!/bin/bash

# Cortex Comprehensive Health Check
# Tests all components of the conversational orchestration system

set -euo pipefail

# Configuration
API_BASE="http://localhost:3001"
TIMEOUT=30
VERBOSE=false

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -v|--verbose)
            VERBOSE=true
            shift
            ;;
        --api-base)
            API_BASE="$2"
            shift 2
            ;;
        --timeout)
            TIMEOUT="$2"
            shift 2
            ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo "Options:"
            echo "  -v, --verbose     Show detailed output"
            echo "  --api-base URL    API base URL (default: http://localhost:3001)"
            echo "  --timeout SEC     Request timeout (default: 30)"
            echo "  -h, --help        Show this help"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Helper functions
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[PASS]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[FAIL]${NC} $1"
}

verbose() {
    if [[ "$VERBOSE" == "true" ]]; then
        echo -e "${NC}$1${NC}"
    fi
}

# Test counter
TESTS_PASSED=0
TESTS_FAILED=0
TESTS_WARNED=0

test_endpoint() {
    local name="$1"
    local endpoint="$2"
    local expected_status="${3:-200}"
    local method="${4:-GET}"

    verbose "Testing $method $endpoint"

    if response=$(curl -s -w "%{http_code}" -m "$TIMEOUT" -X "$method" "$API_BASE$endpoint" 2>/dev/null); then
        status_code="${response: -3}"
        body="${response%???}"

        if [[ "$status_code" == "$expected_status" ]]; then
            log_success "$name"
            ((TESTS_PASSED++))
            verbose "Response: $body"
            return 0
        else
            log_error "$name (expected $expected_status, got $status_code)"
            ((TESTS_FAILED++))
            verbose "Response: $body"
            return 1
        fi
    else
        log_error "$name (connection failed)"
        ((TESTS_FAILED++))
        return 1
    fi
}

test_conversational_flow() {
    local name="$1"
    local message="$2"

    verbose "Testing conversational flow: $message"

    # Test chat endpoint with setup intent
    if response=$(curl -s -w "%{http_code}" -m "$TIMEOUT" \
        -H "Content-Type: application/json" \
        -H "Authorization: Bearer test-token" \
        -X POST \
        -d "{\"message\": \"$message\"}" \
        "$API_BASE/api/chat" 2>/dev/null); then

        status_code="${response: -3}"

        if [[ "$status_code" == "200" ]]; then
            # Check if we get streaming response
            if echo "$response" | grep -q "data:"; then
                log_success "$name"
                ((TESTS_PASSED++))
                verbose "Streaming response received"
                return 0
            else
                log_warning "$name (no streaming data)"
                ((TESTS_WARNED++))
                return 1
            fi
        else
            log_error "$name (status: $status_code)"
            ((TESTS_FAILED++))
            return 1
        fi
    else
        log_error "$name (connection failed)"
        ((TESTS_FAILED++))
        return 1
    fi
}

test_flow_endpoint() {
    local name="$1"
    local message="$2"

    verbose "Testing flow endpoint: $message"

    if response=$(curl -s -w "%{http_code}" -m "$TIMEOUT" \
        -H "Content-Type: application/json" \
        -H "Authorization: Bearer test-token" \
        -X POST \
        -d "{\"message\": \"$message\"}" \
        "$API_BASE/api/flow/test" 2>/dev/null); then

        status_code="${response: -3}"
        body="${response%???}"

        if [[ "$status_code" == "200" ]]; then
            # Check if response contains flow options
            if echo "$body" | jq -e '.options[]' >/dev/null 2>&1; then
                log_success "$name"
                ((TESTS_PASSED++))
                verbose "Flow options found in response"
                return 0
            else
                log_warning "$name (no flow options in response)"
                ((TESTS_WARNED++))
                verbose "Response: $body"
                return 1
            fi
        else
            log_error "$name (status: $status_code)"
            ((TESTS_FAILED++))
            verbose "Response: $body"
            return 1
        fi
    else
        log_error "$name (connection failed)"
        ((TESTS_FAILED++))
        return 1
    fi
}

# Main health check
echo "🏥 Cortex Conversational Orchestration Health Check"
echo "=============================================="
echo

# Basic service health
log_info "Testing basic service health..."
test_endpoint "API Health Check" "/api/health"

# Core API endpoints
log_info "Testing core API endpoints..."
test_endpoint "Authentication Status" "/api/auth/status"
test_endpoint "Conversation List" "/api/conversations"

# Conversational orchestration specific tests
log_info "Testing conversational orchestration..."
test_flow_endpoint "Setup Intent Flow" "I want to build a habit tracker"
test_flow_endpoint "Project Setup Flow" "Create a new React project"
test_flow_endpoint "Template Selection Flow" "Build a task manager"

# Test chat endpoint with setup intents
log_info "Testing chat endpoint with setup intents..."
test_conversational_flow "Chat Setup Intent" "I want to build a habit tracker app"
test_conversational_flow "Chat Project Intent" "Create a new task manager project"

# Test regular chat (non-setup)
log_info "Testing regular chat functionality..."
test_conversational_flow "Regular Chat" "Help me debug this API call"

# Service status
log_info "Checking service status..."
if systemctl is-active --quiet cortex-api; then
    log_success "Systemd service is active"
    ((TESTS_PASSED++))
else
    log_error "Systemd service is not active"
    ((TESTS_FAILED++))
fi

# Process check
if pgrep -f "cortex-server" >/dev/null; then
    log_success "Cortex server process is running"
    ((TESTS_PASSED++))
else
    log_error "Cortex server process not found"
    ((TESTS_FAILED++))
fi

# Port check
if ss -tlnp | grep -q ":3001"; then
    log_success "Port 3001 is listening"
    ((TESTS_PASSED++))
else
    log_error "Port 3001 is not listening"
    ((TESTS_FAILED++))
fi

# Memory usage
if command -v pgrep >/dev/null && command -v ps >/dev/null; then
    if pid=$(pgrep -f "cortex-server"); then
        memory=$(ps -o rss= -p "$pid" | tr -d ' ')
        memory_mb=$((memory / 1024))
        if [[ $memory_mb -lt 1000 ]]; then
            log_success "Memory usage: ${memory_mb}MB (healthy)"
            ((TESTS_PASSED++))
        else
            log_warning "Memory usage: ${memory_mb}MB (high)"
            ((TESTS_WARNED++))
        fi
    fi
fi

# Disk space check
if command -v df >/dev/null; then
    disk_usage=$(df /opt/cortex 2>/dev/null | awk 'NR==2 {print $5}' | sed 's/%//' || echo "0")
    if [[ $disk_usage -lt 90 ]]; then
        log_success "Disk usage: ${disk_usage}% (healthy)"
        ((TESTS_PASSED++))
    else
        log_warning "Disk usage: ${disk_usage}% (high)"
        ((TESTS_WARNED++))
    fi
fi

# Final summary
echo
echo "=============================================="
echo "Health Check Summary:"
echo -e "  ${GREEN}Passed: $TESTS_PASSED${NC}"
if [[ $TESTS_WARNED -gt 0 ]]; then
    echo -e "  ${YELLOW}Warnings: $TESTS_WARNED${NC}"
fi
if [[ $TESTS_FAILED -gt 0 ]]; then
    echo -e "  ${RED}Failed: $TESTS_FAILED${NC}"
fi

echo
if [[ $TESTS_FAILED -eq 0 ]]; then
    log_success "🚀 Cortex conversational orchestration system is healthy!"
    exit 0
else
    log_error "💥 Health check failed - manual investigation required"
    echo
    echo "Troubleshooting:"
    echo "- Check service logs: journalctl -fu cortex-api"
    echo "- Verify configuration: cat /opt/cortex/.env"
    echo "- Test manually: curl http://localhost:3001/api/health"
    exit 1
fi