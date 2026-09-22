/**
 * Pure decision logic for "has the spoken confirm prompt finished playing
 * yet". Fed the model's own transcript deltas (`session.output_transcript.delta`
 * off the `oai-events` data channel) and audio-level samples (0..1, from
 * `RTCRtpReceiver`) with their timestamps; has no timers or DOM of its own so
 * it is trivial to drive from a test with made-up timestamps, or from a real
 * 100ms poll loop.
 *
 * The rule: the server-written confirm prompt always ends with the fixed
 * text ". Say yes, or tap Confirm on screen." -- so the transcript is the
 * anchor. Until the accumulated transcript (normalised: lowercased,
 * whitespace-collapsed, punctuation-stripped) contains "tap confirm on
 * screen", audio level is ignored entirely: speech from *before* the prompt
 * (e.g. the model saying "Sure, I'll delete those" while still deciding)
 * followed by a gap must never look like the prompt ending. Once that anchor
 * text has been seen, `silenceHoldMs` of continuous silence on the audio
 * level -- with no requirement that speech be heard first, since the
 * transcript can lag the audio by the time the anchor lands -- means the
 * prompt has finished playing. If the anchor is never reached, or silence
 * never comes after it, the detector gives up `giveUpMs` after the first
 * sample rather than watching forever.
 */
export type SpokenPromptEndDetectorResult = 'listening' | 'ended' | 'gave-up';

export interface SpokenPromptEndDetectorOptions {
  /** Audio level below this counts as silence. Default 0.02. */
  silenceThreshold?: number;
  /** Continuous silence after the anchor text, in ms, that counts as "ended". Default 700. */
  silenceHoldMs?: number;
  /** Give up (no decision) after this many ms from the first sample. Default 30000. */
  giveUpMs?: number;
}

const DEFAULT_SILENCE_THRESHOLD = 0.02;
const DEFAULT_SILENCE_HOLD_MS = 700;
const DEFAULT_GIVE_UP_MS = 30000;

/** The fixed tail every server-written confirm prompt ends with, normalised. */
const ANCHOR_TEXT = 'tap confirm on screen';

function normalize(text: string): string {
  return text
    .toLowerCase()
    .replace(/[.,!?;:'"()]/g, '')
    .replace(/\s+/g, ' ')
    .trim();
}

export class SpokenPromptEndDetector {
  private readonly silenceThreshold: number;
  private readonly silenceHoldMs: number;
  private readonly giveUpMs: number;

  private startedAtMs: number | null = null;
  private transcriptBuffer = '';
  private anchorReached = false;
  private silenceSinceMs: number | null = null;
  private decided: SpokenPromptEndDetectorResult | null = null;

  constructor(options: SpokenPromptEndDetectorOptions = {}) {
    this.silenceThreshold = options.silenceThreshold ?? DEFAULT_SILENCE_THRESHOLD;
    this.silenceHoldMs = options.silenceHoldMs ?? DEFAULT_SILENCE_HOLD_MS;
    this.giveUpMs = options.giveUpMs ?? DEFAULT_GIVE_UP_MS;
  }

  /**
   * Feeds one `session.output_transcript.delta` chunk of the model's own
   * speech transcript. Only deltas received after this detector was created
   * count -- a fresh detector always starts with an empty transcript, so
   * text from a previous prompt never carries over.
   */
  onTranscript(delta: string): void {
    if (this.decided || this.anchorReached) return;
    this.transcriptBuffer += delta;
    // Anchored at the end of the buffer (after normalising away trailing
    // whitespace/punctuation) so a summary that happens to mention the
    // phrase mid-buffer -- before the real, final occurrence -- never fires
    // early.
    if (normalize(this.transcriptBuffer).endsWith(ANCHOR_TEXT)) {
      this.anchorReached = true;
      // Whatever silence run was accumulating before the anchor landed
      // doesn't count toward the hold -- restart the clock from here so the
      // full `silenceHoldMs` is measured after the prompt's fixed tail.
      this.silenceSinceMs = null;
    }
  }

  /** Feeds one audio-level sample at `atMs` and returns the decision so far. */
  sample(level: number, atMs: number): SpokenPromptEndDetectorResult {
    if (this.decided) return this.decided;
    if (this.startedAtMs === null) this.startedAtMs = atMs;

    if (this.anchorReached) {
      const isSilent = level < this.silenceThreshold;
      if (!isSilent) {
        this.silenceSinceMs = null;
      } else {
        if (this.silenceSinceMs === null) this.silenceSinceMs = atMs;
        if (atMs - this.silenceSinceMs >= this.silenceHoldMs) {
          this.decided = 'ended';
          return this.decided;
        }
      }
    }
    // Before the anchor is reached, audio level is ignored entirely -- even
    // silence that has held a long time means nothing until we know the
    // fixed prompt tail has actually been spoken.

    if (atMs - this.startedAtMs >= this.giveUpMs) {
      this.decided = 'gave-up';
      return this.decided;
    }

    return 'listening';
  }
}
