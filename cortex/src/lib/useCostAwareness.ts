import { useState, useEffect, useCallback } from 'react';
import {
  getCurrentUsage,
  getCostWarnings,
  acknowledgeCostWarning,
  getProviderStatus,
  type UsageData,
  type CostWarning,
  type ProviderStatusInfo,
} from './cortexApi';

export interface CostAwarenessState {
  usage: UsageData | null;
  warnings: CostWarning[];
  providers: ProviderStatusInfo[];
  loading: boolean;
  error: string | null;
}

export interface CostAwarenessHook {
  state: CostAwarenessState;
  refresh: () => void;
  acknowledgeWarning: (warningId: string) => Promise<void>;
  hasActiveWarnings: boolean;
  hasBlockingWarnings: boolean;
}

export function useCostAwareness(autoRefresh = true): CostAwarenessHook {
  const [state, setState] = useState<CostAwarenessState>({
    usage: null,
    warnings: [],
    providers: [],
    loading: true,
    error: null,
  });

  const fetchData = useCallback(async () => {
    try {
      setState(prev => ({ ...prev, loading: true, error: null }));

      const [usage, warnings, providers] = await Promise.all([
        getCurrentUsage().catch(() => null), // Don't fail if usage is unavailable
        getCostWarnings().catch(() => []), // Don't fail if warnings are unavailable
        getProviderStatus().catch(() => []), // Don't fail if provider status is unavailable
      ]);

      setState({
        usage,
        warnings,
        providers,
        loading: false,
        error: null,
      });
    } catch (err) {
      setState(prev => ({
        ...prev,
        loading: false,
        error: err instanceof Error ? err.message : 'Failed to load cost data',
      }));
    }
  }, []);

  const acknowledgeWarning = useCallback(async (warningId: string) => {
    try {
      await acknowledgeCostWarning(warningId);

      // Update local state to mark warning as acknowledged
      setState(prev => ({
        ...prev,
        warnings: prev.warnings.map(warning =>
          warning.id === warningId
            ? { ...warning, acknowledged: true }
            : warning
        ),
      }));
    } catch (err) {
      console.error('Failed to acknowledge warning:', err);
      throw err;
    }
  }, []);

  const refresh = useCallback(() => {
    fetchData();
  }, [fetchData]);

  useEffect(() => {
    fetchData();
  }, [fetchData]);

  // Auto-refresh cost data every minute if enabled
  useEffect(() => {
    if (!autoRefresh) return;

    const interval = setInterval(fetchData, 60000); // Refresh every minute
    return () => clearInterval(interval);
  }, [fetchData, autoRefresh]);

  const hasActiveWarnings = state.warnings.some(w => !w.acknowledged);
  const hasBlockingWarnings = state.warnings.some(w => w.action_required && w.level === 'error' && !w.acknowledged);

  return {
    state,
    refresh,
    acknowledgeWarning,
    hasActiveWarnings,
    hasBlockingWarnings,
  };
}

export function useSessionCosts() {
  const { state } = useCostAwareness(true);

  return {
    currentCost: state.usage?.current_session.cost || 0,
    tokenCount: state.usage?.current_session.token_count || 0,
    requestCount: state.usage?.current_session.request_count || 0,
    duration: state.usage?.current_session.duration_minutes || 0,
    providerBreakdown: state.usage?.provider_breakdown || [],
    loading: state.loading,
  };
}