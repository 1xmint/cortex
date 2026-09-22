// @vitest-environment jsdom
import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { useLiveVoiceToggle } from './useLiveVoiceToggle';

type Listener = (arg?: unknown) => void;

/**
 * Just real enough for `useLiveVoiceToggle` to reach `active`: it can create
 * an offer, take a remote description, complete ICE gathering synchronously,
 * and hand back a data channel + `track` listeners the test can drive by
 * hand -- no real network or WebRTC anywhere. Modeled on the fake used by
 * `ChatComposer.voiceEvents.test.tsx`.
 *
 * A plain factory function (not a class) so the constructor `RTCPeerConnection`
 * stands in for can simply `return` this object -- `new Ctor()` uses the
 * returned object when a constructor explicitly returns one, so there is no
 * need to build it via `this`.
 */
function createFakePeerConnection() {
  const listeners: Record<string, Listener[]> = {};
  const dataChannelListeners: Record<string, Listener[]> = {};
  const dataChannel = {
    readyState: 'open',
    send: vi.fn(),
    close: vi.fn(),
    addEventListener(name: string, cb: Listener) {
      dataChannelListeners[name] ??= [];
      dataChannelListeners[name].push(cb);
    },
  };
  return {
    connectionState: 'new',
    iceGatheringState: 'complete',
    localDescription: { sdp: 'gathered-sdp' } as { sdp: string } | null,
    addTrack: vi.fn(),
    createDataChannel: vi.fn(() => dataChannel),
    createOffer: vi.fn(async () => ({ type: 'offer', sdp: 'fake-offer-sdp' })),
    setLocalDescription: vi.fn(async () => undefined),
    setRemoteDescription: vi.fn(async () => undefined),
    close: vi.fn(),
    addEventListener(name: string, cb: Listener) {
      listeners[name] ??= [];
      listeners[name].push(cb);
    },
    removeEventListener() {},
    /** Test helper: fires a fake remote `track` event with a receiver whose
     *  audio level the test controls, so the prompt watcher has something to
     *  poll. */
    emitTrack(receiver: { getSynchronizationSources: () => Array<{ audioLevel: number }> }) {
      for (const cb of listeners.track ?? []) {
        cb({ track: { kind: 'audio' }, receiver } as unknown);
      }
    },
    /** Test helper: delivers one `oai-events` data-channel message, the same
     *  shape the model's real transcript deltas arrive in. */
    emitTranscriptDelta(delta: string) {
      const data = JSON.stringify({ type: 'session.output_transcript.delta', delta });
      for (const cb of dataChannelListeners.message ?? []) {
        cb({ data } as unknown);
      }
    },
  };
}

type FakePeerConnection = ReturnType<typeof createFakePeerConnection>;

let lastPc: FakePeerConnection | null = null;

function installFakePeerConnection() {
  function Tracked() {
    const pc = createFakePeerConnection();
    lastPc = pc;
    return pc;
  }
  (globalThis as unknown as { RTCPeerConnection: new () => FakePeerConnection }).RTCPeerConnection =
    Tracked as unknown as new () => FakePeerConnection;
}

function installFakeMediaDevices() {
  const fakeTrack = { stop: vi.fn(), addEventListener: vi.fn() };
  const fakeStream = { getTracks: () => [fakeTrack] } as unknown as MediaStream;
  Object.defineProperty(navigator, 'mediaDevices', {
    value: { getUserMedia: vi.fn(async () => fakeStream) },
    configurable: true,
  });
}

/** jsdom has no `MediaStream`; the hook only uses it to hang a remote track
 *  off an `<audio>` element that is never attached to the DOM, so a stub
 *  that just remembers its tracks is enough. */
function installFakeMediaStreamGlobal() {
  (globalThis as unknown as { MediaStream: new (tracks?: unknown[]) => unknown }).MediaStream =
    class {
      tracks: unknown[];
      constructor(tracks: unknown[] = []) {
        this.tracks = tracks;
      }
    };
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

/** A `Response`-alike whose body streams the given SSE `data:` lines, then
 *  stays open (never closes) -- like the real events stream while a call is
 *  live -- so a `confirm_resolved` can be pushed later in the same test. */
function sseResponse(controllerOut: { push: (event: unknown) => void }) {
  const body = new ReadableStream({
    start(controller) {
      const encoder = new TextEncoder();
      controllerOut.push = (event: unknown) => {
        controller.enqueue(encoder.encode(`data: ${JSON.stringify(event)}\n\n`));
      };
    },
  });
  return { ok: true, status: 200, headers: new Headers(), body } as unknown as Response;
}

describe('useLiveVoiceToggle spoken prompt-end wiring', () => {
  beforeEach(() => {
    installFakePeerConnection();
    installFakeMediaDevices();
    installFakeMediaStreamGlobal();
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
    lastPc = null;
  });

  it('posts prompt-ended exactly once from transcript deltas, even if more deltas arrive after', async () => {
    let promptEndedCalls = 0;
    const push: { push: (event: unknown) => void } = { push: () => {} };
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST' && !url.includes('prompt-ended')) {
        return jsonResponse({ session_id: 'sess-1', sdp: 'fake-answer-sdp' });
      }
      if (url.endsWith('/prompt-ended') && method === 'POST') {
        promptEndedCalls += 1;
        return jsonResponse({}, 204);
      }
      if (url.includes('/events')) {
        return sseResponse(push);
      }
      if (method === 'DELETE') return jsonResponse({}, 204);
      return jsonResponse({}, 404);
    });

    const { result } = renderHook(() => useLiveVoiceToggle());

    await act(async () => {
      result.current.toggle();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    await waitFor(() => expect(result.current.status).toBe('active'));
    vi.useFakeTimers();

    // Silent receiver from the start -- audio level never matters until the
    // anchor text has been seen.
    lastPc!.emitTrack({ getSynchronizationSources: () => [{ audioLevel: 0 }] });

    // The events stream reports the risky action, which starts the watcher.
    push.push({
      type: 'confirm_required',
      action_id: 'action-1',
      nonce: 'nonce-1',
      summary: 'Delete 3 drafts',
      expires_at: Math.floor(Date.now() / 1000) + 60,
    });
    await act(async () => {
      await Promise.resolve();
    });

    // Transcript deltas, split across several data-channel messages, ending
    // with the fixed prompt tail.
    act(() => {
      lastPc!.emitTranscriptDelta("Deleting your 3 drafts. Say yes, or tap ");
    });
    act(() => {
      lastPc!.emitTranscriptDelta('Confirm on screen.');
    });

    // 700ms of silence after the anchor -- the watcher polls every 100ms.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(800);
    });

    expect(promptEndedCalls).toBe(1);

    // More deltas arriving after the decision must not cause a second POST
    // -- the watcher (and its detector) is already torn down.
    act(() => {
      lastPc!.emitTranscriptDelta('Say yes, or tap Confirm on screen.');
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });

    expect(promptEndedCalls).toBe(1);
  });

  it('stops the watcher on confirm_resolved and never posts prompt-ended', async () => {
    let promptEndedCalls = 0;
    const push: { push: (event: unknown) => void } = { push: () => {} };
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST' && !url.includes('prompt-ended')) {
        return jsonResponse({ session_id: 'sess-2', sdp: 'fake-answer-sdp' });
      }
      if (url.endsWith('/prompt-ended') && method === 'POST') {
        promptEndedCalls += 1;
        return jsonResponse({}, 204);
      }
      if (url.includes('/events')) {
        return sseResponse(push);
      }
      if (method === 'DELETE') return jsonResponse({}, 204);
      return jsonResponse({}, 404);
    });

    const { result } = renderHook(() => useLiveVoiceToggle());

    await act(async () => {
      result.current.toggle();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    await waitFor(() => expect(result.current.status).toBe('active'));
    vi.useFakeTimers();

    lastPc!.emitTrack({ getSynchronizationSources: () => [{ audioLevel: 0 }] });

    push.push({
      type: 'confirm_required',
      action_id: 'action-2',
      nonce: 'nonce-2',
      summary: 'Cancel the deploy',
      expires_at: Math.floor(Date.now() / 1000) + 60,
    });
    await act(async () => {
      await Promise.resolve();
    });

    act(() => {
      lastPc!.emitTranscriptDelta('Say yes, or tap ');
    });

    // Resolved (e.g. the user tapped Confirm on screen) before the anchor
    // text even finished arriving -- the watcher must stop right away.
    push.push({ type: 'confirm_resolved', action_id: 'action-2', status: 'confirmed' });
    await act(async () => {
      await Promise.resolve();
    });

    // The rest of the anchor text plus a long silence would have fired the
    // watcher had it not already been stopped.
    act(() => {
      lastPc!.emitTranscriptDelta('Confirm on screen.');
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5000);
    });

    expect(promptEndedCalls).toBe(0);
  });
});
