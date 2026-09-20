import { useState, useEffect } from 'react';
import { Save, AlertTriangle, DollarSign, Calendar, Gauge, Loader2 } from 'lucide-react';
import type { BudgetSettings as BudgetSettingsType, UsageData } from '../../lib/cortexApi';

interface BudgetSettingsProps {
  settings: BudgetSettingsType | null;
  usage: UsageData | null;
  onSave: (settings: BudgetSettingsType) => Promise<void>;
  loading?: boolean;
  error?: string;
}

function formatCurrency(amount: number): string {
  return new Intl.NumberFormat('en-US', {
    style: 'currency',
    currency: 'USD',
    minimumFractionDigits: 0,
    maximumFractionDigits: 2,
  }).format(amount);
}

function CurrencyInput({
  label,
  value,
  onChange,
  placeholder = "No limit",
  disabled = false
}: {
  label: string;
  value: number | null;
  onChange: (value: number | null) => void;
  placeholder?: string;
  disabled?: boolean;
}) {
  const [inputValue, setInputValue] = useState(value?.toString() || '');

  useEffect(() => {
    setInputValue(value?.toString() || '');
  }, [value]);

  const handleChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const newValue = e.target.value;
    setInputValue(newValue);

    if (newValue === '') {
      onChange(null);
    } else {
      const numValue = parseFloat(newValue);
      if (!isNaN(numValue) && numValue >= 0) {
        onChange(numValue);
      }
    }
  };

  return (
    <div>
      <label className="mb-1.5 block text-xs font-medium text-[var(--muted)]">
        {label}
      </label>
      <div className="relative">
        <DollarSign className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-[var(--muted)]" />
        <input
          type="number"
          min="0"
          step="0.01"
          value={inputValue}
          onChange={handleChange}
          placeholder={placeholder}
          disabled={disabled}
          className="w-full rounded-lg border border-white/10 bg-[var(--composer)] pl-9 pr-3 py-2 text-sm text-white placeholder:text-white/30 focus:border-[var(--accent)]/50 focus:outline-none disabled:opacity-50"
        />
      </div>
    </div>
  );
}

function WarningThresholdSlider({
  value,
  onChange,
  disabled = false
}: {
  value: number;
  onChange: (value: number) => void;
  disabled?: boolean;
}) {
  return (
    <div>
      <label className="mb-1.5 block text-xs font-medium text-[var(--muted)]">
        Warning threshold ({value}% of budget)
      </label>
      <input
        type="range"
        min="50"
        max="95"
        step="5"
        value={value}
        onChange={(e) => onChange(parseInt(e.target.value))}
        disabled={disabled}
        className="w-full h-2 bg-white/10 rounded-lg appearance-none cursor-pointer disabled:cursor-not-allowed disabled:opacity-50"
        style={{
          background: `linear-gradient(to right, var(--accent) 0%, var(--accent) ${((value - 50) / 45) * 100}%, rgb(255 255 255 / 0.1) ${((value - 50) / 45) * 100}%, rgb(255 255 255 / 0.1) 100%)`
        }}
      />
      <div className="flex justify-between text-[10px] text-[var(--muted)] mt-1">
        <span>50%</span>
        <span>70%</span>
        <span>90%</span>
        <span>95%</span>
      </div>
    </div>
  );
}

function UsageProgressBar({ label, current, budget, className }: {
  label: string;
  current: number;
  budget: number;
  className?: string;
}) {
  const percentage = budget > 0 ? Math.min((current / budget) * 100, 100) : 0;
  const remaining = Math.max(budget - current, 0);

  return (
    <div className={`rounded-lg border border-white/8 bg-white/[0.02] p-3 ${className}`}>
      <div className="flex items-center justify-between mb-2">
        <span className="text-sm font-medium text-white">{label}</span>
        <span className="text-xs text-[var(--muted)]">
          {formatCurrency(current)} / {formatCurrency(budget)}
        </span>
      </div>

      <div className="relative h-2 rounded-full bg-white/8 overflow-hidden mb-2">
        <div
          className="h-full rounded-full transition-all duration-300"
          style={{
            width: `${percentage}%`,
            backgroundColor: percentage > 90 ? '#ef4444' : percentage > 75 ? '#f59e0b' : 'var(--accent)'
          }}
        />
      </div>

      <div className="flex items-center justify-between text-xs">
        <span className="text-[var(--muted)]">
          {remaining > 0 ? `${formatCurrency(remaining)} remaining` : 'Budget exceeded'}
        </span>
        <span className={percentage > 90 ? 'text-red-400' : percentage > 75 ? 'text-amber-400' : 'text-[var(--accent)]'}>
          {percentage.toFixed(1)}%
        </span>
      </div>
    </div>
  );
}

export default function BudgetSettings({
  settings,
  usage,
  onSave,
  loading = false,
  error
}: BudgetSettingsProps) {
  const [formSettings, setFormSettings] = useState<BudgetSettingsType>({
    daily_limit: null,
    weekly_limit: null,
    monthly_limit: null,
    warning_threshold: 80,
    enabled: false,
    provider_limits: {},
  });

  const [isSaving, setIsSaving] = useState(false);
  const [hasChanges, setHasChanges] = useState(false);

  useEffect(() => {
    if (settings) {
      setFormSettings(settings);
      setHasChanges(false);
    }
  }, [settings]);

  const updateSetting = <K extends keyof BudgetSettingsType>(
    key: K,
    value: BudgetSettingsType[K]
  ) => {
    setFormSettings(prev => ({ ...prev, [key]: value }));
    setHasChanges(true);
  };

  const updateProviderLimit = (provider: 'claude' | 'openai', value: number | null) => {
    setFormSettings(prev => ({
      ...prev,
      provider_limits: {
        ...prev.provider_limits,
        [provider]: value,
      }
    }));
    setHasChanges(true);
  };

  const handleSave = async () => {
    setIsSaving(true);
    try {
      await onSave(formSettings);
      setHasChanges(false);
    } catch {
      // Error handling is done by parent component
    } finally {
      setIsSaving(false);
    }
  };

  const handleToggleEnabled = () => {
    updateSetting('enabled', !formSettings.enabled);
  };

  if (loading) {
    return (
      <div className="flex items-center justify-center gap-2 py-10 text-sm text-[var(--muted)]">
        <Loader2 className="h-4 w-4 animate-spin" />
        Loading budget settings...
      </div>
    );
  }

  return (
    <div className="space-y-6">
      {error && (
        <div className="rounded-xl border border-red-400/15 bg-red-400/8 px-4 py-3">
          <div className="flex items-center gap-2">
            <AlertTriangle className="h-4 w-4 text-red-400" />
            <span className="text-sm text-red-200">Failed to load budget settings</span>
          </div>
          <p className="mt-1 text-xs text-red-300">{error}</p>
        </div>
      )}

      {/* Enable/Disable Toggle */}
      <div className="rounded-xl border border-white/8 bg-white/[0.02] p-4">
        <div className="flex items-center justify-between">
          <div>
            <h3 className="text-sm font-medium text-white">Budget controls</h3>
            <p className="mt-0.5 text-xs text-[var(--muted)]">
              Enable cost limits and warnings for BYOK providers
            </p>
          </div>
          <button
            type="button"
            role="switch"
            aria-checked={formSettings.enabled}
            onClick={handleToggleEnabled}
            className={`relative inline-flex h-6 w-11 shrink-0 cursor-pointer items-center rounded-full transition-colors duration-200 ${
              formSettings.enabled ? 'bg-[var(--accent)]' : 'bg-white/20'
            }`}
          >
            <span
              className={`inline-block h-4 w-4 rounded-full bg-white shadow transition-transform duration-200 ${
                formSettings.enabled ? 'translate-x-6' : 'translate-x-1'
              }`}
            />
          </button>
        </div>
      </div>

      {formSettings.enabled && (
        <>
          {/* Budget Limits */}
          <div className="rounded-xl border border-white/8 bg-white/[0.02] p-4">
            <div className="flex items-center gap-2 mb-4">
              <Calendar className="h-4 w-4 text-[var(--accent)]" />
              <h3 className="text-sm font-medium text-white">Budget limits</h3>
            </div>

            <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
              <CurrencyInput
                label="Daily limit"
                value={formSettings.daily_limit ?? null}
                onChange={(value) => updateSetting('daily_limit', value)}
              />
              <CurrencyInput
                label="Weekly limit"
                value={formSettings.weekly_limit ?? null}
                onChange={(value) => updateSetting('weekly_limit', value)}
              />
              <CurrencyInput
                label="Monthly limit"
                value={formSettings.monthly_limit ?? null}
                onChange={(value) => updateSetting('monthly_limit', value)}
              />
            </div>
          </div>

          {/* Provider Limits */}
          <div className="rounded-xl border border-white/8 bg-white/[0.02] p-4">
            <h3 className="text-sm font-medium text-white mb-4">Provider limits</h3>
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              <CurrencyInput
                label="Claude daily limit"
                value={formSettings.provider_limits?.claude || null}
                onChange={(value) => updateProviderLimit('claude', value)}
              />
              <CurrencyInput
                label="OpenAI daily limit"
                value={formSettings.provider_limits?.openai || null}
                onChange={(value) => updateProviderLimit('openai', value)}
              />
            </div>
          </div>

          {/* Warning Threshold */}
          <div className="rounded-xl border border-white/8 bg-white/[0.02] p-4">
            <div className="flex items-center gap-2 mb-4">
              <Gauge className="h-4 w-4 text-[var(--accent)]" />
              <h3 className="text-sm font-medium text-white">Warning settings</h3>
            </div>
            <WarningThresholdSlider
              value={formSettings.warning_threshold || 80}
              onChange={(value) => updateSetting('warning_threshold', value)}
            />
            <p className="mt-2 text-xs text-[var(--muted)]">
              Get notified when spending approaches your budget limits
            </p>
          </div>

          {/* Current Usage (if available) */}
          {usage && (
            <div className="rounded-xl border border-white/8 bg-white/[0.02] p-4">
              <h3 className="text-sm font-medium text-white mb-4">Current usage</h3>
              <div className="space-y-3">
                {formSettings.daily_limit && (
                  <UsageProgressBar
                    label="Daily"
                    current={usage.daily.cost}
                    budget={formSettings.daily_limit}
                  />
                )}
                {formSettings.weekly_limit && (
                  <UsageProgressBar
                    label="Weekly"
                    current={usage.weekly.cost}
                    budget={formSettings.weekly_limit}
                  />
                )}
                {formSettings.monthly_limit && (
                  <UsageProgressBar
                    label="Monthly"
                    current={usage.monthly.cost}
                    budget={formSettings.monthly_limit}
                  />
                )}
              </div>
            </div>
          )}
        </>
      )}

      {/* Save Button */}
      {hasChanges && (
        <div className="sticky bottom-0 border-t border-white/8 bg-[var(--panel)]/90 backdrop-blur-sm p-4 -m-5 mt-6">
          <div className="flex items-center justify-between gap-4">
            <p className="text-xs text-[var(--muted)]">
              You have unsaved changes to your budget settings.
            </p>
            <button
              onClick={handleSave}
              disabled={isSaving}
              className="flex items-center gap-2 rounded-lg bg-[var(--accent)] px-4 py-2 text-sm font-medium text-white transition hover:bg-[var(--accent)]/80 active:scale-95 disabled:opacity-50"
            >
              {isSaving ? (
                <Loader2 className="h-4 w-4 animate-spin" />
              ) : (
                <Save className="h-4 w-4" />
              )}
              {isSaving ? 'Saving...' : 'Save changes'}
            </button>
          </div>
        </div>
      )}

      {!formSettings.enabled && (
        <div className="rounded-xl border border-blue-400/20 bg-blue-400/8 p-4">
          <div className="flex items-start gap-3">
            <DollarSign className="h-5 w-5 text-blue-400 mt-0.5" />
            <div>
              <h4 className="text-sm font-medium text-blue-200">Budget controls disabled</h4>
              <p className="mt-1 text-xs text-blue-300">
                Enable budget controls to set spending limits and receive cost warnings for your BYOK providers.
                BYOS providers are always unlimited.
              </p>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}