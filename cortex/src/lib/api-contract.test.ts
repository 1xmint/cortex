import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

/**
 * The frontend must not ask for an address the backend does not answer.
 *
 * `crates/api/route-manifest.csv` is the list of every route the workspace
 * serves, and `crates/api/tests/route_ownership.rs` already refuses to let that
 * list drift from the code. This is the same check across the boundary: every
 * `/api/...` path written in `cortex/src` has to be a path the manifest serves,
 * or be named below with the reason it is not.
 *
 * Without it the two halves drift silently. They had: fourteen paths were being
 * called against routes that had never existed in any commit, five of them
 * wired to finished UI, and nothing anywhere went red.
 */

const REPO_ROOT = resolve(process.cwd(), '..');
const MANIFEST = join(REPO_ROOT, 'crates', 'api', 'route-manifest.csv');
const SRC = join(process.cwd(), 'src');

/**
 * Paths the frontend calls that the manifest does not serve, and why each one
 * is allowed to stay. This list is exact: the test fails if a path is missing
 * from it, and equally if an entry here is stale -- no longer called, or served
 * by the backend after all. An allowance you cannot forget to remove.
 */
const KNOWN_UNSERVED: Record<string, string> = {
  // Fenced behind a cargo feature that is off by default, so the manifest --
  // generated with default features -- has no soma rows at all. The frontend
  // half of the fence is SOMA_API_ENABLED, also off by default.
  // See docs/adr/ADR-0003-soma-feature-fence.md.
  '/api/soma/identity': 'soma fence',
  '/api/soma/me': 'soma fence',
  '/api/soma/revoke': 'soma fence',
  '/api/soma/session': 'soma fence',
  '/api/soma/spend': 'soma fence',
  '/api/soma/spend/{id}': 'soma fence',

  // Never implemented. Gated by MEMORY_API_ENABLED, off by default.
  '/api/memory/memories': 'memory not implemented',
  '/api/memory/memories/search': 'memory not implemented',
  '/api/memory/memories/{id}/effectiveness': 'memory not implemented',
  '/api/memory/remove': 'memory not implemented',
  '/api/memory/stats': 'memory not implemented',
  '/api/memory/suggestions': 'memory not implemented',
  '/api/memory/chat/auto-capture': 'memory not implemented',
  '/api/memory/chat/process': 'memory not implemented',
  '/api/memory/chat/suggestions/{id}/apply': 'memory not implemented',

  // Never implemented. Gated by BUDGET_API_ENABLED, off by default.
  '/api/budget/settings': 'budget not implemented',
  '/api/budget/usage': 'budget not implemented',
  '/api/budget/warnings': 'budget not implemented',
  '/api/budget/warnings/{id}/acknowledge': 'budget not implemented',

  // Never implemented. Gated by PROJECT_WORKSPACE_API_ENABLED, off by default.
  '/api/projects/validate-name': 'project workspace not implemented',
  '/api/projects/templates': 'project workspace not implemented',
  '/api/projects/import/github': 'project workspace not implemented',
  '/api/projects/{id}/files': 'project workspace not implemented',
  '/api/projects/{id}/sync': 'project workspace not implemented',
  '/api/projects/{id}/workspace': 'project workspace not implemented',

  // `/api/providers/status`, `/api/authority/delegate` and
  // `/api/groups/{id}/tasks/{id}/evidence` were unserved too, but each was a
  // lone endpoint inside a feature that otherwise works, so there was no
  // switch worth exposing -- flipping one would only turn a hidden panel into
  // a 404. Their callers no longer make the request at all: they return a
  // neutral value or refuse, with the reason on each function in cortexApi.ts.
  // Nothing calls those paths now, so they must not be listed here -- the
  // stale-allowance check below would fail if they were.
};

function sourceFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) return sourceFiles(full);
    return /\.tsx?$/.test(entry) && !/\.test\.tsx?$/.test(entry) ? [full] : [];
  });
}

/** Strip block comments, so a path discussed in prose is not read as a call. */
function stripBlockComments(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//g, '');
}

/**
 * `/api/groups/${encodeURIComponent(id)}/tasks` -> `/api/groups/{id}/tasks`.
 *
 * A `${...}` filling a whole segment is a parameter. One stuck to the end of a
 * segment is a query string being appended (`.../approvals${query}`), so it is
 * dropped along with anything a literal `?` introduces.
 */
function normalize(path: string): string {
  const withoutQuery = path.split('?')[0];
  const normalized = withoutQuery
    .replace(/\/\$\{[^}]*\}/g, '/{id}')
    .replace(/\$\{[^}]*\}/g, '')
    .replace(/\{[^}]*\}/g, '{id}')
    .replace(/\/+$/, '');
  return normalized;
}

function frontendPaths(): Map<string, string[]> {
  const found = new Map<string, string[]>();
  for (const file of sourceFiles(SRC)) {
    const source = stripBlockComments(readFileSync(file, 'utf8'));
    for (const match of source.matchAll(/['"`](\/api\/[^'"`\s]*)['"`]/g)) {
      const path = normalize(match[1]);
      // `/api/soma/*` and friends are prose, not addresses.
      if (path.includes('*') || path === '/api') continue;
      const where = found.get(path) ?? [];
      where.push(file.slice(REPO_ROOT.length + 1).replace(/\\/g, '/'));
      found.set(path, where);
    }
  }
  return found;
}

function manifestPaths(): Set<string> {
  const rows = readFileSync(MANIFEST, 'utf8').trim().split(/\r?\n/).slice(1);
  return new Set(rows.map((row) => normalize(row.split(',')[2])));
}

describe('frontend/backend route contract', () => {
  const frontend = frontendPaths();
  const backend = manifestPaths();

  it('reads both sides', () => {
    expect(backend.size).toBeGreaterThan(50);
    expect(frontend.size).toBeGreaterThan(50);
  });

  it('leaves no unresolved template holes', () => {
    const unresolved = [...frontend.keys()].filter((p) => p.includes('$'));
    expect(unresolved, 'normalize() could not read these paths').toEqual([]);
  });

  it('calls no path the backend does not serve', () => {
    const undeclared = [...frontend.entries()]
      .filter(([path]) => !backend.has(path) && !(path in KNOWN_UNSERVED))
      .map(([path, files]) => `${path}  <- ${files.join(', ')}`);

    expect(
      undeclared,
      'These paths are not in crates/api/route-manifest.csv. Add the backend ' +
        'route, or add the path to KNOWN_UNSERVED with the reason and stop the ' +
        'UI from calling it.',
    ).toEqual([]);
  });

  it('keeps no stale allowances', () => {
    const stale = Object.keys(KNOWN_UNSERVED).filter(
      (path) => !frontend.has(path) || backend.has(path),
    );

    expect(
      stale,
      'These KNOWN_UNSERVED entries are no longer earning their place: the ' +
        'frontend stopped calling them, or the backend now serves them. ' +
        'Delete the entry.',
    ).toEqual([]);
  });
});
