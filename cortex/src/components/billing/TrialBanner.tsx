import { CalendarClock, Sparkles, X } from 'lucide-react';
import { useState } from 'react';
import type { BillingStatus } from '../../lib/cortexApi';
import { formatMoney } from './format';

interface TrialBannerProps {
  billing: BillingStatus | null;
  onOpenBilling: () => void;
}

export default function TrialBanner({ billing, onOpenBilling }: TrialBannerProps) {
  const [dismissed, setDismissed] = useState(false);

  if (dismissed) return null;

  // Active trial — show countdown (always, not just last 3 days)
  if (billing?.trial && billing.access_state === 'trial_active') {
    const { days_remaining, auto_charge_amount_cents } = billing.trial;
    const TRIAL_TOTAL_DAYS = 7;
    const progressPct = Math.max(0, Math.min(100, ((TRIAL_TOTAL_DAYS - days_remaining) / TRIAL_TOTAL_DAYS) * 100));
    const isUrgent = days_remaining <= 2;

    return (
      <div className={`border-b px-3 py-2 sm:px-4 ${isUrgent ? 'border-amber-300/20 bg-amber-300/10' : 'border-[var(--accent)]/15 bg-[var(--accent)]/8'}`}>
        <div className="mx-auto max-w-3xl">
          <div className="flex items-center gap-3">
            <CalendarClock className={`h-4 w-4 shrink-0 ${isUrgent ? 'text-amber-300' : 'text-[var(--accent)]'}`} />
            <div className="min-w-0 flex-1">
              <div className="flex flex-wrap items-center gap-x-3 gap-y-0.5">
                <p className={`text-xs font-medium ${isUrgent ? 'text-amber-100' : 'text-white'}`}>
                  {days_remaining === 0
                    ? 'Trial ends today'
                    : `${days_remaining} day${days_remaining === 1 ? '' : 's'} left in trial`}
                </p>
                <p className="text-xs text-[var(--muted)]">
                  {formatMoney(auto_charge_amount_cents)} auto-charge when trial ends
                </p>
              </div>
              {/* Progress bar */}
              <div className="mt-1.5 h-1 w-full overflow-hidden rounded-full bg-white/10">
                <div
                  className={`h-full rounded-full transition-all ${isUrgent ? 'bg-amber-300' : 'bg-[var(--accent)]'}`}
                  style={{ width: `${progressPct}%` }}
                />
              </div>
            </div>
            <button
              type="button"
              onClick={onOpenBilling}
              className="rounded-lg px-2 py-1 text-xs text-white transition hover:bg-white/8 active:scale-95 shrink-0"
            >
              Review
            </button>
            <button
              type="button"
              onClick={() => setDismissed(true)}
              className="rounded-lg p-1 text-[var(--muted)] transition hover:bg-white/8 hover:text-white shrink-0"
              aria-label="Dismiss trial notice"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>
        </div>
      </div>
    );
  }

  // No billing data or needs checkout — show free tier upgrade prompt (dismissible)
  if (!billing || billing.access_state === 'needs_checkout' || billing.access_state === 'needs_phone') {
    return (
      <div className="border-b border-[var(--accent)]/10 bg-[var(--accent)]/6 px-3 py-2 sm:px-4">
        <div className="mx-auto flex max-w-3xl items-center gap-3">
          <Sparkles className="h-4 w-4 shrink-0 text-[var(--accent)]" />
          <p className="min-w-0 flex-1 text-xs text-[var(--muted-strong)]">
            You're in <span className="font-medium text-white">Preview mode</span>. Subscribe to unlock full AI agent capabilities.
          </p>
          <button
            type="button"
            onClick={onOpenBilling}
            className="rounded-lg bg-[var(--accent)]/15 px-2.5 py-1 text-xs font-medium text-[var(--accent)] transition hover:bg-[var(--accent)]/25 active:scale-95 shrink-0"
          >
            Subscribe
          </button>
          <button
            type="button"
            onClick={() => setDismissed(true)}
            className="rounded-lg p-1 text-[var(--muted)] transition hover:bg-white/8 hover:text-white shrink-0"
            aria-label="Dismiss"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </div>
      </div>
    );
  }

  return null;
}
