#!/usr/bin/env bash
set -euo pipefail

# Headless CLI Authentication Setup
# Automatically configures Claude and Codex CLI authentication in headless environments
#
# Usage: bash scripts/setup-headless-auth.sh
# Environment: Set ANTHROPIC_API_KEY and/or OPENAI_API_KEY before running

echo "=== Headless CLI Authentication Setup ==="
echo ""

# Detect environment type
if [ -n "${REPLIT:-}" ]; then
    ENV_TYPE="replit"
elif [ -n "${DOCKER:-}" ] || [ -f "/.dockerenv" ]; then
    ENV_TYPE="docker"
elif [ -n "${CI:-}" ]; then
    ENV_TYPE="ci"
else
    ENV_TYPE="vps"
fi

echo "Environment detected: $ENV_TYPE"
echo ""

# Function to check if CLI is installed
check_cli_installed() {
    local cli_name="$1"
    local install_cmd="$2"

    if ! command -v "$cli_name" >/dev/null 2>&1; then
        echo "⚠️  $cli_name CLI not found"
        echo "   Install with: $install_cmd"
        return 1
    else
        echo "✅ $cli_name CLI found at $(which "$cli_name")"
        return 0
    fi
}

# Function to setup Claude authentication
setup_claude_auth() {
    echo ""
    echo "── Claude Authentication ──────────────────────────────────"

    if ! check_cli_installed "claude" "npm install -g @anthropic-ai/claude-cli"; then
        return 1
    fi

    # Check current auth status
    if claude auth status --json 2>/dev/null | grep -q '"loggedIn":true'; then
        echo "✅ Claude already authenticated"
        return 0
    fi

    # Try environment variable authentication
    if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
        echo "🔧 Setting up Claude with API key..."

        # Create auth directory
        mkdir -p "$HOME/.anthropic"

        # Test the API key first
        if curl -s --fail \
            -H "x-api-key: $ANTHROPIC_API_KEY" \
            -H "anthropic-version: 2023-06-01" \
            -H "content-type: application/json" \
            -d '{"model": "claude-haiku-4-5", "max_tokens": 1, "messages": [{"role": "user", "content": "test"}]}' \
            https://api.anthropic.com/v1/messages >/dev/null; then

            # Create auth file (Claude CLI format)
            cat > "$HOME/.anthropic/auth.json" << EOF
{
  "access_token": "$ANTHROPIC_API_KEY",
  "logged_in": true,
  "email": "api-key-user@headless.local",
  "subscription_type": "api"
}
EOF
            chmod 600 "$HOME/.anthropic/auth.json"
            echo "✅ Claude authentication configured via API key"
        else
            echo "❌ Invalid ANTHROPIC_API_KEY"
            return 1
        fi
    else
        echo "⚠️  No ANTHROPIC_API_KEY environment variable found"
        echo "   For headless auth, set ANTHROPIC_API_KEY=sk-ant-..."
        echo "   Or run: claude auth login (requires interactive browser)"
        return 1
    fi
}

# Function to setup Codex authentication
setup_codex_auth() {
    echo ""
    echo "── Codex Authentication ───────────────────────────────────"

    if ! check_cli_installed "codex" "npm install -g @openai/codex"; then
        return 1
    fi

    # Check current auth status
    if codex login status 2>/dev/null | grep -q "Logged in"; then
        echo "✅ Codex already authenticated"
        return 0
    fi

    # Try environment variable authentication
    if [ -n "${OPENAI_API_KEY:-}" ]; then
        echo "🔧 Setting up Codex with API key..."

        # Test the API key first
        if curl -s --fail \
            -H "Authorization: Bearer $OPENAI_API_KEY" \
            https://api.openai.com/v1/models >/dev/null; then

            # Use codex built-in stdin auth
            echo "$OPENAI_API_KEY" | codex login --with-api-key
            echo "✅ Codex authentication configured via API key"
        else
            echo "❌ Invalid OPENAI_API_KEY"
            return 1
        fi
    else
        echo "⚠️  No OPENAI_API_KEY environment variable found"
        echo "   For headless auth, set OPENAI_API_KEY=sk-proj-..."
        echo "   Or run: codex login --device-auth (requires browser)"
        return 1
    fi
}

# Function to verify authentication
verify_authentication() {
    echo ""
    echo "── Verification ───────────────────────────────────────────"

    local claude_ok=false
    local codex_ok=false

    # Verify Claude
    if command -v claude >/dev/null 2>&1; then
        if claude auth status --json 2>/dev/null | grep -q '"loggedIn":true'; then
            echo "✅ Claude authentication verified"
            claude_ok=true
        else
            echo "❌ Claude authentication failed"
        fi
    fi

    # Verify Codex
    if command -v codex >/dev/null 2>&1; then
        if codex login status 2>/dev/null | grep -q "Logged in"; then
            echo "✅ Codex authentication verified"
            codex_ok=true
        else
            echo "❌ Codex authentication failed"
        fi
    fi

    echo ""
    if [ "$claude_ok" = true ] || [ "$codex_ok" = true ]; then
        echo "✅ At least one provider authenticated successfully"
        echo ""
        echo "You can now use BYOS (Bring Your Own Subscription) chat in Cortex!"
        return 0
    else
        echo "❌ No providers authenticated"
        echo ""
        echo "Next steps:"
        echo "1. Set environment variables: ANTHROPIC_API_KEY and/or OPENAI_API_KEY"
        echo "2. Restart this script"
        echo "3. Or use interactive auth: claude auth login / codex login --device-auth"
        return 1
    fi
}

# Function to create environment template
create_env_template() {
    if [ ! -f ".env.auth-example" ]; then
        cat > ".env.auth-example" << 'EOF'
# Headless CLI Authentication
# Copy to .env and fill in your API keys

# Claude/Anthropic API Key
# Get from: https://console.anthropic.com/account/keys
ANTHROPIC_API_KEY=sk-ant-...

# OpenAI API Key
# Get from: https://platform.openai.com/account/api-keys
OPENAI_API_KEY=sk-proj-...

# Optional: Override CLI binary paths
# CORTEX_CLAUDE_PATH=/custom/path/to/claude
# CORTEX_CODEX_PATH=/custom/path/to/codex
EOF
        echo "📝 Created .env.auth-example template"
    fi
}

# Main execution
main() {
    create_env_template

    local claude_result=0
    local codex_result=0

    setup_claude_auth || claude_result=$?
    setup_codex_auth || codex_result=$?

    if ! verify_authentication; then
        echo ""
        echo "Environment-specific guidance:"
        case "$ENV_TYPE" in
            docker)
                echo "• Add API keys to docker-compose.yml environment section"
                echo "• Or mount .env file with API keys"
                ;;
            ci)
                echo "• Set API keys as CI/CD secrets"
                echo "• Reference them in workflow environment variables"
                ;;
            vps)
                echo "• Create secure env file: sudo nano /etc/cortex/cli-auth.env"
                echo "• Set permissions: sudo chmod 600 /etc/cortex/cli-auth.env"
                echo "• Update systemd service to source the env file"
                ;;
            replit)
                echo "• Set API keys in Replit secrets tab"
                echo "• Or use .env file (not committed to git)"
                ;;
        esac
        exit 1
    fi
}

# Trap for cleanup
cleanup() {
    echo ""
    echo "Setup interrupted"
}
trap cleanup INT TERM

main "$@"