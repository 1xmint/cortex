import { AlertTriangle, Check, Copy, CreditCard, ExternalLink, Loader2, ShieldCheck, Users, X } from 'lucide-react';
import { useState } from 'react';
import type { BillingStatus } from '../../lib/cortexApi';
import { createBillingPortal } from '../../lib/cortexApi';
import BillingHistory from './BillingHistory';
import PricingCards from './PricingCards';
import { formatDate, formatMoney } from './format';

interface BillingPageProps {
  billing: BillingStatus | null;
}

interface UsageData {
  tasks: number;
  runs: number;
  groups: number;
  limits: {
    tasks: number;
    runs: number;
    groups: number;
  };
}

// TODO: Wire up to a real usage API endpoint (e.g. GET /api/billing/usage)
// when one exists. For now, show zeros so new users don't see fake data.
const FREE_TIER_USAGE: UsageData = {
  tasks: 0,
  runs: 0,
  groups: 0,
  limits: {
    tasks: 50,
    runs: 10,
    groups: 5,
  },
};

function UsageBar({ label, value, limit, showUpgrade }: { label: string; value: number; limit: number; showUpgrade?: boolean }) {
  const pct = Math.min(100, Math.round((value / limit) * 100));
  const isWarning = pct >= 80;
  const isCritical = pct >= 95;
  return (
    <div>
      <div className="mb-1 flex items-center justify-between">
        <span className="text-xs text-[var(--muted)]">{label}</span>
        <span className={`text-xs font-medium ${isCritical ? 'text-red-300' : isWarning ? 'text-amber-200' : 'text-[var(--muted-strong)]'}`}>
          {value} / {limit}
        </span>
      </div>
      <div className="h-1.5 w-full overflow-hidden rounded-full bg-white/8">
        <div
          className={`h-full rounded-full transition-all ${isCritical ? 'bg-red-400' : isWarning ? 'bg-amber-300' : 'bg-[var(--accent)]'}`}
          style={{ width: `${pct}%` }}
        />
      </div>
      {isCritical && showUpgrade && (
        <a
          href="#pricing"
          className="mt-1 inline-block text-[11px] font-medium text-[var(--accent)] transition hover:underline"
        >
          Upgrade to Pro &rarr;
        </a>
      )}
    </div>
  );
}

function UsageSummary({ isPro }: { isPro: boolean }) {
  const usage = FREE_TIER_USAGE;
  return (
    <div className="rounded-xl border border-white/8 bg-white/[0.03] p-4">
      <p className="mb-3 text-xs font-medium uppercase tracking-[0.08em] text-[var(--muted)]">
        Usage this month
      </p>
      <div className="space-y-3">
        <UsageBar label="Tasks created" value={usage.tasks} limit={usage.limits.tasks} showUpgrade={!isPro} />
        <UsageBar label="Agent runs" value={usage.runs} limit={usage.limits.runs} showUpgrade={!isPro} />
        <UsageBar label="Team groups" value={usage.groups} limit={usage.limits.groups} showUpgrade={!isPro} />
      </div>
      {!isPro && (
        <p className="mt-3 text-[11px] text-[var(--muted)]">
          Subscribe to Cortex Pro for full access.
        </p>
      )}
    </div>
  );
}

interface CancelFlowProps {
  onPortal: () => void;
  onDismiss: () => void;
  isLoading: boolean;
}

function CancelConfirmation({ onPortal, onDismiss, isLoading }: CancelFlowProps) {
  return (
    <div className="rounded-xl border border-red-400/20 bg-red-400/8 p-4">
      <div className="mb-3 flex items-start justify-between gap-3">
        <div className="flex items-center gap-2">
          <AlertTriangle className="h-4 w-4 text-red-300 shrink-0" />
          <p className="text-sm font-medium text-white">Cancel subscription?</p>
        </div>
        <button type="button" onClick={onDismiss} className="p-0.5 text-[var(--muted)] transition hover:text-white">
          <X className="h-4 w-4" />
        </button>
      </div>
      <p className="mb-2 text-xs text-red-100">You'll lose access to:</p>
      <ul className="mb-4 space-y-1 text-xs text-[var(--muted-strong)]">
        <li className="flex items-center gap-2"><span className="h-1 w-1 rounded-full bg-red-300/60 shrink-0" />Higher task and run limits</li>
        <li className="flex items-center gap-2"><span className="h-1 w-1 rounded-full bg-red-300/60 shrink-0" />Priority routing and response speed</li>
        <li className="flex items-center gap-2"><span className="h-1 w-1 rounded-full bg-red-300/60 shrink-0" />Full sovereignty loop execution</li>
        <li className="flex items-center gap-2"><span className="h-1 w-1 rounded-full bg-red-300/60 shrink-0" />GitHub, Slack, and Replit integrations</li>
        <li className="flex items-center gap-2"><span className="h-1 w-1 rounded-full bg-red-300/60 shrink-0" />Team collaboration controls</li>
      </ul>
      <div className="flex gap-2">
        <button
          type="button"
          onClick={onDismiss}
          className="flex-1 rounded-lg border border-white/10 bg-white/6 px-3 py-2 text-xs font-medium text-white transition hover:bg-white/10 active:scale-95"
        >
          Keep subscription
        </button>
        <button
          type="button"
          onClick={onPortal}
          disabled={isLoading}
          className="flex items-center justify-center gap-1.5 rounded-lg border border-red-400/20 bg-red-400/12 px-3 py-2 text-xs text-red-200 transition hover:bg-red-400/20 active:scale-95 disabled:opacity-50"
        >
          {isLoading ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : null}
          Cancel anyway
        </button>
      </div>
    </div>
  );
}

export default function BillingPage({ billing }: BillingPageProps) {
  const [portalError, setPortalError] = useState<string | null>(null);
  const [portalLoading, setPortalLoading] = useState(false);
  const [copiedCode, setCopiedCode] = useState(false);
  const [showCancelConfirm, setShowCancelConfirm] = useState(false);

  const isActiveSub = billing?.plan?.status === 'active' || billing?.plan?.status === 'trialing';
  const isPro = Boolean(billing?.plan);

  if (!billing || !billing.plan) {
    return (
      <div className="space-y-4">
        <div className="rounded-xl border border-[var(--accent)]/20 bg-[var(--accent)]/10 p-4">
          <div className="flex items-start justify-between gap-3">
            <div>
              <p className="text-xs uppercase tracking-[0.08em] text-[var(--muted)]">Plan</p>
              <h3 className="mt-1 text-base font-semibold text-white">Preview mode</h3>
              <p className="mt-1 text-xs leading-5 text-[var(--muted)]">
                You're exploring Cortex with sample data. Subscribe to Cortex Pro for full access.
              </p>
            </div>
            <ShieldCheck className="h-5 w-5 text-[var(--accent)]" />
          </div>
        </div>
        <UsageSummary isPro={false} />
        <PricingCards compact />
      </div>
    );
  }

  const planLabel = `Cortex Pro ${billing.plan.plan_type === 'annual' ? 'Annual' : 'Monthly'}`;
  const payment = billing.payment_method
    ? `${billing.payment_method.brand.toUpperCase()} ending ${billing.payment_method.last4}`
    : 'Add in Stripe checkout';

  async function openPortal() {
    setPortalError(null);
    setPortalLoading(true);
    try {
      const { portal_url } = await createBillingPortal();
      window.location.assign(portal_url);
    } catch (err) {
      setPortalError(err instanceof Error ? err.message : 'Could not open Stripe portal');
    } finally {
      setPortalLoading(false);
    }
  }

  return (
    <div className="space-y-4">
      <div className="rounded-xl border border-white/8 bg-white/[0.03] p-4">
        <div className="flex items-start justify-between gap-3">
          <div>
            <p className="text-xs uppercase tracking-[0.08em] text-[var(--muted)]">Plan</p>
            <h3 className="mt-1 text-base font-semibold text-white">{planLabel}</h3>
            <p className="mt-1 text-xs capitalize text-[var(--muted)]">{billing.plan?.status ?? billing.access_state}</p>
          </div>
          <ShieldCheck className="h-5 w-5 text-[var(--accent)]" />
        </div>
        <div className="mt-4 grid gap-3 sm:grid-cols-2">
          <div className="rounded-lg bg-white/[0.03] p-3">
            <p className="text-[11px] text-[var(--muted)]">Next billing</p>
            <p className="mt-1 text-sm text-white">{formatDate(billing.plan?.next_charge_date ?? billing.plan?.billing_period_end)}</p>
            <p className="mt-0.5 text-xs text-[var(--muted)]">{formatMoney(billing.plan?.next_charge_amount_cents)}</p>
          </div>
          <div className="rounded-lg bg-white/[0.03] p-3">
            <p className="text-[11px] text-[var(--muted)]">Payment method</p>
            <p className="mt-1 text-sm text-white">{payment}</p>
            <p className="mt-0.5 text-xs text-[var(--muted)]">Managed securely by Stripe</p>
          </div>
        </div>
        <div className="mt-4 flex flex-wrap items-center gap-2">
          <button
            type="button"
            onClick={() => void openPortal()}
            disabled={portalLoading}
            className="inline-flex items-center gap-2 rounded-lg border border-white/10 px-3 py-2 text-sm text-white transition hover:bg-white/8 active:scale-95 disabled:opacity-50"
          >
            {portalLoading ? <Loader2 className="h-4 w-4 animate-spin" /> : <CreditCard className="h-4 w-4" />}
            Manage subscription
            <ExternalLink className="h-3.5 w-3.5 text-[var(--muted)]" />
          </button>
          {isActiveSub && !showCancelConfirm && (
            <button
              type="button"
              onClick={() => setShowCancelConfirm(true)}
              className="inline-flex items-center gap-2 rounded-lg border border-white/8 px-3 py-2 text-sm text-[var(--muted)] transition hover:border-red-400/25 hover:text-red-200 active:scale-95"
            >
              Cancel subscription
            </button>
          )}
        </div>
        {portalError && <p className="mt-3 text-xs text-red-200">{portalError}</p>}
      </div>

      {/* Cancel confirmation */}
      {showCancelConfirm && (
        <CancelConfirmation
          onPortal={() => void openPortal()}
          onDismiss={() => setShowCancelConfirm(false)}
          isLoading={portalLoading}
        />
      )}

      {/* Usage summary */}
      <UsageSummary isPro={isPro} />

      {billing.referral && (
        <div className="rounded-xl border border-white/8 bg-white/[0.03] p-4">
          <div className="flex items-center justify-between gap-3">
            <div className="flex items-center gap-2">
              <Users className="h-4 w-4 text-[var(--accent)]" />
              <p className="text-xs font-medium uppercase tracking-[0.08em] text-[var(--muted)]">Refer friends</p>
            </div>
            {billing.referral.weeks_earned > 0 && (
              <span className="rounded-full bg-emerald-500/15 px-2 py-0.5 text-[10px] font-medium text-emerald-300">
                {billing.referral.weeks_earned} free week{billing.referral.weeks_earned === 1 ? '' : 's'} earned
              </span>
            )}
          </div>
          <p className="mt-2 text-xs text-[var(--muted)]">
            Share your code — friends get 3 weeks free (instead of 7 days). You get 1 free week per signup.
          </p>
          <button
            type="button"
            onClick={() => {
              navigator.clipboard.writeText(billing.referral!.code);
              setCopiedCode(true);
              setTimeout(() => setCopiedCode(false), 2000);
            }}
            className="mt-2 flex items-center gap-2 rounded-lg bg-white/8 px-3 py-2 font-mono text-sm font-bold text-white transition hover:bg-white/12 active:scale-95"
          >
            {billing.referral.code}
            {copiedCode
              ? <Check className="h-3.5 w-3.5 text-emerald-300" />
              : <Copy className="h-3.5 w-3.5 text-[var(--muted)]" />
            }
          </button>
          <p className="mt-1.5 text-[11px] text-[var(--muted)]">
            {billing.referral.total_uses} referral{billing.referral.total_uses === 1 ? '' : 's'} used
            {billing.referral.uses_remaining > 0 && ` · ${billing.referral.uses_remaining} remaining`}
          </p>
        </div>
      )}

      <div>
        <h3 className="mb-2 text-sm font-medium text-white">Purchase history</h3>
        <BillingHistory />
      </div>
    </div>
  );
}
