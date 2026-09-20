import { useEffect, useMemo, useRef, useState } from 'react';
import ChatMessage from './ChatMessage';
import type { ApprovalState, ChatMessage as ChatMessageType } from '../../types';

interface ChatTimelineProps {
  messages: ChatMessageType[];
  isLoading?: boolean;
  showStarters?: boolean;
  starterPrompts?: string[];
  onSelectStarter?: (prompt: string) => void;
  onApprovalAction: (messageId: string, nextState: ApprovalState) => void;
  onSelectFlowOption?: (optionId: string, optionLabel: string) => void;
  flowOptionsDisabled?: boolean;
}

const STARTER_PROMPTS = [
  'Inspect the current changes and tell me what is risky.',
  'Find the next small frontend polish task and implement it.',
  'Review the app for production readiness gaps.',
  'Prepare a commit summary for this Cortex UI work.',
];

const INITIAL_VISIBLE_MESSAGES = 80;
const LOAD_MORE_MESSAGES = 80;

function TimelineSkeleton() {
  return (
    <div className="mx-auto flex w-full max-w-2xl flex-col gap-4">
      {[
        { align: 'start' as const, bubbleW: 'w-3/4', metaW: 'w-24' },
        { align: 'end' as const, bubbleW: 'w-1/2', metaW: 'w-16' },
        { align: 'start' as const, bubbleW: 'w-2/3', metaW: 'w-20' },
      ].map((item, idx) => (
        <div key={idx} className={`flex gap-3 ${item.align === 'end' ? 'justify-end' : 'justify-start'}`}>
          {item.align !== 'end' && (
            <div className="mt-1 h-8 w-8 shrink-0 animate-pulse rounded-full bg-white/8" />
          )}
          <div className={`flex max-w-[78%] flex-col gap-2 ${item.align === 'end' ? 'items-end' : 'items-start'}`}>
            <div className={`h-20 ${item.bubbleW} max-w-[70vw] animate-pulse rounded-3xl bg-white/[0.06]`} />
            <div className={`h-3 ${item.metaW} animate-pulse rounded-full bg-white/[0.05]`} />
          </div>
          {item.align === 'end' && (
            <div className="mt-1 h-8 w-8 shrink-0 animate-pulse rounded-full bg-white/8" />
          )}
        </div>
      ))}
    </div>
  );
}

export default function ChatTimeline({
  messages,
  isLoading = false,
  showStarters = false,
  starterPrompts = STARTER_PROMPTS,
  onSelectStarter,
  onApprovalAction,
  onSelectFlowOption,
  flowOptionsDisabled = false,
}: ChatTimelineProps) {
  const endRef = useRef<HTMLDivElement | null>(null);
  const [visibleCount, setVisibleCount] = useState(INITIAL_VISIBLE_MESSAGES);
  const hiddenCount = Math.max(messages.length - visibleCount, 0);
  const visibleMessages = useMemo(
    () => messages.slice(Math.max(messages.length - visibleCount, 0)),
    [messages, visibleCount],
  );

  useEffect(() => {
    if (messages.length <= INITIAL_VISIBLE_MESSAGES) {
      setVisibleCount(INITIAL_VISIBLE_MESSAGES);
    }
  }, [messages.length]);

  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: 'smooth', block: 'end' });
  }, [visibleMessages]);

  return (
    <section className="flex-1 overflow-y-auto px-3 py-4 sm:px-5" role="log" aria-live="polite" aria-label="Chat messages">
      {isLoading ? (
        <TimelineSkeleton />
      ) : (
        <div className="mx-auto flex w-full max-w-2xl flex-col gap-3">
          {hiddenCount > 0 && (
            <div className="flex justify-center">
              <button
                type="button"
                onClick={() => setVisibleCount((count) => count + LOAD_MORE_MESSAGES)}
                className="rounded-full border border-white/8 bg-white/[0.03] px-3 py-1.5 text-xs text-[var(--muted-strong)] transition hover:bg-white/[0.06] hover:text-white active:scale-95"
              >
                Show {Math.min(hiddenCount, LOAD_MORE_MESSAGES)} older messages
              </button>
            </div>
          )}
          {visibleMessages.map((message) => (
            <ChatMessage
              key={message.id}
              message={message}
              onApprovalAction={onApprovalAction}
              onSelectFlowOption={onSelectFlowOption}
              flowOptionsDisabled={flowOptionsDisabled}
            />
          ))}
          {showStarters && messages.length === 1 && messages[0]?.id === 'm-init' && (
            <div className="grid gap-2 pl-11 sm:grid-cols-2">
              {starterPrompts.map((prompt) => (
                <button
                  key={prompt}
                  type="button"
                  onClick={() => onSelectStarter?.(prompt)}
                  className="rounded-xl border border-white/8 bg-white/[0.03] px-3 py-2.5 text-left text-xs leading-5 text-[var(--muted-strong)] transition hover:border-white/12 hover:bg-white/[0.06] hover:text-white active:scale-[0.99]"
                >
                  {prompt}
                </button>
              ))}
            </div>
          )}
          <div ref={endRef} />
        </div>
      )}
    </section>
  );
}
