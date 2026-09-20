import { GitCommitHorizontal, ShieldQuestion } from 'lucide-react';
import type { VerifierReportPayload } from '../../lib/cortexApi';
import { OutcomeIcon, StatusChip } from './ui';
import { CopyEvidenceButton } from './Receipt';

/**
 * Renders today's verifier evidence: the engine verifier's evaluation of
 * what the worker reported about its own work.
 *
 * This is real evidence and it is rendered fully — but it is not the V6
 * receipt, and the header says so. The trust boundary matters: a check the
 * worker claims to have passed is a different fact from a check Cortex
 * re-executed in an isolated runner. Blurring that line here would ship the
 * hollow-gate disease with better typography.
 */

const NEXT_ACTION_COPY: Record<string, string> = {
  accept: 'accept',
  add_evidence: 'needs more evidence',
  fix_and_retry: 'fix and retry',
  rebase_and_retry: 'rebase and retry',
  narrow_scope: 'narrow the scope',
};

function verdictSummary(verdict: string): string {
  switch (verdict) {
    case 'success':
      return 'The evidence the worker reported satisfies this step’s contract.';
    case 'failed':
      return 'The reported evidence shows the work did not meet its contract.';
    case 'blocked':
      return 'Verification could not proceed — see the violations below.';
    case 'needs_evidence':
      return 'The worker did not report enough evidence to accept this step.';
    default:
      return 'Verifier outcome recorded.';
  }
}

export function EngineReceiptCard({ payload }: { payload: VerifierReportPayload }) {
  const { report, evidence } = payload;
  const engine = evidence.verifier_report;
  const worker = evidence.worker_completed;
  const checks = evidence.verifier_input?.evidence?.checks ?? [];
  const required = engine?.required_check_summary;
  const floor = engine?.evidence_floor;
  const coverage = engine?.acceptance_coverage;
  const violations = engine?.allowed_path_violations ?? [];
  const staleNotes = engine?.stale_base_notes ?? [];
  const verdict = engine?.verdict ?? report.verdict;

  return (
    <div className="overflow-hidden rounded-lg border border-[var(--line)] bg-[var(--surface)]">
      <div className="flex flex-wrap items-center gap-2 border-b border-[var(--line)] px-3 py-2.5">
        <StatusChip status={verdict} label={verdict.replaceAll('_', ' ')} />
        {required && required.total > 0 && (
          <span className="t-micro t-mono text-[var(--muted)]">
            {required.satisfied}/{required.total} required checks satisfied
          </span>
        )}
        {engine?.next_action && engine.next_action !== 'accept' && (
          <span className="t-micro text-[var(--warn-strong)]">
            → {NEXT_ACTION_COPY[engine.next_action] ?? engine.next_action}
          </span>
        )}
        <span className="ml-auto">
          <CopyEvidenceButton payload={payload} />
        </span>
      </div>

      <p className="t-micro border-b border-[var(--line-faint)] px-3 py-2 text-[var(--muted)]">
        {verdictSummary(verdict)}
      </p>

      {/* The trust caveat — the one line this component must never lose. */}
      <p className="t-micro flex items-start gap-1.5 border-b border-[var(--line-faint)] bg-[var(--surface-raised)] px-3 py-2 text-[var(--muted)]">
        <ShieldQuestion className="mt-px h-3.5 w-3.5 shrink-0 text-[var(--warn)]" aria-hidden />
        <span>
          Checks below were reported by the worker and evaluated by the engine
          verifier. Independently re-executed receipts — Cortex running the
          checks itself in an isolated runner — land with migration v61.
        </span>
      </p>

      {checks.length > 0 && (
        <ul>
          {checks.map((check, index) => (
            <li
              key={`${check.name}-${index}`}
              className="flex flex-wrap items-center gap-2 border-b border-[var(--line-faint)] px-3 py-2 last:border-b-0"
            >
              <OutcomeIcon outcome={check.status} />
              <span className="t-meta t-mono text-[var(--fg)]">{check.name}</span>
              <span className="t-micro text-[var(--muted)]">{check.status}</span>
              {check.summary && (
                <span className="t-micro w-full pl-5.5 text-[var(--muted)]">{check.summary}</span>
              )}
            </li>
          ))}
        </ul>
      )}

      {required && required.missing.length > 0 && (
        <div className="border-t border-[var(--line-faint)] px-3 py-2">
          <p className="t-micro text-[var(--warn-strong)]">
            Required but not reported:{' '}
            <span className="t-mono">{required.missing.map((check) => check.name).join(', ')}</span>
          </p>
        </div>
      )}

      {violations.length > 0 && (
        <div className="border-t border-[var(--line-faint)] px-3 py-2">
          <p className="t-micro font-medium text-[var(--err-strong)]">Allowed-path violations</p>
          <ul className="mt-1 space-y-0.5">
            {violations.map((violation) => (
              <li key={violation.path} className="t-micro text-[var(--muted)]">
                <span className="t-mono text-[var(--err)]">{violation.path}</span> — {violation.reason}
              </li>
            ))}
          </ul>
        </div>
      )}

      {staleNotes.length > 0 && (
        <div className="border-t border-[var(--line-faint)] px-3 py-2">
          {staleNotes.map((note, index) => (
            <p key={index} className="t-micro text-[var(--warn-strong)]">{note.note}</p>
          ))}
        </div>
      )}

      {coverage?.evaluated && (
        <p className="t-micro border-t border-[var(--line-faint)] px-3 py-2 text-[var(--muted)]">
          Acceptance criteria: {coverage.covered}/{coverage.total} covered
          {coverage.uncovered.length > 0 && (
            <> · uncovered: {coverage.uncovered.join('; ')}</>
          )}
        </p>
      )}

      {floor && !floor.satisfied && floor.missing.length > 0 && (
        <p className="t-micro border-t border-[var(--line-faint)] px-3 py-2 text-[var(--warn-strong)]">
          Evidence floor not met — missing {floor.missing.join(', ')}
        </p>
      )}

      {/* Provenance: what the worker delivered, pinned. */}
      <div className="t-micro t-mono space-y-0.5 border-t border-[var(--line)] px-3 py-2 leading-relaxed text-[var(--muted)]">
        {worker && (
          <p className="flex items-center gap-1.5">
            <GitCommitHorizontal className="h-3 w-3" aria-hidden />
            {worker.base_commit ? worker.base_commit.slice(0, 10) : 'base?'}
            {' → '}
            {worker.head_commit ? worker.head_commit.slice(0, 10) : 'head?'}
            {worker.branch && <span className="opacity-80">on {worker.branch}</span>}
            {typeof worker.exit_code === 'number' && <span>· exit {worker.exit_code}</span>}
            {worker.files_changed && worker.files_changed.length > 0 && (
              <span>· {worker.files_changed.length} file{worker.files_changed.length === 1 ? '' : 's'}</span>
            )}
          </p>
        )}
        <p>
          verifier {report.verifier} · report {report.id.slice(0, 12)} · lease gen {report.lease_gen}
          {report.worker_id && <> · worker {report.worker_id}</>}
        </p>
      </div>
    </div>
  );
}
