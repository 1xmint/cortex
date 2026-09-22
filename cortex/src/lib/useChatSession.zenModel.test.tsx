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

import { streamChat, ZenKeyRequiredError } from './cortexApi';
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

describe('useChatSession model selection', () => {
  it('sends model: "zen:<id>" on /api/chat when a Zen model is chosen', async () => {
    vi.mocked(streamChat).mockImplementation(() => new AbortController());

    const { result } = renderSession();

    act(() => {
      result.current.onModelChange('zen:glm-4.6');
    });
    act(() => {
      result.current.setDraft('hello');
    });
    act(() => {
      result.current.sendMessage();
    });

    await waitFor(() => expect(streamChat).toHaveBeenCalledTimes(1));

    const call = vi.mocked(streamChat).mock.calls[0];
    expect(call[6]).toBe('zen:glm-4.6');
  });

  it('sends no model when a Claude entry (or nothing) is chosen', async () => {
    vi.mocked(streamChat).mockImplementation(() => new AbortController());

    const { result } = renderSession();

    act(() => {
      result.current.setDraft('hello');
    });
    act(() => {
      result.current.sendMessage();
    });

    await waitFor(() => expect(streamChat).toHaveBeenCalledTimes(1));

    const call = vi.mocked(streamChat).mock.calls[0];
    expect(call[6]).toBeUndefined();
  });

  it('shows the server message on a 409 zen_key_required and makes exactly one /api/chat call', async () => {
    vi.mocked(streamChat).mockImplementation((_msg, _files, _ctx, _onEvent, _onDone, onError) => {
      onError(new ZenKeyRequiredError('Your OpenCode Zen key was rejected. Add a valid key in Settings.'));
      return new AbortController();
    });

    const { result } = renderSession();

    act(() => {
      result.current.onModelChange('zen:glm-4.6');
    });
    act(() => {
      result.current.setDraft('hello');
    });
    act(() => {
      result.current.sendMessage();
    });

    await waitFor(() =>
      expect(result.current.zenKeyError).toBe(
        'Your OpenCode Zen key was rejected. Add a valid key in Settings.',
      ),
    );

    expect(streamChat).toHaveBeenCalledTimes(1);
    expect(result.current.selectedModel).toBe('zen:glm-4.6');
    expect(result.current.isStreaming).toBe(false);
  });

  it('clears zenKeyError when the model changes', async () => {
    vi.mocked(streamChat).mockImplementation((_msg, _files, _ctx, _onEvent, _onDone, onError) => {
      onError(new ZenKeyRequiredError('Zen key rejected.'));
      return new AbortController();
    });

    const { result } = renderSession();

    act(() => result.current.onModelChange('zen:glm-4.6'));
    act(() => result.current.setDraft('hello'));
    act(() => result.current.sendMessage());

    await waitFor(() => expect(result.current.zenKeyError).toBe('Zen key rejected.'));

    act(() => result.current.onModelChange(undefined));

    expect(result.current.zenKeyError).toBeNull();
  });
});
