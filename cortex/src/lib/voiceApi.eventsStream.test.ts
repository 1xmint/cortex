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

  it('stops reconnecting once the stream 401s', async () => {
    let calls = 0;
    const fetchMock = vi.fn(async () => {
      calls += 1;
      return unauthorizedResponse();
    });
    vi.stubGlobal('fetch', fetchMock);

    openLiveVoiceEventsStream('sess-401', () => {});

    await vi.waitFor(() => expect(calls).toBe(1));

    await vi.advanceTimersByTimeAsync(20000);
    expect(calls).toBe(1);
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
