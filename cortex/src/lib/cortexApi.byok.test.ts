// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { getChatModels, saveProviderKey, setAuthTokenGetter, streamChat } from './cortexApi';

function sseResponse(): Response {
  return new Response('data: {"type":"completed"}\n\n', {
    status: 200,
    headers: { 'Content-Type': 'text/event-stream' },
  });
}

beforeEach(() => {
  setAuthTokenGetter(async () => null);
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

// Every mock below records the request `Headers` (and body, where relevant)
// into a variable in the enclosing scope instead of asserting inside the
// `fetch` mock itself. `streamChat` retries on failure, and a thrown
// `expect` failure inside the mock is just another failure to retry -- it
// gets swallowed by the retry wrapper and the test times out with no
// indication of *why*, instead of failing on the actual assertion. Recording
// and asserting after the call resolves gives a real failure message.

describe('X-Cortex-Key-Unlock header', () => {
  it('is attached only on a zen: chat request, when a device key is supplied', async () => {
    let seenHeaders: Headers | undefined;
    const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
      seenHeaders = new Headers(init?.headers);
      return sseResponse();
    });
    vi.stubGlobal('fetch', fetchMock);

    await new Promise<void>((resolve) => {
      streamChat(
        'hi',
        [],
        null,
        () => {},
        resolve,
        () => resolve(),
        'zen:glm-4.6',
        { deviceId: 'device-1', secret: 'super-secret' },
      );
    });

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(seenHeaders?.get('X-Cortex-Key-Unlock')).toBe('device-1.super-secret');
  });

  it('is never attached on a non-zen chat request, even with a device key supplied', async () => {
    let seenHeaders: Headers | undefined;
    const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
      seenHeaders = new Headers(init?.headers);
      return sseResponse();
    });
    vi.stubGlobal('fetch', fetchMock);

    await new Promise<void>((resolve) => {
      streamChat(
        'hi',
        [],
        null,
        () => {},
        resolve,
        () => resolve(),
        undefined,
        { deviceId: 'device-1', secret: 'super-secret' },
      );
    });

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(seenHeaders?.has('X-Cortex-Key-Unlock')).toBe(false);
  });

  it('is never attached on a zen: chat request when no device key is supplied', async () => {
    let seenHeaders: Headers | undefined;
    const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
      seenHeaders = new Headers(init?.headers);
      return sseResponse();
    });
    vi.stubGlobal('fetch', fetchMock);

    await new Promise<void>((resolve) => {
      streamChat('hi', [], null, () => {}, resolve, () => resolve(), 'zen:glm-4.6', null);
    });

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(seenHeaders?.has('X-Cortex-Key-Unlock')).toBe(false);
  });
});

describe('X-Cortex-Key-Device header', () => {
  it('getChatModels sends the device id when provided', async () => {
    let seenHeaders: Headers | undefined;
    const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
      seenHeaders = new Headers(init?.headers);
      return new Response(JSON.stringify({ models: [] }), { status: 200 });
    });
    vi.stubGlobal('fetch', fetchMock);

    await getChatModels('device-1');
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(seenHeaders?.get('X-Cortex-Key-Device')).toBe('device-1');
  });

  it('getChatModels sends no device header when omitted', async () => {
    let seenHeaders: Headers | undefined;
    const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
      seenHeaders = new Headers(init?.headers);
      return new Response(JSON.stringify({ models: [] }), { status: 200 });
    });
    vi.stubGlobal('fetch', fetchMock);

    await getChatModels();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(seenHeaders?.has('X-Cortex-Key-Device')).toBe(false);
  });

  it('saveProviderKey never sends the device header (it sends device_id/unlock in the body instead)', async () => {
    let seenHeaders: Headers | undefined;
    let seenBody: unknown;
    const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
      seenHeaders = new Headers(init?.headers);
      seenBody = JSON.parse(init?.body as string);
      return new Response(null, { status: 204 });
    });
    vi.stubGlobal('fetch', fetchMock);

    await saveProviderKey('zen', 'sk-real-key', 'device-1', 'secret-b64');

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(seenHeaders?.has('X-Cortex-Key-Device')).toBe(false);
    expect(seenHeaders?.has('X-Cortex-Key-Unlock')).toBe(false);
    expect(seenBody).toEqual({ api_key: 'sk-real-key', device_id: 'device-1', unlock: 'secret-b64' });
  });
});
