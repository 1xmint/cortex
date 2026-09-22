import { describe, expect, it } from 'vitest';

import { SpokenPromptEndDetector } from './spokenPromptEndDetector';

describe('SpokenPromptEndDetector', () => {
  it('does not fire on pre-prompt speech followed by a long gap when the anchor is never seen', () => {
    const detector = new SpokenPromptEndDetector();
    // "Sure, I'll delete those" -- speech before the real prompt -- then a
    // gap well past the silence hold, with no anchor text ever reported.
    expect(detector.sample(0.5, 0)).toBe('listening');
    expect(detector.sample(0.4, 100)).toBe('listening');
    expect(detector.sample(0, 200)).toBe('listening');
    expect(detector.sample(0, 5000)).toBe('listening');
    expect(detector.sample(0, 10000)).toBe('listening');
  });

  it('fires only after 700ms of silence once the anchor text is seen, even split across deltas', () => {
    const detector = new SpokenPromptEndDetector();
    detector.onTranscript('Deleting your 3 drafts. Say yes, or tap ');
    // Still mid-prompt -- silence here must not fire yet.
    expect(detector.sample(0, 0)).toBe('listening');
    expect(detector.sample(0, 900)).toBe('listening');
    detector.onTranscript('Confirm on screen.');
    expect(detector.sample(0, 1000)).toBe('listening'); // silence clock restarts at the anchor
    expect(detector.sample(0, 1699)).toBe('listening'); // 699ms since anchor
    expect(detector.sample(0, 1700)).toBe('ended'); // 700ms since anchor
  });

  it('waits for silence when the anchor is seen while audio is still loud', () => {
    const detector = new SpokenPromptEndDetector();
    detector.onTranscript('Say yes, or tap Confirm on screen.');
    expect(detector.sample(0.5, 0)).toBe('listening'); // still speaking
    expect(detector.sample(0.5, 500)).toBe('listening');
    expect(detector.sample(0, 600)).toBe('listening'); // silence starts
    expect(detector.sample(0, 1299)).toBe('listening'); // 699ms silent
    expect(detector.sample(0, 1300)).toBe('ended'); // 700ms silent
  });

  it('gives up after 30s if the anchor is never reached', () => {
    const detector = new SpokenPromptEndDetector();
    expect(detector.sample(0, 0)).toBe('listening');
    expect(detector.sample(0, 29999)).toBe('listening');
    expect(detector.sample(0, 30000)).toBe('gave-up');
  });

  it('gives up after 30s from the first sample even if the anchor is reached but silence never comes', () => {
    const detector = new SpokenPromptEndDetector();
    detector.onTranscript('Say yes, or tap Confirm on screen.');
    expect(detector.sample(0.5, 0)).toBe('listening');
    expect(detector.sample(0.5, 29999)).toBe('listening');
    expect(detector.sample(0.5, 30000)).toBe('gave-up');
  });

  it('matches the anchor text case- and punctuation-insensitively', () => {
    const detector = new SpokenPromptEndDetector();
    detector.onTranscript('...SAY YES, OR TAP CONFIRM ON SCREEN!!');
    expect(detector.sample(0, 0)).toBe('listening'); // silence clock only just started
    expect(detector.sample(0, 700)).toBe('ended');
  });

  it('starts with an empty transcript so a fresh detector never inherits a previous anchor', () => {
    const first = new SpokenPromptEndDetector();
    first.onTranscript('Say yes, or tap Confirm on screen.');
    expect(first.sample(0, 0)).toBe('listening');
    expect(first.sample(0, 700)).toBe('ended');

    const second = new SpokenPromptEndDetector();
    // No onTranscript call at all -- a long silence must not fire.
    expect(second.sample(0, 0)).toBe('listening');
    expect(second.sample(0, 5000)).toBe('listening');
  });

  it('is a one-shot decision: once decided, further samples and transcript do not change it', () => {
    const detector = new SpokenPromptEndDetector();
    detector.onTranscript('Say yes, or tap Confirm on screen.');
    detector.sample(0, 0);
    expect(detector.sample(0, 700)).toBe('ended');
    expect(detector.sample(0.9, 800)).toBe('ended');
  });

  it('does not fire when the anchor mention is split across deltas but more text follows in a later delta', () => {
    const detector = new SpokenPromptEndDetector();
    // The anchor phrase lands at the end of the buffer after the first
    // delta -- but a second delta immediately extends the buffer past it,
    // so the anchor must un-arm rather than staying latched from delta 1.
    detector.onTranscript('I said tap Confirm on screen');
    detector.onTranscript(' last time, but let\'s try again.');
    expect(detector.sample(0, 0)).toBe('listening');
    expect(detector.sample(0, 5000)).toBe('listening'); // no anchor at the end, so no decision
  });

  it('re-arms once a later delta lands the anchor at the end of the buffer again', () => {
    const detector = new SpokenPromptEndDetector();
    detector.onTranscript('I said tap Confirm on screen');
    detector.onTranscript(" last time, but let's try again. Say yes, or tap ");
    detector.onTranscript('Confirm on screen.');
    expect(detector.sample(0, 0)).toBe('listening'); // silence clock only just started
    expect(detector.sample(0, 700)).toBe('ended');
  });

  it('matches the anchor text ending in an ellipsis, a curly quote, or a dash', () => {
    for (const suffix of ['…', '”', ' —']) {
      const detector = new SpokenPromptEndDetector();
      detector.onTranscript(`Say yes, or tap Confirm on screen${suffix}`);
      expect(detector.sample(0, 0)).toBe('listening'); // silence clock only just started
      expect(detector.sample(0, 700)).toBe('ended');
    }
  });

  it('does not fire when the anchor phrase appears mid-buffer but the buffer keeps going', () => {
    const detector = new SpokenPromptEndDetector();
    // The model's summary itself references the confirm phrasing, but more
    // text follows it, so the anchor is not at the end of the buffer yet.
    detector.onTranscript(
      "I said tap Confirm on screen last time, but let's try again. Say yes, or tap ",
    );
    expect(detector.sample(0, 0)).toBe('listening');
    expect(detector.sample(0, 5000)).toBe('listening'); // no anchor at the end yet, so no decision
  });

  it('fires once the anchor phrase finally lands at the end of the buffer, even if mentioned earlier', () => {
    const detector = new SpokenPromptEndDetector();
    detector.onTranscript(
      "I said tap Confirm on screen last time, but let's try again. Say yes, or tap Confirm on screen.",
    );
    expect(detector.sample(0, 0)).toBe('listening'); // silence clock only just started
    expect(detector.sample(0, 700)).toBe('ended');
  });

  it('honors custom thresholds', () => {
    const detector = new SpokenPromptEndDetector({
      silenceThreshold: 0.1,
      silenceHoldMs: 200,
      giveUpMs: 1000,
    });
    detector.onTranscript('Say yes, or tap Confirm on screen.');
    expect(detector.sample(0.5, 10)).toBe('listening'); // above threshold: speech
    expect(detector.sample(0.05, 210)).toBe('listening'); // silence starts, 0ms elapsed
    expect(detector.sample(0.05, 409)).toBe('listening'); // 199ms of silence
    expect(detector.sample(0.05, 410)).toBe('ended'); // 200ms of silence
  });
});
