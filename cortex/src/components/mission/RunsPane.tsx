import { useCallback, useEffect, useRef, useState } from 'react';
import { Link } from 'react-router';
import {
  Activity,
  BadgeCheck,
  ChevronDown,
  ChevronRight,
  ExternalLink,
  GitPullRequest,
  RefreshCw,
  XCircle,
} from 'lucide-react';
import {
  cancelRun,
  createRunPullRequest,
  getRun,
  listRuns,
  streamRun,
  type RunListItem,
  ACCEPTED_STEP_STATUSES,
  UNVERIFIED_STEP_STATUSES,
  type RunStep,
  type RunSummary,
} from '../../lib/cortexApi';
import { EmptyState, ErrorState, PaneHeader, SkeletonRows, StatusChip, StatusIcon } from './ui';

/**
 * Runs — pane one of mission control (SURFACE.md).
 *
 * Live state over SSE, per PLAN §6. WebSockets stay out of the frontend; the
 * WS infrastructure is for workers.
 *
 * Steps that have been verified link straight to their receipt on the
 * Receipts pane — the verdict chip here is a claim, and the link is the
 * evidence for it. Steps without a stored report show no verification badge
 * at all: a badge with nothing behind it is worse than no badge.
 *
 * A step that a worker delivered but Cortex has not yet graded shows no
 * verification badge either, even though a worker report exists for it. The
 * report is the worker's account of its own work; rendering it with a check
 * mark is how a claim we never checked reaches a customer as though we had.
 *
 * A run that finished can have its branch pushed and a pull request opened
 * from here, against POST /api/runs/{id}/pr. That button used to live on
 * components/runs/RunPanel.tsx, which this pane superseded and nothing routed
 * to, so the backend worked and no screen could reach it. The wording under
 * the button says what was not verified, for the same reason the step rows
 * do: a pull request is where this work stops being ours and starts being
 * somebody's to review.
 */

// Delivered and verifying are absent on purpose: work handed over but not
// checked has not finished, and a row that renders it as terminal makes the
// claim the truth model exists to stop.
const TERMINAL = new Set([
  'verified',
  'manual_override',
  'failed',
  'execution_failed',
  'cancelled',
  'completed',
]);

// A run that finished without failing is the only kind worth offering a pull
// request for. The backend decides for real -- it answers 422 "run has no
// branch" when the run changed nothing -- but there is no reason to show a
// button for a run that was cancelled or that crashed.
const SHIPPABLE = new Set(['verified', 'manual_override', 'completed']);

// A run is only worth offering a Cancel button while it can still spend
// money or do work: once it is planning, running, or merely queued, there is
// something to stop. A terminal run has nothing left to cancel.
const CANCELLABLE = new Set(['pending', 'planning', 'running']);

function StepRow({ step, runId }: { step: RunStep; runId: string }) {
  const [expanded, setExpanded] = useState(false);
  const attempts =
    step.attempt_count && step.attempt_count > 1
      ? `attempt ${step.attempt_count}${step.max_attempts ? `/${step.max_attempts}` : ''}`
      : null;
  const error = step.error ?? step.last_error;
  const hasDetail = Boolean(step.output_summary || error || (step.files_changed?.length ?? 0) > 0);
  const Chevron = expanded ? ChevronDown : ChevronRight;
  const accepted = ACCEPTED_STEP_STATUSES.includes(step.status);
  const unverified = UNVERIFIED_STEP_STATUSES.includes(step.status);

  return (
    <li className="border-b border-[var(--line-faint)] last:border-b-0">
      <div
        className={`flex items-start gap-2.5 px-3 py-2 ${hasDetail ? 'cursor-pointer hover:bg-[var(--surface-hover)]' : ''}`}
        onClick={hasDetail ? () => setExpanded((current) => !current) : undefined}
      >
        <span className="mt-0.5">
          <StatusIcon status={step.status} />
        </span>
        <div className="min-w-0 flex-1">
          <p className="t-body flex items-center gap-1.5 text-[var(--fg)]">
            <span className="truncate">{step.title || step.objective || step.goal || step.id}</span>
            {hasDetail && <Chevron className="h-3 w-3 shrink-0 text-[var(--muted)]" aria-hidden />}
          </p>
          <p className="t-micro mt-0.5 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-[var(--muted)]">
            <span>{step.status}</span>
            {step.kind && <span>· {step.kind}</span>}
            {step.tier && <span>· {step.tier}</span>}
            {step.risk && <span>· risk {step.risk}</span>}
            {attempts && <span>· {attempts}</span>}
          </p>
        </div>
        {step.verifier_report_id ? (
          <Link
            to={`/receipts?run=${encodeURIComponent(runId)}&step=${encodeURIComponent(step.id)}&report=${encodeURIComponent(step.verifier_report_id)}`}
            onClick={(event) => event.stopPropagation()}
            className="mt-0.5 inline-flex shrink-0 items-center gap-1"
            title={accepted ? 'Open the verification receipt' : 'Open the worker-reported diagnostics'}
          >
            {accepted ? (
              <StatusChip
                status={step.verification_status ?? step.verifier_verdict}
                label={
                  <>
                    <BadgeCheck className="h-3 w-3" aria-hidden />
                    {(step.verifier_verdict ?? step.verification_status ?? 'report').replaceAll('_', ' ')}
                  </>
                }
              />
            ) : (
              <StatusChip status="unknown" label="worker-reported" />
            )}
          </Link>
        ) : null}
      </div>

      {expanded && hasDetail && (
        <div className="t-micro space-y-1.5 border-t border-[var(--line-faint)] bg-[var(--inset)] px-3 py-2 pl-9 text-[var(--muted)]">
          {step.output_summary && (
            <>
              <p className="t-micro uppercase tracking-wide text-[var(--muted)]">
                Worker-reported diagnostics
              </p>
              <p className="whitespace-pre-wrap">{step.output_summary}</p>
              {unverified && (
                <p className="text-[var(--warn-strong)]">
                  Cortex has not verified this yet. Nothing here has been checked.
                </p>
              )}
            </>
          )}
          {error && <p className="text-[var(--err-strong)]">{error}</p>}
          {step.files_changed && step.files_changed.length > 0 && (
            <p className="t-mono truncate" title={step.files_changed.join('\n')}>
              {step.files_changed.length} file{step.files_changed.length === 1 ? '' : 's'}:{' '}
              {step.files_changed.slice(0, 4).join(', ')}
              {step.files_changed.length > 4 && ' …'}
            </p>
          )}
        </div>
      )}
    </li>
  );
}

export default function RunsPane() {
  const [runs, setRuns] = useState<RunListItem[] | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [detail, setDetail] = useState<RunSummary | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [live, setLive] = useState(false);
  const streamRef = useRef<AbortController | null>(null);

  // Keyed by run, not reset on switch. A RunSummary carries no pr_url, so
  // once you leave a run the app forgets it has a pull request; holding the
  // id means clicking back to it still shows the link instead of a button
  // that would push the same branch a second time. It also makes a slow
  // request that lands after a switch harmless: it belongs to a run that is
  // no longer on screen, so nothing renders it.
  const [pr, setPr] = useState<{ runId: string; url: string; branch: string } | null>(null);
  const [prPending, setPrPending] = useState<string | null>(null);
  const [prError, setPrError] = useState<{ runId: string; message: string } | null>(null);

  // Two-step confirm: the first click just arms the button, so a stray click
  // never stops a run. `cancelConfirm` holds the run id awaiting a second
  // click; it is cleared on any run switch so an armed button never survives
  // into a different run's row.
  const [cancelConfirm, setCancelConfirm] = useState<string | null>(null);
  const [cancelPending, setCancelPending] = useState<string | null>(null);
  const [cancelError, setCancelError] = useState<{ runId: string; message: string } | null>(null);

  const cancelSelectedRun = useCallback(async (runId: string) => {
    setCancelPending(runId);
    setCancelError(null);
    try {
      await cancelRun(runId);
      const refreshed = await getRun(runId);
      setDetail((current) => (current?.id === runId ? refreshed : current));
    } catch (err) {
      setCancelError({
        runId,
        message: err instanceof Error ? err.message : 'could not cancel run',
      });
    } finally {
      setCancelConfirm(null);
      setCancelPending((current) => (current === runId ? null : current));
    }
  }, []);

  const openPullRequest = useCallback(async (runId: string) => {
    setPrPending(runId);
    setPrError(null);
    try {
      const created = await createRunPullRequest(runId);
      setPr({ runId, url: created.pr_url, branch: created.branch });
    } catch (err) {
      // The backend's own words. "run has no branch — no changes were made",
      // "PR creation requires a run-owned write lease" and "access denied"
      // each tell the user something different and something actionable;
      // flattening them into "could not create pull request" does not.
      setPrError({
        runId,
        message: err instanceof Error ? err.message : 'could not open a pull request',
      });
    } finally {
      setPrPending((current) => (current === runId ? null : current));
    }
  }, []);

  const loadRuns = useCallback(async () => {
    try {
      const items = await listRuns(25, 0);
      setRuns(items);
      setError(null);
      setSelected((current) => current ?? items[0]?.id ?? null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'could not load runs');
      setRuns([]);
    }
  }, []);

  useEffect(() => {
    void loadRuns();
  }, [loadRuns]);

  // One stream per selected run, torn down on switch. Without the cleanup a
  // user clicking through five runs would hold five open connections, and the
  // last one to deliver would win — which looks exactly like flapping state.
  useEffect(() => {
    streamRef.current?.abort();
    streamRef.current = null;
    setDetail(null);
    setLive(false);
    setCancelConfirm(null);
    if (!selected) return;

    let cancelled = false;

    void (async () => {
      try {
        const summary = await getRun(selected);
        if (cancelled) return;
        setDetail(summary);
        setError(null);

        if (summary.status && TERMINAL.has(summary.status)) return;

        streamRef.current = streamRun(
          selected,
          (event) => {
            if (cancelled) return;
            setLive(true);
            setDetail((current) =>
              current
                ? {
                    ...current,
                    steps: event.steps ?? current.steps,
                    graph: event.graph ?? current.graph,
                    status: event.status ?? current.status,
                  }
                : current,
            );
            if (event.type === 'run_complete') setLive(false);
          },
          (err) => {
            if (cancelled) return;
            setLive(false);
            setError(err.message);
          },
        );
      } catch (err) {
        if (!cancelled) setError(err instanceof Error ? err.message : 'could not load run');
      }
    })();

    return () => {
      cancelled = true;
      streamRef.current?.abort();
      streamRef.current = null;
    };
  }, [selected]);

  const anyReceipts = detail?.steps.some((step) => step.verifier_report_id) ?? false;

  return (
    <>
      <PaneHeader
        title="Runs"
        meta={
          live ? (
            <span className="inline-flex items-center gap-1 text-[var(--accent)]">
              <span className="h-1.5 w-1.5 rounded-full bg-[var(--accent)]" />
              live
            </span>
          ) : runs ? (
            `${runs.length} recent`
          ) : undefined
        }
        actions={
          <button
            type="button"
            onClick={() => void loadRuns()}
            className="rounded p-1 text-[var(--muted)] transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--fg)]"
            aria-label="Refresh runs"
          >
            <RefreshCw className="h-3.5 w-3.5" aria-hidden />
          </button>
        }
      />

      <div className="flex min-h-0 flex-1">
        {/* "Nothing has run yet" and "we could not ask" are different facts,
            and only one of them is the user's problem. */}
        {runs === null ? (
          <div className="flex-1">
            <SkeletonRows count={6} />
          </div>
        ) : runs.length === 0 ? (
          <div className="flex-1 overflow-y-auto">
            {error ? (
              <ErrorState message={error} onRetry={() => void loadRuns()} />
            ) : (
              <EmptyState icon={<Activity className="h-4 w-4" aria-hidden />} title="Nothing has run yet">
                Start a task from the chat pane and it appears here, step by
                step, as it executes.
              </EmptyState>
            )}
          </div>
        ) : (
          <>
            <aside className="w-72 shrink-0 overflow-y-auto border-r border-[var(--line)]">
              <ul>
                {runs.map((run) => (
                  <li key={run.id}>
                    <button
                      type="button"
                      onClick={() => setSelected(run.id)}
                      className={`flex w-full items-start gap-2 border-b border-[var(--line-faint)] px-3 py-2.5 text-left transition-colors ${
                        selected === run.id ? 'bg-[var(--surface-active)]' : 'hover:bg-[var(--surface-hover)]'
                      }`}
                    >
                      <span className="mt-0.5">
                        <StatusIcon status={run.status} />
                      </span>
                      <span className="min-w-0 flex-1">
                        <span className="t-body block truncate text-[var(--fg)]">{run.goal}</span>
                        <span className="t-micro block text-[var(--muted)]">
                          {run.status} · {run.profile}
                        </span>
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
            </aside>

            <section className="min-w-0 flex-1 overflow-y-auto">
              {error && <ErrorState message={error} compact />}

              {!detail ? (
                <SkeletonRows count={4} />
              ) : (
                <div className="p-4">
                  <header className="mb-3">
                    <h2 className="t-title">{detail.goal}</h2>
                    <p className="t-micro mt-1 flex items-center gap-2 text-[var(--muted)]">
                      <StatusChip status={detail.status} label={detail.status ?? 'unknown'} />
                      {detail.profile && <span>{detail.profile}</span>}
                      {detail.status && CANCELLABLE.has(detail.status) && (
                        <button
                          type="button"
                          disabled={cancelPending === detail.id}
                          onClick={() => {
                            if (cancelConfirm === detail.id) {
                              void cancelSelectedRun(detail.id);
                            } else {
                              setCancelConfirm(detail.id);
                            }
                          }}
                          className="t-micro ml-auto inline-flex items-center gap-1 rounded-md border border-[var(--err-line)] px-2 py-0.5 text-[var(--err-strong)] transition-colors hover:bg-[var(--err-soft)] disabled:cursor-not-allowed disabled:opacity-50"
                        >
                          <XCircle className={`h-3 w-3 ${cancelPending === detail.id ? 'animate-pulse' : ''}`} aria-hidden />
                          {cancelPending === detail.id
                            ? 'Cancelling…'
                            : cancelConfirm === detail.id
                              ? 'Click again to cancel'
                              : 'Cancel run'}
                        </button>
                      )}
                    </p>
                    {cancelError?.runId === detail.id && (
                      <p className="t-micro mt-1.5 rounded border border-[var(--err-line)] bg-[var(--err-soft)] px-2.5 py-1.5 text-[var(--err-strong)]">
                        {cancelError.message}
                      </p>
                    )}
                  </header>

                  <ul className="rounded-lg border border-[var(--line)] bg-[var(--surface)]">
                    {detail.steps.length === 0 ? (
                      <li className="t-body px-3 py-4 text-[var(--muted)]">No steps yet.</li>
                    ) : (
                      detail.steps.map((step) => <StepRow key={step.id} step={step} runId={detail.id} />)
                    )}
                  </ul>

                  {!anyReceipts && detail.steps.length > 0 && (
                    <p className="t-micro mt-3 rounded-lg border border-[var(--line)] bg-[var(--surface)] px-3 py-2 text-[var(--muted)]">
                      No step in this run has a stored verification report. When
                      one does, its verdict chip appears on the step and links to
                      the receipt — a badge with nothing behind it is worse than
                      no badge.
                    </p>
                  )}

                  {/* Below the verification note on purpose. Opening a pull
                      request is the moment this run's work leaves the machine
                      and asks a person to look at it, so what was and was not
                      checked belongs above the button, not after it. */}
                  {detail.status && SHIPPABLE.has(detail.status) && (
                    <div className="mt-3 rounded-lg border border-[var(--line)] bg-[var(--surface)] px-3 py-2.5">
                      {pr?.runId === detail.id ? (
                        <a
                          href={pr.url}
                          target="_blank"
                          rel="noreferrer"
                          className="t-body inline-flex items-center gap-1.5 text-[var(--accent)] hover:underline"
                        >
                          <GitPullRequest className="h-3.5 w-3.5" aria-hidden />
                          Pull request open
                          <ExternalLink className="h-3 w-3" aria-hidden />
                        </a>
                      ) : (
                        <button
                          type="button"
                          disabled={prPending === detail.id}
                          onClick={() => void openPullRequest(detail.id)}
                          className="t-body inline-flex items-center gap-1.5 rounded-md border border-[var(--line)] bg-[var(--surface-raised)] px-2.5 py-1 text-[var(--fg)] transition-colors hover:bg-[var(--surface-hover)] disabled:cursor-not-allowed disabled:opacity-50"
                        >
                          <GitPullRequest
                            className={`h-3.5 w-3.5 ${prPending === detail.id ? 'animate-pulse' : ''}`}
                            aria-hidden
                          />
                          {prPending === detail.id ? 'Pushing the branch…' : 'Open pull request'}
                        </button>
                      )}

                      <p className="t-micro mt-1.5 text-[var(--muted)]">
                        {pr?.runId === detail.id
                          ? `Branch ${pr.branch} is pushed. Nothing is merged — the pull request is where a person decides that.`
                          : anyReceipts
                            ? "Pushes this run's branch and opens a pull request. Nothing is merged."
                            : "Pushes this run's branch and opens a pull request. Nothing in this run has been verified, and nothing is merged."}
                      </p>

                      {prError?.runId === detail.id && (
                        <p className="t-micro mt-1.5 rounded border border-[var(--err-line)] bg-[var(--err-soft)] px-2.5 py-1.5 text-[var(--err-strong)]">
                          {prError.message}
                        </p>
                      )}
                    </div>
                  )}
                </div>
              )}
            </section>
          </>
        )}
      </div>
    </>
  );
}
