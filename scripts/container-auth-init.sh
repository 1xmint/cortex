#!/usr/bin/env bash
set -euo pipefail

# Container Authentication Initialization
# Runs inside Docker container to set up CLI authentication from environment variables
# This script is called automatically on container startup if API keys are present

echo "=== Container CLI Auth Initialization ==="

# Ensure CLI tools are available
if ! command -v claude >/dev/null 2>&1 && ! command -v codex >/dev/null 2>&1; then
    echo "🔧 Installing CLI tools via replit-tools..."
    npm install -g replit-tools >/dev/null 2>&1 || {
        echo "❌ Failed to install CLI tools - falling back to API mode"
        exit 0
    }
    echo "✅ CLI tools installed"
fi

# Function to setup Claude auth from environment
setup_claude_container_auth() {
    if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
        echo "🔧 Setting up Claude authentication..."

        # Create auth directory
        mkdir -p ~/.anthropic

        # Test the API key first
        if curl -s --fail --max-time 10 \
            -H "x-api-key: $ANTHROPIC_API_KEY" \
            -H "anthropic-version: 2023-06-01" \
            -H "content-type: application/json" \
            -d '{"model": "claude-haiku-4-5", "max_tokens": 1, "messages": [{"role": "user", "content": "test"}]}' \
            https://api.anthropic.com/v1/messages >/dev/null 2>&1; then

            # Create Claude CLI compatible auth file
            cat > ~/.anthropic/auth.json << EOF
{
  "access_token": "$ANTHROPIC_API_KEY",
  "logged_in": true,
  "email": "container-user@cortex.local",
  "subscription_type": "api"
}
EOF
            chmod 600 ~/.anthropic/auth.json
            echo "✅ Claude authentication configured"
            return 0
        else
            echo "❌ Invalid ANTHROPIC_API_KEY - Claude auth failed"
            return 1
        fi
    else
        echo "ℹ️  ANTHROPIC_API_KEY not provided - Claude unavailable"
        return 0
    fi
}

# Function to setup Codex auth from environment
setup_codex_container_auth() {
    if [ -n "${OPENAI_API_KEY:-}" ]; then
        echo "🔧 Setting up Codex authentication..."

        # Test the API key first
        if curl -s --fail --max-time 10 \
            -H "Authorization: Bearer $OPENAI_API_KEY" \
            https://api.openai.com/v1/models >/dev/null 2>&1; then

            # Use Codex's built-in stdin auth
            echo "$OPENAI_API_KEY" | codex login --with-api-key >/dev/null 2>&1
            echo "✅ Codex authentication configured"
            return 0
        else
            echo "❌ Invalid OPENAI_API_KEY - Codex auth failed"
            return 1
        fi
    else
        echo "ℹ️  OPENAI_API_KEY not provided - Codex unavailable"
        return 0
    fi
}

# Function to verify authentication
verify_container_auth() {
    local claude_ok=false
    local codex_ok=false

    echo "🔍 Verifying authentication..."

    # Check Claude
    if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
        if claude auth status --json 2>/dev/null | grep -q '"loggedIn":true'; then
            echo "✅ Claude verified"
            claude_ok=true
        else
            echo "❌ Claude verification failed"
        fi
    fi

    # Check Codex
    if [ -n "${OPENAI_API_KEY:-}" ]; then
        if codex login status 2>/dev/null | grep -q "Logged in"; then
            echo "✅ Codex verified"
            codex_ok=true
        else
            echo "❌ Codex verification failed"
        fi
    fi

    if [ "$claude_ok" = true ] || [ "$codex_ok" = true ]; then
        echo "🎉 Container authentication ready for BYOS chat"
        return 0
    elif [ -z "${ANTHROPIC_API_KEY:-}" ] && [ -z "${OPENAI_API_KEY:-}" ]; then
        echo "ℹ️  No API keys provided - using API key mode instead of BYOS"
        return 0
    else
        echo "❌ Authentication setup failed"
        return 1
    fi
}

# Only run if we're in a container environment
if [ -f "/.dockerenv" ] || [ -n "${DOCKER:-}" ]; then
    setup_claude_container_auth || true
    setup_codex_container_auth || true
    verify_container_auth

    # Create a flag file to indicate auth was attempted
    touch /tmp/cli-auth-initialized
else
    echo "Not running in container - skipping container auth setup"
fi

echo "=== Container CLI Auth Initialization Complete ==="