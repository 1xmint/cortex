import {
  Book,
  Bug,
  HelpCircle,
  Keyboard,
  Mail,
  X,
  Zap,
} from 'lucide-react';
import { useEffect, useRef, useState } from 'react';

const SHORTCUTS = [
  { keys: 'Ctrl+K', description: 'Open command palette' },
  { keys: 'Ctrl+N', description: 'New conversation' },
  { keys: 'Ctrl+B', description: 'Toggle sidebar' },
  { keys: 'Ctrl+,', description: 'Open settings' },
  { keys: 'Ctrl+P', description: 'Task Manager switcher' },
  { keys: 'Ctrl+Shift+O', description: 'Pop out Task Manager' },
  { keys: 'Escape', description: 'Close panel / stop stream' },
] as const;

const QUICK_GUIDE = [
  'Type a goal in the chat to create tasks and run agents automatically.',
  'Manage created tasks on the Task Board — drag to reorder, click to inspect.',
  'Switch between team groups using the sidebar or Ctrl+K.',
  'Open the Operations Room from the sidebar to monitor live agent runs.',
] as const;

export default function HelpMenu() {
  const [open, setOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement | null>(null);
  const buttonRef = useRef<HTMLButtonElement | null>(null);

  // Close on Escape
  useEffect(() => {
    if (!open) return;
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    window.addEventListener('keydown', handleKey);
    return () => window.removeEventListener('keydown', handleKey);
  }, [open]);

  // Close on outside click
  useEffect(() => {
    if (!open) return;
    const handleClick = (e: MouseEvent) => {
      const target = e.target as Node;
      if (!menuRef.current?.contains(target) && !buttonRef.current?.contains(target)) {
        setOpen(false);
      }
    };
    document.addEventListener('mousedown', handleClick);
    return () => document.removeEventListener('mousedown', handleClick);
  }, [open]);

  return (
    <div className="relative">
      <button
        ref={buttonRef}
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="inline-flex h-9 w-9 items-center justify-center rounded-lg text-[var(--muted)] transition hover:bg-white/6 hover:text-white active:scale-95"
        aria-label="Help and documentation"
        title="Help"
        aria-expanded={open}
        aria-haspopup="menu"
      >
        <HelpCircle className="h-4 w-4" />
      </button>

      {open && (
        <div
          ref={menuRef}
          role="menu"
          className="absolute right-0 top-11 z-50 w-80 rounded-xl border border-white/10 bg-[#111414] shadow-2xl"
        >
          {/* Header */}
          <div className="flex items-center justify-between border-b border-white/8 px-4 py-3">
            <span className="text-sm font-medium text-white">Help</span>
            <button
              type="button"
              onClick={() => setOpen(false)}
              className="rounded-lg p-1 text-[var(--muted)] transition hover:bg-white/8 hover:text-white"
              aria-label="Close help menu"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>

          <div className="max-h-[calc(100vh-8rem)] overflow-y-auto p-4 space-y-5">
            {/* Keyboard shortcuts */}
            <section>
              <h3 className="mb-2.5 flex items-center gap-2 text-[11px] font-medium uppercase tracking-[0.08em] text-[var(--muted)]">
                <Keyboard className="h-3.5 w-3.5" />
                Keyboard shortcuts
              </h3>
              <div className="space-y-1.5">
                {SHORTCUTS.map(({ keys, description }) => (
                  <div key={keys} className="flex items-center justify-between gap-3">
                    <span className="text-xs text-[var(--muted-strong)]">{description}</span>
                    <kbd className="shrink-0 rounded-md border border-white/8 bg-black/20 px-1.5 py-0.5 text-[10px] font-mono text-[var(--muted)]">
                      {keys}
                    </kbd>
                  </div>
                ))}
              </div>
            </section>

            {/* Quick guide */}
            <section>
              <h3 className="mb-2.5 flex items-center gap-2 text-[11px] font-medium uppercase tracking-[0.08em] text-[var(--muted)]">
                <Zap className="h-3.5 w-3.5" />
                Quick guide
              </h3>
              <ul className="space-y-2">
                {QUICK_GUIDE.map((tip) => (
                  <li key={tip} className="flex items-start gap-2 text-xs text-[var(--muted-strong)]">
                    <span className="mt-1.5 h-1 w-1 shrink-0 rounded-full bg-[var(--accent)]" />
                    {tip}
                  </li>
                ))}
              </ul>
            </section>

            {/* Links */}
            <section>
              <h3 className="mb-2.5 flex items-center gap-2 text-[11px] font-medium uppercase tracking-[0.08em] text-[var(--muted)]">
                <Book className="h-3.5 w-3.5" />
                Resources
              </h3>
              <div className="space-y-1">
                <a
                  href="#"
                  role="menuitem"
                  className="flex items-center gap-2.5 rounded-lg px-2 py-1.5 text-xs text-[var(--muted-strong)] transition hover:bg-white/6 hover:text-white"
                  onClick={() => setOpen(false)}
                >
                  <Book className="h-3.5 w-3.5 text-[var(--muted)]" />
                  Documentation
                </a>
                <a
                  href="mailto:support@heyvera.org"
                  role="menuitem"
                  className="flex items-center gap-2.5 rounded-lg px-2 py-1.5 text-xs text-[var(--muted-strong)] transition hover:bg-white/6 hover:text-white"
                  onClick={() => setOpen(false)}
                >
                  <Mail className="h-3.5 w-3.5 text-[var(--muted)]" />
                  Contact support
                </a>
                <a
                  href="https://github.com/1xmint/cortex/issues"
                  target="_blank"
                  rel="noopener noreferrer"
                  role="menuitem"
                  className="flex items-center gap-2.5 rounded-lg px-2 py-1.5 text-xs text-[var(--muted-strong)] transition hover:bg-white/6 hover:text-white"
                  onClick={() => setOpen(false)}
                >
                  <Bug className="h-3.5 w-3.5 text-[var(--muted)]" />
                  Report a bug
                </a>
              </div>
            </section>
          </div>
        </div>
      )}
    </div>
  );
}
