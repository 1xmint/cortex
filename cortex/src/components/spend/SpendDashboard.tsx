import { ChevronRight, Loader2, ReceiptText, WalletCards } from 'lucide-react';
import { useState } from 'react';
import { useSomaDelegationSpend, useSomaSpend } from '../../lib/useSomaSpend';

function mask(value: string) {
  if (value.length <= 18) return value;
  return `${value.slice(0, 8)}...${value.slice(-6)}`;
}

function money(value: number) {
  return new Intl.NumberFormat(undefined, {
    style: 'currency',
    currency: 'USD',
    maximumFractionDigits: value < 1 ? 4 : 2,
  }).format(value);
}

function time(value: number | null | undefined) {
  if (!value) return 'No activity';
  return new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' })
    .format(new Date(value));
}

export default function SpendDashboard() {
  const { data, loading, error } = useSomaSpend();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const detail = useSomaDelegationSpend(selectedId);
  const selected = data?.delegations.find((item) => item.delegation_id === selectedId) ?? null;

  if (loading) {
    return (
      <div className="flex items-center justify-center gap-2 py-10 text-sm text-[var(--muted)]">
        <Loader2 className="h-4 w-4 animate-spin" />
        Loading spend...
      </div>
    );
  }

  if (error) {
    return (
      <div className="rounded-xl border border-red-400/15 bg-red-400/8 px-4 py-3 text-sm text-red-100">
        {error}
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <div className="rounded-xl border border-[var(--accent)]/15 bg-[var(--accent)]/8 p-4">
        <div className="flex items-center justify-between gap-3">
          <div>
            <p className="text-xs uppercase tracking-[0.08em] text-[var(--muted)]">Total Soma spend</p>
            <p className="mt-1 text-2xl font-semibold text-white">{money(data?.total_spend ?? 0)}</p>
          </div>
          <WalletCards className="h-6 w-6 text-[var(--accent)]" />
        </div>
      </div>

      <div className="overflow-hidden rounded-xl border border-white/8">
        <div className="grid grid-cols-[1.1fr_0.9fr_0.7fr] gap-2 border-b border-white/6 bg-white/[0.03] px-3 py-2 text-[10px] uppercase tracking-[0.08em] text-[var(--muted)]">
          <span>Delegation</span>
          <span>Capability</span>
          <span className="text-right">Spend</span>
        </div>
        {(data?.delegations.length ?? 0) === 0 ? (
          <p className="px-3 py-6 text-center text-sm text-[var(--muted)]">No spend receipts yet.</p>
        ) : (
          data?.delegations.map((item) => (
            <button
              type="button"
              key={item.delegation_id}
              onClick={() => setSelectedId(item.delegation_id)}
              className={`grid w-full grid-cols-[1.1fr_0.9fr_0.7fr] items-center gap-2 border-b border-white/6 px-3 py-3 text-left transition last:border-b-0 hover:bg-white/[0.04] active:scale-[0.99] ${
                selectedId === item.delegation_id ? 'bg-white/[0.04]' : ''
              }`}
            >
              <span className="min-w-0">
                <span className="block truncate font-mono text-xs text-white">{mask(item.delegation_id)}</span>
                <span className="mt-1 block text-[11px] text-[var(--muted)]">
                  {item.receipt_count} receipts · {time(item.last_activity_ms)}
                </span>
              </span>
              <span className="truncate text-xs text-[var(--muted-strong)]">
                {item.capabilities[0] ?? 'delegated'}
              </span>
              <span className="flex items-center justify-end gap-1 text-sm font-medium text-white">
                {money(item.cumulative_spend)}
                <ChevronRight className="h-3.5 w-3.5 text-[var(--muted)]" />
              </span>
            </button>
          ))
        )}
      </div>

      {selected && (
        <div className="rounded-xl border border-white/8 bg-white/[0.02] p-4">
          <div className="mb-3 flex items-center justify-between gap-2">
            <div>
              <p className="text-sm font-medium text-white">{mask(selected.delegation_id)}</p>
              <p className="mt-0.5 text-xs text-[var(--muted)]">{mask(selected.subject_did)}</p>
            </div>
            <ReceiptText className="h-4 w-4 text-[var(--accent)]" />
          </div>
          {detail.loading ? (
            <p className="text-sm text-[var(--muted)]">Loading receipts...</p>
          ) : detail.error ? (
            <p className="text-sm text-red-200">{detail.error}</p>
          ) : (
            <div className="space-y-2">
              {(detail.data?.receipts ?? []).map((receipt, index) => (
                <div key={`${receipt.timestamp}-${index}`} className="flex items-center justify-between gap-3 rounded-lg bg-white/[0.03] px-3 py-2">
                  <span className="min-w-0">
                    <span className="block truncate text-xs text-white">{receipt.capability}</span>
                    <span className="block text-[11px] text-[var(--muted)]">{time(receipt.timestamp)}</span>
                  </span>
                  <span className="text-right text-xs text-[var(--muted-strong)]">
                    {money(receipt.amount)}
                    <span className="block text-[10px] text-[var(--muted)]">{money(receipt.cumulative)} total</span>
                  </span>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
