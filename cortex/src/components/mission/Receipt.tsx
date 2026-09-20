import { useState } from 'react';
import { AlertCircle, CheckCircle2, Copy, HelpCircle, MinusCircle } from 'lucide-react';

/**
 * The receipt: what Cortex ran, what happened, and why a charge is defensible.
 *
 * Shapes mirror `cortex_core::verification` exactly (V1/V5) so this renders
 * the real thing the moment V3 persists it — no translation layer to drift.
 *
 * The rule this component exists to honour: it renders only what a check
 * execution actually reported. No inferred status, no "probably passed", no
 * summary that outruns its evidence. A receipt that flatters the run is worse
 * than no receipt, because the whole product claim rests on this being
 * literally true.
 *
 * Evidence hierarchy, top to bottom: verdict → checks → tails → provenance.
 * A reader in a dispute descends exactly as far as they need to and no
 * further.
 */

export type Verdict = 'verified' | 'failed' | 'inconclusive' | 'unverified';
export type CheckOutcome = 'passed' | 'failed' | 'timed_out' | 'not_executed';

export interface CheckExecution {
  spec_id: string;
  exit_code: number | null;
  outcome: CheckOutcome;
  duration_ms: number;
  output_digest: string;
  output_tail: string;
  runner_image: string;
}

export interface VerdictReport {
  verdict: Verdict;
  required_total: number;
  required_passed: number;
  failed: string[];
  not_executed: string[];
}

/**
 * What the sandbox could reach while the step ran.
 *
 * Absent for a step that ran before scoped egress was recorded. Present with an
 * empty `endpoints` means the sandbox reached nothing — a different fact, and
 * the one worth being able to rely on.
 */
export interface EgressReceipt {
  granted_registries: string[];
  endpoints: string[];
  mediator_image?: string;
}

export interface Receipt {
  verification_id: string;
  run_id: string;
  step_id: string;
  attempt: number;
  tree_hash: string;
  gate: VerdictReport;
  executions: CheckExecution[];
  egress?: EgressReceipt;
}

const VERDICT_COPY: Record<Verdict, { label: string; detail: string; banner: string }> = {
  verified: {
    label: 'Verified',
    detail: 'Every required check ran here and passed.',
    banner: 'border-[var(--ok-line)] bg-[var(--ok-soft)] text-[var(--ok-strong)]',
  },
  failed: {
    label: 'Failed',
    detail:
      'A required check ran and did not pass. No charge; a charge already taken is refunded.',
    banner: 'border-[var(--err-line)] bg-[var(--err-soft)] text-[var(--err-strong)]',
  },
  inconclusive: {
    label: 'Inconclusive',
    // The distinction that keeps the refund promise honest in both directions.
    detail: 'A required check could not be run. That is ours, not yours — no charge and no refund.',
    banner: 'border-[var(--warn-line)] bg-[var(--warn-soft)] text-[var(--warn-strong)]',
  },
  unverified: {
    label: 'Unverified',
    detail:
      'No checks were derivable for this work. Charged at the normal rate, without the verified badge and without the refund promise.',
    banner: 'border-[var(--line-strong)] bg-[var(--surface-raised)] text-[var(--muted-strong)]',
  },
};

function OutcomeGlyph({ outcome }: { outcome: CheckOutcome }) {
  if (outcome === 'passed') return <CheckCircle2 className="h-3.5 w-3.5 text-[var(--ok)]" aria-hidden />;
  if (outcome === 'failed' || outcome === 'timed_out') {
    return <AlertCircle className="h-3.5 w-3.5 text-[var(--err)]" aria-hidden />;
  }
  if (outcome === 'not_executed') return <HelpCircle className="h-3.5 w-3.5 text-[var(--warn)]" aria-hidden />;
  return <MinusCircle className="h-3.5 w-3.5 text-[var(--muted)]" aria-hidden />;
}

function outcomeLabel(execution: CheckExecution): string {
  switch (execution.outcome) {
    case 'passed':
      return `passed (exit ${execution.exit_code ?? 0})`;
    case 'failed':
      return `failed (exit ${execution.exit_code ?? '?'})`;
    case 'timed_out':
      // Stated as a failure of the work, not of our infrastructure — a hung
      // test is the defect the customer is paying to be protected from.
      return 'timed out — counted as a failure';
    case 'not_executed':
      return 'could not be run';
  }
}

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.floor(ms / 60_000)}m ${Math.round((ms % 60_000) / 1000)}s`;
}

/**
 * Copies the machine-readable evidence — the replay affordance. Everything a
 * dispute or a re-run needs (tree hash, runner image, specs, digests) in one
 * paste, byte-identical to what the verdict was computed from.
 */
export function CopyEvidenceButton({ payload, label = 'Copy evidence' }: { payload: unknown; label?: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      onClick={() => {
        void navigator.clipboard?.writeText(JSON.stringify(payload, null, 2)).then(() => {
          setCopied(true);
          window.setTimeout(() => setCopied(false), 1500);
        });
      }}
      className="inline-flex items-center gap-1 rounded border border-[var(--line)] bg-[var(--surface-raised)] px-1.5 py-0.5 t-micro text-[var(--muted)] transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--fg)]"
    >
      <Copy className="h-3 w-3" aria-hidden />
      {copied ? 'Copied' : label}
    </button>
  );
}

export function ReceiptCard({ receipt }: { receipt: Receipt }) {
  const copy = VERDICT_COPY[receipt.gate.verdict];

  return (
    <div className="overflow-hidden rounded-lg border border-[var(--line)] bg-[var(--surface)]">
      {/* 1 — Verdict. The claim, stated where it cannot be missed. */}
      <div className={`flex flex-wrap items-center gap-x-3 gap-y-1 border-b px-3 py-2.5 ${copy.banner}`}>
        <span className="text-sm font-semibold tracking-wide">{copy.label}</span>
        <span className="t-micro t-mono opacity-90">
          {receipt.gate.required_passed}/{receipt.gate.required_total} required checks passed
        </span>
        <span className="ml-auto">
          <CopyEvidenceButton payload={receipt} />
        </span>
      </div>

      <p className="t-micro border-b border-[var(--line-faint)] px-3 py-2 text-[var(--muted)]">
        {copy.detail}
      </p>

      {/* 2 — Checks. Each row is one executed spec; no aggregation hides a failure. */}
      <ul>
        {receipt.executions.length === 0 ? (
          <li className="t-meta px-3 py-2.5 text-[var(--muted)]">No checks were executed.</li>
        ) : (
          receipt.executions.map((execution) => (
            <li
              key={execution.spec_id}
              className="border-b border-[var(--line-faint)] px-3 py-2 last:border-b-0"
            >
              <div className="flex flex-wrap items-center gap-2">
                <OutcomeGlyph outcome={execution.outcome} />
                <span className="t-meta t-mono text-[var(--fg)]">{execution.spec_id}</span>
                <span className="t-micro text-[var(--muted)]">
                  {outcomeLabel(execution)} · {formatDuration(execution.duration_ms)}
                </span>
                {execution.output_digest && (
                  <span
                    className="t-micro t-mono ml-auto text-[var(--muted)] opacity-70"
                    title={`sha of full output: ${execution.output_digest}`}
                  >
                    #{execution.output_digest.slice(0, 8)}
                  </span>
                )}
              </div>
              {/* 3 — Tail. What a dispute reads first, verbatim. */}
              {execution.output_tail && (
                <pre className="t-mono mt-1.5 max-h-44 overflow-auto rounded bg-[var(--inset)] px-2 py-1.5 text-[11px] leading-relaxed whitespace-pre-wrap text-[var(--muted-strong)]">
                  {execution.output_tail}
                </pre>
              )}
            </li>
          ))
        )}
      </ul>

      {/* 4 — Provenance. "It passed" is only reproducible if where it passed
          and what it passed against are both pinned. */}
      <div className="t-micro t-mono border-t border-[var(--line)] px-3 py-2 leading-relaxed text-[var(--muted)]">
        <p title={receipt.tree_hash}>tree {receipt.tree_hash.slice(0, 12)}</p>
        {receipt.executions[0] && <p title={receipt.executions[0].runner_image}>runner {receipt.executions[0].runner_image}</p>}
        <p>
          attempt {receipt.attempt} · verification {receipt.verification_id.slice(0, 12)}
        </p>
        {/* What the sandbox could reach. Rendered only when the job recorded
            it: a step from before scoped egress has no such record, and
            printing "no network" for it would be a claim without evidence.
            "no network" is stated positively when the record says so, because
            that is the strong case and it should not read as missing data. */}
        {receipt.egress && (
          <p title={receipt.egress.endpoints.join(', ')}>
            network{' '}
            {receipt.egress.endpoints.length === 0
              ? 'none'
              : `${receipt.egress.endpoints.join(', ')}${
                  receipt.egress.granted_registries.length > 0
                    ? ` (${receipt.egress.granted_registries.join(', ')})`
                    : ''
                }`}
          </p>
        )}
      </div>
    </div>
  );
}
