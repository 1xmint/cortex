// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import ChatComposer from './ChatComposer';

/**
 * Just real enough for `useLiveVoiceToggle` to reach `active`: it can
 * create an offer, take a remote description, and complete ICE gathering
 * on the next microtask -- no real network anywhere.
 */
class FakePeerConnection {
  connectionState = 'new';
  iceGatheringState = 'complete';
  localDescription: { sdp: string } | null = { sdp: 'gathered-sdp' };
  private listeners: Record<string, Array<(arg?: unknown) => void>> = {};
  addTrack = vi.fn();
  createDataChannel = vi.fn(() => ({
    readyState: 'open',
    send: vi.fn(),
    close: vi.fn(),
    addEventListener: vi.fn(),
  }));
  createOffer = vi.fn(async () => ({ type: 'offer', sdp: 'fake-offer-sdp' }));
  setLocalDescription = vi.fn(async () => undefined);
  setRemoteDescription = vi.fn(async () => undefined);
  close = vi.fn();
  addEventListener(name: string, cb: (arg?: unknown) => void) {
    this.listeners[name] ??= [];
    this.listeners[name].push(cb);
  }
  removeEventListener() {}
}

function installFakePeerConnection() {
  (globalThis as unknown as { RTCPeerConnection: new () => FakePeerConnection }).RTCPeerConnection =
    FakePeerConnection;
}

function installFakeMediaDevices() {
  const fakeTrack = { stop: vi.fn(), addEventListener: vi.fn() };
  const fakeStream = { getTracks: () => [fakeTrack] } as unknown as MediaStream;
  Object.defineProperty(navigator, 'mediaDevices', {
    value: { getUserMedia: vi.fn(async () => fakeStream) },
    configurable: true,
  });
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

/** A `Response`-alike whose body streams the given SSE `data:` lines, then closes. */
function sseResponse(events: unknown[]) {
  const body = new ReadableStream({
    start(controller) {
      const encoder = new TextEncoder();
      for (const event of events) {
        controller.enqueue(encoder.encode(`data: ${JSON.stringify(event)}\n\n`));
      }
      controller.close();
    },
  });
  return {
    ok: true,
    status: 200,
    headers: new Headers(),
    body,
  } as unknown as Response;
}

const noop = () => {};

describe('ChatComposer live voice events', () => {
  beforeEach(() => {
    installFakePeerConnection();
    installFakeMediaDevices();
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('sends conversation_id on start when a conversation is open', async () => {
    let postBody = '';
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        postBody = String(init?.body ?? '');
        return jsonResponse({ session_id: 'sess-conv', sdp: 'fake-answer-sdp' });
      }
      return jsonResponse({}, 404);
    });

    render(
      <ChatComposer
        draft=""
        onDraftChange={noop}
        onSend={noop}
        onLiveVoiceStart={async () => 'conv-open-1'}
      />,
    );

    await act(async () => {
      fireEvent.click(screen.getByLabelText('Live voice'));
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(JSON.parse(postBody)).toMatchObject({ conversation_id: 'conv-open-1' });
  });

  it('shows no error when the events route 404s (stub/dev mode)', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        return jsonResponse({ session_id: 'sess-404', sdp: 'fake-answer-sdp' });
      }
      // The events route -- and everything else -- 404s, as it does in
      // stub/dev mode.
      return jsonResponse({}, 404);
    });

    render(<ChatComposer draft="" onDraftChange={noop} onSend={noop} />);

    await act(async () => {
      fireEvent.click(screen.getByLabelText('Live voice'));
    });

    await waitFor(() => expect(screen.getByLabelText('Live voice')).toHaveAttribute('aria-pressed', 'true'));
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('renders a voice_message and a confirm_required event from the live voice events stream', async () => {
    const onVoiceMessage = vi.fn();
    const onVoiceConfirmRequired = vi.fn();

    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = init?.method ?? 'GET';
      if (url.includes('/api/voice/live/sessions') && method === 'POST') {
        return jsonResponse({ session_id: 'sess-events', sdp: 'fake-answer-sdp' });
      }
      if (url.includes('/api/voice/live/sessions/sess-events/events')) {
        return sseResponse([
          { type: 'voice_message', role: 'user', content: 'Cancel the deploy' },
          {
            type: 'confirm_required',
            action_id: 'action-9',
            nonce: 'nonce-9',
            summary: 'Cancel the running deploy',
            expires_at: new Date(Date.now() + 60_000).toISOString(),
          },
        ]);
      }
      return jsonResponse({}, 404);
    });

    render(
      <ChatComposer
        draft=""
        onDraftChange={noop}
        onSend={noop}
        onVoiceMessage={onVoiceMessage}
        onVoiceConfirmRequired={onVoiceConfirmRequired}
      />,
    );

    await act(async () => {
      fireEvent.click(screen.getByLabelText('Live voice'));
    });

    await waitFor(() => expect(onVoiceMessage).toHaveBeenCalledWith('user', 'Cancel the deploy'));
    await waitFor(() =>
      expect(onVoiceConfirmRequired).toHaveBeenCalledWith(
        expect.objectContaining({ action_id: 'action-9', nonce: 'nonce-9' }),
      ),
    );
  });
});
