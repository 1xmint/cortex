import { Brain, Search, X } from 'lucide-react';
import { useEffect, useState } from 'react';
import { listMemories, searchMemories, type WorkspaceMemory } from '../../lib/cortexApi';

interface MemoryManagementProps {
  workspaceId: string;
  onClose: () => void;
}

export default function MemoryManagement({ workspaceId, onClose }: MemoryManagementProps) {
  const [query, setQuery] = useState('');
  const [memories, setMemories] = useState<WorkspaceMemory[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);

    const request = query.trim()
      ? searchMemories(query.trim(), workspaceId).then((matches) => matches.map((match) => match.memory))
      : listMemories(workspaceId, undefined, 30);

    void request
      .then((nextMemories) => {
        if (!cancelled) setMemories(nextMemories);
      })
      .catch(() => {
        if (!cancelled) setMemories([]);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [query, workspaceId]);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/75 p-3 backdrop-blur-sm">
      <section className="flex max-h-[85vh] w-full max-w-2xl flex-col rounded-xl border border-white/10 bg-[var(--panel)] shadow-2xl">
        <header className="flex items-center justify-between border-b border-white/8 p-4">
          <div className="flex items-center gap-2">
            <Brain className="h-4 w-4 text-[var(--accent)]" />
            <h2 className="text-sm font-semibold text-white">Memory Management</h2>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="inline-flex h-8 w-8 items-center justify-center rounded-lg text-[var(--muted)] transition hover:bg-white/6 hover:text-white"
            aria-label="Close memory management"
          >
            <X className="h-4 w-4" />
          </button>
        </header>

        <div className="border-b border-white/8 p-4">
          <label className="flex items-center gap-2 rounded-lg border border-white/10 bg-black/20 px-3 py-2">
            <Search className="h-4 w-4 text-[var(--muted)]" />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Search saved memory"
              className="min-w-0 flex-1 bg-transparent text-sm text-white outline-none placeholder:text-[var(--muted)]"
            />
          </label>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto p-4">
          {loading ? (
            <p className="text-sm text-[var(--muted)]">Loading memory...</p>
          ) : memories.length === 0 ? (
            <p className="text-sm text-[var(--muted)]">No saved memory found.</p>
          ) : (
            <div className="grid gap-2">
              {memories.map((memory) => (
                <article key={memory.id} className="rounded-lg border border-white/8 bg-white/[0.03] p-3">
                  <div className="mb-1 flex items-center justify-between gap-2">
                    <span className="text-xs font-medium text-[var(--accent)]">{memory.importance}</span>
                    <span className="text-[11px] text-[var(--muted)]">
                      {new Date(memory.created_at).toLocaleDateString()}
                    </span>
                  </div>
                  <p className="text-sm leading-6 text-[var(--fg)]">{memory.content}</p>
                </article>
              ))}
            </div>
          )}
        </div>
      </section>
    </div>
  );
}
