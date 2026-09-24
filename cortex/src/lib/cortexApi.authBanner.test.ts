// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// The "session expired" banner is raised by `cortex:unauthorized` and lowered
// by `cortex:authorized`. These tests pin when each one fires. The module is
// re-imported per test so its "banner is up" flag never leaks between tests.
type Api = typeof import('./cortexApi');
let api: Api;
let events: string[] = [];
const record = (event: Event) => events.push(event.type);

beforeEach(async () => {
  vi.resetModules();
  api = await import('./cortexApi');
  events = [];
  window.addEventListener('cortex:unauthorized', record);
  window.addEventListener('cortex:authorized', record);
  api.setAuthTokenGetter(async () => 'tok');
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

    await expect(api.requestJson('/api/runs')).rejects.toBeInstanceOf(api.CortexApiError);
    await api.requestJson('/api/runs');
    await api.requestJson('/api/runs');

    expect(events).toEqual(['cortex:unauthorized', 'cortex:authorized']);
  });

  it('never fires authorized when no 401 came first', async () => {
    stubStatuses(200, 200);

    await api.requestJson('/api/runs');
    await api.requestJson('/api/runs');

    expect(events).toEqual([]);
  });

  it('keeps the banner up through a second 401', async () => {
    stubStatuses(401, 401);

    await expect(api.requestJson('/api/runs')).rejects.toBeInstanceOf(api.CortexApiError);
    await expect(api.requestJson('/api/runs')).rejects.toBeInstanceOf(api.CortexApiError);

    expect(events).toEqual(['cortex:unauthorized', 'cortex:unauthorized']);
  });

  it('does not lower the banner on a success that carried no credential', async () => {
    stubStatuses(401, 200);
    await expect(api.requestJson('/api/runs')).rejects.toBeInstanceOf(api.CortexApiError);

    api.setAuthTokenGetter(async () => null);
    await api.requestJson('/api/runs');

    expect(events).toEqual(['cortex:unauthorized']);
  });

  it('does not lower the banner on a success from a route that ignores the credential', async () => {
    stubStatuses(401, 200, 200);
    await expect(api.requestJson('/api/runs')).rejects.toBeInstanceOf(api.CortexApiError);

    await api.getDeploymentStatus();
    await api.requestJson('/api/health');

    expect(events).toEqual(['cortex:unauthorized']);
  });

  it('lowers a banner raised elsewhere once a real request succeeds', async () => {
    stubStatuses(200);

    api.markSessionExpired();
    await api.requestJson('/api/runs');

    expect(events).toEqual(['cortex:authorized']);
  });
});
