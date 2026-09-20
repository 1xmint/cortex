import { Brain } from 'lucide-react';
import { useEffect, useState } from 'react';
import { getMemoryStats, type MemoryStats } from '../../lib/cortexApi';

interface MemoryStatusProps {
  workspaceId: string;
  compact?: boolean;
}

export default function MemoryStatus({ workspaceId, compact = false }: MemoryStatusProps) {
  const [stats, setStats] = useState<MemoryStats | null>(null);

  useEffect(() => {
    let cancelled = false;

    void getMemoryStats(workspaceId)
      .then((nextStats) => {
        if (!cancelled) setStats(nextStats);
      })
      .catch(() => {
        if (!cancelled) setStats(null);
      });

    return () => {
      cancelled = true;
    };
  }, [workspaceId]);

  const total = stats?.total ?? 0;

  if (compact) {
    return (
      <div className="inline-flex h-8 items-center gap-1.5 rounded-lg border border-white/8 bg-white/[0.03] px-2 text-xs text-[var(--muted)]">
        <Brain className="h-3.5 w-3.5 text-[var(--accent)]" />
        {total}
      </div>
    );
  }

  return (
    <div className="rounded-lg border border-white/8 bg-white/[0.03] p-3 text-sm">
      <div className="flex items-center gap-2 font-medium text-white">
        <Brain className="h-4 w-4 text-[var(--accent)]" />
        Memory
      </div>
      <p className="mt-1 text-xs text-[var(--muted)]">
        {total} saved memories for this workspace.
      </p>
    </div>
  );
}
