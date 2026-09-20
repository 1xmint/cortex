import { describe, expect, it } from 'vitest';

import { recommendRoute } from './sovereignty';
import type { ChatSessionControls } from '../types';

/**
 * `recommendRoute` decides, among other things, whether the worker may proceed
 * without asking. That is the part worth pinning: the difference between
 * `manual` and `auto` is the difference between a person approving a
 * credentialed action and a machine taking it.
 *
 * No DOM here. `readPreferenceMemory` reaches for `window.localStorage` inside
 * a `try`, so without a browser it falls back to zeroed bias — which is what
 * these assertions are written against.
 */
const controls = (over: Partial<ChatSessionControls> = {}): ChatSessionControls => ({
  autonomy: 'smart_auto',
  intelligence: 'balanced',
  speed: 'balanced',
  ...over,
});

describe('recommendRoute', () => {
  it('requires a human when the prompt touches a credentialed boundary', () => {
    // The safety-relevant case. `deploy`, `credential`, `billing` and friends
    // push the manual score to the threshold on their own, so even under
    // `smart_auto` the answer is manual.
    for (const risky of [
      'rotate the production credential',
      'fix the billing webhook',
      'deploy to production',
      'patch the auth middleware',
      'adjust the soma ledger',
    ]) {
      expect(recommendRoute(risky, controls(), 'balanced').mode).toBe('manual');
    }
  });

  it('requires a human when autonomy is set to manual, whatever the prompt', () => {
    expect(
      recommendRoute('rename a local variable', controls({ autonomy: 'manual' }), 'balanced').mode,
    ).toBe('manual');
  });

  it('lets the worker proceed on ordinary work under auto', () => {
    const rec = recommendRoute('rename a local variable', controls(), 'balanced');
    expect(rec.mode).toBe('auto');
  });

  it('sends risky work to the deeper lane and cheap work to the fast one', () => {
    expect(recommendRoute('x', controls({ intelligence: 'deep' }), 'quality_first').provider).toBe(
      'Cortex worker',
    );
    expect(recommendRoute('x', controls({ speed: 'rapid' }), 'cost_saver').provider).toBe(
      'Cortex fast lane',
    );
  });

  it('never reports certainty it does not have', () => {
    // Confidence is capped. A recommendation that claims 1.0 invites a reader
    // to stop checking it.
    const highest = recommendRoute(
      'deploy the production credential change',
      controls({ autonomy: 'manual', intelligence: 'deep' }),
      'quality_first',
    );
    expect(highest.confidence).toBeLessThanOrEqual(0.92);
    expect(highest.confidence).toBeGreaterThan(0);
  });

  it('always says why, and offers a way out', () => {
    const rec = recommendRoute('implement the parser', controls(), 'balanced');
    expect(rec.rationale.length).toBeGreaterThan(0);
    expect(rec.alternatives.length).toBeGreaterThan(0);
    expect(rec.tradeoffs.length).toBeGreaterThan(0);
  });
});
