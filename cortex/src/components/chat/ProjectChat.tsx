import { useCallback } from 'react';
import ChatComposer from './ChatComposer';
import ChatTimeline from './ChatTimeline';
import type { CortexGroup } from '../../lib/groups';
import type {
  ApprovalState,
  ChatMessage,
  ConfirmActionStatus,
} from '../../types';

interface ProjectChatProps {
  group: CortexGroup;
  userId: string;
  activeConversationId: string | null;
  messages: ChatMessage[];
  draft: string;
  isStreaming: boolean;
  isLoadingConversation: boolean;
  needsSubscription: boolean;
  isPreview?: boolean;
  onDraftChange: (value: string) => void;
  onSend: () => void;
  onStop?: () => void;
  onSubscribe: () => void;
  onApprovalAction: (messageId: string, nextState: ApprovalState) => void;
  onConfirmActionStatusChange?: (messageId: string, status: ConfirmActionStatus) => void;
  onLiveVoiceStart?: () => Promise<string | null | undefined>;
  onVoiceMessage?: (role: 'user' | 'assistant', content: string) => void;
  onVoiceConfirmRequired?: (event: { action_id: string; nonce: string; summary: string; expires_at: number }) => void;
  onVoiceSpokenWindow?: (event: { action_id: string; deadline: number }) => void;
  onVoiceConfirmResolved?: (event: { action_id: string; status: string }) => void;
}

const PROJECT_STARTER_PROMPTS = [
  'I want to build a habit tracker app',
  'Create a new task manager project',
  'Build a REST API service',
  'Help me debug this API call that\'s returning 500 errors.',
  'Review this code for potential security vulnerabilities.',
  'Set up a new React project',
];

export default function ProjectChat({
  group,
  activeConversationId,
  messages,
  draft,
  isStreaming,
  isLoadingConversation,
  needsSubscription,
  isPreview = false,
  onDraftChange,
  onSend,
  onStop,
  onSubscribe,
  onApprovalAction,
  onConfirmActionStatusChange,
  onLiveVoiceStart,
  onVoiceMessage,
  onVoiceConfirmRequired,
  onVoiceSpokenWindow,
  onVoiceConfirmResolved,
}: ProjectChatProps) {
  // Count messages sent by the user (not system/assistant) for preview prompt
  const userMessageCount = messages.filter(msg => msg.role === 'user').length;

  const handleSelectFlowOption = useCallback((optionId: string) => {
    // Send the selected option as a message
    onDraftChange(optionId);
    setTimeout(() => onSend(), 0); // Send after state update
  }, [onDraftChange, onSend]);

  return (
    <main className="flex h-full min-h-0 flex-1 flex-col overflow-hidden" aria-busy={isStreaming}>
      <div className="border-b border-white/6 px-4 py-2">
        <div className="flex items-center justify-between gap-3">
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <h1 className="truncate text-sm font-semibold text-white">Project Chat</h1>
              <span className="truncate text-xs text-[var(--muted)]">{group.name}</span>
            </div>
          </div>
        </div>
      </div>

      <ChatTimeline
        messages={messages}
        isLoading={isLoadingConversation}
        showStarters={!activeConversationId && !isStreaming}
        onSelectStarter={needsSubscription ? undefined : onDraftChange}
        onApprovalAction={onApprovalAction}
        onConfirmActionStatusChange={onConfirmActionStatusChange}
        onSelectFlowOption={needsSubscription ? undefined : handleSelectFlowOption}
        flowOptionsDisabled={isStreaming}
        starterPrompts={PROJECT_STARTER_PROMPTS}
      />

      {/* Soft subscription prompt for preview users who have sent 5+ messages */}
      {isPreview && !needsSubscription && userMessageCount >= 5 && (
        <div className="mx-4 mb-3 rounded-xl border border-[var(--accent)]/20 bg-[var(--accent)]/8 p-3">
          <p className="text-xs text-[var(--muted-strong)]">
            You're exploring Cortex in preview mode with sample data.
            <button
              type="button"
              onClick={onSubscribe}
              className="ml-1 font-medium text-[var(--accent)] hover:underline"
            >
              Subscribe to Cortex Pro
            </button>
            {' '}to connect real AI agents and unlock full capabilities.
          </p>
        </div>
      )}

      <ChatComposer
        draft={draft}
        disabled={isStreaming}
        locked={needsSubscription}
        placeholder="Ask me to help debug, refactor, review, or build anything..."
        onDraftChange={onDraftChange}
        onSend={onSend}
        onStop={onStop}
        onSubscribe={onSubscribe}
        onLiveVoiceStart={onLiveVoiceStart}
        onVoiceMessage={onVoiceMessage}
        onVoiceConfirmRequired={onVoiceConfirmRequired}
        onVoiceSpokenWindow={onVoiceSpokenWindow}
        onVoiceConfirmResolved={onVoiceConfirmResolved}
      />
    </main>
  );
}
