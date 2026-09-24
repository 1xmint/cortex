// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { CortexApiError, requestJson, setAuthTokenGetter } from './cortexApi';

// The "session expired" banner is raised by `cortex:unauthorized` and lowered
// by `cortex:authorized`. These tests pin when each one fires.
let events: string[] = [];
const record = (event: Event) => events.push(event.type);

beforeEach(() => {
  events = [];
  window.addEventListener('cortex:unauthorized', record);
  window.addEventListener('cortex:authorized', record);
  setAuthTokenGetter(async () => 'tok');
});

afterEach(() => {
  window.removeEventListener('cortex:unauthorized', record);
  window.removeEventListener('cortex:authorized', record);
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function stubStatuses(...statuses: number[]) {
  const queue = [...statuses];
  vi.stubGlobal('fetch', vi.fn(async () => {
    const status = queue.shift() ?? 200;
    return new Response(JSON.stringify(status === 200 ? { ok: true } : { error: 'no' }), { status });
  }));
}

describe('session banner signals', () => {
  it('lowers the banner once, on the first success after a 401', async () => {
    stubStatuses(401, 200, 200);

    await expect(requestJson('/api/runs')).rejects.toBeInstanceOf(CortexApiError);
    await requestJson('/api/runs');
    await requestJson('/api/runs');

    expect(events).toEqual(['cortex:unauthorized', 'cortex:authorized']);
  });

  it('never fires authorized when no 401 came first', async () => {
    stubStatuses(200, 200);

    await requestJson('/api/runs');
    await requestJson('/api/runs');

    expect(events).toEqual([]);
  });

  it('does not lower the banner on a success that carried no credential', async () => {
    stubStatuses(401, 200);
    await expect(requestJson('/api/runs')).rejects.toBeInstanceOf(CortexApiError);

    setAuthTokenGetter(async () => null);
    await requestJson('/api/health');

    expect(events).toEqual(['cortex:unauthorized']);
  });
});
