#!/usr/bin/env bash
set -euo pipefail

# CLI Authentication Verification Script
# Tests Claude and Codex CLI authentication in any environment
#
# Usage: bash scripts/verify-cli-auth.sh
# Exit codes: 0=success, 1=partial failure, 2=complete failure

echo "=== CLI Authentication Verification ==="
echo ""

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Results tracking
claude_status="unknown"
codex_status="unknown"
claude_test="unknown"
codex_test="unknown"

# Function to print colored status
print_status() {
    local status="$1"
    local message="$2"

    case "$status" in
        "pass")
            echo -e "${GREEN}✅ $message${NC}"
            ;;
        "fail")
            echo -e "${RED}❌ $message${NC}"
            ;;
        "warn")
            echo -e "${YELLOW}⚠️  $message${NC}"
            ;;
        "info")
            echo -e "${BLUE}ℹ️  $message${NC}"
            ;;
    esac
}

# Check CLI installation
check_cli_installation() {
    echo "── CLI Installation Check ──────────────────────────────────"

    if command -v claude >/dev/null 2>&1; then
        print_status "pass" "Claude CLI installed at $(which claude)"
        print_status "info" "Version: $(claude --version 2>/dev/null || echo 'unknown')"
    else
        print_status "fail" "Claude CLI not found"
        print_status "info" "Install with: npm install -g @anthropic-ai/claude-cli"
    fi

    if command -v codex >/dev/null 2>&1; then
        print_status "pass" "Codex CLI installed at $(which codex)"
        print_status "info" "Version: $(codex --version 2>/dev/null || echo 'unknown')"
    else
        print_status "fail" "Codex CLI not found"
        print_status "info" "Install with: npm install -g @openai/codex"
    fi

    echo ""
}

# Check Claude authentication
check_claude_auth() {
    echo "── Claude Authentication ───────────────────────────────────"

    if ! command -v claude >/dev/null 2>&1; then
        print_status "fail" "Claude CLI not installed"
        claude_status="not_installed"
        echo ""
        return
    fi

    # Check auth status
    if claude_auth_output=$(claude auth status --json 2>/dev/null); then
        if echo "$claude_auth_output" | grep -q '"loggedIn":true'; then
            claude_status="authenticated"
            print_status "pass" "Claude authentication active"

            # Extract user info
            if email=$(echo "$claude_auth_output" | grep -o '"email":"[^"]*"' | cut -d'"' -f4); then
                print_status "info" "Logged in as: $email"
            fi

            if sub_type=$(echo "$claude_auth_output" | grep -o '"subscriptionType":"[^"]*"' | cut -d'"' -f4); then
                print_status "info" "Subscription: $sub_type"
            fi
        else
            claude_status="not_authenticated"
            print_status "fail" "Claude not authenticated"
        fi
    else
        claude_status="error"
        print_status "fail" "Could not check Claude auth status"
    fi

    echo ""
}

# Check Codex authentication
check_codex_auth() {
    echo "── Codex Authentication ────────────────────────────────────"

    if ! command -v codex >/dev/null 2>&1; then
        print_status "fail" "Codex CLI not installed"
        codex_status="not_installed"
        echo ""
        return
    fi

    # Check auth status
    if codex_auth_output=$(codex login status 2>&1); then
        if echo "$codex_auth_output" | grep -q "Logged in"; then
            codex_status="authenticated"
            print_status "pass" "Codex authentication active"

            # Try to extract account info
            if echo "$codex_auth_output" | grep -q "@"; then
                email=$(echo "$codex_auth_output" | grep -o '[a-zA-Z0-9._%+-]\+@[a-zA-Z0-9.-]\+\.[a-zA-Z]\{2,\}' | head -1)
                print_status "info" "Account: $email"
            fi
        else
            codex_status="not_authenticated"
            print_status "fail" "Codex not authenticated"
        fi
    else
        codex_status="error"
        print_status "fail" "Could not check Codex auth status"
    fi

    echo ""
}

# Test Claude with a simple API call
test_claude_functionality() {
    echo "── Claude Functionality Test ───────────────────────────────"

    if [ "$claude_status" != "authenticated" ]; then
        print_status "warn" "Skipping Claude test (not authenticated)"
        claude_test="skipped"
        echo ""
        return
    fi

    print_status "info" "Testing Claude with simple prompt..."

    if timeout 30 claude -p --model claude-haiku-4-5 --no-session-persistence "Respond with exactly: CLAUDE_TEST_OK" 2>/dev/null | grep -q "CLAUDE_TEST_OK"; then
        claude_test="pass"
        print_status "pass" "Claude functionality test passed"
    else
        claude_test="fail"
        print_status "fail" "Claude functionality test failed"
        print_status "info" "This could indicate auth issues or API quota limits"
    fi

    echo ""
}

# Test Codex with a simple API call
test_codex_functionality() {
    echo "── Codex Functionality Test ────────────────────────────────"

    if [ "$codex_status" != "authenticated" ]; then
        print_status "warn" "Skipping Codex test (not authenticated)"
        codex_test="skipped"
        echo ""
        return
    fi

    print_status "info" "Testing Codex with simple prompt..."

    if timeout 30 codex exec -c "model=gpt-4.1-mini" -c "approval_policy=never" "Respond with exactly: CODEX_TEST_OK" 2>/dev/null | grep -q "CODEX_TEST_OK"; then
        codex_test="pass"
        print_status "pass" "Codex functionality test passed"
    else
        codex_test="fail"
        print_status "fail" "Codex functionality test failed"
        print_status "info" "This could indicate auth issues or API quota limits"
    fi

    echo ""
}

# Check environment variables
check_environment() {
    echo "── Environment Variables ───────────────────────────────────"

    if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
        key_preview=$(echo "$ANTHROPIC_API_KEY" | sed 's/\(.\{8\}\).*/\1.../')
        print_status "pass" "ANTHROPIC_API_KEY set ($key_preview)"
    else
        print_status "info" "ANTHROPIC_API_KEY not set (using CLI auth)"
    fi

    if [ -n "${OPENAI_API_KEY:-}" ]; then
        key_preview=$(echo "$OPENAI_API_KEY" | sed 's/\(.\{8\}\).*/\1.../')
        print_status "pass" "OPENAI_API_KEY set ($key_preview)"
    else
        print_status "info" "OPENAI_API_KEY not set (using CLI auth)"
    fi

    # Check custom CLI paths
    if [ -n "${CORTEX_CLAUDE_PATH:-}" ]; then
        print_status "info" "Custom Claude path: $CORTEX_CLAUDE_PATH"
    fi

    if [ -n "${CORTEX_CODEX_PATH:-}" ]; then
        print_status "info" "Custom Codex path: $CORTEX_CODEX_PATH"
    fi

    echo ""
}

# Generate summary report
generate_summary() {
    echo "── Summary Report ──────────────────────────────────────────"

    local exit_code=0

    # Provider readiness
    if [ "$claude_status" = "authenticated" ] && [ "$claude_test" = "pass" ]; then
        print_status "pass" "Claude ready for BYOS chat"
    elif [ "$claude_status" = "authenticated" ]; then
        print_status "warn" "Claude authenticated but functionality test failed"
        exit_code=1
    elif [ "$claude_status" = "not_installed" ]; then
        print_status "fail" "Claude CLI not installed"
        exit_code=2
    else
        print_status "fail" "Claude not ready"
        exit_code=2
    fi

    if [ "$codex_status" = "authenticated" ] && [ "$codex_test" = "pass" ]; then
        print_status "pass" "Codex ready for BYOS chat"
    elif [ "$codex_status" = "authenticated" ]; then
        print_status "warn" "Codex authenticated but functionality test failed"
        exit_code=1
    elif [ "$codex_status" = "not_installed" ]; then
        print_status "fail" "Codex CLI not installed"
        exit_code=2
    else
        print_status "fail" "Codex not ready"
        exit_code=2
    fi

    echo ""

    # Recommendations
    if [ $exit_code -ne 0 ]; then
        echo "── Recommendations ─────────────────────────────────────────"

        if [ "$claude_status" != "authenticated" ] || [ "$codex_status" != "authenticated" ]; then
            print_status "info" "Run setup script: bash scripts/setup-headless-auth.sh"
            print_status "info" "Or authenticate manually:"
            [ "$claude_status" != "authenticated" ] && echo "  claude auth login"
            [ "$codex_status" != "authenticated" ] && echo "  codex login --device-auth"
        fi

        if [ "$claude_test" = "fail" ] || [ "$codex_test" = "fail" ]; then
            print_status "info" "Check API quotas and billing in provider dashboards"
            print_status "info" "Verify API keys are valid and have sufficient credits"
        fi

        echo ""
    fi

    return $exit_code
}

# Export results for other scripts
export_results() {
    cat > "/tmp/cli-auth-status.env" << EOF
CLAUDE_STATUS=$claude_status
CODEX_STATUS=$codex_status
CLAUDE_TEST=$claude_test
CODEX_TEST=$codex_test
EOF

    if [ "${CI:-}" ]; then
        # GitHub Actions / GitLab CI output
        echo "claude_status=$claude_status" >> "${GITHUB_OUTPUT:-/dev/null}" 2>/dev/null || true
        echo "codex_status=$codex_status" >> "${GITHUB_OUTPUT:-/dev/null}" 2>/dev/null || true
        echo "claude_test=$claude_test" >> "${GITHUB_OUTPUT:-/dev/null}" 2>/dev/null || true
        echo "codex_test=$codex_test" >> "${GITHUB_OUTPUT:-/dev/null}" 2>/dev/null || true
    fi
}

# Main execution
main() {
    check_environment
    check_cli_installation
    check_claude_auth
    check_codex_auth
    test_claude_functionality
    test_codex_functionality

    if generate_summary; then
        print_status "pass" "All checks passed - BYOS chat ready!"
        export_results
        exit 0
    else
        exit_code=$?
        export_results
        exit $exit_code
    fi
}

# Handle interrupts
cleanup() {
    echo ""
    print_status "info" "Verification interrupted"
    exit 130
}
trap cleanup INT TERM

main "$@"