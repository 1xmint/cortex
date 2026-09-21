/**
 * The seam a later PR routes tool control through.
 *
 * Every event the model sends over the live voice WebRTC data channel
 * (`oai-events`) is parsed and handed to this one function before
 * `useLiveVoiceToggle` does anything else with it. Tool control from voice is
 * a separate PR and is not built here -- this function is intentionally a
 * no-op today, so that PR has exactly one place to start from instead of
 * needing to thread a new callback through the hook and the composer.
 */
export function routeLiveVoiceEvent(event: unknown): void {
  void event;
}
