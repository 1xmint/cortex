#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# load-test.sh — Lightweight load test for Cortex API endpoints
#
# Uses only curl and bash (no external dependencies).
# Safe for production: mostly read-heavy, minimal writes.
#
# Usage:
#   ./scripts/load-test.sh [BASE_URL]
#
# Environment variables:
#   BASE_URL             API base URL (default: http://localhost:3402)
#   REQUESTS_PER_ENDPOINT  Number of requests per endpoint (default: 100)
#   CONCURRENCY          Number of parallel workers (default: 10)
# ---------------------------------------------------------------------------
set -euo pipefail

BASE_URL="${1:-${BASE_URL:-http://localhost:3402}}"
REQUESTS_PER_ENDPOINT="${REQUESTS_PER_ENDPOINT:-100}"
CONCURRENCY="${CONCURRENCY:-10}"

RESULTS_DIR="$(mktemp -d)"
trap 'rm -rf "$RESULTS_DIR"' EXIT

echo "========================================"
echo " Cortex API Load Test"
echo "========================================"
echo " Target:      $BASE_URL"
echo " Requests:    $REQUESTS_PER_ENDPOINT per endpoint"
echo " Concurrency: $CONCURRENCY"
echo " Started:     $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "========================================"
echo ""

# --- Helper: run concurrent requests against one endpoint ---

run_endpoint_test() {
  local method="$1"
  local path="$2"
  local label="$3"
  local data="${4:-}"
  local url="${BASE_URL}${path}"
  local timing_file="${RESULTS_DIR}/${label}.times"
  local errors_file="${RESULTS_DIR}/${label}.errors"

  : > "$timing_file"
  : > "$errors_file"

  echo "--- Testing: ${method} ${path} ---"

  local batch=0
  local sent=0

  while [ "$sent" -lt "$REQUESTS_PER_ENDPOINT" ]; do
    local pids=""
    local batch_size=0

    while [ "$batch_size" -lt "$CONCURRENCY" ] && [ "$sent" -lt "$REQUESTS_PER_ENDPOINT" ]; do
      (
        local curl_args=(
          --silent --output /dev/null
          --write-out '%{http_code} %{time_total}\n'
          --max-time 30
          -X "$method"
        )
        if [ -n "$data" ]; then
          curl_args+=(-H "Content-Type: application/json" -d "$data")
        fi
        curl_args+=("$url")

        result="$(curl "${curl_args[@]}" 2>/dev/null || echo "000 0.000")"
        status="${result%% *}"
        time_s="${result##* }"

        echo "$time_s" >> "$timing_file"
        if [ "$status" -lt 200 ] || [ "$status" -ge 300 ]; then
          echo "$status" >> "$errors_file"
        fi
      ) &

      sent=$((sent + 1))
      batch_size=$((batch_size + 1))
    done

    wait
    batch=$((batch + 1))
  done

  # --- Calculate stats ---

  local total error_count
  total="$(wc -l < "$timing_file")"
  error_count="$(wc -l < "$errors_file")"

  if [ "$total" -eq 0 ]; then
    echo "  No responses received"
    return
  fi

  local error_rate
  error_rate="$(awk "BEGIN { printf \"%.1f\", ($error_count / $total) * 100 }")"

  # Sort times for percentile calculation
  local sorted_file="${RESULTS_DIR}/${label}.sorted"
  sort -n "$timing_file" > "$sorted_file"

  local avg min_t max_t p95
  avg="$(awk '{ sum += $1; n++ } END { if (n>0) printf "%.3f", sum/n; else print "0" }' "$sorted_file")"
  min_t="$(head -1 "$sorted_file")"
  max_t="$(tail -1 "$sorted_file")"

  # p95 = value at the 95th percentile index
  local p95_idx
  p95_idx="$(awk "BEGIN { idx = int($total * 0.95); if (idx < 1) idx = 1; print idx }")"
  p95="$(sed -n "${p95_idx}p" "$sorted_file")"

  printf "  Requests:  %d total, %d errors (%.1f%%)\n" "$total" "$error_count" "$error_rate"
  printf "  Latency:   avg=%.3fs  min=%.3fs  max=%.3fs  p95=%.3fs\n" "$avg" "$min_t" "$max_t" "$p95"
  echo ""
}

# --- Endpoint tests ---

# Read-heavy: safe for production
run_endpoint_test "GET" "/v1/health" "health"
run_endpoint_test "GET" "/v1/social/feed/home" "feed_home"

# Light write: create a test post (minimal impact)
# Uses a clearly-marked load test payload
run_endpoint_test "POST" "/v1/social/posts" "create_post" \
  '{"content":"[load-test] automated test post — safe to delete","visibility":"private"}'

# Like endpoint: use post ID 1 as a safe target
run_endpoint_test "POST" "/v1/social/posts/1/like" "like_post"

# --- Deep pagination test ---
# Fetches pages 1-10 sequentially using cursor-based pagination.
# Page 1 is fetched without a cursor; subsequent pages use the cursor
# extracted from the previous response. Measures per-page and total time.

run_deep_pagination_test() {
  local feed_path="${1:-/v1/social/feed}"
  local pages="${2:-10}"
  local page_size="${3:-20}"

  echo "--- Deep Pagination: GET ${feed_path} (pages 1-${pages}, limit=${page_size}) ---"

  local page_errors=0
  local pages_fetched=0
  local cursor=""
  local total_start total_end elapsed_total avg_per_page
  local page_times_file="${RESULTS_DIR}/deep_pagination_times.txt"
  : > "$page_times_file"

  total_start="$(date +%s%N)"

  for page in $(seq 1 "$pages"); do
    # Build URL: omit cursor param on first page
    local url
    if [ -z "$cursor" ]; then
      url="${BASE_URL}${feed_path}?limit=${page_size}"
    else
      url="${BASE_URL}${feed_path}?limit=${page_size}&cursor=${cursor}"
    fi

    local page_start page_end page_elapsed_ms response http_status
    page_start="$(date +%s%N)"

    # Capture HTTP status code separately via write-out
    response="$(curl --silent --max-time 15 \
      --write-out '\n__STATUS__%{http_code}' \
      "$url" 2>/dev/null || echo "")"

    page_end="$(date +%s%N)"
    page_elapsed_ms=$(( (page_end - page_start) / 1000000 ))

    # Split status from body
    http_status="$(printf '%s' "$response" | grep -o '__STATUS__[0-9]*' | sed 's/__STATUS__//' || echo "000")"
    response="$(printf '%s' "$response" | sed 's/__STATUS__[0-9]*$//')"

    if [ -z "$response" ] || [ "$http_status" = "000" ]; then
      echo "  [ERROR] Page ${page}: no response (url=${url})"
      page_errors=$((page_errors + 1))
      continue
    fi

    if [ "$http_status" -lt 200 ] || [ "$http_status" -ge 300 ] 2>/dev/null; then
      echo "  [ERROR] Page ${page}: HTTP ${http_status} (url=${url})"
      page_errors=$((page_errors + 1))
      continue
    fi

    # Extract next cursor from response JSON.
    # Supports: {"cursor":"<val>"}, {"next_cursor":"<val>"}, {"meta":{"cursor":"<val>"}}
    local next_cursor
    next_cursor="$(printf '%s' "$response" \
      | grep -o '"next_cursor":"[^"]*"' \
      | head -1 \
      | sed 's/"next_cursor":"//;s/"//' 2>/dev/null || true)"

    if [ -z "$next_cursor" ]; then
      next_cursor="$(printf '%s' "$response" \
        | grep -o '"cursor":"[^"]*"' \
        | head -1 \
        | sed 's/"cursor":"//;s/"//' 2>/dev/null || true)"
    fi

    # Count items returned (rough heuristic: count "id" occurrences)
    local item_count
    item_count="$(printf '%s' "$response" | grep -o '"id"' | wc -l || echo "?")"

    printf '  Page %2d: HTTP %s | %d items | cursor=%s | %dms\n' \
      "$page" "$http_status" "$item_count" "${next_cursor:-<none>}" "$page_elapsed_ms"

    echo "$page_elapsed_ms" >> "$page_times_file"
    pages_fetched=$((pages_fetched + 1))

    # Advance cursor; stop early if no next cursor returned
    if [ -n "$next_cursor" ]; then
      cursor="$next_cursor"
    else
      echo "  [INFO] No next cursor returned at page ${page} — end of feed reached"
      break
    fi
  done

  total_end="$(date +%s%N)"
  elapsed_total=$(( (total_end - total_start) / 1000000 ))

  # Compute average ms per page
  if [ "$pages_fetched" -gt 0 ]; then
    avg_per_page="$(awk '{ sum += $1; n++ } END { if (n>0) printf "%d", sum/n; else print 0 }' "$page_times_file")"
  else
    avg_per_page=0
  fi

  echo ""
  echo "  --- Deep Pagination Results ---"
  printf '  Pages fetched:     %d / %d\n' "$pages_fetched" "$pages"
  printf '  Page errors:       %d\n' "$page_errors"
  printf '  Avg time per page: %dms\n' "$avg_per_page"
  printf '  Total time:        %dms\n' "$elapsed_total"

  if [ "$page_errors" -eq 0 ] && [ "$pages_fetched" -gt 0 ]; then
    echo "  [OK] Deep pagination completed without errors"
  elif [ "$page_errors" -gt 0 ]; then
    echo "  [WARN] ${page_errors} page(s) returned errors — check feed endpoint and cursor logic"
  fi
  echo ""
}

run_deep_pagination_test "/v1/social/feed" 10 20

# --- Summary ---

echo "========================================"
echo " Summary"
echo "========================================"

total_requests=0
total_errors=0

for label in health feed_home create_post like_post; do
  t="$(wc -l < "${RESULTS_DIR}/${label}.times" 2>/dev/null || echo 0)"
  e="$(wc -l < "${RESULTS_DIR}/${label}.errors" 2>/dev/null || echo 0)"
  total_requests=$((total_requests + t))
  total_errors=$((total_errors + e))
done

overall_error_rate="$(awk "BEGIN { if ($total_requests > 0) printf \"%.1f\", ($total_errors / $total_requests) * 100; else print \"0\" }")"

echo " Total requests: $total_requests"
echo " Total errors:   $total_errors ($overall_error_rate%)"
echo " Finished:       $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "========================================"

if [ "$total_errors" -gt 0 ]; then
  echo ""
  echo "WARNING: ${total_errors} request(s) returned non-2xx status"
fi
