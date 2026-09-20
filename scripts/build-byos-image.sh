#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"

echo "[byos] Building cortex-byos:latest image..."
docker build -f "$REPO_DIR/Dockerfile.byos" -t cortex-byos:latest "$REPO_DIR"
echo "[byos] Done. Verify: docker run --rm cortex-byos claude --version"
