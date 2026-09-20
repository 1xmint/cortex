import { Check, Copy, Bot, Sparkles, User } from 'lucide-react';
import { useEffect, useState } from 'react';
import ApprovalCard from './ApprovalCard';
import FlowOptions from './FlowOptions';
import type { ApprovalState, ChatMessage as ChatMessageType } from '../../types';

interface ChatMessageProps {
  message: ChatMessageType;
  onApprovalAction: (messageId: string, nextState: ApprovalState) => void;
  onSelectFlowOption?: (optionId: string, optionLabel: string) => void;
  flowOptionsDisabled?: boolean;
}

function formatTime(timestamp: string) {
  return new Date(timestamp).toLocaleTimeString([], {
    hour: 'numeric',
    minute: '2-digit',
  });
}

export default function ChatMessage({
  message,
  onApprovalAction,
  onSelectFlowOption,
  flowOptionsDisabled = false
}: ChatMessageProps) {
  const isUser = message.role === 'user';
  const [copied, setCopied] = useState(false);
  const canCopy = message.content.trim().length > 0;

  useEffect(() => {
    if (!copied) return;
    const timeout = window.setTimeout(() => setCopied(false), 1400);
    return () => window.clearTimeout(timeout);
  }, [copied]);

  async function copyMessage() {
    if (!canCopy) return;
    try {
      await navigator.clipboard.writeText(message.content);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  }

  return (
    <div role="article" aria-label={`${isUser ? 'You' : message.providerLabel} at ${formatTime(message.createdAt)}`} className={`group flex gap-3 animate-fade-in ${isUser ? 'justify-end' : 'justify-start'}`}>
      {!isUser ? (
        <div className="mt-1 flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-[var(--accent-soft)] text-white">
          {message.isStreaming ? <Sparkles className="h-4 w-4" /> : <Bot className="h-4 w-4" />}
        </div>
      ) : null}

      <div className={`max-w-[85%] sm:max-w-[78%] ${isUser ? 'items-end' : 'items-start'} flex flex-col`}>
        <div
          className={[
            'rounded-[24px] px-4 py-2 text-sm leading-6 shadow-[0_1px_0_rgba(255,255,255,0.02)]',
            isUser
              ? 'border border-white/8 bg-[var(--user-bubble)] text-white shadow-[0_8px_24px_rgba(156,199,184,0.1),inset_0_1px_0_rgba(255,255,255,0.06)]'
              : 'border border-white/6 bg-[var(--assistant-bubble)] text-[var(--fg)]',
          ].join(' ')}
        >
          <p className="whitespace-pre-wrap">{message.content}</p>
          {message.approvalRequest ? (
            <ApprovalCard request={message.approvalRequest} onAction={onApprovalAction} />
          ) : null}
          {message.conversationFlow && message.conversationFlow.options.length > 0 && onSelectFlowOption ? (
            <FlowOptions
              options={message.conversationFlow.options}
              onSelectOption={onSelectFlowOption}
              disabled={flowOptionsDisabled}
            />
          ) : null}
        </div>
        <div className="mt-1.5 flex items-center gap-1.5 px-1 text-[11px] text-[var(--muted)]">
          <div className="flex min-w-0 items-center gap-1.5">
            {isUser ? <User className="h-3 w-3" /> : <Bot className="h-3 w-3" />}
            <span className="truncate">{message.role === 'assistant' ? message.providerLabel : 'You'}</span>
            <span>·</span>
            <span>{formatTime(message.createdAt)}</span>
          </div>
          {canCopy && (
            <>
              <span>·</span>
              <button
                type="button"
                onClick={copyMessage}
                className="inline-flex items-center gap-1 rounded-md px-1 py-0.5 text-[var(--muted)] opacity-0 transition hover:bg-white/6 hover:text-white active:scale-95 group-hover:opacity-100 group-focus-within:opacity-100"
                aria-label="Copy message"
              >
                {copied ? <Check className="h-3 w-3" /> : <Copy className="h-3 w-3" />}
                <span>{copied ? 'Copied' : 'Copy'}</span>
              </button>
            </>
          )}
        </div>
      </div>

      {isUser ? (
        <div className="mt-1 flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-white/8 text-white">
          <User className="h-4 w-4" />
        </div>
      ) : null}
    </div>
  );
}
