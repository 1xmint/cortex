#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# monitoring-check.sh — Production health check for Cortex API
#
# Checks critical endpoints for availability and response time.
# Exits 1 on any failure — suitable for cron-based alerting.
#
# Usage:
#   ./scripts/monitoring-check.sh [BASE_URL]
#
# Environment variables:
#   BASE_URL               API base URL (default: http://localhost:3402)
#   TIMEOUT_THRESHOLD      Max acceptable response time in seconds (default: 2)
#   CURL_TIMEOUT           Per-request timeout in seconds (default: 10)
#   SLACK_WEBHOOK_URL      If set, POST a failure summary to this Slack webhook
#   ALERT_EMAIL            If set, send a failure email via sendmail/mailx
#   PAGERDUTY_ROUTING_KEY  If set, send a PagerDuty Events API v2 alert on failure
#
# ── Slack setup ─────────────────────────────────────────────────────────────
# 1. In Slack, go to https://api.slack.com/apps → Create an App → Incoming Webhooks
# 2. Enable Incoming Webhooks and click "Add New Webhook to Workspace"
# 3. Choose a channel and copy the Webhook URL
# 4. Set: export SLACK_WEBHOOK_URL="https://hooks.slack.com/services/T.../B.../..."
#
# ── PagerDuty setup ─────────────────────────────────────────────────────────
# 1. In PagerDuty, go to Services → select your service → Integrations tab
# 2. Add an integration: choose "Events API v2"
# 3. Copy the "Integration Key" (this is your routing key)
# 4. Set: export PAGERDUTY_ROUTING_KEY="your_32char_routing_key"
# 5. Alerts fire at https://events.pagerduty.com/v2/enqueue
#
# Install: crontab -e, add:
#   */5 * * * * /path/to/monitoring-check.sh >> /var/log/heyvera-monitor.log 2>&1
# ---------------------------------------------------------------------------
set -euo pipefail

BASE_URL="${1:-${BASE_URL:-http://localhost:3402}}"
TIMEOUT_THRESHOLD="${TIMEOUT_THRESHOLD:-2}"
CURL_TIMEOUT="${CURL_TIMEOUT:-10}"
SLACK_WEBHOOK_URL="${SLACK_WEBHOOK_URL:-}"
ALERT_EMAIL="${ALERT_EMAIL:-}"
PAGERDUTY_ROUTING_KEY="${PAGERDUTY_ROUTING_KEY:-}"

failures=0
checks=0

check_endpoint() {
  local method="$1"
  local path="$2"
  local label="$3"
  local expect_json="${4:-true}"
  local url="${BASE_URL}${path}"

  checks=$((checks + 1))

  local result curl_exit
  set +e
  result="$(curl --silent --show-error \
    --max-time "$CURL_TIMEOUT" \
    --output /dev/null \
    --write-out '%{http_code} %{time_total}' \
    -X "$method" \
    "$url" 2>&1)"
  curl_exit=$?
  set -e

  if [ "$curl_exit" -ne 0 ]; then
    echo "[FAIL] ${label}: connection failed (curl exit ${curl_exit})"
    failures=$((failures + 1))
    return
  fi

  local status time_s
  status="${result%% *}"
  time_s="${result##* }"

  # Check HTTP status
  if [ "$status" -lt 200 ] || [ "$status" -ge 300 ]; then
    echo "[FAIL] ${label}: HTTP ${status} (expected 2xx)"
    failures=$((failures + 1))
    return
  fi

  # Check response time against threshold
  local over_threshold
  over_threshold="$(awk "BEGIN { print ($time_s > $TIMEOUT_THRESHOLD) ? 1 : 0 }")"
  if [ "$over_threshold" -eq 1 ]; then
    echo "[WARN] ${label}: ${time_s}s (threshold: ${TIMEOUT_THRESHOLD}s)"
    failures=$((failures + 1))
    return
  fi

  echo "[ OK ] ${label}: HTTP ${status} in ${time_s}s"
}

echo "Cortex Monitoring Check — $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "Target: ${BASE_URL}"
echo "Threshold: ${TIMEOUT_THRESHOLD}s"
echo "---"

# --- Critical endpoints ---

# Health/readiness (includes DB check)
check_endpoint "GET" "/v1/health" "Health endpoint"
check_endpoint "GET" "/v1/ready" "Readiness endpoint (DB accessible)"

# Core API endpoints
check_endpoint "GET" "/v1/social/feed/home" "Social feed"

# --- Results ---

echo "---"
echo "Checks: ${checks}, Failures: ${failures}"

if [ "$failures" -gt 0 ]; then
  echo "STATUS: UNHEALTHY"

  FAILURE_MSG="Cortex health check FAILED at $(date -u +%Y-%m-%dT%H:%M:%SZ) — ${failures}/${checks} checks failed (target: ${BASE_URL})"

  # Optional: Slack webhook notification
  if [ -n "$SLACK_WEBHOOK_URL" ]; then
    curl --silent --max-time 5 -X POST "$SLACK_WEBHOOK_URL" \
      -H "Content-Type: application/json" \
      -d "{\"text\": \":rotating_light: ${FAILURE_MSG}\"}" > /dev/null 2>&1 || true
  fi

  # Optional: email notification via sendmail or mailx
  if [ -n "$ALERT_EMAIL" ]; then
    if command -v mailx > /dev/null 2>&1; then
      printf "Subject: ALERT: Cortex health check FAILED\n\n%s\n" "$FAILURE_MSG" \
        | mailx -s "ALERT: Cortex down" "$ALERT_EMAIL" 2>/dev/null || true
    elif command -v sendmail > /dev/null 2>&1; then
      printf "To: %s\nSubject: ALERT: Cortex health check FAILED\n\n%s\n" \
        "$ALERT_EMAIL" "$FAILURE_MSG" \
        | sendmail "$ALERT_EMAIL" 2>/dev/null || true
    fi
  fi

  # Optional: PagerDuty Events API v2 notification
  if [ -n "$PAGERDUTY_ROUTING_KEY" ]; then
    PD_PAYLOAD="{
      \"routing_key\": \"${PAGERDUTY_ROUTING_KEY}\",
      \"event_action\": \"trigger\",
      \"payload\": {
        \"summary\": \"${FAILURE_MSG}\",
        \"severity\": \"critical\",
        \"source\": \"${BASE_URL}\",
        \"component\": \"cortex-api\",
        \"custom_details\": {
          \"checks_total\": ${checks},
          \"checks_failed\": ${failures},
          \"target\": \"${BASE_URL}\"
        }
      }
    }"
    curl --silent --max-time 5 -X POST \
      "https://events.pagerduty.com/v2/enqueue" \
      -H "Content-Type: application/json" \
      -d "$PD_PAYLOAD" > /dev/null 2>&1 || true
  fi

  exit 1
fi

echo "STATUS: HEALTHY"
exit 0
