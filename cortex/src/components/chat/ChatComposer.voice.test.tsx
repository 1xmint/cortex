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
  // Real ICE gathering starts out incomplete; `localDescription` only picks
  // up every candidate once it reaches 'complete'. Starting here the same
  // way catches a hook that reads the offer's `sdp` instead of waiting for
  // `pc.localDescription`.
  iceGatheringState = 'gathering';
  localDescription: { sdp: string } | null = null;
  // A subclass can set this to false before construction finishes gathering
  // on its own timeline instead of the default next-microtask auto-complete.
  autoCompleteIceGathering = true;
  private listeners: Record<string, Array<(arg?: unknown) => void>> = {};
  addTrack = vi.fn();
  lastDataChannel: FakeDataChannel | null = null;
  createDataChannel = vi.fn(() => {
    const dc = new FakeDataChannel();
    this.lastDataChannel = dc;
    return dc;
  });
  constructor() {
    // Test fake exposes the most recently constructed instance so assertions can reach into it.
    // eslint-disable-next-line @typescript-eslint/no-this-alias
    lastPeerConnection = this;
    // Tests that don't care about ICE gathering timing shouldn't have to
    // drive it themselves -- complete it on the next microtask by default.
    // A test that does care (e.g. asserting the gathered SDP) can call
    // `completeIceGathering()` itself before that microtask runs.
    queueMicrotask(() => {
      if (this.autoCompleteIceGathering && this.iceGatheringState !== 'complete') {
        this.completeIceGathering();
      }
    });
  }
  createOffer = vi.fn(async () => ({ type: 'offer', sdp: 'fake-offer-sdp' }));
  setLocalDescription = vi.fn(async () => {
    // Real `localDescription` picks up the SDP as gathered so far the
    // moment `setLocalDescription` resolves -- it does not yet have every
    // candidate. A test asserting the final `gathered-sdp` only appears
    // after `completeIceGathering()` catches a hook that reads this too
    // early instead of waiting on `iceGatheringState`.
    this.localDescription = { sdp: 'partial-sdp' };
  });
  setRemoteDescription = vi.fn(async () => undefined);
  close = vi.fn();
  addEventListener(name: string, cb: (arg?: unknown) => void) {
    this.listeners[name] ??= [];
    this.listeners[name].push(cb);
  }
  removeEventListener(name: string, cb: (arg?: unknown) => void) {
    this.listeners[name] = (this.listeners[name] ?? []).filter((listener) => listener !== cb);
  }
  emit(name: string, arg?: unknown) {
    for (const cb of this.listeners[name] ?? []) cb(arg);
  }
  completeIceGathering() {
    this.iceGatheringState = 'complete';
    if (this.localDescription) this.localDescription = { sdp: 'gathered-sdp' };
    this.emit('icegatheringstatechange');
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
    const micIndex = buttons.findIndex((b) => b.getAttribute('aria-label') === 'Dictation');
    const liveIndex = buttons.findIndex((b) => b.getAttribute('aria-label') === 'Live voice');
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
      fireEvent.click(screen.getByLabelText('Dictation'));
    });

    expect(await screen.findByRole('alert')).toHaveTextContent(/microphone access was denied/i);
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it('waits for ICE gathering to complete and sends the gathered SDP, not the raw offer', async () => {
    installFakeMediaDevices();
    class ManualIceFakePeerConnection extends FakePeerConnection {
      autoCompleteIceGathering = false;
    }
    (
      globalThis as unknown as { RTCPeerConnection: new () => FakePeerConnection }
    ).RTCPeerConnection = ManualIceFakePeerConnection;
    let postBody = '';
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        postBody = String(init?.body ?? '');
        return jsonResponse({ session_id: 'sess-ice', sdp: 'fake-answer-sdp' });
      }
      if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
        return jsonResponse({});
      }
      return jsonResponse({}, 404);
    });

    render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    fireEvent.click(screen.getByLabelText('Live voice'));

    // Let getUserMedia/getAuthToken/createOffer/setLocalDescription settle
    // so the peer connection is sitting at iceGatheringState 'gathering'.
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(lastPeerConnection?.iceGatheringState).toBe('gathering');
    expect(lastPeerConnection?.localDescription?.sdp).toBe('partial-sdp');
    expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'false');
    // Nothing should have been posted yet -- if `waitForIceGatheringComplete`
    // were removed, the POST would already have gone out with `partial-sdp`
    // at this point.
    const postCallsBeforeGathering = fetchSpy.mock.calls.filter(
      ([url, init]) => String(url).includes('/api/voice/live/sessions') && (init?.method ?? 'GET') === 'POST',
    );
    expect(postCallsBeforeGathering.length).toBe(0);

    await act(async () => {
      lastPeerConnection?.completeIceGathering();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true'));
    expect(postBody).toContain('gathered-sdp');
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
    const button = screen.getByLabelText('Live voice');

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

    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true'));

    await act(async () => {
      fireEvent.click(screen.getByLabelText('Live voice'));
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
      fireEvent.click(screen.getByLabelText('Live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true'));

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
      fireEvent.click(screen.getByLabelText('Live voice'));
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
    fireEvent.click(screen.getByLabelText('Live voice'));

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
    fireEvent.click(screen.getByLabelText('Live voice'));
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
      setRemoteDescription = vi.fn(async (): Promise<undefined> => {
        call += 1;
        if (call === 1) throw new Error('boom');
        return undefined;
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
      fireEvent.click(screen.getByLabelText('Live voice'));
    });

    expect(await screen.findByRole('alert')).toBeInTheDocument();
    const deleteCalls = fetchSpy.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'DELETE');
    expect(deleteCalls.length).toBe(1);

    // The next start is not blocked by the failed one.
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true'));
  });

  it('stopping while setRemoteDescription is still pending (and it later rejects) shows no error, sends one DELETE', async () => {
    installFakeMediaDevices();
    let rejectSetRemoteDescription: (err: unknown) => void = () => {};
    const setRemoteDescriptionPromise = new Promise<undefined>((_resolve, reject) => {
      rejectSetRemoteDescription = reject;
    });
    (
      globalThis as unknown as { RTCPeerConnection: new () => FakePeerConnection }
    ).RTCPeerConnection = class extends FakePeerConnection {
      setRemoteDescription = vi.fn(() => setRemoteDescriptionPromise);
    };
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        return jsonResponse({ session_id: 'sess-cancel', sdp: 'fake-answer-sdp' });
      }
      if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
        return jsonResponse({});
      }
      return jsonResponse({}, 404);
    });

    render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    fireEvent.click(screen.getByLabelText('Live voice'));
    // Let the POST resolve so setRemoteDescription is called and pending.
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    // The user cancels before setRemoteDescription settles.
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Live voice'));
    });

    await act(async () => {
      rejectSetRemoteDescription(new Error('boom'));
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    const deleteCalls = fetchSpy.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'DELETE');
    expect(deleteCalls.length).toBe(1);
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
      fireEvent.click(screen.getByLabelText('Live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true'));

    const pc = lastPeerConnection;
    const dc = pc?.lastDataChannel;
    await act(async () => {
      window.dispatchEvent(new Event('pagehide'));
    });

    expect(dc?.send).toHaveBeenCalledWith(JSON.stringify({ type: 'session.close' }));
    const deleteCalls = fetchSpy.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'DELETE');
    expect(deleteCalls.length).toBe(1);
    expect(deleteCalls[0][1]?.keepalive).toBe(true);
  });

  it('does not end the call on beforeunload (a cancelled "Leave site?" prompt should not hang up)', async () => {
    installFakeMediaDevices();
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        return jsonResponse({ session_id: 'sess-5b', sdp: 'fake-answer-sdp' });
      }
      if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
        return jsonResponse({});
      }
      return jsonResponse({}, 404);
    });

    render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true'));

    await act(async () => {
      window.dispatchEvent(new Event('beforeunload'));
    });

    expect(lastPeerConnection?.lastDataChannel?.send).not.toHaveBeenCalled();
    const deleteCalls = fetchSpy.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'DELETE');
    expect(deleteCalls.length).toBe(0);
    expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true');
  });

  it('a bfcache pagehide (persisted) tears down the connection and resets to idle', async () => {
    installFakeMediaDevices();
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        return jsonResponse({ session_id: 'sess-bfcache', sdp: 'fake-answer-sdp' });
      }
      if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
        return jsonResponse({});
      }
      return jsonResponse({}, 404);
    });

    render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true'));

    const pc = lastPeerConnection;
    await act(async () => {
      const pageHideEvent = new Event('pagehide') as PageTransitionEvent;
      Object.defineProperty(pageHideEvent, 'persisted', { value: true });
      window.dispatchEvent(pageHideEvent);
    });

    // A page going into bfcache still sends the same beacon as a normal
    // pagehide, but also tears down the (unrecoverable) connection locally
    // and resets the UI to idle rather than leaving it showing a live
    // session that no longer exists.
    expect(pc?.close).toHaveBeenCalled();
    expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'false');
    const deleteCalls = fetchSpy.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'DELETE');
    expect(deleteCalls.length).toBe(1);
    expect(deleteCalls[0][1]?.keepalive).toBe(true);
  });

  it('a bfcache pagehide while getUserMedia is still pending cancels the in-flight start', async () => {
    let resolveMedia: (stream: MediaStream) => void = () => {};
    const mediaPromise = new Promise<MediaStream>((resolve) => {
      resolveMedia = resolve;
    });
    const fakeTrack = { stop: vi.fn(), addEventListener: vi.fn() };
    const fakeStream = { getTracks: () => [fakeTrack] } as unknown as MediaStream;
    Object.defineProperty(navigator, 'mediaDevices', {
      value: { getUserMedia: vi.fn(() => mediaPromise) },
      configurable: true,
    });
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        return jsonResponse({ session_id: 'sess-pending', sdp: 'fake-answer-sdp' });
      }
      if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
        return jsonResponse({});
      }
      return jsonResponse({}, 404);
    });

    render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    fireEvent.click(screen.getByLabelText('Live voice'));

    await act(async () => {
      const pageHideEvent = new Event('pagehide') as PageTransitionEvent;
      Object.defineProperty(pageHideEvent, 'persisted', { value: true });
      window.dispatchEvent(pageHideEvent);
    });

    expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'false');

    await act(async () => {
      resolveMedia(fakeStream);
      for (let i = 0; i < 20; i += 1) {
        await Promise.resolve();
      }
    });

    const postCalls = fetchSpy.mock.calls.filter(
      ([url, init]) => String(url).includes('/api/voice/live/sessions') && (init?.method ?? 'GET') === 'POST',
    );
    const deleteCalls = fetchSpy.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'DELETE');
    // A cancelled start must never leave a session running with no UI
    // pointed at it: either it never got as far as the POST, or it did and
    // the cancellation's DELETE cleaned it up.
    expect(deleteCalls.length).toBe(postCalls.length);
    expect(fakeTrack.stop).toHaveBeenCalled();
    expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'false');
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
        fireEvent.click(screen.getByLabelText('Live voice'));
        await vi.runOnlyPendingTimersAsync();
      });
      expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true');
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
      fireEvent.click(screen.getByLabelText('Live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true'));

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

  it('a server-initiated session.closed (credits ran out, reason close_requested) shows the credits message and sends one DELETE', async () => {
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
      fireEvent.click(screen.getByLabelText('Live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true'));

    // No toggle-off happened first -- this is the server's own credits
    // close, which (per crates/api/src/voice_session.rs's billing loop)
    // arrives as `close_requested`, the same reason a user stop produces.
    act(() => {
      lastPeerConnection?.lastDataChannel?.emitMessage({ type: 'session.closed', reason: 'close_requested' });
    });

    expect(await screen.findByRole('alert')).toHaveTextContent(/credit or session limit/i);
    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'false'));
    const deleteCalls = fetchSpy.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'DELETE');
    expect(deleteCalls.length).toBe(1);
  });

  it('a session.closed with reason content shows the safety-filter message', async () => {
    installFakeMediaDevices();
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        return jsonResponse({ session_id: 'sess-8d', sdp: 'fake-answer-sdp' });
      }
      if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
        return jsonResponse({});
      }
      return jsonResponse({}, 404);
    });

    render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true'));

    act(() => {
      lastPeerConnection?.lastDataChannel?.emitMessage({ type: 'session.closed', reason: 'content' });
    });

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent(/safety filter/i);
  });

  it('a session.closed with reason expired shows the time-limit message, not the credits message', async () => {
    installFakeMediaDevices();
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        return jsonResponse({ session_id: 'sess-8b', sdp: 'fake-answer-sdp' });
      }
      if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
        return jsonResponse({});
      }
      return jsonResponse({}, 404);
    });

    render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true'));

    act(() => {
      lastPeerConnection?.lastDataChannel?.emitMessage({ type: 'session.closed', reason: 'expired' });
    });

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent(/time limit/i);
    expect(alert).not.toHaveTextContent(/credits/i);
  });

  it('a user toggle-off followed by session.closed close_requested shows no message', async () => {
    installFakeMediaDevices();
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        return jsonResponse({ session_id: 'sess-8c', sdp: 'fake-answer-sdp' });
      }
      if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
        return jsonResponse({});
      }
      return jsonResponse({}, 404);
    });

    render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true'));

    const dc = lastPeerConnection?.lastDataChannel;
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Live voice'));
    });

    act(() => {
      dc?.emitMessage({ type: 'session.closed', reason: 'close_requested' });
    });

    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('starting again after a toggle-off clears the close-requested flag, so a later credits close still alerts', async () => {
    installFakeMediaDevices();
    let sessionCounter = 0;
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        sessionCounter += 1;
        return jsonResponse({ session_id: `sess-restart-${sessionCounter}`, sdp: 'fake-answer-sdp' });
      }
      if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
        return jsonResponse({});
      }
      return jsonResponse({}, 404);
    });

    render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);

    // Start, then a user-requested toggle-off -- this sets
    // `closeRequestedRef` to true.
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true'));
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'false'));

    // Start again -- if `start()` did not reset `closeRequestedRef` back to
    // false, a server-initiated close on this new session would be
    // mistaken for the toggle-off that just happened and stay silent.
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true'));

    act(() => {
      lastPeerConnection?.lastDataChannel?.emitMessage({ type: 'session.closed', reason: 'close_requested' });
    });

    expect(await screen.findByRole('alert')).toHaveTextContent(/credit or session limit/i);
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
      fireEvent.click(screen.getByLabelText('Live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true'));

    await act(async () => {
      fireEvent.click(screen.getByLabelText('Live voice'));
    });

    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('shows a generic message and sends one DELETE when the connection fails', async () => {
    installFakeMediaDevices();
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        return jsonResponse({ session_id: 'sess-fail-1', sdp: 'fake-answer-sdp' });
      }
      if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
        return jsonResponse({});
      }
      return jsonResponse({}, 404);
    });

    render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
    await act(async () => {
      fireEvent.click(screen.getByLabelText('Live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true'));

    act(() => {
      if (lastPeerConnection) lastPeerConnection.connectionState = 'failed';
      lastPeerConnection?.emit('connectionstatechange');
    });

    expect(await screen.findByRole('alert')).toHaveTextContent(/live voice ended\./i);
    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'false'));
    const deleteCalls = fetchSpy.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'DELETE');
    expect(deleteCalls.length).toBe(1);
  });

  it('debounces a disconnected state -- a recovery within the window keeps the session alive, only a sustained drop ends it', async () => {
    vi.useFakeTimers();
    try {
      installFakeMediaDevices();
      const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
        const url = String(input);
        const method = init?.method ?? 'GET';
        if (url.includes('/api/voice/live/sessions') && method === 'POST') {
          return jsonResponse({ session_id: 'sess-fail-2', sdp: 'fake-answer-sdp' });
        }
        if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
          return jsonResponse({});
        }
        return jsonResponse({}, 404);
      });

      render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
      await act(async () => {
        fireEvent.click(screen.getByLabelText('Live voice'));
        await vi.runOnlyPendingTimersAsync();
      });
      expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true');

      act(() => {
        if (lastPeerConnection) lastPeerConnection.connectionState = 'disconnected';
        lastPeerConnection?.emit('connectionstatechange');
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(4000);
      });
      expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true');

      act(() => {
        if (lastPeerConnection) lastPeerConnection.connectionState = 'connected';
        lastPeerConnection?.emit('connectionstatechange');
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2000);
      });
      expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true');
      let deleteCalls = fetchSpy.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'DELETE');
      expect(deleteCalls.length).toBe(0);

      act(() => {
        if (lastPeerConnection) lastPeerConnection.connectionState = 'disconnected';
        lastPeerConnection?.emit('connectionstatechange');
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(5000);
      });
      expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'false');
      deleteCalls = fetchSpy.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'DELETE');
      expect(deleteCalls.length).toBe(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it('clears the disconnect timer on recovery -- a stale timer from the first drop must not fire for a later one', async () => {
    vi.useFakeTimers();
    try {
      installFakeMediaDevices();
      const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
        const url = String(input);
        const method = init?.method ?? 'GET';
        if (url.includes('/api/voice/live/sessions') && method === 'POST') {
          return jsonResponse({ session_id: 'sess-fail-3', sdp: 'fake-answer-sdp' });
        }
        if (url.includes('/api/voice/live/sessions/') && method === 'DELETE') {
          return jsonResponse({});
        }
        return jsonResponse({}, 404);
      });

      render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);
      await act(async () => {
        fireEvent.click(screen.getByLabelText('Live voice'));
        await vi.runOnlyPendingTimersAsync();
      });
      expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true');

      // First drop at t=0 arms a 5s timer with deadline t=5000.
      act(() => {
        if (lastPeerConnection) lastPeerConnection.connectionState = 'disconnected';
        lastPeerConnection?.emit('connectionstatechange');
      });
      // Recovers late, just before the original deadline -- if recovery
      // does not clear that timer, it is still pending and due at t=5000.
      await act(async () => {
        await vi.advanceTimersByTimeAsync(4000);
      });
      act(() => {
        if (lastPeerConnection) lastPeerConnection.connectionState = 'connected';
        lastPeerConnection?.emit('connectionstatechange');
      });

      // Drops again within 1s of the recovery (at t=4500), arming its own
      // fresh 5s timer with a deadline of t=9500 -- far past the point
      // this test checks below.
      await act(async () => {
        await vi.advanceTimersByTimeAsync(500);
      });
      act(() => {
        if (lastPeerConnection) lastPeerConnection.connectionState = 'disconnected';
        lastPeerConnection?.emit('connectionstatechange');
      });

      // Advance to t=6000 -- 1s past the ORIGINAL t=5000 deadline, well
      // short of the fresh timer's t=9500 one. A correct implementation is
      // still counting down the fresh timer here, so the session stays
      // active; a stale, uncleared first timer would instead have fired at
      // t=5000 (while still disconnected) and ended it already.
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1500);
      });
      expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true');
      const deleteCalls = fetchSpy.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === 'DELETE');
      expect(deleteCalls.length).toBe(0);
    } finally {
      vi.useRealTimers();
    }
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
      fireEvent.click(screen.getByLabelText('Live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true'));

    act(() => {
      (fakeTrack as unknown as { emit: (name: string) => void }).emit('ended');
    });

    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'false'));
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
      fireEvent.click(screen.getByLabelText('Live voice'));
    });
    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true'));

    rerender(<ChatComposer draft="" disabled onDraftChange={noop} onSend={noop} />);
    const button = screen.getByLabelText('Live voice');
    expect(button).not.toBeDisabled();

    await act(async () => {
      fireEvent.click(button);
    });

    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'false'));
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
    fireEvent.click(screen.getByLabelText('Dictation'));

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

  it('lands two transcript deltas that arrive before a re-render, and adds a space on completed', async () => {
    installFakeMediaDevices();
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
      const url = String(input);
      if (url.includes('/api/voice/dictation/token')) {
        return jsonResponse({ token: 'tok', expires_at: 0, seconds: 60 });
      }
      if (url.includes('api.openai.com')) {
        return {
          ok: true,
          status: 200,
          headers: new Headers(),
          json: async () => ({}),
          text: async () => 'fake-answer-sdp',
        } as Response;
      }
      return jsonResponse({}, 404);
    });

    let draft = '';
    const onDraftChange = vi.fn((value: string) => {
      draft = value;
    });
    function Wrapper() {
      return <ChatComposer draft={draft} onDraftChange={onDraftChange} onSend={noop} />;
    }
    const { rerender } = render(<Wrapper />);

    await act(async () => {
      fireEvent.click(screen.getByLabelText('Dictation'));
    });
    await waitFor(() => expect(screen.getByLabelText('Dictation')).toHaveAttribute('aria-pressed', 'true'));

    const dc = lastPeerConnection?.lastDataChannel;
    act(() => {
      dc?.emitMessage({ type: 'conversation.item.input_audio_transcription.delta', delta: 'hello' });
      dc?.emitMessage({ type: 'conversation.item.input_audio_transcription.delta', delta: ' world' });
    });

    expect(draft).toBe('hello world');

    act(() => {
      dc?.emitMessage({ type: 'conversation.item.input_audio_transcription.completed' });
    });
    expect(draft).toBe('hello world ');

    rerender(<Wrapper />);
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
      fireEvent.click(screen.getByLabelText('Dictation'));
    });

    expect(await screen.findByRole('alert')).toHaveTextContent(/not enough credits for dictation/i);
  });
});
