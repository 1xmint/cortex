import { describe, expect, it } from 'vitest';

import { hasFailedStep, hasPendingVerification } from './runVerdict';
import type { RunStep } from '../../lib/cortexApi';

function makeStep(overrides: Partial<RunStep> = {}): RunStep {
  return {
    id: 's-1',
    status: 'verified',
    ...overrides,
  } as RunStep;
}

describe('hasFailedStep', () => {
  it('is false when every step verified', () => {
    expect(hasFailedStep([makeStep(), makeStep({ id: 's-2' })])).toBe(false);
  });

  it('is true when a step status is failed', () => {
    expect(hasFailedStep([makeStep(), makeStep({ id: 's-2', status: 'failed' })])).toBe(true);
  });

  it('is true when only verification_status reads failed', () => {
    expect(
      hasFailedStep([makeStep({ status: 'verified', verification_status: 'failed' })]),
    ).toBe(true);
  });

  it('is true when only verifier_verdict reads failed', () => {
    expect(hasFailedStep([makeStep({ status: 'verified', verifier_verdict: 'failed' })])).toBe(
      true,
    );
  });

  it('is false for an empty run', () => {
    expect(hasFailedStep([])).toBe(false);
  });
});

describe('hasPendingVerification', () => {
  it('is false when every step is verified or failed', () => {
    expect(
      hasPendingVerification([makeStep(), makeStep({ id: 's-2', status: 'failed' })]),
    ).toBe(false);
  });

  it('is true when a step is still verifying', () => {
    expect(
      hasPendingVerification([makeStep(), makeStep({ id: 's-2', status: 'verifying' })]),
    ).toBe(true);
  });
});
