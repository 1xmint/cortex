#!/usr/bin/env bash
# Installs the pinned browser toolchain for browser checks (ADR 0006) at
# /usr/local/lib/cortex-browser, where the sandbox's read-only /usr shows it.
# Safe to run again. Needs sudo. Used by CI and in WSL before a real run.
set -euo pipefail

NODE_VERSION=v24.21.0
NODE_SHA256=fd8e59d5a511510f6a298afb548f18c7d2b1be404d8b4a27d94fbe49f56cb2d6
PREFIX=/usr/local/lib/cortex-browser
HERE="$(cd "$(dirname "$0")/.." && pwd)"

if [ "$(uname -m)" != "x86_64" ]; then
  echo "only x86_64 is pinned so far" >&2
  exit 1
fi

sudo mkdir -p "$PREFIX"

if [ "$("$PREFIX/node/bin/node" --version 2>/dev/null || true)" != "$NODE_VERSION" ]; then
  tmp="$(mktemp -d)"
  tarball="node-$NODE_VERSION-linux-x64.tar.xz"
  curl -fsSL "https://nodejs.org/dist/$NODE_VERSION/$tarball" -o "$tmp/$tarball"
  echo "$NODE_SHA256  $tmp/$tarball" | sha256sum -c -
  sudo rm -rf "$PREFIX/node"
  sudo mkdir -p "$PREFIX/node"
  sudo tar -xJf "$tmp/$tarball" -C "$PREFIX/node" --strip-components=1
  rm -rf "$tmp"
fi

export PATH="$PREFIX/node/bin:$PATH"
export PLAYWRIGHT_BROWSERS_PATH="$PREFIX/browsers"

sudo cp "$HERE/bench/browser/package.json" "$HERE/bench/browser/package-lock.json" "$PREFIX/"
(cd "$PREFIX" && sudo env "PATH=$PATH" npm ci --no-audit --no-fund --ignore-scripts)
sudo env "PATH=$PATH" "PLAYWRIGHT_BROWSERS_PATH=$PLAYWRIGHT_BROWSERS_PATH" \
  "$PREFIX/node_modules/.bin/playwright" install --with-deps --only-shell chromium
sudo chmod -R a+rX "$PREFIX"
echo "browser tools ready at $PREFIX"
