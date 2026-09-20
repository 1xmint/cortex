import { describe, expect, it } from 'vitest';

import { toneForStatus } from './status';

/**
 * The status vocabulary is the one place the UI decides what a backend word
 * *means*. Every pane funnels through it, so a wrong answer here is wrong
 * everywhere at once — and it is wrong quietly, as a colour.
 *
 * These assert the distinctions the module was written to make, not that the
 * switch has the cases it has.
 */
describe('toneForStatus', () => {
  it('does not call delivered or verifying a success', () => {
    // The distinction the whole verification model rests on: work exists and
    // is being graded, and nothing has been checked yet. Showing these green
    // would claim a verdict that has not been reached.
    expect(toneForStatus('delivered')).toBe('busy');
    expect(toneForStatus('verifying')).toBe('busy');
  });

  it('treats an independent pass as ok and an independent fail as err', () => {
    expect(toneForStatus('verified')).toBe('ok');
    expect(toneForStatus('verified_pass')).toBe('ok');
    expect(toneForStatus('verified_fail')).toBe('err');
  });

  it('does not treat inconclusive as either a pass or a failure', () => {
    // Inconclusive means the verifier could not tell. Rendering it as a
    // failure blames the customer for our outage; rendering it as a pass
    // claims a verdict nobody reached.
    expect(toneForStatus('inconclusive')).toBe('warn');
    expect(toneForStatus('not_executed')).toBe('warn');
    expect(toneForStatus('needs_evidence')).toBe('warn');
  });

  it('marks a manual override as a warning rather than a clean pass', () => {
    // Somebody overrode the gate. That is a real outcome and it is not the
    // same as the work having been verified.
    expect(toneForStatus('manual_override')).toBe('warn');
  });

  it('has no tone for `succeeded`, which a step never reaches', () => {
    // Documented in the module: `completed` covers a run, and a step has no
    // `succeeded` state at all. If one ever appears, this falls to `idle`
    // rather than silently reading as a pass.
    expect(toneForStatus('succeeded')).toBe('idle');
  });

  it('is case-insensitive and safe on absent input', () => {
    expect(toneForStatus('VERIFIED')).toBe('ok');
    expect(toneForStatus(null)).toBe('idle');
    expect(toneForStatus(undefined)).toBe('idle');
    expect(toneForStatus('')).toBe('idle');
  });

  it('falls back to idle rather than guessing at an unknown word', () => {
    expect(toneForStatus('something-the-backend-added-later')).toBe('idle');
  });
});
