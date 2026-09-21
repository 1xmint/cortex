import { requestJson } from './cortexApi';

/**
 * The frontend half of `POST /api/voice/dictation/token`
 * (`crates/api/src/voice.rs`). The route charges up front and mints a
 * short-lived OpenAI ephemeral token; the browser then talks to OpenAI
 * directly with it. Nothing here ever sees a real OpenAI key.
 */
export interface DictationTokenResponse {
  token: string;
  expires_at: number;
  seconds: number;
}

/**
 * Mints a dictation token. `idempotencyKey` should be stable for a single
 * user gesture (one mic press) so a client retry of this exact request never
 * charges twice -- the server keys its charge on the `Idempotency-Key`
 * header.
 */
export async function requestDictationToken(idempotencyKey: string): Promise<DictationTokenResponse> {
  return requestJson<DictationTokenResponse>('/api/voice/dictation/token', {
    method: 'POST',
    headers: { 'Idempotency-Key': idempotencyKey },
  });
}

/**
 * The frontend half of `POST /api/voice/live/sessions` /
 * `DELETE /api/voice/live/sessions/{id}` (`crates/api/src/voice_session.rs`).
 * Cortex brokers the WebRTC handshake with OpenAI and bills the session in
 * segments server-side; the browser only ever exchanges SDP and a session id
 * with Cortex, never with OpenAI directly, and never sees a key.
 */
export interface LiveSessionStartResponse {
  session_id: string;
  sdp: string;
}

export async function startLiveVoiceSession(sdp: string): Promise<LiveSessionStartResponse> {
  return requestJson<LiveSessionStartResponse>('/api/voice/live/sessions', {
    method: 'POST',
    body: JSON.stringify({ sdp }),
  });
}

/**
 * Ends a live voice session. Safe to call more than once from different call
 * sites in the same lifecycle -- callers are still responsible for making
 * sure it is only *sent* once per session (see `useLiveVoiceToggle`), since
 * every call here is a real request against a paid session.
 */
export async function closeLiveVoiceSession(sessionId: string): Promise<void> {
  await requestJson<unknown>(`/api/voice/live/sessions/${encodeURIComponent(sessionId)}`, {
    method: 'DELETE',
  });
}
