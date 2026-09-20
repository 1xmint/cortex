import { Clock3, Lock, ShieldAlert } from 'lucide-react';
import type { ResourceLeaseConflict } from '../../lib/cortexApi';

interface ConflictViewerProps {
  conflicts: ResourceLeaseConflict[];
}

function secondsUntilExpiry(expiresAt: number): number {
  return Math.max(0, Math.round((expiresAt * 1000 - Date.now()) / 1000));
}

function formatCountdown(seconds: number): string {
  if (seconds <= 0) return 'expired';
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  const remaining = seconds % 60;
  if (minutes < 60) return `${minutes}m ${remaining}s`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${minutes % 60}m`;
}

function conflictTone(secondsLeft: number): string {
  if (secondsLeft <= 0) return 'border-zinc-500/20 bg-zinc-500/10 text-zinc-300';
  if (secondsLeft < 120) return 'border-amber-300/20 bg-amber-300/10 text-amber-100';
  return 'border-red-300/20 bg-red-400/10 text-red-100';
}

function resourcePath(conflict: ResourceLeaseConflict): string {
  return `${conflict.repo_key}:${conflict.resource_key}`;
}

export default function ConflictViewer({ conflicts }: ConflictViewerProps) {
  if (conflicts.length === 0) return null;

  return (
    <div className="rounded-md border border-white/8 bg-black/10 p-2.5">
      <div className="flex items-center gap-1.5 text-xs font-medium text-[var(--muted-strong)]">
        <ShieldAlert className="h-3.5 w-3.5 text-amber-300" />
        Resource Conflicts
      </div>

      <div className="mt-1.5 rounded-md border border-amber-300/20 bg-amber-300/10 px-2.5 py-1.5">
        <p className="text-[11px] leading-4 text-amber-100">
          Waiting for resource{conflicts.length > 1 ? 's' : ''}... {conflicts.length} active conflict{conflicts.length > 1 ? 's' : ''} blocking this run.
        </p>
      </div>

      <div className="mt-2 max-h-36 space-y-1.5 overflow-y-auto pr-1">
        {conflicts.map((conflict) => {
          const remaining = secondsUntilExpiry(conflict.expires_at);
          const tone = conflictTone(remaining);
          return (
            <div key={conflict.lease_id} className={`rounded-md border px-2 py-1.5 ${tone}`}>
              <div className="flex items-center justify-between gap-2">
                <div className="flex min-w-0 items-center gap-1.5">
                  <Lock className="h-3 w-3 shrink-0" />
                  <span className="truncate text-[11px] font-medium">
                    {resourcePath(conflict)}
                  </span>
                </div>
                <div className="flex shrink-0 items-center gap-1 text-[10px]">
                  <Clock3 className="h-3 w-3" />
                  {formatCountdown(remaining)}
                </div>
              </div>
              <div className="mt-1 flex items-center gap-2 text-[10px] leading-4 opacity-80">
                <span className="capitalize">{conflict.mode} lock</span>
                <span>holder: {conflict.holder_type}</span>
                <span className="truncate">run {conflict.run_id.slice(0, 8)}</span>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
