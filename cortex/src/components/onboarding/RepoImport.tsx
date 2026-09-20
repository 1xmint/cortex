import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  AlertTriangle,
  CheckCircle,
  Download,
  GitBranch,
  Loader2,
  Lock,
  RefreshCw,
  Search,
} from 'lucide-react';
import {
  CortexApiError,
  getGitHubImportStatus,
  importGitHubRepo,
  listGitHubImports,
  listGitHubRepos,
  syncGitHubRepo,
  type GitHubImportStatus,
  type GitHubRepo,
} from '../../lib/cortexApi';

interface RepoImportProps {
  /** Called after a repo finishes importing successfully. */
  onImported?: (status: GitHubImportStatus) => void;
}

const TERMINAL_STATES = new Set(['ready', 'failed']);

/**
 * Browse GitHub repos, import one into the user's Cortex container with live
 * progress, and sync imported repos back to GitHub. Designed to feel like
 * GitHub's repo picker, with fast feedback during import.
 */
export default function RepoImport({ onImported }: RepoImportProps) {
  const [loading, setLoading] = useState(true);
  const [linked, setLinked] = useState(false);
  const [repos, setRepos] = useState<GitHubRepo[]>([]);
  const [imports, setImports] = useState<Record<string, GitHubImportStatus>>({});
  const [query, setQuery] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [syncing, setSyncing] = useState<Set<string>>(new Set());
  const [syncMsg, setSyncMsg] = useState<Record<string, string>>({});

  const pollers = useRef<Map<string, ReturnType<typeof setInterval>>>(new Map());

  // Index imports by repo full_name for quick lookup in the repo list.
  const importsByRepo = useMemo(() => {
    const map: Record<string, GitHubImportStatus> = {};
    for (const imp of Object.values(imports)) map[imp.repo_full_name] = imp;
    return map;
  }, [imports]);

  const upsertImport = useCallback((status: GitHubImportStatus) => {
    setImports((prev) => ({ ...prev, [status.import_id]: status }));
  }, []);

  const stopPolling = useCallback((importId: string) => {
    const handle = pollers.current.get(importId);
    if (handle) {
      clearInterval(handle);
      pollers.current.delete(importId);
    }
  }, []);

  const pollImport = useCallback(
    (importId: string) => {
      if (pollers.current.has(importId)) return;
      const handle = setInterval(async () => {
        try {
          const status = await getGitHubImportStatus(importId);
          upsertImport(status);
          if (TERMINAL_STATES.has(status.status)) {
            stopPolling(importId);
            if (status.status === 'ready') onImported?.(status);
          }
        } catch {
          // Transient errors are ignored; polling continues until terminal.
        }
      }, 1500);
      pollers.current.set(importId, handle);
    },
    [onImported, stopPolling, upsertImport],
  );

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [reposRes, existing] = await Promise.all([
        listGitHubRepos(),
        listGitHubImports().catch(() => [] as GitHubImportStatus[]),
      ]);
      setLinked(reposRes.linked);
      setRepos(reposRes.repos);
      const map: Record<string, GitHubImportStatus> = {};
      for (const imp of existing) {
        map[imp.import_id] = imp;
        // Resume polling for any import still in flight.
        if (!TERMINAL_STATES.has(imp.status)) pollImport(imp.import_id);
      }
      setImports(map);
    } catch (e) {
      setError(e instanceof CortexApiError ? e.message : 'Failed to load repositories');
    } finally {
      setLoading(false);
    }
  }, [pollImport]);

  useEffect(() => {
    load();
    const active = pollers.current;
    return () => {
      for (const handle of active.values()) clearInterval(handle);
      active.clear();
    };
  }, [load]);

  const handleImport = useCallback(
    async (repo: GitHubRepo) => {
      setError(null);
      // Optimistic placeholder so the UI reflects the import immediately.
      try {
        const res = await importGitHubRepo(repo.full_name);
        upsertImport({
          import_id: res.import_id,
          repo_full_name: repo.full_name,
          status: 'importing',
          progress: 5,
          stage: 'starting',
          clone_path: res.clone_path,
          error: null,
          last_synced_at: null,
          head_commit: null,
        });
        pollImport(res.import_id);
      } catch (e) {
        setError(e instanceof CortexApiError ? e.message : `Failed to import ${repo.full_name}`);
      }
    },
    [pollImport, upsertImport],
  );

  const handleSync = useCallback(async (imp: GitHubImportStatus) => {
    setSyncing((prev) => new Set(prev).add(imp.import_id));
    setSyncMsg((prev) => ({ ...prev, [imp.import_id]: '' }));
    try {
      const res = await syncGitHubRepo(imp.import_id);
      setSyncMsg((prev) => ({
        ...prev,
        [imp.import_id]: res.conflict ? `Conflict: ${res.detail}` : res.detail,
      }));
    } catch (e) {
      setSyncMsg((prev) => ({
        ...prev,
        [imp.import_id]: e instanceof CortexApiError ? e.message : 'Sync failed',
      }));
    } finally {
      setSyncing((prev) => {
        const next = new Set(prev);
        next.delete(imp.import_id);
        return next;
      });
    }
  }, []);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return repos;
    return repos.filter(
      (r) =>
        r.full_name.toLowerCase().includes(q) ||
        (r.description?.toLowerCase().includes(q) ?? false),
    );
  }, [query, repos]);

  if (loading) {
    return (
      <div className="flex items-center justify-center gap-2 p-8 text-sm text-[var(--muted)]">
        <Loader2 className="h-4 w-4 animate-spin" />
        Loading repositories...
      </div>
    );
  }

  if (!linked) {
    return (
      <div className="flex flex-col items-center gap-4 rounded-xl border border-white/8 bg-white/4 px-6 py-8">
        <GitBranch className="h-10 w-10 text-[var(--muted)]" />
        <p className="text-center text-sm text-[var(--muted)]">
          Link your GitHub account in profile settings to browse and import your
          repositories.
        </p>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-3">
      <div className="relative">
        <Search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-[var(--muted)]" />
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search repositories..."
          className="w-full rounded-lg border border-white/8 bg-white/4 py-2 pl-9 pr-3 text-sm text-white placeholder:text-[var(--muted)] focus:border-[var(--accent)]/40 focus:outline-none"
        />
      </div>

      {error && (
        <div className="flex items-center gap-2 rounded-lg border border-red-500/20 bg-red-500/10 px-3 py-2 text-xs text-red-300">
          <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
          {error}
        </div>
      )}

      <div className="max-h-80 divide-y divide-white/6 overflow-y-auto rounded-xl border border-white/8">
        {filtered.length === 0 ? (
          <p className="px-4 py-6 text-center text-sm text-[var(--muted)]">
            No repositories match your search.
          </p>
        ) : (
          filtered.map((repo) => {
            const imp = importsByRepo[repo.full_name];
            const importing = imp && imp.status !== 'ready' && imp.status !== 'failed';
            const ready = imp?.status === 'ready';
            const failed = imp?.status === 'failed';
            const isSyncing = imp ? syncing.has(imp.import_id) : false;
            const msg = imp ? syncMsg[imp.import_id] : undefined;

            return (
              <div key={repo.id} className="px-4 py-3">
                <div className="flex items-center gap-3">
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="truncate text-sm font-medium text-white">
                        {repo.full_name}
                      </span>
                      {repo.private && (
                        <Lock className="h-3 w-3 shrink-0 text-[var(--muted)]" />
                      )}
                      {repo.language && (
                        <span className="shrink-0 text-[10px] text-[var(--muted)]">
                          {repo.language}
                        </span>
                      )}
                    </div>
                    {repo.description && (
                      <p className="truncate text-xs text-[var(--muted)]">
                        {repo.description}
                      </p>
                    )}
                  </div>

                  {!imp && (
                    <button
                      onClick={() => handleImport(repo)}
                      className="inline-flex shrink-0 items-center gap-1.5 rounded-lg bg-[var(--accent-soft)] px-3 py-1.5 text-xs font-medium text-[var(--accent)] transition hover:bg-[var(--accent)]/20"
                    >
                      <Download className="h-3.5 w-3.5" />
                      Import
                    </button>
                  )}

                  {ready && (
                    <button
                      onClick={() => handleSync(imp)}
                      disabled={isSyncing}
                      className="inline-flex shrink-0 items-center gap-1.5 rounded-lg border border-white/10 px-3 py-1.5 text-xs font-medium text-white transition hover:bg-white/5 disabled:opacity-50"
                    >
                      <RefreshCw className={`h-3.5 w-3.5 ${isSyncing ? 'animate-spin' : ''}`} />
                      {isSyncing ? 'Syncing' : 'Sync'}
                    </button>
                  )}

                  {failed && (
                    <button
                      onClick={() => handleImport(repo)}
                      className="inline-flex shrink-0 items-center gap-1.5 rounded-lg border border-red-500/20 px-3 py-1.5 text-xs font-medium text-red-300 transition hover:bg-red-500/10"
                    >
                      <RefreshCw className="h-3.5 w-3.5" />
                      Retry
                    </button>
                  )}
                </div>

                {importing && imp && (
                  <div className="mt-2">
                    <div className="mb-1 flex items-center justify-between text-[10px] text-[var(--muted)]">
                      <span>{imp.stage}</span>
                      <span>{imp.progress}%</span>
                    </div>
                    <div className="h-1 overflow-hidden rounded-full bg-white/10">
                      <div
                        className="h-full bg-[var(--accent)] transition-all duration-500"
                        style={{ width: `${imp.progress}%` }}
                      />
                    </div>
                  </div>
                )}

                {ready && (
                  <div className="mt-1.5 flex items-center gap-1 text-[10px] text-emerald-300">
                    <CheckCircle className="h-3 w-3" />
                    Imported{imp?.head_commit ? ` · ${imp.head_commit.slice(0, 7)}` : ''}
                  </div>
                )}

                {failed && imp?.error && (
                  <p className="mt-1.5 text-[10px] text-red-300">{imp.error}</p>
                )}

                {msg && (
                  <p className="mt-1.5 text-[10px] text-[var(--muted)]">{msg}</p>
                )}
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
