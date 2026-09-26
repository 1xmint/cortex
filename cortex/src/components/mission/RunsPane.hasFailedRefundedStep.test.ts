import { describe, expect, it } from 'vitest';

import { hasFailedRefundedStep } from './RunsPane';
import type { RunStep } from '../../lib/cortexApi';

function makeStep(overrides: Partial<RunStep> = {}): RunStep {
  return {
    id: 's-1',
    status: 'verified',
    ...overrides,
  } as RunStep;
}

describe('hasFailedRefundedStep', () => {
  it('is false when every step verified', () => {
    expect(hasFailedRefundedStep([makeStep(), makeStep({ id: 's-2' })])).toBe(false);
  });

  it('is true when a step status is failed', () => {
    expect(hasFailedRefundedStep([makeStep(), makeStep({ id: 's-2', status: 'failed' })])).toBe(true);
  });

  it('is true when only verification_status reads failed', () => {
    expect(
      hasFailedRefundedStep([
        makeStep({ status: 'verified', verification_status: 'failed' }),
      ]),
    ).toBe(true);
  });

  it('is true when only verifier_verdict reads failed', () => {
    expect(
      hasFailedRefundedStep([makeStep({ status: 'verified', verifier_verdict: 'failed' })]),
    ).toBe(true);
  });

  it('is false for an empty run', () => {
    expect(hasFailedRefundedStep([])).toBe(false);
  });
});
