// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import * as cortexApi from '../../lib/cortexApi';
import ChatComposer from './ChatComposer';

/**
 * A fake RTCPeerConnection just real enough for the two hooks: it can create
 * an offer, take a remote description, and fire `connectionstatechange`
 * listeners when a test flips `connectionState` -- no real network anywhere.
 */
let lastPeerConnection: FakePeerConnection | null = null;

class FakeDataChannel {
  readyState = 'open';
  send = vi.fn();
  close = vi.fn();
  private listeners: Record<string, Array<(arg?: unknown) => void>> = {};
  addEventListener(name: string, cb: (arg?: unknown) => void) {
    this.listeners[name] ??= [];
    this.listeners[name].push(cb);
  }
  emitMessage(data: unknown) {
    for (const cb of this.listeners.message ?? []) cb({ data: JSON.stringify(data) });
  }
}

class FakePeerConnection {
  connectionState = 'new';
  iceGatheringState = 'complete';
  private listeners: Record<string, Array<(arg?: unknown) => void>> = {};
  addTrack = vi.fn();
  lastDataChannel: FakeDataChannel | null = null;
  createDataChannel = vi.fn(() => {
    const dc = new FakeDataChannel();
    this.lastDataChannel = dc;
    return dc;
  });
  constructor() {
    lastPeerConnection = this;
  }
  createOffer = vi.fn(async () => ({ type: 'offer', sdp: 'fake-offer-sdp' }));
  setLocalDescription = vi.fn(async () => undefined);
  setRemoteDescription = vi.fn(async () => undefined);
  close = vi.fn();
  addEventListener(name: string, cb: (arg?: unknown) => void) {
    this.listeners[name] ??= [];
    this.listeners[name].push(cb);
  }
  emit(name: string, arg?: unknown) {
    for (const cb of this.listeners[name] ?? []) cb(arg);
  }
}

function installFakePeerConnection() {
  (globalThis as unknown as { RTCPeerConnection: new () => FakePeerConnection }).RTCPeerConnection =
    FakePeerConnection;
}

function installFakeMediaDevices(overrides: { getUserMedia?: () => Promise<MediaStream> } = {}) {
  const trackListeners: Record<string, Array<() => void>> = {};
  const fakeTrack = {
    stop: vi.fn(),
    addEventListener: (name: string, cb: () => void) => {
      trackListeners[name] ??= [];
      trackListeners[name].push(cb);
    },
    emit: (name: string) => {
      for (const cb of trackListeners[name] ?? []) cb();
    },
  };
  const fakeStream = { getTracks: () => [fakeTrack] } as unknown as MediaStream;
  const getUserMedia = overrides.getUserMedia ?? vi.fn(async () => fakeStream);
  Object.defineProperty(navigator, 'mediaDevices', {
    value: { getUserMedia },
    configurable: true,
  });
  return { getUserMedia, fakeTrack };
}

function jsonResponse(body: unknown, status = 200) {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: new Headers(),
    json: async () => body,
    text: async () => JSON.stringify(body),
  } as Response;
}

const noop = () => {};

describe('ChatComposer voice controls', () => {
  beforeEach(() => {
    installFakePeerConnection();
    if (!('randomUUID' in crypto)) {
      Object.defineProperty(crypto, 'randomUUID', { value: () => 'fake-uuid', configurable: true });
    }
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('renders the mic button then the live-voice toggle, at the right of the box', () => {
    installFakeMediaDevices();
    render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    const buttons = screen.getAllByRole('button');
    const micIndex = buttons.findIndex((b) => b.getAttribute('aria-label') === 'Start dictation');
    const liveIndex = buttons.findIndex((b) => b.getAttribute('aria-label') === 'Start live voice');
    const sendIndex = buttons.findIndex((b) => b.getAttribute('aria-label') === 'Send message');
    expect(micIndex).toBeGreaterThanOrEqual(0);
    expect(liveIndex).toBe(micIndex + 1);
    expect(sendIndex).toBeGreaterThan(liveIndex);
  });

  it('does not request a token when microphone permission is denied', async () => {
    installFakeMediaDevices({
      getUserMedia: vi.fn(async () => {
        const err = new Error('denied');
        err.name = 'NotAllowedError';
        throw err;
      }),
    });
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);

    await act(async () => {
      fireEvent.click(screen.getByLabelText('Start dictation'));
    });

    expect(await screen.findByRole('alert')).toHaveTextContent(/microphone access was denied/i);
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it('starting live voice twice fast sends exactly one POST, and toggling off sends exactly one DELETE', async () => {
    installFakeMediaDevices();
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        return jsonResponse({ session_id: 'sess-1', sdp: 'fake-answer-sdp' });
      }
      if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
        return jsonResponse({});
      }
      return jsonResponse({}, 404);
    });

    render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    const button = screen.getByLabelText('Start live voice');

    await act(async () => {
      fireEvent.click(button);
      fireEvent.click(button);
      await Promise.resolve();
      await Promise.resolve();
    });

    const postCalls = fetchSpy.mock.calls.filter(
      ([url, init]) => String(url).includes('/api/voice/live/sessions') && (init?.method ?? 'GET') === 'POST',
    );
    expect(postCalls.length).toBe(1);

    await waitFor(() => expect(screen.getByLabelText('End live voice')).toBeInTheDocument());

    await act(async () => {
      fireEvent.click(screen.getByLabelText('End live voice'));
    });

    const deleteCalls = fetchSpy.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'DELETE');
    expect(deleteCalls.length).toBe(1);
  });

  it('sends exactly one DELETE on unmount while a live voice session is open', async () => {
    installFakeMediaDevices();
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        return jsonResponse({ session_id: 'sess-2', sdp: 'fake-answer-sdp' });
      }
      if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
        return jsonResponse({});
      }
      return jsonResponse({}, 404);
    });

    const { unmount } = render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);

    await act(async () => {
      fireEvent.click(screen.getByLabelText('Start live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('End live voice')).toBeInTheDocument());

    await act(async () => {
      unmount();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    const deleteCalls = fetchSpy.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'DELETE');
    expect(deleteCalls.length).toBe(1);
  });

  it('shows the server message on a 409 (already open)', async () => {
    installFakeMediaDevices();
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        return jsonResponse(
          { error: 'You already have a live voice session open. End it first.' },
          409,
        );
      }
      return jsonResponse({}, 404);
    });

    render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Start live voice'));
    });

    expect(await screen.findByRole('alert')).toHaveTextContent(/already have a live voice session open/i);
  });

  it('unmounting while getUserMedia is pending never leaves a session open', async () => {
    let resolveMedia: (stream: MediaStream) => void = () => {};
    const mediaPromise = new Promise<MediaStream>((resolve) => {
      resolveMedia = resolve;
    });
    const fakeTrack = { stop: vi.fn() };
    const fakeStream = { getTracks: () => [fakeTrack] } as unknown as MediaStream;
    Object.defineProperty(navigator, 'mediaDevices', {
      value: { getUserMedia: vi.fn(() => mediaPromise) },
      configurable: true,
    });
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        return jsonResponse({ session_id: 'sess-3', sdp: 'fake-answer-sdp' });
      }
      if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
        return jsonResponse({});
      }
      return jsonResponse({}, 404);
    });

    const { unmount } = render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    fireEvent.click(screen.getByLabelText('Start live voice'));

    await act(async () => {
      unmount();
    });
    await act(async () => {
      resolveMedia(fakeStream);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    const postCalls = fetchSpy.mock.calls.filter(
      ([url, init]) => String(url).includes('/api/voice/live/sessions') && (init?.method ?? 'GET') === 'POST',
    );
    const deleteCalls = fetchSpy.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'DELETE');
    expect(postCalls.length).toBe(0);
    expect(deleteCalls.length).toBeLessThanOrEqual(1);
    expect(fakeTrack.stop).toHaveBeenCalled();
  });

  it('unmounting while the session POST is pending sends exactly one DELETE and tears down', async () => {
    installFakeMediaDevices();
    let resolvePost: (value: Response) => void = () => {};
    const postPromise = new Promise<Response>((resolve) => {
      resolvePost = resolve;
    });
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        return postPromise;
      }
      if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
        return jsonResponse({});
      }
      return jsonResponse({}, 404);
    });

    const { unmount } = render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    fireEvent.click(screen.getByLabelText('Start live voice'));
    await act(async () => {
      await Promise.resolve();
    });

    await act(async () => {
      unmount();
    });
    await act(async () => {
      resolvePost(jsonResponse({ session_id: 'sess-4', sdp: 'fake-answer-sdp' }));
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    const deleteCalls = fetchSpy.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'DELETE');
    expect(deleteCalls.length).toBe(1);
  });

  it('a post-POST failure (setRemoteDescription rejecting) still sends one DELETE and unblocks the next start', async () => {
    installFakeMediaDevices();
    let call = 0;
    (
      globalThis as unknown as {
        RTCPeerConnection: new () => FakePeerConnection;
      }
    ).RTCPeerConnection = class extends FakePeerConnection {
      setRemoteDescription = vi.fn(async () => {
        call += 1;
        if (call === 1) throw new Error('boom');
      });
    };
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        return jsonResponse({ session_id: `sess-${call}`, sdp: 'fake-answer-sdp' });
      }
      if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
        return jsonResponse({});
      }
      return jsonResponse({}, 404);
    });

    render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Start live voice'));
    });

    expect(await screen.findByRole('alert')).toBeInTheDocument();
    const deleteCalls = fetchSpy.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'DELETE');
    expect(deleteCalls.length).toBe(1);

    // The next start is not blocked by the failed one.
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Start live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('End live voice')).toBeInTheDocument());
  });

  it('pagehide sends session.close on the data channel then one keepalive DELETE', async () => {
    installFakeMediaDevices();
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        return jsonResponse({ session_id: 'sess-5', sdp: 'fake-answer-sdp' });
      }
      if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
        return jsonResponse({});
      }
      return jsonResponse({}, 404);
    });

    render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Start live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('End live voice')).toBeInTheDocument());

    await act(async () => {
      window.dispatchEvent(new Event('pagehide'));
    });

    expect(lastPeerConnection?.lastDataChannel?.send).toHaveBeenCalledWith(
      JSON.stringify({ type: 'session.close' }),
    );
    const deleteCalls = fetchSpy.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'DELETE');
    expect(deleteCalls.length).toBe(1);
  });

  it('refreshes the auth token every ~30s so the pagehide DELETE carries a fresh one', async () => {
    vi.useFakeTimers();
    try {
      installFakeMediaDevices();
      let tokenCount = 0;
      const getAuthTokenSpy = vi
        .spyOn(cortexApi, 'getAuthToken')
        .mockImplementation(async () => `token-${(tokenCount += 1)}`);
      const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
        const url = String(input);
        const method = init?.method ?? 'GET';
        if (url.includes('/api/voice/live/sessions') && method === 'POST') {
          return jsonResponse({ session_id: 'sess-6', sdp: 'fake-answer-sdp' });
        }
        if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
          return jsonResponse({});
        }
        return jsonResponse({}, 404);
      });

      render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
      await act(async () => {
        fireEvent.click(screen.getByLabelText('Start live voice'));
        await vi.runOnlyPendingTimersAsync();
      });
      expect(screen.getByLabelText('End live voice')).toBeInTheDocument();
      const tokenAtStart = await getAuthTokenSpy.mock.results[0].value;

      await act(async () => {
        await vi.advanceTimersByTimeAsync(31000);
      });

      await act(async () => {
        window.dispatchEvent(new Event('pagehide'));
      });

      const deleteCalls = fetchSpy.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'DELETE');
      expect(deleteCalls.length).toBe(1);
      const deleteAuthHeader = new Headers(deleteCalls[0][1]?.headers).get('Authorization');
      expect(deleteAuthHeader).toBe(`Bearer token-${tokenCount}`);
      expect(deleteAuthHeader).not.toBe(`Bearer ${tokenAtStart}`);
    } finally {
      vi.useRealTimers();
    }
  });

  it('plays a remote track and clears it on teardown', async () => {
    installFakeMediaDevices();
    class FakeMediaStream {
      tracks: unknown[];
      constructor(tracks: unknown[] = []) {
        this.tracks = tracks;
      }
    }
    const instances: FakeAudioInstance[] = [];
    class FakeAudioInstance {
      autoplay = false;
      srcObject: unknown = null;
      constructor() {
        instances.push(this);
      }
    }
    (globalThis as unknown as { MediaStream: typeof FakeMediaStream }).MediaStream = FakeMediaStream;
    (globalThis as unknown as { Audio: typeof FakeAudioInstance }).Audio = FakeAudioInstance;

    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        return jsonResponse({ session_id: 'sess-7', sdp: 'fake-answer-sdp' });
      }
      if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
        return jsonResponse({});
      }
      return jsonResponse({}, 404);
    });

    const { unmount } = render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Start live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('End live voice')).toBeInTheDocument());

    const remoteTrack = { id: 'remote-track' };
    act(() => {
      lastPeerConnection?.emit('track', { track: remoteTrack });
    });

    expect(instances.length).toBe(1);
    expect(instances[0].srcObject).toBeInstanceOf(FakeMediaStream);

    await act(async () => {
      unmount();
    });

    expect(instances[0].srcObject).toBeNull();
  });

  it('a server-initiated session.closed (credits ran out) shows a message and sends one DELETE', async () => {
    installFakeMediaDevices();
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        return jsonResponse({ session_id: 'sess-8', sdp: 'fake-answer-sdp' });
      }
      if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
        return jsonResponse({});
      }
      return jsonResponse({}, 404);
    });

    render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Start live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('End live voice')).toBeInTheDocument());

    act(() => {
      lastPeerConnection?.lastDataChannel?.emitMessage({ type: 'session.closed', reason: 'expired' });
    });

    expect(await screen.findByRole('alert')).toHaveTextContent(/credits ran out/i);
    await waitFor(() => expect(screen.getByLabelText('Start live voice')).toBeInTheDocument());
    const deleteCalls = fetchSpy.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'DELETE');
    expect(deleteCalls.length).toBe(1);
  });

  it('a user-requested toggle-off shows no error', async () => {
    installFakeMediaDevices();
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        return jsonResponse({ session_id: 'sess-9', sdp: 'fake-answer-sdp' });
      }
      if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
        return jsonResponse({});
      }
      return jsonResponse({}, 404);
    });

    render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Start live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('End live voice')).toBeInTheDocument());

    await act(async () => {
      fireEvent.click(screen.getByLabelText('End live voice'));
    });

    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('ends the session when the mic track ends on its own', async () => {
    const { fakeTrack } = installFakeMediaDevices();
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        return jsonResponse({ session_id: 'sess-10', sdp: 'fake-answer-sdp' });
      }
      if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
        return jsonResponse({});
      }
      return jsonResponse({}, 404);
    });

    render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Start live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('End live voice')).toBeInTheDocument());

    act(() => {
      (fakeTrack as unknown as { emit: (name: string) => void }).emit('ended');
    });

    await waitFor(() => expect(screen.getByLabelText('Start live voice')).toBeInTheDocument());
    const deleteCalls = fetchSpy.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'DELETE');
    expect(deleteCalls.length).toBe(1);
  });

  it('stays enabled while disabled=true and an active session runs, so it can still be switched off', async () => {
    installFakeMediaDevices();
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        return jsonResponse({ session_id: 'sess-11', sdp: 'fake-answer-sdp' });
      }
      if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
        return jsonResponse({});
      }
      return jsonResponse({}, 404);
    });

    const { rerender } = render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Start live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('End live voice')).toBeInTheDocument());

    rerender(<ChatComposer draft="" disabled onDraftChange={noop} onSend={noop} />);
    const button = screen.getByLabelText('End live voice');
    expect(button).not.toBeDisabled();

    await act(async () => {
      fireEvent.click(button);
    });

    await waitFor(() => expect(screen.getByLabelText('Start live voice')).toBeInTheDocument());
  });

  it('unmounting while the mic prompt is pending never requests a dictation token', async () => {
    let resolveMedia: (stream: MediaStream) => void = () => {};
    const mediaPromise = new Promise<MediaStream>((resolve) => {
      resolveMedia = resolve;
    });
    const fakeTrack = { stop: vi.fn() };
    const fakeStream = { getTracks: () => [fakeTrack] } as unknown as MediaStream;
    Object.defineProperty(navigator, 'mediaDevices', {
      value: { getUserMedia: vi.fn(() => mediaPromise) },
      configurable: true,
    });
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => jsonResponse({}, 404));

    const { unmount } = render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    fireEvent.click(screen.getByLabelText('Start dictation'));

    await act(async () => {
      unmount();
    });
    await act(async () => {
      resolveMedia(fakeStream);
      await Promise.resolve();
      await Promise.resolve();
    });

    const tokenCalls = fetchSpy.mock.calls.filter(([url]) => String(url).includes('/api/voice/dictation/token'));
    expect(tokenCalls.length).toBe(0);
    expect(fakeTrack.stop).toHaveBeenCalled();
  });

  it('shows the server refusal message for dictation (e.g. insufficient credits)', async () => {
    installFakeMediaDevices();
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
      const url = String(input);
      if (url.includes('/api/voice/dictation/token')) {
        return jsonResponse({ error: 'Not enough credits for dictation.' }, 402);
      }
      return jsonResponse({}, 404);
    });

    render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Start dictation'));
    });

    expect(await screen.findByRole('alert')).toHaveTextContent(/not enough credits for dictation/i);
  });
});
