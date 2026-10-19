# 0006. Browser checks: Playwright with a pinned Chromium headless shell

Status: Accepted
Date: 2026-10-18

## Context

Slice 2's bench is browser games and small web apps (design v2, section
6). Their checks must open the page, click, type, wait for things to
happen and read what is on screen. Like every check, they run inside the
bubblewrap + gVisor sandbox with no network (ADR 0004), so the browser
must run there too, with the work served over loopback.

Facts checked on 2026-10-18: Playwright 1.63.0 and Node.js v24.21.0 are
current stable releases more than two weeks old. Playwright ships a
"headless shell" build of Chromium that needs no display.

## Options

Scored on the gold-rule criteria (AGENTS.md invariant 5).

1. **Playwright (Node) with its pinned Chromium headless shell.**
   - Reliability: auto-waits for elements to be ready before acting, which
     removes the commonest cause of flaky browser checks.
   - Security: the same sandbox as every check; Chromium's own sandbox is
     turned off (`--no-sandbox`) because the outer one is the boundary.
   - Maintenance: one exact version pins both the library and the browser
     build; maintained by a large team; Firefox and WebKit are a flag away.
   - Ecosystem: the most used tool for testing web apps, so models write
     good Playwright checks.
   - Performance: fine for a check run of seconds.
   - Language boundary: check scripts are JavaScript run by Node, a
     separate process the Rust core already starts through the sandbox, so
     no new boundary inside the core.
2. **Puppeteer.** Similar, but no auto-waiting by default, so checks need
   hand-written waits; Chromium-first, with weaker other-browser support.
3. **Raw DevTools Protocol from Rust.** No Node, but every wait, click and
   query would be ours to build and maintain, and models write such checks
   poorly.

## Decision

Option 1.
- `scripts/install-browser-tools.sh` installs a SHA-256-verified Node
  tarball, `npm ci` of `bench/browser/` (exact pinned versions with a
  lockfile, install scripts off) and the Chromium headless shell into
  `/usr/local/lib/cortex-browser`. Under `/usr`, the sandbox's read-only
  bind already shows it; the only widening is a read-only `/etc/fonts`.
- `bench/browser/cortex-check.mjs` is the shared helper every check set
  carries: it serves `/work` on `127.0.0.1`, opens the page, runs the
  check, fails on any page error and always closes the browser.
- Check scripts are run with the absolute path
  `/usr/local/lib/cortex-browser/node/bin/node`, since the sandbox's PATH
  is only `/usr/bin:/bin`.

## Consequences

- Hosts that run checks need the install script once (CI does it every
  run; WSL before a real run). Only x86_64 is pinned so far.
- A hung page is stopped by the sandbox's own timeout, which kills the
  whole process group.
- Upgrading Playwright means changing `package.json`, the lockfile and
  nothing else; the browser build follows the library version.
