// @vitest-environment jsdom
import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

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

import { createConversation } from './cortexApi';
import { useChatSession } from './useChatSession';
import { DEFAULT_GROUPS } from './groups';

afterEach(() => {
  vi.clearAllMocks();
});

const CONTROLS = { speed: 'balanced', intelligence: 'balanced', autonomy: 'guided' } as const;

function renderSession(activeConversationId: string | null = 'conv-open') {
  return renderHook(() =>
    useChatSession({
      activeConversationId,
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

describe('useChatSession live voice event handling', () => {
  it('shows a ConfirmActionCard in the open conversation for a voice confirm_required event', async () => {
    const { result } = renderSession();

    act(() => {
      result.current.handleVoiceConfirmRequired({
        action_id: 'voice-action-1',
        nonce: 'voice-nonce-1',
        summary: 'Delete the staging database',
        expires_at: new Date(Date.now() + 60_000).toISOString(),
      });
    });

    await waitFor(() => {
      const confirmMsg = result.current.messages.find((m) => m.confirmAction);
      expect(confirmMsg?.confirmAction?.actionId).toBe('voice-action-1');
      expect(confirmMsg?.confirmAction?.nonce).toBe('voice-nonce-1');
      expect(confirmMsg?.confirmAction?.status).toBe('pending');
    });
  });

  it('appends a voice_message turn the same way a typed message appears', async () => {
    const { result } = renderSession();
    const initialCount = result.current.messages.length;

    act(() => {
      result.current.appendVoiceMessage('user', 'Cancel the deploy');
    });
    act(() => {
      result.current.appendVoiceMessage('assistant', 'Cancelling the deploy now.');
    });

    await waitFor(() => {
      expect(result.current.messages.length).toBe(initialCount + 2);
    });

    const userTurn = result.current.messages.find((m) => m.content === 'Cancel the deploy');
    const assistantTurn = result.current.messages.find((m) => m.content === 'Cancelling the deploy now.');
    expect(userTurn?.role).toBe('user');
    expect(assistantTurn?.role).toBe('assistant');
  });

  it('ensureConversationId creates a conversation via the same call as a new chat when none is open', async () => {
    const { result } = renderSession(null);

    let id: string | null = null;
    await act(async () => {
      id = await result.current.ensureConversationId();
    });

    expect(id).toBe('conv-1');
    expect(createConversation).toHaveBeenCalledWith('user-1');
  });

  it('ensureConversationId reuses the open conversation without creating a new one', async () => {
    const { result } = renderSession('conv-open');

    let id: string | null = null;
    await act(async () => {
      id = await result.current.ensureConversationId();
    });

    expect(id).toBe('conv-open');
    expect(createConversation).not.toHaveBeenCalled();
  });
});
