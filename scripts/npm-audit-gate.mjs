#!/usr/bin/env node
/**
 * npm audit as a gate, with justified exceptions.
 *
 * `npm audit` has no ignore file, so the choice has been between a check that
 * is red forever and no check at all. This repo already learned where that
 * ends: the `web` job could not fail, the midnight flake failed for reasons
 * nobody could action, and CI stopped being read.
 *
 * So: every unresolved advisory must be written down with a reason and a
 * review date. Anything not on the list fails the build. An expired entry
 * fails the build too — an exception nobody has revisited is an exception
 * nobody is accountable for.
 *
 * Usage: node scripts/npm-audit-gate.mjs <app-dir> [--level=high]
 */

import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const SEVERITY_ORDER = ['info', 'low', 'moderate', 'high', 'critical'];

const here = dirname(fileURLToPath(import.meta.url));
const allowlistPath = join(here, '..', '.github', 'npm-audit-allowlist.json');

const [, , appDirArg, ...flags] = process.argv;
if (!appDirArg) {
  console.error('usage: npm-audit-gate.mjs <app-dir> [--level=high]');
  process.exit(2);
}

const appDir = resolve(appDirArg);
const app = appDirArg.replace(/[\\/]+$/, '').split(/[\\/]/).pop();
const levelFlag = flags.find((flag) => flag.startsWith('--level='));
const minimumSeverity = levelFlag ? levelFlag.split('=')[1] : 'high';
const minimumIndex = SEVERITY_ORDER.indexOf(minimumSeverity);
if (minimumIndex < 0) {
  console.error(`unknown severity: ${minimumSeverity}`);
  process.exit(2);
}

function runAudit() {
  try {
    // npm audit exits non-zero when it finds anything, so a throw here is the
    // normal path and the payload still arrives on stdout.
    // --omit=dev: a devDependency advisory does not reach production, and
    // including them is what makes audit output unreadable and then ignored.
    // Build-time supply-chain risk is real, but it belongs to the sandbox and
    // secret-scanning workstreams, not to a gate on shipped code.
    return execFileSync('npm', ['audit', '--json', '--omit=dev'], {
      cwd: appDir,
      encoding: 'utf8',
      maxBuffer: 64 * 1024 * 1024,
      shell: process.platform === 'win32',
    });
  } catch (error) {
    if (error.stdout) return error.stdout;
    throw error;
  }
}

function loadAllowlist() {
  let raw;
  try {
    raw = readFileSync(allowlistPath, 'utf8');
  } catch {
    return {};
  }
  const parsed = JSON.parse(raw);
  return parsed[app] ?? {};
}

const report = JSON.parse(runAudit());
const allowlist = loadAllowlist();
const today = new Date().toISOString().slice(0, 10);

const blocking = [];
const accepted = [];
const expired = [];

for (const [name, vulnerability] of Object.entries(report.vulnerabilities ?? {})) {
  if (SEVERITY_ORDER.indexOf(vulnerability.severity) < minimumIndex) continue;

  const exception = allowlist[name];
  if (!exception) {
    blocking.push({ name, vulnerability });
    continue;
  }
  if (exception.review_by && exception.review_by < today) {
    expired.push({ name, exception });
    continue;
  }
  accepted.push({ name, exception });
}

const titleOf = (vulnerability) =>
  (vulnerability.via ?? [])
    .map((via) => (typeof via === 'string' ? via : via.title))
    .filter(Boolean)
    .join('; ');

for (const { name, exception } of accepted) {
  console.log(`accepted  ${name} — ${exception.reason} (review by ${exception.review_by})`);
}
for (const { name, exception } of expired) {
  console.error(`EXPIRED   ${name} — exception lapsed on ${exception.review_by}; re-justify or fix`);
}
for (const { name, vulnerability } of blocking) {
  console.error(`BLOCKING  ${name} (${vulnerability.severity}) — ${titleOf(vulnerability)}`);
}

if (blocking.length === 0 && expired.length === 0) {
  console.log(`npm audit gate passed for ${app} at >=${minimumSeverity} (${accepted.length} accepted)`);
  process.exit(0);
}

console.error(
  `\n${blocking.length} unreviewed and ${expired.length} expired advisories at >=${minimumSeverity}.` +
    `\nFix them, or add an entry with a reason and a review date to .github/npm-audit-allowlist.json.`,
);
process.exit(1);
