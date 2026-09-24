// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { cancelRun, CortexApiError, setAuthTokenGetter } from './cortexApi';

beforeEach(() => {
  setAuthTokenGetter(async () => null);
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('cancelRun', () => {
  it('POSTs to the cancel endpoint and returns the outcome', async () => {
    const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
      expect(url).toContain('/api/runs/run-1/cancel');
      expect(init?.method).toBe('POST');
      expect(JSON.parse(init?.body as string)).toEqual({ reason: 'user requested' });
      return new Response(
        JSON.stringify({
          run_id: 'run-1',
          status: 'cancelled',
          already_terminal: false,
          cancelled_steps: 2,
          signalled_steps: 1,
        }),
        { status: 200 },
      );
    });
    vi.stubGlobal('fetch', fetchMock);

    const result = await cancelRun('run-1', 'user requested');
    expect(result).toEqual({
      run_id: 'run-1',
      status: 'cancelled',
      already_terminal: false,
      cancelled_steps: 2,
      signalled_steps: 1,
    });
  });

  it('sends an empty body when no reason is given', async () => {
    const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
      expect(JSON.parse(init?.body as string)).toEqual({});
      return new Response(
        JSON.stringify({
          run_id: 'run-1',
          status: 'cancelled',
          already_terminal: false,
          cancelled_steps: 0,
          signalled_steps: 0,
        }),
        { status: 200 },
      );
    });
    vi.stubGlobal('fetch', fetchMock);

    await cancelRun('run-1');
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('surfaces a 404 for a run that is missing or not owned by the caller', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => new Response(JSON.stringify({ error: 'not found' }), { status: 404 })),
    );

    await expect(cancelRun('run-1')).rejects.toBeInstanceOf(CortexApiError);
  });
});
