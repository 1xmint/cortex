import { describe, expect, it } from 'vitest';

import { hasFailedStep, hasFailedVerificationStep, hasPendingVerification } from './runVerdict';
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

  it('is true for a step that failed execution, before verification ever ran', () => {
    // fail_step (crates/api/src/db/mod.rs) lands a step on `failed` straight
    // from `leased`/`running`, with no verification_status/verifier_verdict
    // ever set -- still a failure `hasFailedStep` must catch.
    expect(hasFailedStep([makeStep({ status: 'failed' })])).toBe(true);
  });
});

describe('hasFailedVerificationStep', () => {
  it('is false when every step verified', () => {
    expect(hasFailedVerificationStep([makeStep(), makeStep({ id: 's-2' })])).toBe(false);
  });

  it('is false for a step that failed execution, not verification', () => {
    expect(hasFailedVerificationStep([makeStep({ status: 'failed' })])).toBe(false);
  });

  it('is true when verification_status reads failed', () => {
    expect(
      hasFailedVerificationStep([makeStep({ status: 'failed', verification_status: 'failed' })]),
    ).toBe(true);
  });

  it('is true when verifier_verdict reads failed', () => {
    expect(
      hasFailedVerificationStep([makeStep({ status: 'failed', verifier_verdict: 'failed' })]),
    ).toBe(true);
  });

  it('is false for an empty run', () => {
    expect(hasFailedVerificationStep([])).toBe(false);
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

  it('is true when a step is delivered but not yet handed to the verifier', () => {
    expect(
      hasPendingVerification([makeStep(), makeStep({ id: 's-2', status: 'delivered' })]),
    ).toBe(true);
  });

  it('is false for a recovered step -- its retry is what is verifying now, not it', () => {
    // F1 regression: `mark_step_recovered` (crates/api/src/db/mod.rs) flips
    // the original of a heal to `recovered` once its retry exists. The
    // original keeps its frozen check specs forever but is not "in flight"
    // any more, so it must not hide the "Open pull request" button or read
    // as pending.
    expect(
      hasPendingVerification([makeStep(), makeStep({ id: 's-2', status: 'recovered' })]),
    ).toBe(false);
  });
});
