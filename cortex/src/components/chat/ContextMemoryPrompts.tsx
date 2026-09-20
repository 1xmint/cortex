import { Lightbulb } from 'lucide-react';

interface ContextMemoryPromptsProps {
  context: {
    message: string;
    files?: string[];
    recentMessages?: string[];
    workspaceId?: string;
  };
  onPromptSelect: (prompt: string) => void;
  visible: boolean;
}

export default function ContextMemoryPrompts({
  context,
  onPromptSelect,
  visible,
}: ContextMemoryPromptsProps) {
  if (!visible) return null;

  const prompts = [
    context.files && context.files.length > 0
      ? `Recall memory related to ${context.files[0]}`
      : 'Recall relevant project memory before answering',
    'Remember this decision for future sessions',
  ];

  return (
    <div className="flex flex-wrap gap-2">
      {prompts.map((prompt) => (
        <button
          key={prompt}
          type="button"
          onClick={() => onPromptSelect(prompt)}
          className="inline-flex items-center gap-1.5 rounded-lg border border-white/8 bg-white/[0.03] px-2.5 py-1.5 text-xs text-[var(--muted)] transition hover:bg-white/6 hover:text-white"
        >
          <Lightbulb className="h-3.5 w-3.5 text-[var(--accent)]" />
          {prompt}
        </button>
      ))}
    </div>
  );
}
