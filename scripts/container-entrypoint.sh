#!/usr/bin/env bash
set -euo pipefail

# Container Entrypoint
# Initializes CLI authentication and starts the Cortex server

echo "🚀 Starting Cortex container..."

# Initialize CLI authentication if API keys are available
if [ -n "${ANTHROPIC_API_KEY:-}" ] || [ -n "${OPENAI_API_KEY:-}" ]; then
    echo "🔐 Initializing CLI authentication..."
    /usr/local/bin/container-auth-init.sh
else
    echo "ℹ️  No API keys provided - CLI authentication skipped"
    echo "   Set ANTHROPIC_API_KEY and/or OPENAI_API_KEY for BYOS mode"
fi

# Start the main application
echo "🎯 Starting Cortex server..."
exec cortex-server "$@"