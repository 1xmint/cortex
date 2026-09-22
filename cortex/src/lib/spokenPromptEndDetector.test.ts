import { describe, expect, it } from 'vitest';

import { SpokenPromptEndDetector } from './spokenPromptEndDetector';

describe('SpokenPromptEndDetector', () => {
  it('stays listening through silence before any speech is heard', () => {
    const detector = new SpokenPromptEndDetector();
    expect(detector.sample(0, 0)).toBe('listening');
    expect(detector.sample(0, 500)).toBe('listening');
    expect(detector.sample(0, 1000)).toBe('listening');
  });

  it('decides "ended" after speech then 700ms of continuous silence', () => {
    const detector = new SpokenPromptEndDetector();
    expect(detector.sample(0.5, 0)).toBe('listening'); // speech
    expect(detector.sample(0.4, 100)).toBe('listening'); // still speech
    expect(detector.sample(0, 200)).toBe('listening'); // silence starts
    expect(detector.sample(0, 899)).toBe('listening'); // 699ms of silence
    expect(detector.sample(0, 900)).toBe('ended'); // 700ms of silence
  });

  it('resets the silence clock if speech resumes before the hold elapses', () => {
    const detector = new SpokenPromptEndDetector();
    detector.sample(0.5, 0);
    expect(detector.sample(0, 100)).toBe('listening');
    expect(detector.sample(0, 600)).toBe('listening'); // 500ms silent, not yet 700
    expect(detector.sample(0.5, 650)).toBe('listening'); // speech resumes
    expect(detector.sample(0, 700)).toBe('listening'); // silence clock restarted at 700
    expect(detector.sample(0, 1399)).toBe('listening');
    expect(detector.sample(0, 1400)).toBe('ended');
  });

  it('gives up after 30s with no decision', () => {
    const detector = new SpokenPromptEndDetector();
    expect(detector.sample(0.5, 0)).toBe('listening');
    // Continuous quiet speech (never silent, never enough to decide "ended"),
    // right up to the give-up horizon.
    expect(detector.sample(0.5, 29999)).toBe('listening');
    expect(detector.sample(0.5, 30000)).toBe('timed-out');
  });

  it('is a one-shot decision: once decided, further samples do not change it', () => {
    const detector = new SpokenPromptEndDetector();
    detector.sample(0.5, 0);
    detector.sample(0, 0);
    expect(detector.sample(0, 700)).toBe('ended');
    expect(detector.sample(0.9, 800)).toBe('ended');
  });

  it('honors custom thresholds', () => {
    const detector = new SpokenPromptEndDetector({
      silenceThreshold: 0.1,
      silenceHoldMs: 200,
      giveUpMs: 1000,
    });
    expect(detector.sample(0.5, 10)).toBe('listening'); // above threshold: speech
    expect(detector.sample(0.05, 210)).toBe('listening'); // silence starts, 0ms elapsed
    expect(detector.sample(0.05, 409)).toBe('listening'); // 199ms of silence
    expect(detector.sample(0.05, 410)).toBe('ended'); // 200ms of silence
  });
});
