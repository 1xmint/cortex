#!/bin/sh
set -eu

gateway-stub-proof > /scratch/gateway.log 2>&1 &
gateway_pid=$!
cleanup() {
  status=$?
  trap - EXIT
  kill "$gateway_pid" 2>/dev/null || true
  if [ "$status" -ne 0 ]; then
    cat /scratch/gateway.log >&2
  fi
  exit "$status"
}
trap cleanup EXIT

for delay in 1 1 1 1 1; do
  if curl --noproxy "*" -fsS http://127.0.0.1:18080/health >/dev/null; then
    break
  fi
  sleep "$delay"
done

test -s /scratch/gateway-token
ANTHROPIC_AUTH_TOKEN=$(cat /scratch/gateway-token)
export ANTHROPIC_AUTH_TOKEN

set +e
timeout 60 claude -p \
  --output-format stream-json \
  --verbose \
  --no-session-persistence \
  --model claude-sonnet-4-6 \
  "Reply with exactly: cortex gateway rust listener stub"
claude_status=$?
set -e

proof_status=$(curl --noproxy "*" -fsS http://127.0.0.1:18080/proof/status)
PROOF_STATUS="$proof_status" node -e '
  const status = JSON.parse(process.env.PROOF_STATUS);
  if (status.rows < 1 || status.rows > 4 || status.settled !== status.rows) {
    throw new Error(`expected one to four fully settled spend rows, received ${JSON.stringify(status)}`);
  }
'
exit "$claude_status"
