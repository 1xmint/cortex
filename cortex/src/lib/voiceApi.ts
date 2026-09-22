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
 * `POST /api/voice/live/sessions/{id}/prompt-ended` (owner only). Tells the
 * server the spoken confirm prompt for `actionId` has finished playing, so
 * it opens the 45s "say yes" window. A 409 means the action id is stale or
 * the window was already opened, and a 404 means the session or action is
 * unknown -- both are routine races (the confirm resolved, or another
 * detector already reported it) rather than failures worth surfacing.
 */
export async function postPromptEnded(sessionId: string, actionId: string): Promise<void> {
  const headers: Record<string, string> = { 'Content-Type': 'application/json' };
  const token = await getAuthToken();
  if (token) headers.Authorization = `Bearer ${token}`;
  const res = await fetch(
    apiUrl(`/api/voice/live/sessions/${encodeURIComponent(sessionId)}/prompt-ended`),
    { method: 'POST', headers, body: JSON.stringify({ action_id: actionId }) },
  );
  if (res.status === 204 || res.status === 409 || res.status === 404) return;
  if (!res.ok) {
    throw new CortexApiError(res.status, `Cortex API ${res.status}`, res.headers.get('Retry-After'));
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
  /** Unix seconds. */
  expires_at: number;
}

/** Accepted and typed now; the server does not emit these yet. */
export interface VoiceSpokenWindowEvent {
  type: 'spoken_window';
  action_id: string;
  /** Unix seconds. */
  deadline: number;
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

/** Reconnect backoff schedule (ms), holding at the last value thereafter. */
const RECONNECT_BACKOFF_MS = [1000, 2000, 4000, 10000];

/** Result of one connection attempt in the reconnect loop below. */
interface ConnectOnceResult {
  /** True when reconnecting should stop for good (404/401/403). */
  terminal: boolean;
  /** True when this connection actually delivered at least one parsed
   *  event -- proof it was a real, working connection rather than one that
   *  opened and immediately failed, so the backoff counter can be reset. */
  delivered: boolean;
}

/**
 * Opens the live voice session's text-mirror SSE stream and hands each
 * parsed event to `onEvent`. Reconnects with backoff (1s, 2s, 4s, capped at
 * 10s) whenever the stream ends on its own (the server lagging under
 * overload closes it without a terminal event) -- there is no limit on the
 * number of reconnect attempts while the caller keeps voice active. The
 * backoff counter resets to the start of the schedule once a connection
 * actually delivers an event, so a stream that reconnects after a long,
 * healthy run doesn't inherit a stale, maxed-out delay from earlier flaky
 * attempts.
 *
 * A 404 means the deployment is in stub/dev mode, where this route does not
 * exist yet, and is treated as terminal: "no events" rather than an error,
 * so the caller shows nothing rather than an error banner, and reconnecting
 * stops rather than hammering a route that will never succeed. A 401/403
 * usually means the same -- the session's auth is no longer good for it --
 * but can also just mean the token went briefly stale, so it gets one
 * retry with a fresh token before being treated the same way.
 *
 * Returns an `AbortController` the caller closes when voice stops; closing
 * it never surfaces as an error and stops any pending reconnect.
 */
export function openLiveVoiceEventsStream(
  sessionId: string,
  onEvent: (event: LiveVoiceSessionEvent) => void,
): AbortController {
  const controller = new AbortController();

  const connectOnce = async (isAuthRetry = false): Promise<ConnectOnceResult> => {
    const headers: Record<string, string> = {};
    // The retry attempt must skip whatever short-lived cache the token
    // getter keeps -- asking again without `skipCache` (e.g. Clerk's
    // `getToken()`) just returns the same still-cached, possibly-stale
    // token, which would make the "retry" indistinguishable from doing
    // nothing.
    const token = await getAuthToken(isAuthRetry ? { skipCache: true } : undefined);
    if (token) headers.Authorization = `Bearer ${token}`;

    const res = await fetch(
      apiUrl(`/api/voice/live/sessions/${encodeURIComponent(sessionId)}/events`),
      { headers, signal: controller.signal },
    );

    if (res.status === 401 || res.status === 403) {
      // A briefly stale token looks identical to a truly bad one -- get a
      // fresh token and retry exactly once before giving up on this stream
      // for good, so a token that rotated moments ago doesn't end the whole
      // call.
      if (!isAuthRetry) return connectOnce(true);
      return { terminal: true, delivered: false };
    }
    if (res.status === 404) {
      return { terminal: true, delivered: false };
    }
    if (!res.ok) {
      throw new CortexApiError(res.status, `Cortex API ${res.status}`, res.headers.get('Retry-After'));
    }

    const reader = res.body?.getReader();
    if (!reader) return { terminal: false, delivered: false };

    const decoder = new TextDecoder();
    let buffer = '';
    let delivered = false;
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
          delivered = true;
        } catch {
          // Not JSON, or not an event shape we recognize -- skip the line.
        }
      }
    }
    return { terminal: false, delivered };
  };

  const sleep = (ms: number) =>
    new Promise<void>((resolve) => {
      const onAbort = () => {
        clearTimeout(timer);
        resolve();
      };
      const timer = setTimeout(() => {
        controller.signal.removeEventListener('abort', onAbort);
        resolve();
      }, ms);
      controller.signal.addEventListener('abort', onAbort, { once: true });
    });

  void (async () => {
    let attempt = 0;
    while (!controller.signal.aborted) {
      try {
        const { terminal, delivered } = await connectOnce();
        if (terminal) return;
        if (controller.signal.aborted) return;
        if (delivered) attempt = 0;
      } catch (err) {
        if (controller.signal.aborted) return;
        if (err instanceof Error && err.name === 'AbortError') return;
        // Best-effort: the spoken conversation keeps going even if this text
        // mirror drops, so a broken events stream is not surfaced as an
        // error -- it just reconnects below.
      }

      const delay = RECONNECT_BACKOFF_MS[Math.min(attempt, RECONNECT_BACKOFF_MS.length - 1)];
      attempt += 1;
      await sleep(delay);
    }
  })();

  return controller;
}
