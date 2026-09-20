import { Loader2 } from 'lucide-react';
import { useEffect, useState } from 'react';
import { getBillingHistory, type BillingHistoryEntry } from '../../lib/cortexApi';
import { formatDate, formatMoney } from './format';

export default function BillingHistory() {
  const [items, setItems] = useState<BillingHistoryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    getBillingHistory()
      .then((history) => {
        if (!cancelled) {
          setItems(history);
          setError(null);
        }
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : 'Could not load billing history');
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  if (loading) {
    return <p className="flex items-center gap-2 text-sm text-[var(--muted)]"><Loader2 className="h-4 w-4 animate-spin" />Loading history...</p>;
  }

  if (error) {
    return <p className="rounded-lg border border-red-400/15 bg-red-400/8 px-3 py-2 text-sm text-red-100">{error}</p>;
  }

  if (items.length === 0) {
    return <p className="rounded-lg border border-white/8 bg-white/[0.03] px-3 py-4 text-center text-sm text-[var(--muted)]">No transactions yet.</p>;
  }

  return (
    <div className="overflow-hidden rounded-xl border border-white/8">
      {items.map((item) => (
        <div key={`${item.date}-${item.description}`} className="grid grid-cols-[0.8fr_1fr_0.6fr] gap-2 border-b border-white/6 px-3 py-2.5 text-xs last:border-b-0">
          <span className="text-[var(--muted)]">{formatDate(item.date)}</span>
          <span className="truncate text-[var(--muted-strong)]">{item.description}</span>
          <span className="text-right text-white">{formatMoney(item.amount_cents)}</span>
        </div>
      ))}
    </div>
  );
}
