#!/usr/bin/env bash
set -euo pipefail

# Install the Cortex TUI as a global `cortex` command.
#
# Builds crates/tui in release mode and installs the resulting `cortex` binary
# into a directory on the user's PATH (default: ~/.local/bin).
#
# Usage:
#   scripts/install-cortex-tui.sh [--prefix DIR]
#
# Environment:
#   CORTEX_API     baked-in default can be overridden at runtime via this env var

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'

PREFIX="${HOME}/.local/bin"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix) PREFIX="$2"; shift 2 ;;
    --prefix=*) PREFIX="${1#*=}"; shift ;;
    *) echo -e "${RED}unknown argument: $1${NC}"; exit 1 ;;
  esac
done

# Resolve repo root relative to this script.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# Make cargo available (Rustup-style env, if present).
if [[ -f "$HOME/.cargo/env" ]]; then
  # shellcheck disable=SC1091
  source "$HOME/.cargo/env"
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo -e "${RED}error: cargo not found on PATH. Install Rust (https://rustup.rs) first.${NC}"
  exit 1
fi

echo -e "${YELLOW}Building cortex-tui (release)...${NC}"
cargo build --release -p cortex-tui

BIN="$ROOT/target/release/cortex"
if [[ ! -f "$BIN" ]]; then
  echo -e "${RED}error: build succeeded but $BIN not found${NC}"
  exit 1
fi

mkdir -p "$PREFIX"
install -m 0755 "$BIN" "$PREFIX/cortex"
echo -e "${GREEN}Installed cortex -> $PREFIX/cortex${NC}"

# PATH guidance.
case ":$PATH:" in
  *":$PREFIX:"*) ;;
  *)
    echo -e "${YELLOW}note:${NC} $PREFIX is not on your PATH."
    echo "  Add this to your shell profile:"
    echo "    export PATH=\"$PREFIX:\$PATH\""
    ;;
esac

echo
echo "Next steps:"
echo "  cortex login <clerk-jwt>     # authenticate (or set CORTEX_TOKEN)"
echo "  cd your-project && cortex     # launch scoped to the project"
