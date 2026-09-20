#!/bin/bash
# replit-auth-setup.sh
# Run this once in any new Replit project to set up Claude + Codex auth.
# Usage: bash scripts/replit-auth-setup.sh

set -e

echo "=== Replit Auth Setup ==="
echo ""

# ── BROWSER helper (intercepts OAuth open calls) ──────────────────────────
cat > /tmp/oauth-helper.sh << 'EOF'
#!/bin/bash
curl -s "$1"
EOF
chmod +x /tmp/oauth-helper.sh
echo "✅ Browser helper ready"

# ── Claude ─────────────────────────────────────────────────────────────────
echo ""
echo "── Claude ────────────────────────────────────────────────────────────"

CLAUDE_BIN=$(which claude 2>/dev/null || echo "$HOME/.local/bin/claude")

if [ ! -f "$CLAUDE_BIN" ]; then
  echo "⚠️  claude not found. Install replit-tools first: npx -y replit-tools"
else
  STATUS=$("$CLAUDE_BIN" auth status 2>&1 | grep -o '"loggedIn":[^,}]*' | head -1)
  if echo "$STATUS" | grep -q "true"; then
    echo "✅ Claude already logged in — nothing to do"
  else
    echo "Logging into Claude..."
    BROWSER=/tmp/oauth-helper.sh "$CLAUDE_BIN" auth login --claudeai
    echo "✅ Claude auth complete"
  fi
fi

# ── Codex ──────────────────────────────────────────────────────────────────
echo ""
echo "── Codex ─────────────────────────────────────────────────────────────"

CODEX_AUTH="${CODEX_HOME:-$HOME/.codex}/auth.json"

if [ -f "$CODEX_AUTH" ]; then
  echo "✅ Codex already logged in — nothing to do"
else
  echo ""
  echo "Codex needs a manual auth step (one-time per project):"
  echo ""
  echo "  1. In a SEPARATE shell tab, run:  codex"
  echo "  2. A browser opens to OpenAI — click Authorize"
  echo "  3. Your browser will fail with a localhost error — that's expected"
  echo "  4. Copy the full URL from your browser address bar"
  echo "     (looks like: http://localhost:PORT/auth/callback?code=...)"
  echo "  5. Paste and run it here:"
  echo ""
  echo "     curl \"PASTE_URL_HERE\""
  echo ""
  echo "Once you do that, Codex saves the token and you're done permanently."
fi

echo ""
echo "=== Setup complete ==="
