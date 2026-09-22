/**
 * Pure decision logic for "has the spoken confirm prompt finished playing
 * yet". Fed audio-level samples (0..1, from `RTCRtpReceiver`) with their
 * timestamps; has no timers or DOM of its own so it is trivial to drive from
 * a test with made-up timestamps, or from a real 100ms poll loop.
 *
 * The rule: once speech has been heard at all after the prompt started,
 * `silenceHoldMs` of continuous silence afterward means the prompt ended.
 * Silence before any speech is heard doesn't count -- the poll can start a
 * beat before playback does. If neither happens within `giveUpMs`, the
 * detector gives up rather than watching forever.
 */
export type SpokenPromptEndDetectorResult = 'listening' | 'ended' | 'timed-out';

export interface SpokenPromptEndDetectorOptions {
  /** Audio level below this counts as silence. Default 0.02. */
  silenceThreshold?: number;
  /** Continuous silence after speech, in ms, that counts as "ended". Default 700. */
  silenceHoldMs?: number;
  /** Give up (no decision) after this many ms from the first sample. Default 30000. */
  giveUpMs?: number;
}

const DEFAULT_SILENCE_THRESHOLD = 0.02;
const DEFAULT_SILENCE_HOLD_MS = 700;
const DEFAULT_GIVE_UP_MS = 30000;

export class SpokenPromptEndDetector {
  private readonly silenceThreshold: number;
  private readonly silenceHoldMs: number;
  private readonly giveUpMs: number;

  private startedAtMs: number | null = null;
  private heardSpeech = false;
  private silenceSinceMs: number | null = null;
  private decided: SpokenPromptEndDetectorResult | null = null;

  constructor(options: SpokenPromptEndDetectorOptions = {}) {
    this.silenceThreshold = options.silenceThreshold ?? DEFAULT_SILENCE_THRESHOLD;
    this.silenceHoldMs = options.silenceHoldMs ?? DEFAULT_SILENCE_HOLD_MS;
    this.giveUpMs = options.giveUpMs ?? DEFAULT_GIVE_UP_MS;
  }

  /** Feeds one audio-level sample at `atMs` and returns the decision so far. */
  sample(level: number, atMs: number): SpokenPromptEndDetectorResult {
    if (this.decided) return this.decided;
    if (this.startedAtMs === null) this.startedAtMs = atMs;

    const isSilent = level < this.silenceThreshold;
    if (!isSilent) {
      this.heardSpeech = true;
      this.silenceSinceMs = null;
    } else if (this.heardSpeech) {
      if (this.silenceSinceMs === null) this.silenceSinceMs = atMs;
      if (atMs - this.silenceSinceMs >= this.silenceHoldMs) {
        this.decided = 'ended';
        return this.decided;
      }
    }

    if (atMs - this.startedAtMs >= this.giveUpMs) {
      this.decided = 'timed-out';
      return this.decided;
    }

    return 'listening';
  }
}
