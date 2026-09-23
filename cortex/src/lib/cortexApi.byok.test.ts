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

describe('X-Cortex-Key-Unlock header', () => {
  it('is attached only on a zen: chat request, when a device key is supplied', async () => {
    const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
      const headers = new Headers(init?.headers);
      expect(headers.get('X-Cortex-Key-Unlock')).toBe('device-1.super-secret');
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
  });

  it('is never attached on a non-zen chat request, even with a device key supplied', async () => {
    const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
      const headers = new Headers(init?.headers);
      expect(headers.has('X-Cortex-Key-Unlock')).toBe(false);
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
  });

  it('is never attached on a zen: chat request when no device key is supplied', async () => {
    const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
      const headers = new Headers(init?.headers);
      expect(headers.has('X-Cortex-Key-Unlock')).toBe(false);
      return sseResponse();
    });
    vi.stubGlobal('fetch', fetchMock);

    await new Promise<void>((resolve) => {
      streamChat('hi', [], null, () => {}, resolve, () => resolve(), 'zen:glm-4.6', null);
    });

    expect(fetchMock).toHaveBeenCalledTimes(1);
  });
});

describe('X-Cortex-Key-Device header', () => {
  it('getChatModels sends the device id when provided', async () => {
    const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
      const headers = new Headers(init?.headers);
      expect(headers.get('X-Cortex-Key-Device')).toBe('device-1');
      return new Response(JSON.stringify({ models: [] }), { status: 200 });
    });
    vi.stubGlobal('fetch', fetchMock);

    await getChatModels('device-1');
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('getChatModels sends no device header when omitted', async () => {
    const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
      const headers = new Headers(init?.headers);
      expect(headers.has('X-Cortex-Key-Device')).toBe(false);
      return new Response(JSON.stringify({ models: [] }), { status: 200 });
    });
    vi.stubGlobal('fetch', fetchMock);

    await getChatModels();
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('saveProviderKey never sends the device header (it sends device_id/unlock in the body instead)', async () => {
    const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
      const headers = new Headers(init?.headers);
      expect(headers.has('X-Cortex-Key-Device')).toBe(false);
      expect(headers.has('X-Cortex-Key-Unlock')).toBe(false);
      const body = JSON.parse(init?.body as string);
      expect(body).toEqual({ api_key: 'sk-real-key', device_id: 'device-1', unlock: 'secret-b64' });
      return new Response(null, { status: 204 });
    });
    vi.stubGlobal('fetch', fetchMock);

    await saveProviderKey('zen', 'sk-real-key', 'device-1', 'secret-b64');
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });
});
