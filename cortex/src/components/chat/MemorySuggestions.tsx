import { Brain, Search, Trash2 } from 'lucide-react';
import { useEffect, useState } from 'react';
import { getMemorySuggestions, type MemorySuggestion } from '../../lib/cortexApi';

interface MemorySuggestionsProps {
  currentInput: string;
  context?: {
    files?: string[];
    recentMessages?: string[];
  };
  onSelect: (command: string) => void;
  visible: boolean;
}

const FALLBACK_SUGGESTIONS: MemorySuggestion[] = [
  {
    text: 'Remember this project preference',
    category: 'remember',
    command: 'Remember that ',
    description: 'Save a durable note for future Cortex sessions.',
  },
  {
    text: 'Recall related project memory',
    category: 'recall',
    command: 'Recall what we know about ',
    description: 'Search saved context before continuing.',
  },
  {
    text: 'Forget outdated context',
    category: 'forget',
    command: 'Forget memories about ',
    description: 'Remove stale or incorrect saved context.',
  },
];

function iconForCategory(category: MemorySuggestion['category']) {
  if (category === 'recall' || category === 'list') return Search;
  if (category === 'forget') return Trash2;
  return Brain;
}

export default function MemorySuggestions({
  currentInput,
  context,
  onSelect,
  visible,
}: MemorySuggestionsProps) {
  const [suggestions, setSuggestions] = useState<MemorySuggestion[]>(FALLBACK_SUGGESTIONS);

  useEffect(() => {
    if (!visible) return;
    let cancelled = false;

    void getMemorySuggestions({
      message: currentInput,
      files: context?.files,
      recentMessages: context?.recentMessages,
    })
      .then((nextSuggestions) => {
        if (!cancelled && nextSuggestions.length > 0) {
          setSuggestions(nextSuggestions);
        }
      })
      .catch(() => {
        if (!cancelled) setSuggestions(FALLBACK_SUGGESTIONS);
      });

    return () => {
      cancelled = true;
    };
  }, [context?.files, context?.recentMessages, currentInput, visible]);

  if (!visible) return null;

  return (
    <div className="absolute bottom-full left-3 right-3 z-20 mb-2 rounded-xl border border-white/10 bg-[var(--panel)] p-2 shadow-2xl">
      <div className="mb-2 flex items-center gap-2 px-2 text-xs font-medium text-[var(--muted)]">
        <Brain className="h-3.5 w-3.5 text-[var(--accent)]" />
        Memory commands
      </div>
      <div className="grid gap-1">
        {suggestions.slice(0, 4).map((suggestion) => {
          const Icon = iconForCategory(suggestion.category);
          return (
            <button
              key={`${suggestion.category}-${suggestion.command}`}
              type="button"
              onClick={() => onSelect(suggestion.command)}
              className="flex items-start gap-2 rounded-lg px-2 py-2 text-left transition hover:bg-white/6"
            >
              <Icon className="mt-0.5 h-3.5 w-3.5 shrink-0 text-[var(--accent)]" />
              <span className="min-w-0">
                <span className="block text-xs font-medium text-white">{suggestion.text}</span>
                {suggestion.description ? (
                  <span className="mt-0.5 block text-[11px] leading-4 text-[var(--muted)]">
                    {suggestion.description}
                  </span>
                ) : null}
              </span>
            </button>
          );
        })}
      </div>
    </div>
  );
}
