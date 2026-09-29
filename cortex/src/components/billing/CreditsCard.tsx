import { Loader2, Wallet } from 'lucide-react';
import { useEffect, useState } from 'react';
import { createTopupCheckout, getCreditsBalance } from '../../lib/cortexApi';
import { creditsForDollars, formatCreditGrant, formatDollars } from '../../lib/formatDollars';

// Must match `credits_for_topup_amount` (crates/api/src/billing.rs).
const TOPUP_AMOUNTS_USD = [10, 25, 50, 100] as const;

/**
 * Balance and top-up. The balance is shown in dollars. The purchase buttons are
 * the one place a raw credit count is stated, because that is the grant the
 * dollars buy: "Pay $25.00 · get 250 credits".
 */
export default function CreditsCard() {
  const [balance, setBalance] = useState<number | null>(null);
  const [pending, setPending] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    getCreditsBalance()
      .then((credits) => {
        if (!cancelled) setBalance(credits);
      })
      .catch(() => {
        // No balance is not an error worth a banner; the buttons still work.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function buy(amountUsd: number) {
    setError(null);
    setPending(amountUsd);
    try {
      const { checkout_url } = await createTopupCheckout(amountUsd);
      window.location.assign(checkout_url);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not start checkout');
      setPending(null);
    }
  }

  return (
    <div className="rounded-xl border border-white/8 bg-white/[0.03] p-4">
      <div className="flex items-center gap-2">
        <Wallet className="h-4 w-4 text-[var(--accent)]" />
        <p className="text-xs font-medium uppercase tracking-[0.08em] text-[var(--muted)]">Balance</p>
      </div>
      {balance !== null && (
        <p className="mt-2 text-2xl font-semibold text-white" data-testid="credits-balance">
          {formatDollars(balance)}
        </p>
      )}
      <p className="mt-1 text-xs text-[var(--muted)]">
        You pay exactly what model calls cost. Add funds any time.
      </p>
      <div className="mt-3 grid grid-cols-2 gap-2">
        {TOPUP_AMOUNTS_USD.map((amountUsd) => (
          <button
            key={amountUsd}
            type="button"
            disabled={pending !== null}
            onClick={() => void buy(amountUsd)}
            className="flex items-center justify-center gap-1.5 rounded-lg border border-white/10 bg-white/6 px-3 py-2 text-xs text-white transition hover:bg-white/10 active:scale-95 disabled:opacity-50"
          >
            {pending === amountUsd ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : null}
            Pay {formatDollars(creditsForDollars(amountUsd))} · get {formatCreditGrant(creditsForDollars(amountUsd))}
          </button>
        ))}
      </div>
      {error && (
        <p className="mt-2 rounded-lg border border-red-400/15 bg-red-400/8 px-3 py-2 text-xs text-red-100">{error}</p>
      )}
    </div>
  );
}
