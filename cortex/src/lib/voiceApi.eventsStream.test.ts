// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { openLiveVoiceEventsStream } from './voiceApi';
import { setAuthTokenGetter } from './cortexApi';

/** A `Response`-alike whose body streams the given SSE `data:` lines, then closes. */
function sseResponse(events: unknown[]): Response {
  const body = new ReadableStream({
    start(controller) {
      const encoder = new TextEncoder();
      for (const event of events) {
        controller.enqueue(encoder.encode(`data: ${JSON.stringify(event)}\n\n`));
      }
      controller.close();
    },
  });
  return { ok: true, status: 200, headers: new Headers(), body } as unknown as Response;
}

function notFoundResponse(): Response {
  return { ok: false, status: 404, headers: new Headers(), body: null } as unknown as Response;
}

function unauthorizedResponse(): Response {
  return { ok: false, status: 401, headers: new Headers(), body: null } as unknown as Response;
}

function forbiddenResponse(): Response {
  return { ok: false, status: 403, headers: new Headers(), body: null } as unknown as Response;
}

beforeEach(() => {
  setAuthTokenGetter(async () => null);
  vi.useFakeTimers();
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe('openLiveVoiceEventsStream reconnect', () => {
  it('reconnects with backoff (1s, 2s, 4s, capped at 10s) with no attempt limit', async () => {
    let calls = 0;
    const fetchMock = vi.fn(async () => {
      calls += 1;
      return sseResponse([]);
    });
    vi.stubGlobal('fetch', fetchMock);

    const controller = openLiveVoiceEventsStream('sess-1', () => {});

    await vi.waitFor(() => expect(calls).toBe(1));

    await vi.advanceTimersByTimeAsync(1000);
    await vi.waitFor(() => expect(calls).toBe(2));

    await vi.advanceTimersByTimeAsync(2000);
    await vi.waitFor(() => expect(calls).toBe(3));

    await vi.advanceTimersByTimeAsync(4000);
    await vi.waitFor(() => expect(calls).toBe(4));

    // From here on the backoff caps at 10s and keeps going indefinitely.
    await vi.advanceTimersByTimeAsync(10000);
    await vi.waitFor(() => expect(calls).toBe(5));

    await vi.advanceTimersByTimeAsync(10000);
    await vi.waitFor(() => expect(calls).toBe(6));

    controller.abort();
  });

  it('stops reconnecting once the stream 404s', async () => {
    let calls = 0;
    const fetchMock = vi.fn(async () => {
      calls += 1;
      return notFoundResponse();
    });
    vi.stubGlobal('fetch', fetchMock);

    openLiveVoiceEventsStream('sess-2', () => {});

    await vi.waitFor(() => expect(calls).toBe(1));

    await vi.advanceTimersByTimeAsync(20000);
    expect(calls).toBe(1);
  });

  it('stops reconnecting once a 401 retry with a fresh token also 401s', async () => {
    let calls = 0;
    const fetchMock = vi.fn(async () => {
      calls += 1;
      return unauthorizedResponse();
    });
    vi.stubGlobal('fetch', fetchMock);

    openLiveVoiceEventsStream('sess-401', () => {});

    // One outer attempt makes two requests: the original 401, then the
    // single fresh-token retry, which also 401s -- so the stream stops for
    // good without ever looping past that pair.
    await vi.waitFor(() => expect(calls).toBe(2));

    await vi.advanceTimersByTimeAsync(20000);
    expect(calls).toBe(2);
  });

  it('retries once with a fresh token on 403 and keeps the stream going if that retry succeeds', async () => {
    let calls = 0;
    const events: unknown[] = [];
    const fetchMock = vi.fn(async () => {
      calls += 1;
      if (calls === 1) return forbiddenResponse();
      return sseResponse([{ type: 'voice_message', role: 'assistant', content: 'hi' }]);
    });
    vi.stubGlobal('fetch', fetchMock);

    openLiveVoiceEventsStream('sess-403-retry', (event) => events.push(event));

    // The 403 retry succeeds within the same outer attempt (no backoff
    // delay between the two), delivering an event.
    await vi.waitFor(() => expect(calls).toBe(2));
    await vi.waitFor(() =>
      expect(events).toEqual([{ type: 'voice_message', role: 'assistant', content: 'hi' }]),
    );
  });

  it('asks the token getter for a fresh (non-cached) token on the auth retry', async () => {
    const tokens = ['stale', 'fresh'];
    setAuthTokenGetter(async (opts) => {
      if (opts?.skipCache) return 'fresh';
      return tokens.shift() ?? 'fresh';
    });

    let calls = 0;
    const authHeaders: Array<string | null> = [];
    const fetchMock = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      calls += 1;
      authHeaders.push(new Headers(init?.headers).get('Authorization'));
      if (calls === 1) return forbiddenResponse();
      return sseResponse([]);
    });
    vi.stubGlobal('fetch', fetchMock);

    openLiveVoiceEventsStream('sess-403-fresh-token', () => {});

    await vi.waitFor(() => expect(calls).toBe(2));
    expect(authHeaders).toEqual(['Bearer stale', 'Bearer fresh']);
  });

  it('stops for good, without looping, when a 403 retry also 403s', async () => {
    let calls = 0;
    const fetchMock = vi.fn(async () => {
      calls += 1;
      return forbiddenResponse();
    });
    vi.stubGlobal('fetch', fetchMock);

    openLiveVoiceEventsStream('sess-403-twice', () => {});

    await vi.waitFor(() => expect(calls).toBe(2));

    // No infinite loop: no further requests happen, even after plenty of
    // time for any number of backoff-scheduled reconnects.
    await vi.advanceTimersByTimeAsync(60000);
    expect(calls).toBe(2);
  });

  it('resets the backoff after a connection that delivered an event', async () => {
    let calls = 0;
    const fetchMock = vi.fn(async () => {
      calls += 1;
      // The first two connections fail immediately (no events); the third
      // delivers one event before closing, which should reset the backoff
      // so the *next* reconnect (after this one ends) uses the 1s delay
      // again instead of continuing the 1s/2s/4s escalation.
      if (calls === 3) return sseResponse([{ type: 'voice_message', role: 'assistant', content: 'hi' }]);
      return sseResponse([]);
    });
    vi.stubGlobal('fetch', fetchMock);

    openLiveVoiceEventsStream('sess-reset', () => {});

    await vi.waitFor(() => expect(calls).toBe(1)); // attempt 0 used up
    await vi.advanceTimersByTimeAsync(1000);
    await vi.waitFor(() => expect(calls).toBe(2)); // attempt 1 used up
    await vi.advanceTimersByTimeAsync(2000);
    await vi.waitFor(() => expect(calls).toBe(3)); // attempt 2 used up, delivered an event

    // Backoff should have reset to the front of the schedule (1s), not
    // continued to the 4s step attempt 3 would otherwise use.
    await vi.advanceTimersByTimeAsync(1000);
    await vi.waitFor(() => expect(calls).toBe(4));
  });

  it('stops reconnecting once the caller aborts (voice stopped)', async () => {
    let calls = 0;
    const fetchMock = vi.fn(async () => {
      calls += 1;
      return sseResponse([]);
    });
    vi.stubGlobal('fetch', fetchMock);

    const controller = openLiveVoiceEventsStream('sess-3', () => {});
    await vi.waitFor(() => expect(calls).toBe(1));

    controller.abort();

    await vi.advanceTimersByTimeAsync(20000);
    expect(calls).toBe(1);
  });
});
