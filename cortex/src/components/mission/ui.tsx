import type { ReactNode } from 'react';
import { toneForStatus, type StatusTone } from './status';
import { AlertCircle, CheckCircle2, Circle, HelpCircle, Loader2, MinusCircle, RefreshCw } from 'lucide-react';

/**
 * The shared component vocabulary of mission control.
 *
 * Every pane renders the same header, the same status dots, the same
 * loading/empty/error states. Two panes disagreeing about what "failed" looks
 * like is a credibility bug on a trust surface, so the mapping from status
 * words to color lives here and nowhere else.
 */

/* ------------------------------------------------------------------ */
/* Status vocabulary                                                   */
/* ------------------------------------------------------------------ */

const TONE_TEXT: Record<StatusTone, string> = {
  ok: 'text-[var(--ok)]',
  err: 'text-[var(--err)]',
  warn: 'text-[var(--warn)]',
  busy: 'text-[var(--accent)]',
  idle: 'text-[var(--muted)]',
};

const TONE_CHIP: Record<StatusTone, string> = {
  ok: 'text-[var(--ok-strong)] bg-[var(--ok-soft)] border-[var(--ok-line)]',
  err: 'text-[var(--err-strong)] bg-[var(--err-soft)] border-[var(--err-line)]',
  warn: 'text-[var(--warn-strong)] bg-[var(--warn-soft)] border-[var(--warn-line)]',
  busy: 'text-[var(--accent)] bg-[var(--accent-soft)] border-[var(--line)]',
  idle: 'text-[var(--muted)] bg-[var(--surface)] border-[var(--line)]',
};

export function StatusIcon({ status, className = 'h-3.5 w-3.5' }: { status: string | null | undefined; className?: string }) {
  const tone = toneForStatus(status);
  const cls = `${className} ${TONE_TEXT[tone]}`;
  if (tone === 'ok') return <CheckCircle2 className={cls} aria-hidden />;
  if (tone === 'err') return <AlertCircle className={cls} aria-hidden />;
  if (tone === 'warn') return <HelpCircle className={cls} aria-hidden />;
  if (tone === 'busy') return <Loader2 className={`${cls} animate-spin`} aria-hidden />;
  return <Circle className={cls} aria-hidden />;
}

export function OutcomeIcon({ outcome, className = 'h-3.5 w-3.5' }: { outcome: string | null | undefined; className?: string }) {
  const tone = toneForStatus(outcome);
  const cls = `${className} ${TONE_TEXT[tone]}`;
  if (tone === 'ok') return <CheckCircle2 className={cls} aria-hidden />;
  if (tone === 'err') return <AlertCircle className={cls} aria-hidden />;
  if (tone === 'warn') return <HelpCircle className={cls} aria-hidden />;
  return <MinusCircle className={cls} aria-hidden />;
}

/** Compact status chip — `verified · 14/14` style claims live in these. */
export function StatusChip({ status, label, className = '' }: { status: string | null | undefined; label?: ReactNode; className?: string }) {
  const tone = toneForStatus(status);
  return (
    <span
      className={`inline-flex items-center gap-1 rounded border px-1.5 py-px t-micro font-medium ${TONE_CHIP[tone]} ${className}`}
    >
      {label ?? status}
    </span>
  );
}

/* ------------------------------------------------------------------ */
/* Layout                                                              */
/* ------------------------------------------------------------------ */

/** Standard pane header: fixed height so pane switches never shift layout. */
export function PaneHeader({ title, meta, actions }: { title: ReactNode; meta?: ReactNode; actions?: ReactNode }) {
  return (
    <header className="flex h-11 shrink-0 items-center justify-between gap-3 border-b border-[var(--line)] px-4">
      <div className="flex min-w-0 items-baseline gap-3">
        <h1 className="t-title truncate">{title}</h1>
        {meta && <span className="t-micro shrink-0 text-[var(--muted)]">{meta}</span>}
      </div>
      {actions && <div className="flex shrink-0 items-center gap-1.5">{actions}</div>}
    </header>
  );
}

export function Card({ children, className = '' }: { children: ReactNode; className?: string }) {
  return (
    <div className={`rounded-lg border border-[var(--line)] bg-[var(--surface)] ${className}`}>
      {children}
    </div>
  );
}

export function SectionLabel({ children }: { children: ReactNode }) {
  return <h2 className="t-label">{children}</h2>;
}

/* ------------------------------------------------------------------ */
/* States: loading, empty, error                                       */
/* ------------------------------------------------------------------ */

export function LoadingState({ label = 'Loading…' }: { label?: string }) {
  return (
    <div className="flex items-center gap-2 p-6 t-body text-[var(--muted)]" role="status">
      <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden />
      {label}
    </div>
  );
}

/** Skeleton list rows sized like real rows, so data arriving shifts nothing. */
export function SkeletonRows({ count = 3, height = 'h-12' }: { count?: number; height?: string }) {
  return (
    <div className="space-y-px p-2" aria-hidden>
      {Array.from({ length: count }, (_, index) => (
        <div key={index} className={`${height} animate-pulse rounded bg-[var(--surface-raised)]`} />
      ))}
    </div>
  );
}

/**
 * Honest empty state. `detail` says why the pane is empty — "nothing has
 * happened" and "we could not ask" are different facts and never conflated.
 */
export function EmptyState({ icon, title, children, action }: { icon?: ReactNode; title: string; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className="mx-auto max-w-md px-6 py-12 text-center">
      {icon && <div className="mx-auto mb-3 flex h-9 w-9 items-center justify-center rounded-lg bg-[var(--surface-raised)] text-[var(--muted)]">{icon}</div>}
      <h2 className="t-title">{title}</h2>
      {children && <p className="t-body mt-2 text-[var(--muted)]">{children}</p>}
      {action && <div className="mt-4">{action}</div>}
    </div>
  );
}

export function ErrorState({ message, onRetry, compact = false }: { message: string; onRetry?: () => void; compact?: boolean }) {
  const body = (
    <>
      <p className="t-micro rounded border border-[var(--err-line)] bg-[var(--err-soft)] px-2.5 py-1.5 text-[var(--err-strong)]">
        {message}
      </p>
      {onRetry && (
        <button
          type="button"
          onClick={onRetry}
          className="mt-2 inline-flex items-center gap-1.5 rounded-md border border-[var(--line)] bg-[var(--surface-raised)] px-2.5 py-1 t-micro text-[var(--fg)] transition-colors hover:bg-[var(--surface-hover)]"
        >
          <RefreshCw className="h-3 w-3" aria-hidden />
          Try again
        </button>
      )}
    </>
  );
  if (compact) return <div className="px-4 py-2">{body}</div>;
  return (
    <div className="mx-auto max-w-md px-6 py-10">
      <h2 className="t-title mb-2">Could not reach the Cortex API</h2>
      <p className="t-body mb-3 text-[var(--muted)]">
        The list shown may be incomplete — this is not the same as the data
        not existing.
      </p>
      {body}
    </div>
  );
}

/** Inline keyboard hint, e.g. in nav tooltips and buttons. */
export function Kbd({ children }: { children: ReactNode }) {
  return (
    <kbd className="rounded border border-[var(--line)] bg-[var(--inset)] px-1 t-micro t-mono text-[var(--muted)]">
      {children}
    </kbd>
  );
}
