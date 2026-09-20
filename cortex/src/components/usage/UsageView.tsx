import { useEffect, useMemo, useState } from 'react';
import { BarChart3, Loader2 } from 'lucide-react';
import { CortexApiError, getDailyUsage, getUsage, type DailyUsage, type UsageSummary } from '../../lib/cortexApi';

function readNumber(value: unknown, keys: string[]) {
  if (typeof value === 'number') return value;
  if (!value || typeof value !== 'object') return 0;
  const record = value as Record<string, unknown>;
  for (const key of keys) {
    if (typeof record[key] === 'number') return record[key];
  }
  return 0;
}

function formatCost(value: number) {
  return new Intl.NumberFormat([], {
    style: 'currency',
    currency: 'USD',
    maximumFractionDigits: value < 1 ? 4 : 2,
  }).format(value);
}

function formatCompact(value: number) {
  return new Intl.NumberFormat([], {
    notation: 'compact',
    maximumFractionDigits: 1,
  }).format(value);
}

function getGateLabel(gate: unknown) {
  if (!gate) return 'No gate';
  if (typeof gate === 'string') return gate;
  if (typeof gate === 'object') {
    const record = gate as Record<string, unknown>;
    if (typeof record.status === 'string') return record.status;
    if (typeof record.state === 'string') return record.state;
  }
  return 'Active';
}

export default function UsageView() {
  const [summary, setSummary] = useState<UsageSummary | null>(null);
  const [daily, setDaily] = useState<DailyUsage[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const totals = useMemo(() => {
    const fallbackCost = daily.reduce((sum, item) => sum + (item.cost ?? 0), 0);
    const fallbackSteps = daily.reduce((sum, item) => sum + (item.steps ?? 0), 0);
    const fallbackTokens = daily.reduce(
      (sum, item) => sum + (item.tokens_in ?? 0) + (item.tokens_out ?? 0),
      0,
    );
    return {
      cost: readNumber(summary?.last_30d, ['cost', 'cost_usd', 'estimated_cost_usd']) || fallbackCost,
      steps: readNumber(summary?.last_30d, ['steps', 'step_count']) || fallbackSteps,
      tokens: readNumber(summary?.last_30d, ['tokens', 'total_tokens']) || fallbackTokens,
    };
  }, [daily, summary]);

  useEffect(() => {
    let cancelled = false;

    async function fetchUsage() {
      try {
        const [nextSummary, nextDaily] = await Promise.all([getUsage(), getDailyUsage(14)]);
        if (!cancelled) {
          setSummary(nextSummary);
          setDaily(Array.isArray(nextDaily) ? nextDaily : []);
          setError(null);
        }
      } catch (err) {
        if (!cancelled) {
          if (err instanceof CortexApiError && err.status === 503) {
            setError('Starting up...');
          } else {
            setError(err instanceof Error ? err.message : 'Could not load usage');
          }
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    }

    void fetchUsage();
    const interval = window.setInterval(fetchUsage, 30_000);
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
            Usage
          </h3>
          <p className="mt-1 text-xs leading-5 text-[var(--muted)]">
            Last 30 days and current gate state.
          </p>
        </div>
        <span className="shrink-0 rounded-full border border-white/8 bg-white/4 px-2.5 py-1 text-[11px] capitalize text-[var(--muted)]">
          {getGateLabel(summary?.gate)}
        </span>
      </div>

      {loading ? (
        <div className="flex items-center gap-2 rounded-xl border border-white/8 bg-white/[0.02] px-3 py-3 text-xs text-[var(--muted)]">
          <Loader2 className="h-3.5 w-3.5 animate-spin" />
          Loading usage...
        </div>
      ) : error ? (
        <div className="rounded-xl border border-amber-400/20 bg-amber-400/10 px-3 py-2 text-xs text-amber-100">
          {error}
        </div>
      ) : (
        <div className="rounded-xl border border-white/8 bg-white/[0.03] p-3">
          <div className="grid grid-cols-3 gap-2">
            {[
              { label: 'Cost', value: formatCost(totals.cost) },
              { label: 'Tokens', value: formatCompact(totals.tokens) },
              { label: 'Steps', value: formatCompact(totals.steps) },
            ].map((item) => (
              <div key={item.label} className="rounded-lg border border-white/6 bg-black/10 px-2 py-2">
                <p className="text-[10px] uppercase tracking-[0.08em] text-[var(--muted)]">{item.label}</p>
                <p className="mt-1 truncate text-sm font-medium text-white">{item.value}</p>
              </div>
            ))}
          </div>
          <div className="mt-3 flex h-12 items-end gap-1">
            {daily.slice(-14).map((item) => {
              const maxSteps = Math.max(...daily.map((day) => day.steps ?? 0), 1);
              const height = Math.max(8, ((item.steps ?? 0) / maxSteps) * 48);
              return (
                <div key={item.date} className="flex min-w-0 flex-1 flex-col items-center gap-1">
                  <div
                    className="w-full rounded-t bg-[var(--accent)]/70"
                    style={{ height }}
                    title={`${item.date}: ${item.steps ?? 0} steps`}
                  />
                </div>
              );
            })}
            {daily.length === 0 && <BarChart3 className="h-5 w-5 text-[var(--muted)]" />}
          </div>
        </div>
      )}
    </section>
  );
}
