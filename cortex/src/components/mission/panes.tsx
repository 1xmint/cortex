import { lazy, Suspense } from 'react';

/**
 * The six panes of SURFACE.md, mounted onto what exists today.
 *
 * Where a pane has real machinery it mounts it. Where it does not, it says so
 * and names the gating task rather than rendering a plausible-looking screen.
 */

const LedgerView = lazy(() => import('../ledger/LedgerView'));
const UsageView = lazy(() => import('../usage/UsageView'));
const AdminView = lazy(() => import('../admin/AdminView'));

function Loading() {
  return <div className="p-8 text-sm text-[var(--muted)]">Loading…</div>;
}

/// Runs and Receipts are real now — see RunsPane.tsx / ReceiptsPane.tsx.
/// Re-exported here so the route table in App.tsx keeps importing every pane
/// from one place.
export { default as RunsPane } from './RunsPane';
export { default as ReceiptsPane } from './ReceiptsPane';

/**
 * Spend above, then why each model was picked.
 *
 * The nav calls this pane "credits, spend, refunds", and until now it showed
 * none of those — LedgerView renders routing decisions, which is a different
 * question. UsageView had the spend the whole time, against GET /api/usage
 * and GET /api/usage/daily, both served, and nothing in the app rendered it.
 *
 * They go together rather than into a pane of their own: they are the same
 * card twice, built to the same shape, and they answer two halves of one
 * question. SURFACE.md says six panes, and a seventh for one card would be a
 * worse answer than a truthful nav hint.
 */
export function LedgerPane() {
  return (
    <Suspense fallback={<Loading />}>
      <div className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4">
        <UsageView />
        <LedgerView />
      </div>
    </Suspense>
  );
}

export { default as LeasesPane } from './LeasesPane';

export function AdminPane() {
  return (
    <Suspense fallback={<Loading />}>
      <AdminView />
    </Suspense>
  );
}
