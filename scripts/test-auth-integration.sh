#!/usr/bin/env bash
set -euo pipefail

# Comprehensive Authentication Integration Test
# Tests all authentication methods and fallback scenarios
#
# Usage: bash scripts/test-auth-integration.sh

echo "=== Authentication Integration Test ==="
echo ""

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Test results tracking
tests_passed=0
tests_failed=0

print_result() {
    local status="$1"
    local test_name="$2"
    local message="$3"

    if [ "$status" = "PASS" ]; then
        echo -e "${GREEN}✅ PASS${NC}: $test_name - $message"
        ((tests_passed++))
    else
        echo -e "${RED}❌ FAIL${NC}: $test_name - $message"
        ((tests_failed++))
    fi
}

# Test 1: CLI Tool Installation Check
test_cli_installation() {
    echo -e "${BLUE}── Test 1: CLI Tool Installation ──${NC}"

    local claude_found=false
    local codex_found=false

    if command -v claude >/dev/null 2>&1; then
        claude_found=true
        print_result "PASS" "Claude CLI" "Found at $(which claude)"
    else
        print_result "FAIL" "Claude CLI" "Not found in PATH"
    fi

    if command -v codex >/dev/null 2>&1; then
        codex_found=true
        print_result "PASS" "Codex CLI" "Found at $(which codex)"
    else
        print_result "FAIL" "Codex CLI" "Not found in PATH"
    fi

    echo ""
}

# Test 2: Environment Variable Detection
test_env_vars() {
    echo -e "${BLUE}── Test 2: Environment Variable Detection ──${NC}"

    if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
        local key_preview=$(echo "$ANTHROPIC_API_KEY" | sed 's/\(.\{8\}\).*/\1.../')
        print_result "PASS" "ANTHROPIC_API_KEY" "Set ($key_preview)"
    else
        print_result "FAIL" "ANTHROPIC_API_KEY" "Not set"
    fi

    if [ -n "${OPENAI_API_KEY:-}" ]; then
        local key_preview=$(echo "$OPENAI_API_KEY" | sed 's/\(.\{8\}\).*/\1.../')
        print_result "PASS" "OPENAI_API_KEY" "Set ($key_preview)"
    else
        print_result "FAIL" "OPENAI_API_KEY" "Not set"
    fi

    echo ""
}

# Test 3: API Key Validation
test_api_keys() {
    echo -e "${BLUE}── Test 3: API Key Validation ──${NC}"

    if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
        if curl -s --fail --max-time 10 \
            -H "x-api-key: $ANTHROPIC_API_KEY" \
            -H "anthropic-version: 2023-06-01" \
            -H "content-type: application/json" \
            -d '{"model": "claude-haiku-4-5", "max_tokens": 1, "messages": [{"role": "user", "content": "test"}]}' \
            https://api.anthropic.com/v1/messages >/dev/null 2>&1; then
            print_result "PASS" "Claude API Key" "Valid and working"
        else
            print_result "FAIL" "Claude API Key" "Invalid or quota exceeded"
        fi
    else
        print_result "FAIL" "Claude API Key" "Not provided for testing"
    fi

    if [ -n "${OPENAI_API_KEY:-}" ]; then
        if curl -s --fail --max-time 10 \
            -H "Authorization: Bearer $OPENAI_API_KEY" \
            https://api.openai.com/v1/models >/dev/null 2>&1; then
            print_result "PASS" "OpenAI API Key" "Valid and working"
        else
            print_result "FAIL" "OpenAI API Key" "Invalid or quota exceeded"
        fi
    else
        print_result "FAIL" "OpenAI API Key" "Not provided for testing"
    fi

    echo ""
}

# Test 4: CLI Authentication Status
test_cli_auth() {
    echo -e "${BLUE}── Test 4: CLI Authentication Status ──${NC}"

    if command -v claude >/dev/null 2>&1; then
        if claude auth status --json 2>/dev/null | grep -q '"loggedIn":true'; then
            print_result "PASS" "Claude CLI Auth" "Authenticated"
        else
            print_result "FAIL" "Claude CLI Auth" "Not authenticated"
        fi
    else
        print_result "FAIL" "Claude CLI Auth" "CLI not available"
    fi

    if command -v codex >/dev/null 2>&1; then
        if codex login status 2>/dev/null | grep -q "Logged in"; then
            print_result "PASS" "Codex CLI Auth" "Authenticated"
        else
            print_result "FAIL" "Codex CLI Auth" "Not authenticated"
        fi
    else
        print_result "FAIL" "Codex CLI Auth" "CLI not available"
    fi

    echo ""
}

# Test 5: Headless Setup Script
test_setup_script() {
    echo -e "${BLUE}── Test 5: Headless Setup Script ──${NC}"

    if [ -f "scripts/setup-headless-auth.sh" ]; then
        print_result "PASS" "Setup Script" "File exists"

        if [ -x "scripts/setup-headless-auth.sh" ]; then
            print_result "PASS" "Setup Script Permissions" "Executable"
        else
            print_result "FAIL" "Setup Script Permissions" "Not executable"
        fi

        # Test dry run
        if timeout 30 bash scripts/setup-headless-auth.sh >/dev/null 2>&1; then
            print_result "PASS" "Setup Script Execution" "Runs successfully"
        else
            print_result "FAIL" "Setup Script Execution" "Exits with error"
        fi
    else
        print_result "FAIL" "Setup Script" "File not found"
    fi

    echo ""
}

# Test 6: Verification Script
test_verification_script() {
    echo -e "${BLUE}── Test 6: Verification Script ──${NC}"

    if [ -f "scripts/verify-cli-auth.sh" ]; then
        print_result "PASS" "Verification Script" "File exists"

        if [ -x "scripts/verify-cli-auth.sh" ]; then
            print_result "PASS" "Verification Script Permissions" "Executable"
        else
            print_result "FAIL" "Verification Script Permissions" "Not executable"
        fi

        # Test execution (allow exit codes 0-2)
        local exit_code=0
        timeout 60 bash scripts/verify-cli-auth.sh >/dev/null 2>&1 || exit_code=$?
        if [ $exit_code -le 2 ]; then
            print_result "PASS" "Verification Script Execution" "Runs and exits cleanly"
        else
            print_result "FAIL" "Verification Script Execution" "Unexpected exit code: $exit_code"
        fi

        # Check if status file was created
        if [ -f "/tmp/cli-auth-status.env" ]; then
            print_result "PASS" "Status Export" "Creates status file"
        else
            print_result "FAIL" "Status Export" "No status file created"
        fi
    else
        print_result "FAIL" "Verification Script" "File not found"
    fi

    echo ""
}

# Test 7: Container Scripts
test_container_scripts() {
    echo -e "${BLUE}── Test 7: Container Scripts ──${NC}"

    if [ -f "scripts/container-auth-init.sh" ]; then
        print_result "PASS" "Container Init Script" "File exists"
        if [ -x "scripts/container-auth-init.sh" ]; then
            print_result "PASS" "Container Init Permissions" "Executable"
        else
            print_result "FAIL" "Container Init Permissions" "Not executable"
        fi
    else
        print_result "FAIL" "Container Init Script" "File not found"
    fi

    if [ -f "scripts/container-entrypoint.sh" ]; then
        print_result "PASS" "Container Entrypoint" "File exists"
        if [ -x "scripts/container-entrypoint.sh" ]; then
            print_result "PASS" "Entrypoint Permissions" "Executable"
        else
            print_result "FAIL" "Entrypoint Permissions" "Not executable"
        fi
    else
        print_result "FAIL" "Container Entrypoint" "File not found"
    fi

    echo ""
}

# Test 8: Documentation
test_documentation() {
    echo -e "${BLUE}── Test 8: Documentation ──${NC}"

    local docs=(
        "docs/how-to/headless-cli-auth.md"
        "docs/operations/cli-auth-setup.md"
    )

    for doc in "${docs[@]}"; do
        if [ -f "$doc" ]; then
            print_result "PASS" "Documentation" "$doc exists"
        else
            print_result "FAIL" "Documentation" "$doc missing"
        fi
    done

    if [ -f ".env.example" ]; then
        if grep -q "ANTHROPIC_API_KEY" ".env.example" && grep -q "OPENAI_API_KEY" ".env.example"; then
            print_result "PASS" "Environment Template" "Contains API key examples"
        else
            print_result "FAIL" "Environment Template" "Missing API key examples"
        fi
    else
        print_result "FAIL" "Environment Template" ".env.example not found"
    fi

    echo ""
}

# Test 9: Authentication Resilience
test_auth_resilience() {
    echo -e "${BLUE}── Test 9: Authentication Resilience ──${NC}"

    # Test with no environment variables temporarily
    local orig_anthropic="${ANTHROPIC_API_KEY:-}"
    local orig_openai="${OPENAI_API_KEY:-}"

    export ANTHROPIC_API_KEY=""
    export OPENAI_API_KEY=""

    if timeout 30 bash scripts/setup-headless-auth.sh >/dev/null 2>&1; then
        exit_code=0
    else
        exit_code=$?
    fi

    # Restore original values
    if [ -n "$orig_anthropic" ]; then
        export ANTHROPIC_API_KEY="$orig_anthropic"
    else
        unset ANTHROPIC_API_KEY
    fi

    if [ -n "$orig_openai" ]; then
        export OPENAI_API_KEY="$orig_openai"
    else
        unset OPENAI_API_KEY
    fi

    if [ $exit_code -eq 1 ]; then
        print_result "PASS" "No Keys Handling" "Gracefully handles missing API keys"
    else
        print_result "FAIL" "No Keys Handling" "Unexpected behavior with no keys"
    fi

    echo ""
}

# Generate final report
generate_final_report() {
    echo -e "${BLUE}── Final Report ──${NC}"

    local total_tests=$((tests_passed + tests_failed))
    local success_rate=0

    if [ $total_tests -gt 0 ]; then
        success_rate=$(( (tests_passed * 100) / total_tests ))
    fi

    echo ""
    echo "Test Results:"
    echo "  Total Tests: $total_tests"
    echo "  Passed: $tests_passed"
    echo "  Failed: $tests_failed"
    echo "  Success Rate: ${success_rate}%"
    echo ""

    if [ $tests_failed -eq 0 ]; then
        echo -e "${GREEN}🎉 All tests passed! Authentication system is ready.${NC}"
        return 0
    elif [ $success_rate -ge 80 ]; then
        echo -e "${YELLOW}⚠️  Most tests passed ($success_rate%). Review failed tests.${NC}"
        return 1
    else
        echo -e "${RED}❌ Many tests failed ($success_rate%). Authentication needs work.${NC}"
        return 2
    fi
}

# Main execution
main() {
    test_cli_installation
    test_env_vars
    test_api_keys
    test_cli_auth
    test_setup_script
    test_verification_script
    test_container_scripts
    test_documentation
    test_auth_resilience

    generate_final_report
}

# Handle interrupts
cleanup() {
    echo ""
    echo -e "${YELLOW}Test interrupted${NC}"
    exit 130
}
trap cleanup INT TERM

main "$@"