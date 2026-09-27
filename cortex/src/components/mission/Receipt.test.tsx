// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';

import { ReceiptCard, type Receipt } from './Receipt';

afterEach(() => {
  cleanup();
});

function baseReceipt(overrides: Partial<Receipt> = {}): Receipt {
  return {
    verification_id: 'v-1',
    run_id: 'run-1',
    step_id: 'step-1',
    attempt: 1,
    tree_hash: 'abc123def456',
    gate: {
      verdict: 'verified',
      required_total: 2,
      required_passed: 2,
      failed: [],
      not_executed: [],
    },
    executions: [],
    ...overrides,
  };
}

describe('ReceiptCard verdict class', () => {
  it('shows the authored disclosure in plain words, not independently verified', () => {
    render(<ReceiptCard receipt={baseReceipt({ verdict_class: 'authored', charged_credits: 5 })} />);
    expect(screen.getByText('Authored')).toBeInTheDocument();
    expect(
      screen.getByText('Graded by checks this task was allowed to change — the exam was not locked.'),
    ).toBeInTheDocument();
    expect(screen.getByText('5 credits')).toBeInTheDocument();
  });

  it('shows the strong claim distinctly from authored', () => {
    render(<ReceiptCard receipt={baseReceipt({ verdict_class: 'strong', charged_credits: 10 })} />);
    expect(screen.getByText('Strong')).toBeInTheDocument();
    expect(screen.queryByText('Authored')).not.toBeInTheDocument();
    expect(screen.getByText('Graded by checks that existed before this task.')).toBeInTheDocument();
  });

  it('does not claim verification for a strong-declared step that failed', () => {
    // `strong` describes the battery, not the outcome. A failed verdict must
    // not read as though the checks were passed -- the copy has to say the
    // grading happened and the checks were not passed, not something that
    // could be misread as "independently verified".
    render(
      <ReceiptCard
        receipt={baseReceipt({
          verdict_class: 'strong',
          gate: {
            verdict: 'failed',
            required_total: 2,
            required_passed: 1,
            failed: ['c2'],
            not_executed: [],
          },
        })}
      />,
    );
    expect(screen.getByText('Strong')).toBeInTheDocument();
    expect(
      screen.getByText('Graded by checks that existed before this task -- it did not pass them.'),
    ).toBeInTheDocument();
    expect(screen.queryByText(/independently verified/i)).not.toBeInTheDocument();
  });

  it('uses neutral copy for a strong-declared step that never ran (inconclusive)', () => {
    // Inconclusive means the required checks could not be run at all -- there
    // is nothing to have "not passed". The `failed`-only copy above would
    // overstate what happened, so this must read as "not graded", not as a
    // loss.
    render(
      <ReceiptCard
        receipt={baseReceipt({
          verdict_class: 'strong',
          gate: {
            verdict: 'inconclusive',
            required_total: 2,
            required_passed: 0,
            failed: [],
            not_executed: ['c1', 'c2'],
          },
        })}
      />,
    );
    expect(screen.getByText('Strong')).toBeInTheDocument();
    expect(
      screen.getByText('Checks that existed before this task were not run -- no grade was given.'),
    ).toBeInTheDocument();
    expect(screen.queryByText(/it did not pass them/)).not.toBeInTheDocument();
  });

  // `verdict_class` is `Option<VerdictClass>` on the backend
  // (`Receipt.verdict_class`, `skip_serializing_if = "Option::is_none"`) and
  // is skipped from the JSON entirely when `None`. That is what a contract
  // whose work-contract row could not be read serializes as. It is *not*
  // what a pre-PR contract serializes as: `TaskContract::verdict_class` has
  // written the literal string `"authored"` since the root commit, so a
  // pre-PR contract reads back as `Some(Authored)`, not `None` -- the
  // fixture below (no `verdict_class` key at all) stands in only for "the
  // work contract could not be read", not for "predates this field".
  it('renders no class badge or charged-credits line when the contract never declared a verdict class', () => {
    render(<ReceiptCard receipt={baseReceipt()} />);
    expect(screen.queryByText('Strong')).not.toBeInTheDocument();
    expect(screen.queryByText('Authored')).not.toBeInTheDocument();
    expect(screen.queryByText(/credits$/)).not.toBeInTheDocument();
  });
});
