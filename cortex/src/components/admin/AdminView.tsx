import { useEffect, useState } from 'react';
import { ShieldCheck } from 'lucide-react';
import {
  CortexApiError,
  getAdminStats,
  getAdminWorkers,
  type AdminStats,
  type AdminWorkers,
} from '../../lib/cortexApi';

function readCount(value: unknown) {
  if (typeof value === 'number') return value;
  if (Array.isArray(value)) return value.length;
  if (value && typeof value === 'object') {
    const record = value as Record<string, unknown>;
    for (const key of ['count', 'total', 'active']) {
      if (typeof record[key] === 'number') return record[key];
    }
  }
  return 0;
}

export default function AdminView() {
  const [stats, setStats] = useState<AdminStats | null>(null);
  const [workers, setWorkers] = useState<AdminWorkers | null>(null);
  const [visible, setVisible] = useState(false);

  useEffect(() => {
    let cancelled = false;

    async function fetchAdmin() {
      try {
        const [nextStats, nextWorkers] = await Promise.all([getAdminStats(), getAdminWorkers()]);
        if (!cancelled) {
          setStats(nextStats);
          setWorkers(nextWorkers);
          setVisible(true);
        }
      } catch (err) {
        if (
          err instanceof CortexApiError
          && (err.status === 401 || err.status === 403 || err.status === 404)
        ) {
          if (!cancelled) setVisible(false);
          return;
        }
      }
    }

    void fetchAdmin();
    const interval = window.setInterval(fetchAdmin, 30_000);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, []);

  if (!visible || !stats) return null;

  const items = [
    { label: 'Runs', value: readCount(stats.runs) },
    { label: 'Steps', value: readCount(stats.steps) },
    { label: 'Workers', value: workers?.count ?? readCount(stats.workers) },
    { label: 'Users', value: readCount(stats.users) },
    { label: 'Decisions', value: readCount(stats.decisions) },
  ];

  return (
    <section className="space-y-2">
      <div className="flex items-center justify-between gap-3">
        <div>
          <h3 className="text-xs font-medium uppercase tracking-[0.08em] text-[var(--muted)]">
            Admin
          </h3>
          <p className="mt-1 text-xs leading-5 text-[var(--muted)]">
            System operator snapshot.
          </p>
        </div>
        <ShieldCheck className="h-4 w-4 text-[var(--accent)]" />
      </div>
      <div className="grid grid-cols-2 gap-2">
        {items.map((item) => (
          <div key={item.label} className="rounded-xl border border-white/8 bg-white/[0.03] px-3 py-2">
            <p className="text-[10px] uppercase tracking-[0.08em] text-[var(--muted)]">{item.label}</p>
            <p className="mt-1 text-sm font-medium text-white">{item.value}</p>
          </div>
        ))}
      </div>
    </section>
  );
}
