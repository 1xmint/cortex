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

  it('is false for a step that failed execution, before delivery -- fail_attempt set latest_attempt.status to failed', () => {
    expect(
      hasFailedVerificationStep([
        makeStep({ status: 'failed', latest_attempt: { status: 'failed' } }),
      ]),
    ).toBe(false);
  });

  it('is true for a step that failed after delivery -- deliver_attempt set latest_attempt.status to delivered and the verdict never changes it', () => {
    // RunsPane uses this to show "delivered as a draft pull request marked
    // failed checks" copy -- the backend still opens the PR (as a draft) for
    // this case, it does not withhold it. `hasFailedStep` without this
    // distinction (an execution failure) still gets the neutral
    // "not delivered" copy, since execution failures deliver nothing.
    expect(
      hasFailedVerificationStep([
        makeStep({ status: 'failed', latest_attempt: { status: 'delivered' } }),
      ]),
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
