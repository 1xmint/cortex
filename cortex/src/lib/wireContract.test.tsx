// @vitest-environment jsdom
//
// Feeds `test/fixtures/wire-events.json` -- the JSON the server's own
// serializer actually emits (checked against it in
// `crates/api/src/chat.rs`'s and `crates/api/src/voice_session.rs`'s
// `wire_contract`/`wire_contract_tests` modules) -- through the browser's
// real ingest code, so a server wire-format change (e.g. `expires_at`
// switching from Unix seconds to milliseconds or an ISO string) breaks a
// test here instead of only breaking hand-written ISO fixtures that can't
// drift out of sync with the server on their own.
import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { WorkerEvent } from './cortexApi';
import type { VoiceConfirmRequiredEvent, VoiceSpokenWindowEvent, VoiceConfirmResolvedEvent, VoiceMessageEvent } from './voiceApi';

import wireEvents from '../test/fixtures/wire-events.json';

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

// Waits for the open conversation's initial load to finish: that load
// replaces `messages`, so anything a test appends before it lands is wiped.
async function renderSession(activeConversationId: string | null = 'conv-open') {
  const rendered = renderHook(() =>
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
  await waitFor(() => expect(rendered.result.current.isLoadingConversation).toBe(false));
  return rendered;
}

// The fixed inputs behind every fixture entry (crates/api/src/chat.rs and
// crates/api/src/voice_session.rs's wire-contract tests use the same ones).
const EXPECTED_EXPIRES_AT_ISO = new Date(1_790_000_000 * 1000).toISOString();
const EXPECTED_DEADLINE_ISO = new Date(1_790_000_045 * 1000).toISOString();

describe('wire contract: chat SSE confirm_required', () => {
  it('the real chat stream ingest accepts the server fixture and produces the right card', async () => {
    let capturedOnEvent: ((event: WorkerEvent) => void) | null = null;
    vi.mocked(streamChat).mockImplementation((_msg, _files, _ctx, onEvent) => {
      capturedOnEvent = onEvent;
      return new AbortController();
    });

    const { result } = await renderSession(null);

    act(() => {
      result.current.setDraft('do the risky thing');
    });
    act(() => {
      result.current.sendMessage();
    });
    await waitFor(() => expect(capturedOnEvent).not.toBeNull());

    act(() => {
      capturedOnEvent!(wireEvents.chat_confirm_required as WorkerEvent);
    });

    await waitFor(() => {
      const card = result.current.messages.find((m) => m.confirmAction)?.confirmAction;
      expect(card).toBeDefined();
      expect(card!.actionId).toBe('fixture-action');
      expect(card!.nonce).toBe('fixture-nonce');
      expect(card!.summary).toBe('Fixture summary text');
      expect(card!.expiresAt).toBe(EXPECTED_EXPIRES_AT_ISO);
      expect(card!.status).toBe('pending');
    });
  });
});

describe('wire contract: live voice events', () => {
  it('handleVoiceConfirmRequired accepts the server fixture and produces the right card', async () => {
    const { result } = await renderSession();
    const event: VoiceConfirmRequiredEvent = wireEvents.voice_confirm_required as VoiceConfirmRequiredEvent;

    act(() => {
      result.current.handleVoiceConfirmRequired(event);
    });

    await waitFor(() => {
      const card = result.current.messages.find((m) => m.confirmAction)?.confirmAction;
      expect(card).toBeDefined();
      expect(card!.actionId).toBe('fixture-action');
      expect(card!.nonce).toBe('fixture-nonce');
      expect(card!.summary).toBe('Fixture summary text');
      expect(card!.expiresAt).toBe(EXPECTED_EXPIRES_AT_ISO);
      expect(card!.status).toBe('pending');
    });
  });

  // The two `act`s below run back-to-back with no `await` between them --
  // an intervening `await waitFor` here would give the mocked
  // `getConversation` conversation-load effect (`conv-open` is an "already
  // open" conversation, so `useChatSession` refetches it on mount) a chance
  // to resolve and replace `messages` wholesale before the second event
  // lands, same as `useChatSession.voiceEvents.test.tsx`'s own
  // spoken-window test avoids it.
  it('handleVoiceSpokenWindow accepts the server fixture and sets the countdown deadline', async () => {
    const { result } = await renderSession();

    act(() => {
      result.current.handleVoiceConfirmRequired(wireEvents.voice_confirm_required as VoiceConfirmRequiredEvent);
    });
    act(() => {
      result.current.handleVoiceSpokenWindow(wireEvents.voice_spoken_window as VoiceSpokenWindowEvent);
    });

    await waitFor(() => {
      const card = result.current.messages.find((m) => m.confirmAction)?.confirmAction;
      expect(card).toBeDefined();
      expect(card!.actionId).toBe('fixture-action');
      expect(card!.spokenWindowDeadline).toBe(EXPECTED_DEADLINE_ISO);
    });
  });

  it('handleVoiceConfirmResolved accepts the server "confirmed" fixture and resolves the card', async () => {
    const { result } = await renderSession();

    act(() => {
      result.current.handleVoiceConfirmRequired(wireEvents.voice_confirm_required as VoiceConfirmRequiredEvent);
    });
    act(() => {
      result.current.handleVoiceConfirmResolved(
        wireEvents.voice_confirm_resolved_confirmed as VoiceConfirmResolvedEvent,
      );
    });

    await waitFor(() => {
      const card = result.current.messages.find((m) => m.confirmAction)?.confirmAction;
      expect(card).toBeDefined();
      expect(card!.status).toBe('confirmed');
    });
  });

  it('handleVoiceConfirmResolved accepts the server "cancelled" fixture and resolves the card', async () => {
    const { result } = await renderSession();

    act(() => {
      result.current.handleVoiceConfirmRequired(wireEvents.voice_confirm_required as VoiceConfirmRequiredEvent);
    });
    act(() => {
      result.current.handleVoiceConfirmResolved(
        wireEvents.voice_confirm_resolved_cancelled as VoiceConfirmResolvedEvent,
      );
    });

    await waitFor(() => {
      const card = result.current.messages.find((m) => m.confirmAction)?.confirmAction;
      expect(card).toBeDefined();
      expect(card!.status).toBe('cancelled');
    });
  });

  it('appendVoiceMessage accepts the server voice_message fixture', async () => {
    const { result } = await renderSession();
    const event: VoiceMessageEvent = wireEvents.voice_message as VoiceMessageEvent;

    act(() => {
      result.current.appendVoiceMessage(event.role, event.content);
    });

    await waitFor(() => {
      const msg = result.current.messages.find((m) => m.content === 'Fixture message content');
      expect(msg?.role).toBe('assistant');
    });
  });
});
