import { ArrowRight, ArrowUp, AudioLines, Mic, MicOff, Square } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { FormEvent, KeyboardEvent } from 'react';
import { useDictation } from '../../hooks/useDictation';
import { useLiveVoiceToggle } from '../../hooks/useLiveVoiceToggle';

const BUILTIN_PHRASES = [
  'create task',
  'assign to',
  'pause',
  'resume',
  'retry',
  'cancel',
  'mark done',
  'set priority',
  'open in chat',
];

const PHRASE_HISTORY_KEY = 'cortex:phrase-history';
const MAX_PHRASE_HISTORY = 50;

function loadPhraseHistory(): string[] {
  try {
    const raw = localStorage.getItem(PHRASE_HISTORY_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

function savePhraseToHistory(phrase: string) {
  const trimmed = phrase.trim();
  if (!trimmed) return;
  const history = loadPhraseHistory().filter((p) => p !== trimmed);
  history.unshift(trimmed);
  if (history.length > MAX_PHRASE_HISTORY) history.length = MAX_PHRASE_HISTORY;
  try {
    localStorage.setItem(PHRASE_HISTORY_KEY, JSON.stringify(history));
  } catch {
    // localStorage full or unavailable
  }
}

function computeGhostText(input: string): string {
  const trimmed = input.toLowerCase().trim();
  if (!trimmed) return '';
  // Check phrase history first
  const history = loadPhraseHistory();
  for (const phrase of history) {
    if (phrase.toLowerCase().startsWith(trimmed) && phrase.toLowerCase() !== trimmed) {
      return phrase.slice(input.trimEnd().length);
    }
  }
  // Fall back to built-in phrases
  for (const phrase of BUILTIN_PHRASES) {
    if (phrase.startsWith(trimmed) && phrase !== trimmed) {
      return phrase.slice(trimmed.length);
    }
  }
  return '';
}

interface ChatComposerProps {
  draft: string;
  disabled?: boolean;
  locked?: boolean;
  placeholder?: string;
  onDraftChange: (value: string) => void;
  onSend: () => void;
  onStop?: () => void;
  onSubscribe?: () => void;
}

export default function ChatComposer({
  draft,
  disabled = false,
  locked = false,
  placeholder = "Describe what you need...",
  onDraftChange,
  onSend,
  onStop,
  onSubscribe,
}: ChatComposerProps) {
  const canSend = draft.trim().length > 0 && !disabled && !locked;
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const [ghostText, setGhostText] = useState('');
  const draftRef = useRef(draft);
  useEffect(() => {
    draftRef.current = draft;
  }, [draft]);

  const dictation = useDictation({
    onTranscript: useCallback((text: string) => {
      // Update the ref synchronously, before notifying the parent: two
      // transcript fragments can arrive before React re-renders and hands
      // this closure a fresh `draft` prop, and each must build on the
      // other rather than both starting from the same stale draft.
      const next = draftRef.current + text;
      draftRef.current = next;
      onDraftChange(next);
    }, [onDraftChange]),
  });
  const liveVoice = useLiveVoiceToggle();
  const voiceError = dictation.error ?? liveVoice.error;

  const recomputeGhost = useCallback((value: string) => {
    setGhostText(computeGhostText(value));
  }, []);

  useEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    textarea.style.height = 'auto';
    textarea.style.height = `${Math.min(textarea.scrollHeight, 192)}px`;
  }, [draft]);

  // Mobile keyboard visibility: keep composer visible when virtual keyboard opens
  useEffect(() => {
    const viewport = window.visualViewport;
    if (!viewport) return;
    const formRef = textareaRef.current?.closest('form') as HTMLFormElement | null;
    if (!formRef) return;

    function handleResize() {
      if (!viewport || !formRef) return;
      // When keyboard opens, visualViewport height shrinks.
      // Adjust the form's bottom padding to stay above the keyboard.
      const offsetFromBottom = window.innerHeight - viewport.height - viewport.offsetTop;
      if (offsetFromBottom > 0) {
        formRef.style.paddingBottom = `${offsetFromBottom}px`;
      } else {
        formRef.style.paddingBottom = '';
      }
    }

    viewport.addEventListener('resize', handleResize);
    viewport.addEventListener('scroll', handleResize);
    return () => {
      viewport.removeEventListener('resize', handleResize);
      viewport.removeEventListener('scroll', handleResize);
      if (formRef) formRef.style.paddingBottom = '';
    };
  }, []);

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (locked && onSubscribe) {
      onSubscribe();
      return;
    }
    if (canSend) {
      savePhraseToHistory(draft);
      setGhostText('');
      onSend();
    }
  }

  function handleKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === 'Tab' && ghostText) {
      event.preventDefault();
      const accepted = draft + ghostText;
      onDraftChange(accepted);
      setGhostText('');
      return;
    }

    if (event.key === 'Escape') {
      setGhostText('');
      return;
    }

    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      if (locked && onSubscribe) {
        onSubscribe();
        return;
      }
      if (canSend) {
        savePhraseToHistory(draft);
        setGhostText('');
        onSend();
      }
    }
  }

  if (locked) {
    return (
      <div className="border-t border-white/6 p-3 shadow-[0_-18px_40px_rgba(0,0,0,0.26),inset_0_1px_0_rgba(156,199,184,0.12)] sm:p-4">
        <div className="glass-strong rounded-[24px] p-2 shadow-[inset_0_1px_0_rgba(255,255,255,0.03)]">
          <div className="flex min-h-[52px] items-center px-3 py-2">
            <p className="flex-1 text-sm text-[var(--muted)]">
              Your payment method needs updating to continue.
            </p>
            <button
              type="button"
              onClick={onSubscribe}
              className="inline-flex h-10 items-center gap-2 rounded-full bg-[var(--accent)] px-5 text-sm font-semibold text-black transition hover:brightness-110 active:scale-95"
            >
              Fix billing
              <ArrowRight className="h-4 w-4" />
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <form className="sticky bottom-0 relative border-t border-white/6 bg-[var(--panel)] p-3 shadow-[0_-18px_40px_rgba(0,0,0,0.26),inset_0_1px_0_rgba(156,199,184,0.12)] sm:p-4" onSubmit={handleSubmit} aria-label="Chat message composer">
      <div className="glass-strong rounded-[24px] p-2 shadow-[inset_0_1px_0_rgba(255,255,255,0.03)]">
        <div className="relative">
          <textarea
            ref={textareaRef}
            value={draft}
            disabled={disabled}
            rows={1}
            placeholder={placeholder}
            enterKeyHint="send"
            aria-label="Message input"
            aria-busy={disabled}
            className="max-h-48 min-h-[52px] w-full resize-none bg-transparent px-3 py-2 text-sm text-white outline-none placeholder:text-[var(--muted)]"
            onChange={(event) => { onDraftChange(event.target.value); recomputeGhost(event.target.value); }}
            onKeyDown={handleKeyDown}
          />
          {ghostText && (
            <div aria-hidden="true" className="pointer-events-none absolute left-0 top-0 max-h-48 min-h-[52px] w-full overflow-hidden px-3 py-2 text-sm">
              <span className="invisible">{draft}</span>
              <span className="text-white opacity-30">{ghostText}</span>
            </div>
          )}
        </div>
        {voiceError && (
          <p className="px-3 pb-1 text-xs text-red-400" role="alert">
            {voiceError}
          </p>
        )}
        <div className="flex items-center justify-end px-1 pb-1">
          {/* Action Buttons */}
          <div className="flex items-center gap-2">
            <button
              type="button"
              disabled={disabled || liveVoice.status !== 'idle'}
              aria-label={dictation.status === 'listening' ? 'Stop dictation' : 'Start dictation'}
              aria-pressed={dictation.status === 'listening'}
              onClick={dictation.toggle}
              className={`inline-flex h-10 w-10 min-h-[44px] min-w-[44px] items-center justify-center rounded-full border transition active:scale-95 disabled:cursor-not-allowed disabled:opacity-50 sm:min-h-0 sm:min-w-0 ${
                dictation.status === 'listening'
                  ? 'border-transparent bg-[var(--accent)] text-black'
                  : 'border-white/10 bg-white/8 text-white hover:bg-white/12'
              }`}
            >
              {dictation.status === 'listening' ? (
                <MicOff className="h-4 w-4" />
              ) : (
                <Mic className="h-4 w-4" />
              )}
            </button>
            <button
              type="button"
              disabled={(disabled && liveVoice.status === 'idle') || dictation.status !== 'idle'}
              aria-label={liveVoice.status === 'active' ? 'End live voice' : 'Start live voice'}
              aria-pressed={liveVoice.status === 'active'}
              onClick={liveVoice.toggle}
              className={`inline-flex h-10 w-10 min-h-[44px] min-w-[44px] items-center justify-center rounded-full border transition active:scale-95 disabled:cursor-not-allowed disabled:opacity-50 sm:min-h-0 sm:min-w-0 ${
                liveVoice.status === 'active'
                  ? 'border-transparent bg-[var(--accent)] text-black'
                  : 'border-white/10 bg-white/8 text-white hover:bg-white/12'
              }`}
            >
              <AudioLines className={`h-4 w-4 ${liveVoice.status === 'connecting' ? 'animate-pulse' : ''}`} />
            </button>
            {disabled && onStop ? (
              <button
                type="button"
                className="inline-flex h-10 items-center gap-2 rounded-full border border-white/10 bg-white/8 px-4 text-sm text-white transition hover:bg-white/12 active:scale-95"
                aria-label="Stop response"
                onClick={onStop}
              >
                <Square className="h-3.5 w-3.5 fill-current" />
                Stop
              </button>
            ) : (
              <button
                type="submit"
                disabled={!canSend}
                className="inline-flex h-10 w-10 min-h-[44px] min-w-[44px] items-center justify-center rounded-full bg-[var(--accent)] text-black shadow-[0_8px_20px_rgba(156,199,184,0.18)] transition hover:brightness-110 hover:shadow-[0_10px_24px_rgba(156,199,184,0.24)] active:scale-95 disabled:cursor-not-allowed disabled:opacity-50 sm:min-h-0 sm:min-w-0"
                aria-label="Send message"
              >
                <ArrowUp className="h-4 w-4" />
              </button>
            )}
          </div>
        </div>
      </div>
    </form>
  );
}
