import { useCallback, useEffect, useMemo, useState } from 'react';
import { useSearchParams } from 'react-router';
import { BadgeCheck, RefreshCw } from 'lucide-react';
import {
  getReceipt,
  getRun,
  getVerifierReport,
  listRuns,
  type VerifierReportPayload,
} from '../../lib/cortexApi';
import { ReceiptCard, type Receipt } from './Receipt';
import { EngineReceiptCard } from './EngineReceipt';
import { EmptyState, ErrorState, PaneHeader, SkeletonRows, StatusChip, StatusIcon } from './ui';

/**
 * Receipts — pane two of mission control (SURFACE.md), the trust page.
 *
 * The index is assembled from recent runs' steps that carry a
 * verifier_report_id; a dedicated list endpoint arrives with migration v61
 * and replaces the scan. Every receipt shown is fetched from storage —
 * nothing on this pane is synthesized, and when there is nothing stored the
 * pane says so instead of showing a plausible verification that never
 * happened.
 */

const SCAN_RUNS = 12;

interface ReceiptRef {
  runId: string;
  runGoal: string;
  stepId: string;
  stepTitle: string;
  reportId: string;
  verdict: string | null;
  status: string | null;
}

/** V6 evidence carries the full receipt object; detect it structurally. */
function asV6Receipt(payload: VerifierReportPayload): Receipt | null {
  const { evidence, report } = payload;
  if (!evidence.gate || !Array.isArray(evidence.executions)) return null;
  return {
    verification_id: evidence.verification_id ?? report.id,
    run_id: report.run_id,
    step_id: report.step_id,
    attempt: evidence.attempt ?? 1,
    tree_hash: evidence.tree_hash ?? '',
    gate: evidence.gate,
    executions: evidence.executions,
  };
}

export default function ReceiptsPane() {
  const [searchParams, setSearchParams] = useSearchParams();
  const [refs, setRefs] = useState<ReceiptRef[] | null>(null);
  const [scanError, setScanError] = useState<string | null>(null);
  const [payload, setPayload] = useState<VerifierReportPayload | null>(null);
  const [payloadError, setPayloadError] = useState<string | null>(null);
  /**
   * The independently-executed receipt, when V3 has one for this step.
   *
   * Fetched separately from the legacy report rather than read out of it:
   * V3 stores verdicts in their own tables and serves them from their own
   * endpoint, so the evidence blob on a `verifier_reports` row will never
   * carry them.
   */
  const [receipt, setReceipt] = useState<Receipt | null>(null);
  const [loadingPayload, setLoadingPayload] = useState(false);

  const selected = useMemo(() => {
    const runId = searchParams.get('run');
    const stepId = searchParams.get('step');
    const reportId = searchParams.get('report');
    return runId && stepId && reportId ? { runId, stepId, reportId } : null;
  }, [searchParams]);

  const scan = useCallback(async () => {
    try {
      const runs = await listRuns(SCAN_RUNS, 0);
      const details = await Promise.allSettled(runs.map((run) => getRun(run.id)));
      const found: ReceiptRef[] = [];
      details.forEach((result) => {
        if (result.status !== 'fulfilled') return;
        const run = result.value;
        for (const step of run.steps) {
          if (!step.verifier_report_id) continue;
          found.push({
            runId: run.id,
            runGoal: run.goal,
            stepId: step.id,
            stepTitle: step.title || step.objective || step.goal || step.id,
            reportId: step.verifier_report_id,
            verdict: step.verifier_verdict ?? null,
            status: step.verification_status ?? null,
          });
        }
      });
      setRefs(found);
      setScanError(
        details.some((result) => result.status === 'rejected') && found.length === 0
          ? 'Some runs could not be read — the list may be incomplete.'
          : null,
      );
    } catch (err) {
      setRefs([]);
      setScanError(err instanceof Error ? err.message : 'could not scan for receipts');
    }
  }, []);

  useEffect(() => {
    void scan();
  }, [scan]);

  // Auto-select the newest receipt when nothing is deep-linked.
  useEffect(() => {
    if (!selected && refs && refs.length > 0) {
      const first = refs[0];
      setSearchParams(
        { run: first.runId, step: first.stepId, report: first.reportId },
        { replace: true },
      );
    }
  }, [refs, selected, setSearchParams]);

  useEffect(() => {
    setPayload(null);
    setPayloadError(null);
    setReceipt(null);
    if (!selected) return;
    let cancelled = false;
    setLoadingPayload(true);

    // Independent of the legacy fetch: a step can have a V3 receipt, a legacy
    // report, or both, and neither failing should hide the other. A null here
    // is the ordinary answer for anything verified before V3 shipped.
    void getReceipt(selected.runId, selected.stepId)
      .then((next) => {
        if (!cancelled) setReceipt(next);
      })
      .catch(() => {
        // Leave `receipt` null and let the legacy report render. A receipt
        // that cannot be loaded must never be shown as a verdict.
      });

    void getVerifierReport(selected.runId, selected.stepId, selected.reportId)
      .then((next) => {
        if (!cancelled) setPayload(next);
      })
      .catch((err) => {
        if (!cancelled) {
          setPayloadError(err instanceof Error ? err.message : 'could not load the report');
        }
      })
      .finally(() => {
        if (!cancelled) setLoadingPayload(false);
      });
    return () => {
      cancelled = true;
    };
  }, [selected]);

  // Prefer the independently-executed receipt. `asV6Receipt` remains as the
  // fallback for evidence blobs that embedded a gate before V3 had its own
  // tables, so no already-rendered receipt regresses.
  const v6 = receipt ?? (payload ? asV6Receipt(payload) : null);

  return (
    <>
      <PaneHeader
        title="Receipts"
        meta={refs ? `${refs.length} across the last ${SCAN_RUNS} runs` : undefined}
        actions={
          <button
            type="button"
            onClick={() => void scan()}
            className="rounded p-1 text-[var(--muted)] transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--fg)]"
            aria-label="Rescan recent runs for receipts"
          >
            <RefreshCw className="h-3.5 w-3.5" aria-hidden />
          </button>
        }
      />

      <div className="flex min-h-0 flex-1">
        <aside className="w-80 shrink-0 overflow-y-auto border-r border-[var(--line)]">
          {refs === null ? (
            <SkeletonRows count={5} />
          ) : refs.length === 0 ? (
            <div className="p-4">
              {scanError ? (
                <ErrorState message={scanError} onRetry={() => void scan()} compact />
              ) : (
                <p className="t-micro text-[var(--muted)]">
                  No verification reports in the last {SCAN_RUNS} runs. A receipt
                  appears here the moment a step is verified — never before.
                </p>
              )}
            </div>
          ) : (
            <ul>
              {scanError && (
                <li className="t-micro border-b border-[var(--warn-line)] bg-[var(--warn-soft)] px-3 py-1.5 text-[var(--warn-strong)]">
                  {scanError}
                </li>
              )}
              {refs.map((ref) => {
                const isActive =
                  selected?.runId === ref.runId
                  && selected?.stepId === ref.stepId
                  && selected?.reportId === ref.reportId;
                return (
                  <li key={`${ref.stepId}-${ref.reportId}`}>
                    <button
                      type="button"
                      onClick={() =>
                        setSearchParams({ run: ref.runId, step: ref.stepId, report: ref.reportId })
                      }
                      className={`flex w-full items-start gap-2 border-b border-[var(--line-faint)] px-3 py-2.5 text-left transition-colors ${
                        isActive ? 'bg-[var(--surface-active)]' : 'hover:bg-[var(--surface-hover)]'
                      }`}
                    >
                      <span className="mt-0.5">
                        <StatusIcon status={ref.status ?? ref.verdict} />
                      </span>
                      <span className="min-w-0 flex-1">
                        <span className="t-body block truncate text-[var(--fg)]">{ref.stepTitle}</span>
                        <span className="t-micro block truncate text-[var(--muted)]">{ref.runGoal}</span>
                      </span>
                      {ref.verdict && <StatusChip status={ref.verdict} label={ref.verdict} className="mt-0.5" />}
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
        </aside>

        <section className="min-w-0 flex-1 overflow-y-auto">
          {refs !== null && refs.length === 0 && !scanError ? (
            <EmptyState
              icon={<BadgeCheck className="h-4 w-4" aria-hidden />}
              title="Nothing verified yet"
            >
              This page will hold every verification: the commands that ran,
              their exit codes, and enough output to settle a dispute. It is
              deliberately empty until a real verdict exists — a plausible
              receipt for a verification that never happened is exactly what
              this pane exists to make impossible.
            </EmptyState>
          ) : loadingPayload ? (
            <div className="p-4">
              <SkeletonRows count={3} height="h-20" />
            </div>
          ) : payloadError ? (
            <ErrorState
              message={payloadError}
              onRetry={() => setSearchParams(new URLSearchParams(searchParams), { replace: false })}
            />
          ) : payload ? (
            <div className="mx-auto max-w-3xl space-y-3 p-4">
              {v6 ? <ReceiptCard receipt={v6} /> : <EngineReceiptCard payload={payload} />}
            </div>
          ) : null}
        </section>
      </div>
    </>
  );
}
