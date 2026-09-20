import { lazy, Suspense } from 'react';

/**
 * The six panes of SURFACE.md, mounted onto what exists today.
 *
 * Where a pane has real machinery it mounts it. Where it does not, it says so
 * and names the gating task rather than rendering a plausible-looking screen.
 */

const LedgerView = lazy(() => import('../ledger/LedgerView'));
const AdminView = lazy(() => import('../admin/AdminView'));

function Loading() {
  return <div className="p-8 text-sm text-[var(--muted)]">Loading…</div>;
}

/// Runs and Receipts are real now — see RunsPane.tsx / ReceiptsPane.tsx.
/// Re-exported here so the route table in App.tsx keeps importing every pane
/// from one place.
export { default as RunsPane } from './RunsPane';
export { default as ReceiptsPane } from './ReceiptsPane';

export function LedgerPane() {
  return (
    <Suspense fallback={<Loading />}>
      <LedgerView />
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
