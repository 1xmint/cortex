import { useEffect, useState } from 'react';
import {
  getSomaDelegationSpend,
  getSomaSpend,
  SOMA_API_ENABLED,
  type SomaDelegationSpend,
  type SomaSpendSummary,
} from './cortexApi';

export function useSomaSpend() {
  const [data, setData] = useState<SomaSpendSummary | null>(null);
  // Starts false when the fence is up: `loading` is what the dashboard renders
  // a spinner for, and a spinner that never resolves is the worst of the
  // available lies about a subsystem that is not there.
  const [loading, setLoading] = useState(SOMA_API_ENABLED);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!SOMA_API_ENABLED) return;
    let cancelled = false;
    setLoading(true);
    getSomaSpend()
      .then((summary) => {
        if (!cancelled) {
          setData(summary);
          setError(null);
        }
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : 'Could not load spend');
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return { data, loading, error };
}

export function useSomaDelegationSpend(delegationId: string | null) {
  const [data, setData] = useState<SomaDelegationSpend | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!SOMA_API_ENABLED) return;
    if (!delegationId) {
      setData(null);
      return;
    }
    let cancelled = false;
    setLoading(true);
    getSomaDelegationSpend(delegationId)
      .then((detail) => {
        if (!cancelled) {
          setData(detail);
          setError(null);
        }
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : 'Could not load receipts');
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [delegationId]);

  return { data, loading, error };
}
