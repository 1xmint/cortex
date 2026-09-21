import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join, resolve, sep } from 'node:path';
import { describe, expect, it } from 'vitest';

/**
 * A file nobody imports is not dead code. It is a feature nobody can reach.
 *
 * `api-contract.test.ts` is the same idea one layer down: it catches the
 * frontend calling a route the backend does not serve. This catches the other
 * shape of the same rot -- a component that was written, finished and then
 * never wired into anything, so it cannot fail, cannot be used, and drifts
 * out of date in silence.
 *
 * It found seven when it was written, including the only screen that could
 * open a pull request for a finished run, against a route the backend serves.
 * That one is settled: the button lives on RunsPane now and the old screen is
 * deleted, which is what an entry below is supposed to end in.
 *
 * Every unreachable file has to be named below with what is true about it and
 * what would resolve it. That is the point: an orphan you have to write a
 * sentence about is an orphan somebody decides on.
 */

const SRC = resolve(process.cwd(), 'src');

/**
 * Files nothing imports, why each one is still here, and what settles it.
 *
 * Exact, like the allowance list in `api-contract.test.ts`: a new orphan fails
 * the test, and so does an entry here that got wired up or deleted.
 */
const KNOWN_UNREACHABLE: Record<string, string> = {
  // Finished, and every route it needs is in route-manifest.csv: GET
  // /api/usage and GET /api/usage/daily. It shows real spend and needs no
  // backend work at all -- only a route and a way in. Wire it or delete it.
  'components/usage/UsageView.tsx': 'built, backend served, never wired',

  // Same: GET /api/github/repos, POST /api/github/import and GET
  // /api/github/status/{import_id} are all served. Importing a repo is
  // reachable nowhere in the running app.
  'components/onboarding/RepoImport.tsx': 'built, backend served, never wired',

  // The budget UI. Unreachable because /api/budget/* has never existed in any
  // commit -- see BUDGET_API_ENABLED in cortexApi.ts. These stay until that
  // backend is built or the feature is dropped; they are the reason the
  // decision is worth making rather than a pile to clear.
  'components/CostGauge.tsx': 'budget UI, no backend',
  'components/cost/CostStatus.tsx': 'budget UI, no backend',
  'components/cost/CostWarning.tsx': 'budget UI, no backend',
  'components/cost/EnhancedCostGauge.tsx': 'budget UI, no backend',
  'lib/useCostAwareness.ts': 'budget UI, no backend',
};

const rel = (file: string): string => file.slice(SRC.length + 1).split(sep).join('/');

function sourceFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) return sourceFiles(full);
    return /\.tsx?$/.test(entry) ? [full] : [];
  });
}

/**
 * Where the app starts, plus every test. A file a test reaches is reachable:
 * something runs it, which is the thing being checked.
 */
function entryPoints(files: string[]): string[] {
  return files.filter((f) => /^(main|App)\.tsx?$/.test(rel(f)) || /\.test\.tsx?$/.test(f));
}

/** `./Foo` -> the real file, trying the extensions a bundler would try. */
function resolveImport(fromFile: string, spec: string): string | null {
  if (!spec.startsWith('.')) return null;
  const base = resolve(dirname(fromFile), spec);
  const candidates = [base, `${base}.ts`, `${base}.tsx`, join(base, 'index.ts'), join(base, 'index.tsx')];
  return candidates.find((c) => existsSync(c) && statSync(c).isFile()) ?? null;
}

function reachable(files: string[]): Set<string> {
  const seen = new Set<string>();
  const queue = entryPoints(files);
  while (queue.length > 0) {
    const file = queue.pop() as string;
    if (seen.has(file)) continue;
    seen.add(file);
    const source = readFileSync(file, 'utf8');
    // Covers `import x from '...'`, `export ... from '...'` and `import('...')`.
    for (const match of source.matchAll(/(?:from|import)\s*\(?\s*['"]([^'"]+)['"]/g)) {
      const target = resolveImport(file, match[1]);
      if (target !== null && !seen.has(target)) queue.push(target);
    }
  }
  return seen;
}

describe('every file is reachable', () => {
  const files = sourceFiles(SRC);
  const seen = reachable(files);
  const orphans = files.filter((f) => !seen.has(f)).map(rel).sort();

  it('reads the import graph', () => {
    expect(entryPoints(files).map(rel)).toContain('App.tsx');
    expect(seen.size).toBeGreaterThan(50);
  });

  it('has no unreachable file that is not accounted for', () => {
    const unaccounted = orphans.filter((f) => !(f in KNOWN_UNREACHABLE));

    expect(
      unaccounted,
      'Nothing imports these, so nothing in the running app can reach them. ' +
        'Wire each one up, delete it, or add it to KNOWN_UNREACHABLE with what ' +
        'is true about it and what would settle it.',
    ).toEqual([]);
  });

  it('keeps no stale entry', () => {
    const stale = Object.keys(KNOWN_UNREACHABLE).filter((f) => !orphans.includes(f));

    expect(
      stale,
      'These KNOWN_UNREACHABLE entries no longer describe anything: the file ' +
        'was wired up, renamed or deleted. Delete the entry.',
    ).toEqual([]);
  });
});
