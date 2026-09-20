import { useCallback } from 'react';
import { ChevronRight, Lightbulb, Code, Database, Palette, Briefcase, Globe } from 'lucide-react';

interface FlowOption {
  id: string;
  label: string;
  description?: string;
  category: string;
}

interface FlowOptionsProps {
  options: FlowOption[];
  onSelectOption: (optionId: string, optionLabel: string) => void;
  disabled?: boolean;
}

function getOptionIcon(category: string) {
  switch (category) {
    case 'popular':
    case 'confirm':
      return Lightbulb;
    case 'productivity':
      return Briefcase;
    case 'finance':
      return Database;
    case 'social':
      return Globe;
    case 'backend':
      return Code;
    case 'web':
      return Palette;
    default:
      return ChevronRight;
  }
}

function getOptionStyle(category: string) {
  switch (category) {
    case 'popular':
    case 'confirm':
      return 'border-emerald-500/20 bg-emerald-500/10 hover:border-emerald-400/30 hover:bg-emerald-500/15';
    case 'alternative':
      return 'border-blue-500/20 bg-blue-500/10 hover:border-blue-400/30 hover:bg-blue-500/15';
    case 'action':
      return 'border-purple-500/20 bg-purple-500/10 hover:border-purple-400/30 hover:bg-purple-500/15';
    default:
      return 'border-white/8 bg-white/[0.03] hover:border-white/12 hover:bg-white/[0.06]';
  }
}

export default function FlowOptions({ options, onSelectOption, disabled = false }: FlowOptionsProps) {
  const handleOptionClick = useCallback((option: FlowOption) => {
    if (disabled) return;
    onSelectOption(option.id, option.label);
  }, [onSelectOption, disabled]);

  if (options.length === 0) return null;

  // Group options by category for better display
  const popularOptions = options.filter(opt => opt.category === 'popular' || opt.category === 'confirm');
  const actionOptions = options.filter(opt => opt.category === 'action');
  const otherOptions = options.filter(opt =>
    !['popular', 'confirm', 'action'].includes(opt.category)
  );

  const renderOption = (option: FlowOption) => {
    const Icon = getOptionIcon(option.category);
    const styleClass = getOptionStyle(option.category);

    return (
      <button
        key={option.id}
        type="button"
        onClick={() => handleOptionClick(option)}
        disabled={disabled}
        className={`
          group relative flex w-full items-start gap-3 rounded-xl border p-3 text-left transition-all
          disabled:opacity-50 disabled:cursor-not-allowed active:scale-[0.99]
          ${styleClass}
        `}
      >
        <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-white/10">
          <Icon className="h-4 w-4" />
        </div>
        <div className="min-w-0 flex-1">
          <div className="font-medium text-white">{option.label}</div>
          {option.description && (
            <div className="mt-1 text-sm text-[var(--muted-strong)]">
              {option.description}
            </div>
          )}
        </div>
        <ChevronRight className="h-4 w-4 text-[var(--muted)] transition-transform group-hover:translate-x-0.5" />
      </button>
    );
  };

  return (
    <div className="mt-4 space-y-3">
      {/* Popular/Confirm options - highlighted */}
      {popularOptions.length > 0 && (
        <div className="grid gap-2">
          {popularOptions.map(renderOption)}
        </div>
      )}

      {/* Action options - secondary */}
      {actionOptions.length > 0 && (
        <div className="grid gap-2">
          {actionOptions.map(renderOption)}
        </div>
      )}

      {/* Other options - grid layout for multiple options */}
      {otherOptions.length > 0 && (
        <div className={`grid gap-2 ${otherOptions.length > 2 ? 'sm:grid-cols-2' : ''}`}>
          {otherOptions.map(renderOption)}
        </div>
      )}

      {/* Helper text */}
      <div className="mt-3 text-xs text-[var(--muted)]">
        💡 Click an option above, or type your own message
      </div>
    </div>
  );
}