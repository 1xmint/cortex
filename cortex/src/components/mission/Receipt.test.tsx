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
      screen.getByText('Checked by tests the agent wrote — not independently verified.'),
    ).toBeInTheDocument();
    expect(screen.getByText('5 credits')).toBeInTheDocument();
  });

  it('shows the strong claim distinctly from authored', () => {
    render(<ReceiptCard receipt={baseReceipt({ verdict_class: 'strong', charged_credits: 10 })} />);
    expect(screen.getByText('Strong')).toBeInTheDocument();
    expect(screen.queryByText('Authored')).not.toBeInTheDocument();
    expect(
      screen.getByText('Independently verified against the battery you already had.'),
    ).toBeInTheDocument();
  });

  it('renders no class badge or charged-credits line when the receipt predates the field', () => {
    render(<ReceiptCard receipt={baseReceipt()} />);
    expect(screen.queryByText('Strong')).not.toBeInTheDocument();
    expect(screen.queryByText('Authored')).not.toBeInTheDocument();
    expect(screen.queryByText(/credits$/)).not.toBeInTheDocument();
  });
});
