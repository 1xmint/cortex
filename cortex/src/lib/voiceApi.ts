import { apiUrl, CortexApiError, getAuthToken, requestJson } from './cortexApi';

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

/**
 * `conversationId` is the open conversation a voice turn's user text and
 * assistant answer get saved to. A conversation the caller does not own
 * comes back as a 404 "No such conversation." -- surfaced the same way as
 * any other `CortexApiError`. Omitted entirely when there is no open
 * conversation to attach the session to.
 */
export async function startLiveVoiceSession(
  sdp: string,
  conversationId?: string | null,
): Promise<LiveSessionStartResponse> {
  return requestJson<LiveSessionStartResponse>('/api/voice/live/sessions', {
    method: 'POST',
    body: JSON.stringify(
      conversationId ? { sdp, conversation_id: conversationId } : { sdp },
    ),
  });
}

/**
 * Ends a live voice session. Safe to call more than once from different call
 * sites in the same lifecycle -- callers are still responsible for making
 * sure it is only *sent* once per session (see `useLiveVoiceToggle`), since
 * every call here is a real request against a paid session.
 */
export async function closeLiveVoiceSession(sessionId: string): Promise<void> {
  const headers: Record<string, string> = {};
  const token = await getAuthToken();
  if (token) headers.Authorization = `Bearer ${token}`;
  const res = await fetch(apiUrl(`/api/voice/live/sessions/${encodeURIComponent(sessionId)}`), {
    method: 'DELETE',
    headers,
  });
  if (!res.ok) {
    let message = `Cortex API ${res.status}`;
    try {
      const body = await res.json();
      if (typeof body?.error === 'string') message = body.error;
    } catch {
      // No JSON body -- fall back to the generic message above.
    }
    throw new CortexApiError(res.status, message, res.headers.get('Retry-After'));
  }
  // A successful DELETE is commonly a 204 with no body; parsing that as
  // JSON throws even though the request succeeded.
  if (res.status === 204) return;
  try {
    await res.json();
  } catch {
    // No body to parse -- still a success.
  }
}

/**
 * `GET /api/voice/live/sessions/{id}/events` (SSE, owner only). Same
 * `type`-tagged event shapes the chat stream's `confirm_required` uses --
 * see `WorkerEvent` in `cortexApi.ts` -- so a voice-proposed risky action
 * renders through the same `ConfirmActionCard` path.
 */
export interface VoiceMessageEvent {
  type: 'voice_message';
  role: 'user' | 'assistant';
  content: string;
}

export interface VoiceConfirmRequiredEvent {
  type: 'confirm_required';
  action_id: string;
  nonce: string;
  summary: string;
  expires_at: string;
}

/** Accepted and typed now; the server does not emit these yet. */
export interface VoiceSpokenWindowEvent {
  type: 'spoken_window';
  action_id: string;
  deadline: string;
}

/** Accepted and typed now; the server does not emit these yet. */
export interface VoiceConfirmResolvedEvent {
  type: 'confirm_resolved';
  action_id: string;
  status: string;
}

export type LiveVoiceSessionEvent =
  | VoiceMessageEvent
  | VoiceConfirmRequiredEvent
  | VoiceSpokenWindowEvent
  | VoiceConfirmResolvedEvent;

/**
 * Opens the live voice session's text-mirror SSE stream and hands each
 * parsed event to `onEvent`. Reconnects once if the stream ends on its own
 * (the server lagging under overload closes it without a terminal event) --
 * a second natural end is left alone rather than looped forever.
 *
 * A 404 means the deployment is in stub/dev mode, where this route does not
 * exist yet: treated as "no events" rather than an error, so the caller
 * shows nothing rather than an error banner.
 *
 * Returns an `AbortController` the caller closes when voice stops; closing
 * it never surfaces as an error.
 */
export function openLiveVoiceEventsStream(
  sessionId: string,
  onEvent: (event: LiveVoiceSessionEvent) => void,
): AbortController {
  const controller = new AbortController();

  const connectOnce = async (): Promise<'ended' | 'not-found'> => {
    const headers: Record<string, string> = {};
    const token = await getAuthToken();
    if (token) headers.Authorization = `Bearer ${token}`;

    const res = await fetch(
      apiUrl(`/api/voice/live/sessions/${encodeURIComponent(sessionId)}/events`),
      { headers, signal: controller.signal },
    );

    if (res.status === 404) return 'not-found';
    if (!res.ok) {
      throw new CortexApiError(res.status, `Cortex API ${res.status}`, res.headers.get('Retry-After'));
    }

    const reader = res.body?.getReader();
    if (!reader) return 'ended';

    const decoder = new TextDecoder();
    let buffer = '';
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;

      buffer += decoder.decode(value, { stream: true });
      const lines = buffer.split('\n');
      buffer = lines.pop() ?? '';

      for (const line of lines) {
        if (!line.startsWith('data: ')) continue;
        const json = line.slice(6).trim();
        if (!json) continue;
        try {
          onEvent(JSON.parse(json) as LiveVoiceSessionEvent);
        } catch {
          // Not JSON, or not an event shape we recognize -- skip the line.
        }
      }
    }
    return 'ended';
  };

  void (async () => {
    try {
      const first = await connectOnce();
      if (first === 'ended' && !controller.signal.aborted) {
        await connectOnce();
      }
    } catch (err) {
      if (controller.signal.aborted) return;
      if (err instanceof Error && err.name === 'AbortError') return;
      // Best-effort: the spoken conversation keeps going even if this text
      // mirror drops, so a broken events stream is not surfaced as an error.
    }
  })();

  return controller;
}
