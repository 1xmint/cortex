import { useState, useEffect } from 'react';
import { DollarSign, Gauge, Clock, TrendingUp, AlertTriangle, CheckCircle } from 'lucide-react';
import type { UsageData, ProviderStatusInfo } from '../../lib/cortexApi';

interface CostStatusProps {
  usage: UsageData | null;
  providers: ProviderStatusInfo[] | null;
  loading?: boolean;
  error?: string;
}

function formatCurrency(amount: number): string {
  return new Intl.NumberFormat('en-US', {
    style: 'currency',
    currency: 'USD',
    maximumFractionDigits: 4,
  }).format(amount);
}

function formatNumber(num: number): string {
  if (num >= 1_000_000) return `${(num / 1_000_000).toFixed(1)}M`;
  if (num >= 1_000) return `${(num / 1_000).toFixed(1)}K`;
  return String(num);
}

function formatDuration(minutes: number): string {
  if (minutes < 1) return '< 1min';
  if (minutes < 60) return `${Math.round(minutes)}m`;
  const hours = Math.floor(minutes / 60);
  const remainingMinutes = Math.round(minutes % 60);
  if (remainingMinutes === 0) return `${hours}h`;
  return `${hours}h ${remainingMinutes}m`;
}

function ProgressBar({ value, max, color = 'var(--accent)' }: { value: number; max: number; color?: string }) {
  const percentage = max > 0 ? Math.min((value / max) * 100, 100) : 0;

  return (
    <div className="relative h-2 rounded-full bg-white/8 overflow-hidden">
      <div
        className="h-full rounded-full transition-all duration-300"
        style={{
          width: `${percentage}%`,
          backgroundColor: percentage > 90 ? '#ef4444' : percentage > 75 ? '#f59e0b' : color
        }}
      />
    </div>
  );
}

function ProviderStatusIndicator({ provider }: { provider: ProviderStatusInfo }) {
  const getStatusColor = () => {
    if (!provider.authenticated) return 'text-red-400';
    if (provider.status === 'byos') return 'text-emerald-400';
    if (provider.status === 'byok') return 'text-blue-400';
    return 'text-gray-400';
  };

  const getStatusLabel = () => {
    if (!provider.authenticated) return 'Not connected';
    if (provider.status === 'byos') return 'BYOS (Unlimited)';
    if (provider.status === 'byok') return 'BYOK (Pay-per-use)';
    return 'Unavailable';
  };

  const StatusIcon = provider.authenticated ? CheckCircle : AlertTriangle;

  return (
    <div className="flex items-center justify-between rounded-lg border border-white/8 bg-white/[0.02] px-3 py-2">
      <div className="flex items-center gap-2">
        <div className="flex h-6 w-6 items-center justify-center rounded-md bg-white/6 text-[10px] font-bold uppercase text-[var(--muted-strong)]">
          {provider.provider === 'claude' ? 'CL' : 'OA'}
        </div>
        <span className="text-sm text-white">
          {provider.provider === 'claude' ? 'Claude' : 'OpenAI'}
        </span>
      </div>
      <div className="flex items-center gap-1.5 text-xs">
        <StatusIcon className={`h-3 w-3 ${getStatusColor()}`} />
        <span className={getStatusColor()}>{getStatusLabel()}</span>
      </div>
    </div>
  );
}

export default function CostStatus({ usage, providers, loading, error }: CostStatusProps) {
  const [currentTime, setCurrentTime] = useState(new Date());

  useEffect(() => {
    const timer = setInterval(() => setCurrentTime(new Date()), 30000); // Update every 30 seconds
    return () => clearInterval(timer);
  }, []);

  if (loading) {
    return (
      <div className="rounded-xl border border-white/8 bg-white/[0.02] p-4">
        <div className="flex items-center gap-2 text-sm text-[var(--muted)]">
          <div className="h-4 w-4 animate-spin rounded-full border-2 border-[var(--accent)] border-t-transparent" />
          Loading cost data...
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="rounded-xl border border-red-400/15 bg-red-400/8 p-4">
        <div className="flex items-center gap-2">
          <AlertTriangle className="h-4 w-4 text-red-400" />
          <span className="text-sm text-red-200">Failed to load cost data</span>
        </div>
        <p className="mt-1 text-xs text-red-300">{error}</p>
      </div>
    );
  }

  if (!usage || !providers) {
    return (
      <div className="rounded-xl border border-white/8 bg-white/[0.02] p-4">
        <p className="text-sm text-[var(--muted)]">No cost data available</p>
      </div>
    );
  }

  const hasTrackingEnabled = providers.some(p => p.cost_tracking_enabled && p.authenticated);

  return (
    <div className="space-y-4" role="status" aria-label="Cost and budget status">
      {/* Session Status */}
      <div className="rounded-xl border border-white/8 bg-white/[0.02] p-4">
        <div className="flex items-center gap-2 mb-3">
          <DollarSign className="h-4 w-4 text-[var(--accent)]" />
          <h3 className="text-sm font-medium text-white">Current Session</h3>
        </div>

        <div className="grid grid-cols-2 gap-4">
          <div>
            <p className="text-lg font-semibold text-white">
              {formatCurrency(usage.current_session.cost)}
            </p>
            <p className="text-xs text-[var(--muted)]">Total cost</p>
          </div>
          <div>
            <p className="text-lg font-semibold text-white">
              {formatDuration(usage.current_session.duration_minutes)}
            </p>
            <p className="text-xs text-[var(--muted)]">Duration</p>
          </div>
        </div>

        <div className="grid grid-cols-2 gap-4 mt-3">
          <div>
            <p className="text-sm font-mono text-white">
              {formatNumber(usage.current_session.token_count)}
            </p>
            <p className="text-xs text-[var(--muted)]">Tokens</p>
          </div>
          <div>
            <p className="text-sm font-mono text-white">
              {usage.current_session.request_count}
            </p>
            <p className="text-xs text-[var(--muted)]">Requests</p>
          </div>
        </div>
      </div>

      {/* Budget Overview - only show if cost tracking is enabled */}
      {hasTrackingEnabled && (
        <div className="rounded-xl border border-white/8 bg-white/[0.02] p-4">
          <div className="flex items-center gap-2 mb-3">
            <Gauge className="h-4 w-4 text-[var(--accent)]" />
            <h3 className="text-sm font-medium text-white">Budget Status</h3>
          </div>

          <div className="space-y-3">
            {/* Daily */}
            <div>
              <div className="flex items-center justify-between mb-1">
                <span className="text-xs text-[var(--muted)]">Daily</span>
                <span className="text-xs font-mono text-white">
                  {formatCurrency(usage.daily.cost)} / {formatCurrency(usage.daily.cost + usage.daily.budget_remaining)}
                </span>
              </div>
              <ProgressBar
                value={usage.daily.cost}
                max={usage.daily.cost + usage.daily.budget_remaining}
              />
            </div>

            {/* Weekly */}
            <div>
              <div className="flex items-center justify-between mb-1">
                <span className="text-xs text-[var(--muted)]">Weekly</span>
                <span className="text-xs font-mono text-white">
                  {formatCurrency(usage.weekly.cost)} / {formatCurrency(usage.weekly.cost + usage.weekly.budget_remaining)}
                </span>
              </div>
              <ProgressBar
                value={usage.weekly.cost}
                max={usage.weekly.cost + usage.weekly.budget_remaining}
              />
            </div>

            {/* Monthly */}
            <div>
              <div className="flex items-center justify-between mb-1">
                <span className="text-xs text-[var(--muted)]">Monthly</span>
                <span className="text-xs font-mono text-white">
                  {formatCurrency(usage.monthly.cost)} / {formatCurrency(usage.monthly.cost + usage.monthly.budget_remaining)}
                </span>
              </div>
              <ProgressBar
                value={usage.monthly.cost}
                max={usage.monthly.cost + usage.monthly.budget_remaining}
              />
            </div>
          </div>
        </div>
      )}

      {/* Provider Status */}
      <div className="rounded-xl border border-white/8 bg-white/[0.02] p-4">
        <div className="flex items-center gap-2 mb-3">
          <TrendingUp className="h-4 w-4 text-[var(--accent)]" />
          <h3 className="text-sm font-medium text-white">Provider Status</h3>
        </div>

        <div className="space-y-2">
          {providers.map(provider => (
            <ProviderStatusIndicator key={provider.provider} provider={provider} />
          ))}
        </div>

        {!hasTrackingEnabled && (
          <div className="mt-3 rounded-lg border border-amber-400/20 bg-amber-400/8 p-3">
            <p className="text-xs text-amber-200">
              Connect BYOK providers to enable detailed cost tracking and budget controls.
            </p>
          </div>
        )}
      </div>

      {/* Provider Breakdown - only show if cost tracking is enabled and there's usage */}
      {hasTrackingEnabled && usage.provider_breakdown.length > 0 && (
        <div className="rounded-xl border border-white/8 bg-white/[0.02] p-4">
          <h3 className="text-sm font-medium text-white mb-3">Provider Breakdown</h3>
          <div className="space-y-2">
            {usage.provider_breakdown.map(provider => (
              <div key={provider.provider} className="flex items-center justify-between rounded-lg bg-white/[0.03] px-3 py-2">
                <div className="flex items-center gap-2">
                  <div className="flex h-5 w-5 items-center justify-center rounded bg-white/10 text-[9px] font-bold uppercase text-[var(--muted-strong)]">
                    {provider.provider === 'claude' ? 'CL' : 'OA'}
                  </div>
                  <span className="text-sm text-white capitalize">{provider.provider}</span>
                </div>
                <div className="text-right">
                  <div className="text-xs font-mono text-white">
                    {formatCurrency(provider.cost)}
                  </div>
                  <div className="text-[10px] text-[var(--muted)]">
                    {formatNumber(provider.token_count)} tokens
                  </div>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Last updated */}
      <div className="flex items-center gap-1 text-[10px] text-[var(--muted)]">
        <Clock className="h-3 w-3" />
        Updated {currentTime.toLocaleTimeString()}
      </div>
    </div>
  );
}