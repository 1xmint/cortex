import { useEffect, useMemo, useState } from 'react';
import { BrainCircuit, Loader2, Route, ShieldAlert } from 'lucide-react';
import { CortexApiError, getLedger, type LedgerEntry } from '../../lib/cortexApi';

function formatTime(timestamp: string) {
  return new Date(timestamp).toLocaleTimeString([], {
    hour: 'numeric',
    minute: '2-digit',
  });
}

function formatProvider(provider?: string, model?: string) {
  if (!provider && !model) return 'Unknown provider';
  if (!model) return provider ?? 'Unknown provider';
  return `${provider ?? 'Provider'} / ${model}`;
}

function formatRationale(rationale?: string[]) {
  if (!rationale || rationale.length === 0) return 'No rationale codes recorded';
  return rationale.map((code) => code.replaceAll('_', ' ')).join(', ');
}

function scoreLabel(score?: number) {
  if (typeof score !== 'number') return 'Score n/a';
  return `Score ${score.toFixed(1)}`;
}

function isRoutingDecision(entry: LedgerEntry) {
  return entry.event.type === 'routing_decision';
}

function LedgerCard({ entry }: { entry: LedgerEntry }) {
  const event = entry.event;
  const routing = isRoutingDecision(entry);
  const Icon = routing ? Route : event.type === 'provider_status' ? ShieldAlert : BrainCircuit;
  const alternatives = event.alternatives_considered ?? [];

  return (
    <div className="rounded-xl border border-white/8 bg-white/[0.03] px-3 py-2.5">
      <div className="flex items-start gap-2">
        <div className="mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-white/[0.04] text-[var(--muted)]">
          <Icon className="h-3.5 w-3.5" />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-center justify-between gap-2">
            <p className="truncate text-sm font-medium text-white">
              {routing ? formatProvider(event.provider, event.model) : event.type.replaceAll('_', ' ')}
            </p>
            <span className="shrink-0 text-[11px] text-[var(--muted)]">{formatTime(entry.timestamp)}</span>
          </div>
          <p className="mt-1 text-xs leading-5 text-[var(--muted)]">
            {routing
              ? `${scoreLabel(event.score)} · ${event.tier ?? 'tier n/a'} · ${formatRationale(event.rationale)}`
              : event.status
                ? `${event.status}${typeof event.duration_ms === 'number' ? ` · ${event.duration_ms}ms` : ''}`
                : 'Ledger event recorded'}
          </p>
          {routing && (
            <p className="mt-1 text-[11px] leading-5 text-[var(--muted)]">
              Alternatives: {alternatives.length > 0
                ? alternatives
                    .map((alternative) => `${alternative.provider ?? 'provider'} ${scoreLabel(alternative.score)}`)
                    .join(', ')
                : 'not recorded by backend yet'}
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

export default function LedgerView() {
  const [entries, setEntries] = useState<LedgerEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const routingDecisions = useMemo(() => entries.filter(isRoutingDecision), [entries]);

  useEffect(() => {
    let cancelled = false;

    async function fetchLedger() {
      try {
        const nextEntries = await getLedger();
        if (!cancelled) {
          setEntries(Array.isArray(nextEntries) ? nextEntries : []);
          setError(null);
        }
      } catch (err) {
        if (!cancelled) {
          if (err instanceof CortexApiError && err.status === 503) {
            setError('Starting up...');
          } else {
            setError(err instanceof Error ? err.message : 'Could not load decision ledger');
          }
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    }

    void fetchLedger();
    const interval = window.setInterval(fetchLedger, 5000);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, []);

  return (
    <section className="space-y-2">
      <div className="flex items-center justify-between gap-3">
        <div>
          <h3 className="text-xs font-medium uppercase tracking-[0.08em] text-[var(--muted)]">
            Decision Ledger
          </h3>
          <p className="mt-1 text-xs leading-5 text-[var(--muted)]">
            Routing choices, scores, and rationale codes.
          </p>
        </div>
        <span className="shrink-0 rounded-full border border-white/8 bg-white/4 px-2.5 py-1 text-[11px] text-[var(--muted)]">
          {routingDecisions.length} routes
        </span>
      </div>

      {loading ? (
        <div className="flex items-center gap-2 rounded-xl border border-white/8 bg-white/[0.02] px-3 py-3 text-xs text-[var(--muted)]">
          <Loader2 className="h-3.5 w-3.5 animate-spin" />
          Loading ledger...
        </div>
      ) : error ? (
        <div className="rounded-xl border border-amber-400/20 bg-amber-400/10 px-3 py-2 text-xs text-amber-100">
          {error}
        </div>
      ) : entries.length === 0 ? (
        <div className="rounded-xl border border-white/8 bg-white/[0.02] px-3 py-3 text-xs text-[var(--muted)]">
          No routing decisions yet.
        </div>
      ) : (
        <div className="space-y-2">
          {entries.slice(0, 6).map((entry) => (
            <LedgerCard key={entry.id} entry={entry} />
          ))}
        </div>
      )}
    </section>
  );
}
