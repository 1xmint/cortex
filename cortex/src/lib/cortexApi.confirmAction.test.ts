// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  cancelAgentAction,
  confirmAgentAction,
  CortexApiError,
  streamChat,
  setAuthTokenGetter,
  type WorkerEvent,
} from './cortexApi';

function sseResponse(lines: string[]): Response {
  const body = lines.map((line) => `data: ${line}\n\n`).join('');
  return new Response(body, { status: 200, headers: { 'Content-Type': 'text/event-stream' } });
}

beforeEach(() => {
  setAuthTokenGetter(async () => null);
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('confirmAgentAction / cancelAgentAction', () => {
  it('POSTs the nonce to the confirm endpoint', async () => {
    const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
      expect(url).toContain('/api/agent/actions/action-1/confirm');
      expect(init?.method).toBe('POST');
      expect(JSON.parse(init?.body as string)).toEqual({ nonce: 'nonce-1' });
      return new Response(JSON.stringify({ status: 'confirmed', result: null }), { status: 200 });
    });
    vi.stubGlobal('fetch', fetchMock);

    const result = await confirmAgentAction('action-1', 'nonce-1');
    expect(result.status).toBe('confirmed');
  });

  it('POSTs the nonce to the cancel endpoint', async () => {
    const fetchMock = vi.fn(async (url: string) => {
      expect(url).toContain('/api/agent/actions/action-1/cancel');
      return new Response(JSON.stringify({ status: 'cancelled' }), { status: 200 });
    });
    vi.stubGlobal('fetch', fetchMock);

    const result = await cancelAgentAction('action-1', 'nonce-1');
    expect(result.status).toBe('cancelled');
  });

  it('surfaces a 409 as a CortexApiError', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => new Response(JSON.stringify({ error: 'already used' }), { status: 409 })),
    );

    await expect(confirmAgentAction('action-1', 'nonce-1')).rejects.toBeInstanceOf(CortexApiError);
  });
});

describe('streamChat event parsing', () => {
  it('maps a confirm_required SSE line into a WorkerEvent', async () => {
    const event = {
      type: 'confirm_required',
      action_id: 'action-1',
      nonce: 'nonce-1',
      summary: 'Delete 3 stale branches',
      expires_at: '2026-09-22T12:00:00Z',
    };
    vi.stubGlobal('fetch', vi.fn(async () => sseResponse([JSON.stringify(event)])));

    const received: WorkerEvent[] = [];
    await new Promise<void>((resolve) => {
      streamChat(
        'do it',
        [],
        null,
        (e) => received.push(e),
        () => resolve(),
        () => resolve(),
      );
    });

    expect(received).toHaveLength(1);
    expect(received[0]).toMatchObject({
      type: 'confirm_required',
      action_id: 'action-1',
      nonce: 'nonce-1',
      summary: 'Delete 3 stale branches',
      expires_at: '2026-09-22T12:00:00Z',
    });
  });
});
