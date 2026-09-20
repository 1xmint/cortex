import { useCallback, useEffect, type ReactNode } from 'react';

interface BottomSheetProps {
  open: boolean;
  onClose: () => void;
  children: ReactNode;
}

export default function BottomSheet({ open, onClose, children }: BottomSheetProps) {
  const handleKeyDown = useCallback(
    (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose();
    },
    [onClose],
  );

  useEffect(() => {
    if (open) {
      document.addEventListener('keydown', handleKeyDown);
      return () => document.removeEventListener('keydown', handleKeyDown);
    }
    return undefined;
  }, [open, handleKeyDown]);

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-50 sm:hidden" role="dialog" aria-modal="true">
      {/* Backdrop */}
      <div
        className="absolute inset-0 bg-black/50 transition-opacity"
        onClick={onClose}
        aria-hidden="true"
      />
      {/* Sheet */}
      <div className="absolute inset-x-0 bottom-0 transform rounded-t-2xl border-t border-zinc-700 bg-zinc-800 p-4 shadow-xl transition-transform duration-300 ease-out animate-[slideUp_0.3s_ease-out]">
        {/* Drag handle indicator */}
        <div className="mx-auto mb-4 h-1 w-10 rounded-full bg-zinc-600" />
        {children}
      </div>
      <style>{`
        @keyframes slideUp {
          from { transform: translateY(100%); }
          to { transform: translateY(0); }
        }
      `}</style>
    </div>
  );
}
