// Shared helper for browser checks (ADR 0006). Copied into every frozen
// check set as files/lib/cortex-check.mjs; check scripts import it from
// /checks/files/lib/cortex-check.mjs and run inside the sandbox.
//
// It serves /work over HTTP on loopback, opens the page in headless
// Chromium, runs the check's function, and fails on any page error.

import http from "node:http";
import fs from "node:fs";
import path from "node:path";

const PREFIX = "/usr/local/lib/cortex-browser";
const ROOT = "/work";

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".svg": "image/svg+xml",
  ".ico": "image/x-icon",
  ".wav": "audio/wav",
  ".mp3": "audio/mpeg",
  ".woff2": "font/woff2",
};

function serve() {
  const server = http.createServer((req, res) => {
    const url = new URL(req.url, "http://127.0.0.1");
    const rel = path.normalize(decodeURIComponent(url.pathname)).replace(/^(\.\.[/\\])+/, "");
    let file = path.join(ROOT, rel);
    if (!file.startsWith(ROOT)) {
      res.writeHead(403).end();
      return;
    }
    if (fs.existsSync(file) && fs.statSync(file).isDirectory()) file = path.join(file, "index.html");
    fs.readFile(file, (err, body) => {
      if (err) {
        res.writeHead(404).end();
        return;
      }
      res.writeHead(200, { "content-type": TYPES[path.extname(file)] ?? "application/octet-stream" });
      res.end(body);
    });
  });
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server)));
}

let failed = false;

/** Prints PASS or FAIL for one assertion; a failure sets exit code 1. */
export function check(name, cond, detail = "") {
  if (cond) {
    console.log(`PASS ${name}`);
  } else {
    failed = true;
    process.exitCode = 1;
    console.log(`FAIL ${name}: ${detail}`);
  }
}

/**
 * Opens `entry` from /work in headless Chromium and runs `fn(page, ctx)`.
 * `ctx.url(p)` gives the address of another file under /work. Any page
 * error or console error fails the check, even if `fn` succeeded.
 */
export async function withPage(fn, { entry = "index.html", timeoutMs = 60000 } = {}) {
  process.env.PLAYWRIGHT_BROWSERS_PATH = `${PREFIX}/browsers`;
  const { chromium } = await import(`${PREFIX}/node_modules/playwright/index.mjs`);
  const server = await serve();
  const base = `http://127.0.0.1:${server.address().port}/`;
  const errors = [];
  const browser = await chromium.launch({
    args: ["--no-sandbox", "--disable-dev-shm-usage", "--disable-gpu"],
    timeout: timeoutMs,
  });
  try {
    const context = await browser.newContext({ acceptDownloads: true });
    context.setDefaultTimeout(timeoutMs);
    const page = await context.newPage();
    page.on("pageerror", (e) => errors.push(`page error: ${e.message}`));
    page.on("console", (m) => {
      if (m.type() === "error") errors.push(`console error: ${m.text()}`);
    });
    await page.goto(base + entry, { timeout: timeoutMs });
    await fn(page, { url: (p) => base + p });
  } catch (e) {
    check("check script ran", false, String(e?.stack ?? e));
  } finally {
    await browser.close().catch(() => {});
    server.close();
  }
  check("no page errors", errors.length === 0, errors.join("; "));
  if (failed) process.exitCode = 1;
}
