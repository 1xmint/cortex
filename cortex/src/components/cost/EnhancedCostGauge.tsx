import { useState } from 'react';
import { DollarSign, AlertTriangle, Settings } from 'lucide-react';
import { useCostAwareness } from '../../lib/useCostAwareness';
import CostWarning from './CostWarning';
import CostStatus from './CostStatus';
import type { CostsState } from '../../types';

interface EnhancedCostGaugeProps {
  data?: CostsState | null;
  onOpenSettings?: () => void;
  compact?: boolean;
}

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}

function formatCurrency(amount: number): string {
  return new Intl.NumberFormat('en-US', {
    style: 'currency',
    currency: 'USD',
    maximumFractionDigits: 4,
  }).format(amount);
}

export default function EnhancedCostGauge({
  data,
  onOpenSettings,
  compact = false
}: EnhancedCostGaugeProps) {
  const { state, acknowledgeWarning, hasActiveWarnings } = useCostAwareness();
  const [showDetails, setShowDetails] = useState(false);

  // Use the new usage data if available, fall back to legacy data
  const sessionData = state.usage?.current_session || {
    cost: data?.session.estimatedCostUsd || 0,
    token_count: data?.session.totalTokens || 0,
    request_count: 0,
    duration_minutes: 0,
  };

  const providerBreakdown = state.usage?.provider_breakdown ||
    Object.entries(data?.byProvider || {}).map(([provider, info]) => ({
      provider,
      cost: info.estimatedCostUsd,
      token_count: info.tokens,
      request_count: 0,
    }));

  if (!data && !state.usage && !state.loading) {
    return (
      <div className="rounded-lg p-6 border border-white/8 bg-[var(--panel)]">
        <h2 className="text-lg font-semibold text-white mb-4">Cost Gauge</h2>
        <p className="text-[var(--muted)] text-sm">No cost data available</p>
      </div>
    );
  }

  if (compact) {
    return (
      <div className="rounded-lg p-4 border border-white/8 bg-[var(--panel)]">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-purple-500/20">
              <DollarSign className="h-4 w-4 text-purple-400" />
            </div>
            <div>
              <span className="text-lg font-bold text-white">
                {formatCurrency(sessionData.cost)}
              </span>
              <p className="text-xs text-[var(--muted)]">session cost</p>
            </div>
          </div>

          <div className="flex items-center gap-2">
            {hasActiveWarnings && (
              <div className="flex h-6 w-6 items-center justify-center rounded-full bg-amber-400/20">
                <AlertTriangle className="h-3 w-3 text-amber-400" />
              </div>
            )}

            {onOpenSettings && (
              <button
                onClick={onOpenSettings}
                className="flex h-6 w-6 items-center justify-center rounded-lg text-[var(--muted)] transition hover:bg-white/10 hover:text-white"
                title="Open budget settings"
              >
                <Settings className="h-3 w-3" />
              </button>
            )}
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="rounded-lg border border-white/8 bg-[var(--panel)]">
      {/* Header */}
      <div className="flex items-center justify-between p-6 pb-4">
        <h2 className="text-lg font-semibold text-white">Cost Gauge</h2>

        <div className="flex items-center gap-2">
          {hasActiveWarnings && (
            <div className="flex h-6 w-6 items-center justify-center rounded-full bg-amber-400/20">
              <AlertTriangle className="h-3 w-3 text-amber-400" />
            </div>
          )}

          {onOpenSettings && (
            <button
              onClick={onOpenSettings}
              className="rounded-lg p-1.5 text-[var(--muted)] transition hover:bg-white/10 hover:text-white"
              title="Open budget settings"
            >
              <Settings className="h-4 w-4" />
            </button>
          )}
        </div>
      </div>

      {/* Cost Warnings */}
      {hasActiveWarnings && (
        <div className="px-6 pb-4">
          <CostWarning
            warnings={state.warnings.filter(w => !w.acknowledged)}
            onAcknowledge={acknowledgeWarning}
          />
        </div>
      )}

      <div className="p-6 pt-0">
        {/* Main Cost Display */}
        <div className="text-center mb-6">
          <span className="text-3xl font-bold text-white">
            {formatCurrency(sessionData.cost)}
          </span>
          <p className="text-[var(--muted)] text-xs mt-1">session total</p>
        </div>

        {/* Token Stats */}
        <div className="grid grid-cols-3 gap-4 mb-6 text-center">
          <div>
            <p className="text-sm font-mono text-[var(--muted-strong)]">
              {formatTokens(data?.session.inputTokens || 0)}
            </p>
            <p className="text-xs text-[var(--muted)]">input</p>
          </div>
          <div>
            <p className="text-sm font-mono text-[var(--muted-strong)]">
              {formatTokens(data?.session.outputTokens || 0)}
            </p>
            <p className="text-xs text-[var(--muted)]">output</p>
          </div>
          <div>
            <p className="text-sm font-mono text-[var(--muted-strong)]">
              {formatTokens(sessionData.token_count)}
            </p>
            <p className="text-xs text-[var(--muted)]">total</p>
          </div>
        </div>

        {/* Provider Breakdown */}
        {providerBreakdown.length > 0 && (
          <div className="space-y-2 mb-4">
            {providerBreakdown.map((provider) => {
              const percentage = sessionData.cost > 0
                ? (provider.cost / sessionData.cost) * 100
                : 0;

              return (
                <div key={provider.provider}>
                  <div className="flex justify-between text-xs mb-1">
                    <span className="text-[var(--muted-strong)] capitalize">
                      {provider.provider}
                    </span>
                    <span className="text-[var(--muted)]">
                      {formatCurrency(provider.cost)} ({formatTokens(provider.token_count)})
                    </span>
                  </div>
                  <div className="w-full bg-white/8 rounded-full h-1.5">
                    <div
                      className="h-1.5 rounded-full bg-purple-500 transition-all"
                      style={{ width: `${percentage}%` }}
                    />
                  </div>
                </div>
              );
            })}
          </div>
        )}

        {/* Toggle Details Button */}
        <button
          onClick={() => setShowDetails(!showDetails)}
          className="w-full rounded-lg border border-white/8 px-3 py-2 text-xs text-[var(--muted-strong)] transition hover:bg-white/6 hover:text-white"
        >
          {showDetails ? 'Hide Details' : 'Show Details'}
        </button>
      </div>

      {/* Detailed Cost Information */}
      {showDetails && (
        <div className="border-t border-white/8 p-6">
          <CostStatus
            usage={state.usage}
            providers={state.providers}
            loading={state.loading}
            error={state.error || undefined}
          />
        </div>
      )}
    </div>
  );
}