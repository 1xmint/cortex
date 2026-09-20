import { useCallback, useEffect, useState } from 'react';
import { getBillingStatus, type BillingStatus } from './cortexApi';

interface UseBillingResult {
  status: BillingStatus | null;
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
}

export function useBilling(enabled: boolean): UseBillingResult {
  const [status, setStatus] = useState<BillingStatus | null>(null);
  const [loading, setLoading] = useState(enabled);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!enabled) {
      setLoading(false);
      return;
    }
    setLoading(true);

    // Resilient billing check with retries to prevent deploy drift
    let lastError: Error | string | null = null;
    for (let attempt = 1; attempt <= 3; attempt++) {
      try {
        const next = await getBillingStatus();
        setStatus(next);
        setError(null);
        return;
      } catch (err) {
        lastError = err instanceof Error ? err.message : 'Billing is unavailable';
        if (attempt < 3) {
          // Exponential backoff: 1s, 2s, then give up
          await new Promise(resolve => setTimeout(resolve, attempt * 1000));
        }
      }
    }

    // Only set error after all retries failed
    setError(lastError);
    setLoading(false);
  }, [enabled]);

  useEffect(() => {
    void refresh();
    if (!enabled) return undefined;
    const interval = window.setInterval(() => {
      void refresh();
    }, 30_000);
    return () => window.clearInterval(interval);
  }, [enabled, refresh]);

  return { status, loading, error, refresh };
}
