// @vitest-environment jsdom
import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { WorkerEvent } from './cortexApi';

vi.mock('./cortexApi', async () => {
  const actual = await vi.importActual<typeof import('./cortexApi')>('./cortexApi');
  return {
    ...actual,
    createConversation: vi.fn(async () => ({ id: 'conv-1' })),
    addMessageToConversation: vi.fn(async () => ({})),
    updateConversationTitle: vi.fn(async () => ({})),
    getConversation: vi.fn(async () => ({ id: 'conv-1', title: null, messages: [] })),
    streamChat: vi.fn(),
  };
});

import { streamChat } from './cortexApi';
import { useChatSession } from './useChatSession';
import { DEFAULT_GROUPS } from './groups';

afterEach(() => {
  vi.clearAllMocks();
});

const CONTROLS = { speed: 'balanced', intelligence: 'balanced', autonomy: 'guided' } as const;

function renderSession() {
  return renderHook(() =>
    useChatSession({
      activeConversationId: null,
      userId: 'user-1',
      isSignedIn: true,
      group: DEFAULT_GROUPS[0],
      sessionControls: CONTROLS,
      runProfile: 'auto',
      onConversationCreated: vi.fn(),
      onConversationsChanged: vi.fn(),
    }),
  );
}

describe('useChatSession confirm_required handling', () => {
  it('maps a confirm_required stream event into a pending confirm card', async () => {
    let capturedOnEvent: ((event: WorkerEvent) => void) | null = null;
    vi.mocked(streamChat).mockImplementation((_msg, _files, _ctx, onEvent) => {
      capturedOnEvent = onEvent;
      return new AbortController();
    });

    const { result } = renderHook(() =>
      useChatSession({
        activeConversationId: null,
        userId: 'user-1',
        isSignedIn: true,
        group: DEFAULT_GROUPS[0],
        sessionControls: CONTROLS,
        runProfile: 'auto',
        onConversationCreated: vi.fn(),
        onConversationsChanged: vi.fn(),
      }),
    );

    act(() => {
      result.current.setDraft('do the risky thing');
    });
    act(() => {
      result.current.sendMessage();
    });

    await waitFor(() => expect(capturedOnEvent).not.toBeNull());

    act(() => {
      capturedOnEvent!({
        type: 'confirm_required',
        action_id: 'action-1',
        nonce: 'nonce-1',
        summary: 'Delete 3 stale branches',
        expires_at: new Date(Date.now() + 60_000).toISOString(),
      });
    });

    await waitFor(() => {
      const confirmMsg = result.current.messages.find((m) => m.confirmAction);
      expect(confirmMsg?.confirmAction?.actionId).toBe('action-1');
      expect(confirmMsg?.confirmAction?.status).toBe('pending');
    });
  });

  it('replaces an older pending card when a newer confirm_required arrives', async () => {
    let capturedOnEvent: ((event: WorkerEvent) => void) | null = null;
    vi.mocked(streamChat).mockImplementation((_msg, _files, _ctx, onEvent) => {
      capturedOnEvent = onEvent;
      return new AbortController();
    });

    const { result } = renderSession();

    act(() => {
      result.current.setDraft('do the risky thing');
    });
    act(() => {
      result.current.sendMessage();
    });
    await waitFor(() => expect(capturedOnEvent).not.toBeNull());

    act(() => {
      capturedOnEvent!({
        type: 'confirm_required',
        action_id: 'action-1',
        nonce: 'nonce-1',
        summary: 'First risky action',
        expires_at: new Date(Date.now() + 60_000).toISOString(),
      });
    });
    await waitFor(() =>
      expect(result.current.messages.some((m) => m.confirmAction?.actionId === 'action-1')).toBe(true),
    );

    act(() => {
      capturedOnEvent!({
        type: 'confirm_required',
        action_id: 'action-2',
        nonce: 'nonce-2',
        summary: 'Second risky action',
        expires_at: new Date(Date.now() + 60_000).toISOString(),
      });
    });

    await waitFor(() => {
      const first = result.current.messages.find((m) => m.confirmAction?.actionId === 'action-1');
      const second = result.current.messages.find((m) => m.confirmAction?.actionId === 'action-2');
      expect(first?.confirmAction?.status).toBe('replaced');
      expect(second?.confirmAction?.status).toBe('pending');
    });
  });
});
