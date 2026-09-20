import { ArrowRight, Check, CheckCircle, Loader2, Minus, Sparkles, Tag, XCircle, Zap } from 'lucide-react';
import { useState } from 'react';
import { createBillingCheckout, validateReferralCode, type BillingStatus, type DiscountOption, type ReferralValidateResponse } from '../../lib/cortexApi';

interface PricingCardsProps {
  compact?: boolean;
  billing?: BillingStatus | null;
}

const DEFAULT_TRIAL_DAYS = 7;

const PRO_FEATURES = [
  'Higher Task Manager usage limits',
  'More groups and saved workspaces',
  'Priority routing and response speed',
  'Full sovereignty loop execution',
  'GitHub, Slack, and Replit integrations',
  'Team collaboration controls',
] as const;

const PREVIEW_LIMITS = [
  'Sample tasks and data only',
  'AI responses use demo mode',
  'Up to 3 preview groups',
  'Standard routing speed',
  'No integrations',
  'No team controls',
] as const;

export default function PricingCards({ compact = false, billing }: PricingCardsProps) {
  const isActivePro = Boolean(billing?.plan && (billing.plan.status === 'active' || billing.plan.status === 'trialing'));
  const planType = billing?.plan?.plan_type ?? null;
  const [plan, setPlan] = useState<'monthly' | 'annual'>('monthly');
  const [promoCode, setPromoCode] = useState('');
  const [promoResult, setPromoResult] = useState<ReferralValidateResponse | null>(null);
  const [promoLoading, setPromoLoading] = useState(false);
  const [chosenOptionIndex, setChosenOptionIndex] = useState<number | null>(null);
  const [chosenOption, setChosenOption] = useState<DiscountOption | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const promoTrialDays = chosenOption?.discount_type === 'trial_extension' || chosenOption?.discount_type === 'free_trial'
    ? chosenOption.discount_value
    : null;
  const promoPercentOff = chosenOption?.discount_type === 'percent_off'
    ? chosenOption.discount_value
    : null;

  const effectiveTrialDays = promoTrialDays
    ? DEFAULT_TRIAL_DAYS + promoTrialDays
    : DEFAULT_TRIAL_DAYS;

  const annualRaw = 69;
  const annualDiscounted = promoPercentOff ? annualRaw * (1 - promoPercentOff / 100) : annualRaw;
  const monthlyPrice = '$6.99';
  const annualPrice = promoPercentOff ? `$${annualDiscounted.toFixed(0)}` : `$${annualRaw}`;
  const annualMonthly = `$${(annualDiscounted / 12).toFixed(2)}`;

  async function validateCode() {
    const code = promoCode.trim();
    if (!code) return;
    setPromoLoading(true);
    setError(null);
    try {
      const result = await validateReferralCode(code);
      setPromoResult(result);
      if (result.valid && result.options.length === 1) {
        setChosenOptionIndex(0);
        setChosenOption(result.options[0]);
      } else {
        setChosenOptionIndex(null);
        setChosenOption(null);
      }
    } catch {
      setPromoResult({ valid: false, discount_type: null, discount_value: null, description: null, options: [], uses_remaining: null, error: 'Could not validate code' });
    } finally {
      setPromoLoading(false);
    }
  }

  function pickOption(idx: number, opt: DiscountOption) {
    setChosenOptionIndex(idx);
    setChosenOption(opt);
    if (opt.discount_type === 'percent_off' && plan === 'monthly') {
      setPlan('annual');
    }
  }

  async function startCheckout() {
    setLoading(true);
    setError(null);
    try {
      const result = await createBillingCheckout({
        plan,
        referral_code: promoCode.trim() || undefined,
        referral_choice: chosenOptionIndex != null ? String(chosenOptionIndex) : undefined,
      });
      window.location.assign(result.checkout_url);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not start checkout');
      setLoading(false);
    }
  }

  const hasValidCode = promoResult?.valid === true;
  const hasMultipleOptions = hasValidCode && promoResult!.options.length > 1;
  const needsChoice = hasMultipleOptions && chosenOptionIndex === null;

  return (
    <div className={compact ? 'space-y-4' : 'flex min-h-screen items-center justify-center bg-[var(--bg)] p-4'}>
      <div className="w-full max-w-lg">

        {/* Header */}
        <div className="mb-6 text-center">
          <div className="mx-auto mb-3 flex h-12 w-12 items-center justify-center rounded-2xl bg-[var(--accent)]/15">
            <Sparkles className="h-6 w-6 text-[var(--accent)]" />
          </div>
          <h1 className="text-2xl font-bold text-white">Cortex Pro</h1>
          <p className="mt-1.5 text-sm text-[var(--muted)]">
            Upgrade when preview becomes part of your workflow.
          </p>
        </div>

        {/* Promo code — primary position */}
        <div className="mb-5 rounded-xl border border-white/8 bg-white/[0.03] p-4">
          <label className="mb-2 flex items-center gap-2 text-xs font-medium text-[var(--muted-strong)]">
            <Tag className="h-3.5 w-3.5 text-[var(--accent)]" />
            Have a promo or referral code?
          </label>
          <div className="flex gap-2">
            <input
              value={promoCode}
              onChange={(e) => {
                setPromoCode(e.target.value.toUpperCase());
                setPromoResult(null);
                setChosenOptionIndex(null);
                setChosenOption(null);
              }}
              onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); void validateCode(); } }}
              placeholder="Enter code"
              className="min-w-0 flex-1 rounded-lg border border-white/10 bg-[var(--composer)] px-3 py-2.5 font-mono text-sm text-white outline-none transition focus:border-[var(--accent)]/50"
            />
            <button
              type="button"
              onClick={() => void validateCode()}
              disabled={!promoCode.trim() || promoLoading}
              className="rounded-lg bg-white/8 px-4 py-2.5 text-xs font-medium text-white transition hover:bg-white/12 active:scale-95 disabled:opacity-40"
            >
              {promoLoading ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : 'Apply'}
            </button>
          </div>

          {/* Validation result */}
          {promoResult && (
            <div className="mt-3">
              {promoResult.valid ? (
                <div>
                  <p className="flex items-center gap-1.5 text-xs text-emerald-300">
                    <CheckCircle className="h-3.5 w-3.5" />
                    {hasMultipleOptions ? 'Choose your discount:' : promoResult.options[0]?.label ?? 'Code applied!'}
                  </p>
                  {promoResult.description && (
                    <p className="mt-1 text-xs text-[var(--muted)]">{promoResult.description}</p>
                  )}

                  {/* Multi-option picker */}
                  {hasMultipleOptions && (
                    <div className="mt-3 space-y-2">
                      {promoResult.options.map((opt, idx) => (
                        <button
                          key={idx}
                          type="button"
                          onClick={() => pickOption(idx, opt)}
                          className={`flex w-full items-center gap-3 rounded-xl border p-3.5 text-left transition hover:bg-white/[0.04] active:scale-[0.99] ${
                            chosenOptionIndex === idx
                              ? 'border-[var(--accent)]/50 bg-[var(--accent)]/10'
                              : 'border-white/8 bg-white/[0.02]'
                          }`}
                        >
                          <div className={`flex h-5 w-5 shrink-0 items-center justify-center rounded-full border-2 transition ${
                            chosenOptionIndex === idx
                              ? 'border-[var(--accent)] bg-[var(--accent)]'
                              : 'border-white/20'
                          }`}>
                            {chosenOptionIndex === idx && <Check className="h-3 w-3 text-black" />}
                          </div>
                          <div>
                            <span className="text-sm font-medium text-white">{opt.label}</span>
                            <p className="mt-0.5 text-[11px] text-[var(--muted)]">
                              {opt.discount_type === 'percent_off' && `Saves you $${(annualRaw * opt.discount_value / 100).toFixed(0)} on your first year`}
                              {(opt.discount_type === 'trial_extension' || opt.discount_type === 'free_trial') && `${DEFAULT_TRIAL_DAYS + opt.discount_value} days free before billing starts`}
                            </p>
                          </div>
                        </button>
                      ))}
                    </div>
                  )}

                  {promoResult.uses_remaining != null && (
                    <p className="mt-2 text-[11px] text-[var(--muted)]">
                      {promoResult.uses_remaining} use{promoResult.uses_remaining === 1 ? '' : 's'} remaining
                    </p>
                  )}
                </div>
              ) : (
                <p className="flex items-center gap-1.5 text-xs text-red-300">
                  <XCircle className="h-3.5 w-3.5" />
                  {promoResult.error ?? 'Invalid code'}
                </p>
              )}
            </div>
          )}
        </div>

        {/* Plan selection */}
        <div className="mb-5 grid gap-3 sm:grid-cols-2">
          {/* Monthly */}
          <button
            type="button"
            onClick={() => setPlan('monthly')}
            className={`relative rounded-xl border p-4 text-left transition hover:bg-white/[0.04] active:scale-[0.99] ${
              plan === 'monthly' ? 'border-[var(--accent)]/50 bg-[var(--accent)]/8' : 'border-white/8 bg-white/[0.02]'
            }`}
          >
            {isActivePro && planType === 'monthly' && (
              <span className="absolute -top-2.5 left-3 rounded-full bg-emerald-500/20 px-2 py-0.5 text-[10px] font-medium text-emerald-300">
                Current plan
              </span>
            )}
            <div className="flex items-center justify-between">
              <span className="text-sm font-medium text-white">Monthly</span>
              {plan === 'monthly' && <Check className="h-4 w-4 text-[var(--accent)]" />}
            </div>
            <p className="mt-2 text-2xl font-bold text-white">{monthlyPrice}<span className="text-sm font-normal text-[var(--muted)]">/mo</span></p>
            <p className="mt-1 text-xs text-[var(--muted)]">{effectiveTrialDays}-day free trial</p>
          </button>

          {/* Annual */}
          <button
            type="button"
            onClick={() => setPlan('annual')}
            className={`relative rounded-xl border p-4 text-left transition hover:bg-white/[0.04] active:scale-[0.99] ${
              plan === 'annual' ? 'border-[var(--accent)]/50 bg-[var(--accent)]/8' : 'border-white/8 bg-white/[0.02]'
            }`}
          >
            {isActivePro && planType === 'annual' ? (
              <span className="absolute -top-2.5 right-3 rounded-full bg-emerald-500/20 px-2 py-0.5 text-[10px] font-medium text-emerald-300">
                Current plan
              </span>
            ) : promoPercentOff ? (
              <span className="absolute -top-2.5 right-3 rounded-full bg-[var(--accent)] px-2 py-0.5 text-[10px] font-bold text-black">
                {promoPercentOff}% OFF
              </span>
            ) : (
              <span className="absolute -top-2.5 right-3 rounded-full bg-emerald-500/20 px-2 py-0.5 text-[10px] font-medium text-emerald-300">
                Save 17%
              </span>
            )}
            <div className="flex items-center justify-between">
              <span className="text-sm font-medium text-white">Annual</span>
              {plan === 'annual' && <Check className="h-4 w-4 text-[var(--accent)]" />}
            </div>
            <div className="mt-2 flex items-baseline gap-2">
              <p className="text-2xl font-bold text-white">{annualPrice}<span className="text-sm font-normal text-[var(--muted)]">/yr</span></p>
              {promoPercentOff && (
                <p className="text-sm text-[var(--muted)] line-through">${annualRaw}/yr</p>
              )}
            </div>
            <p className="mt-1 text-xs text-[var(--muted)]">{annualMonthly}/mo billed annually</p>
          </button>
        </div>

        <div className="mb-5 grid gap-3 sm:grid-cols-2">
          {/* Preview limits */}
          <div className="rounded-xl border border-white/6 bg-white/[0.02] p-4">
            <p className="mb-3 flex items-center gap-2 text-xs font-medium text-[var(--muted)]">
              <Minus className="h-3.5 w-3.5 text-[var(--muted)]" />
              Preview limits
            </p>
            <div className="space-y-2">
              {PREVIEW_LIMITS.map((f) => (
                <div key={f} className="flex items-start gap-2">
                  <Minus className="mt-0.5 h-3 w-3 shrink-0 text-[var(--muted)]" />
                  <span className="text-xs text-[var(--muted)]">{f}</span>
                </div>
              ))}
            </div>
          </div>
          {/* Pro features */}
          <div className="rounded-xl border border-white/6 bg-white/[0.02] p-4">
            <p className="mb-3 flex items-center gap-2 text-xs font-medium text-[var(--muted-strong)]">
              <Zap className="h-3.5 w-3.5 text-[var(--accent)]" />
              Pro unlocks
            </p>
            <div className="space-y-2">
              {PRO_FEATURES.map((f) => (
                <div key={f} className="flex items-start gap-2">
                  <Check className="mt-0.5 h-3 w-3 shrink-0 text-[var(--accent)]" />
                  <span className="text-xs text-[var(--muted-strong)]">{f}</span>
                </div>
              ))}
            </div>
          </div>
        </div>

        {/* Error */}
        {error && (
          <div className="mb-4 rounded-xl border border-red-400/15 bg-red-400/8 px-4 py-3">
            <p className="text-sm text-red-100">{error}</p>
          </div>
        )}

        {/* CTA */}
        <button
          type="button"
          onClick={() => void startCheckout()}
          disabled={loading || needsChoice}
          className="flex w-full items-center justify-center gap-2 rounded-xl bg-[var(--accent)] px-4 py-3.5 text-sm font-bold text-black transition hover:brightness-110 active:scale-[0.99] disabled:opacity-50"
        >
          {loading ? (
            <Loader2 className="h-4 w-4 animate-spin" />
          ) : needsChoice ? (
            'Pick a discount option above'
          ) : (
            <>Start {effectiveTrialDays}-day free trial<ArrowRight className="h-4 w-4" /></>
          )}
        </button>

        <p className="mt-3 text-center text-xs text-[var(--muted)]">
          Card required. Cancel anytime from billing settings.
        </p>
      </div>
    </div>
  );
}
