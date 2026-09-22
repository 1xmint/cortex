import { useEffect, useRef, useState } from 'react';
import { Check, ShieldAlert, X } from 'lucide-react';
import { cancelAgentAction, confirmAgentAction, CortexApiError } from '../../lib/cortexApi';
import type { ConfirmActionRequest, ConfirmActionStatus } from '../../types';

interface ConfirmActionCardProps {
  request: ConfirmActionRequest;
  onStatusChange: (messageId: string, status: ConfirmActionStatus) => void;
}

const RESOLVED_LABELS: Partial<Record<ConfirmActionStatus, string>> = {
  confirmed: 'Confirmed',
  cancelled: 'Cancelled',
  expired: 'Expired',
  unavailable: 'No longer available',
  replaced: 'Replaced',
};

/** Whole seconds remaining until `expiresAt`, floored at 0. */
function secondsRemaining(expiresAt: string): number {
  const ms = new Date(expiresAt).getTime() - Date.now();
  return Math.max(0, Math.ceil(ms / 1000));
}

function formatCountdown(totalSeconds: number): string {
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${minutes}:${seconds.toString().padStart(2, '0')}`;
}

export default function ConfirmActionCard({ request, onStatusChange }: ConfirmActionCardProps) {
  const [remaining, setRemaining] = useState(() => secondsRemaining(request.expiresAt));
  const [isSubmitting, setIsSubmitting] = useState(false);
  const inFlightRef = useRef(false);

  // Recompute a fresh countdown whenever a new pending request takes over
  // this card's slot (replaced-in-place is not expected today, but the
  // effect stays correct if it ever happens).
  useEffect(() => {
    setRemaining(secondsRemaining(request.expiresAt));
  }, [request.expiresAt]);

  useEffect(() => {
    if (request.status !== 'pending') return;
    const interval = window.setInterval(() => {
      const next = secondsRemaining(request.expiresAt);
      setRemaining(next);
      if (next <= 0) {
        window.clearInterval(interval);
        onStatusChange(request.messageId, 'expired');
      }
    }, 1000);
    return () => window.clearInterval(interval);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [request.status, request.expiresAt, request.messageId]);

  async function handleDecision(decision: 'confirm' | 'cancel') {
    if (inFlightRef.current || request.status !== 'pending') return;
    inFlightRef.current = true;
    setIsSubmitting(true);
    try {
      if (decision === 'confirm') {
        await confirmAgentAction(request.actionId, request.nonce);
        onStatusChange(request.messageId, 'confirmed');
      } else {
        await cancelAgentAction(request.actionId, request.nonce);
        onStatusChange(request.messageId, 'cancelled');
      }
    } catch (err) {
      if (err instanceof CortexApiError && (err.status === 404 || err.status === 409)) {
        onStatusChange(request.messageId, 'unavailable');
      } else {
        // Leave it pending so the user can retry rather than silently drop it.
        inFlightRef.current = false;
        setIsSubmitting(false);
      }
      return;
    }
    inFlightRef.current = false;
    setIsSubmitting(false);
  }

  if (request.status !== 'pending') {
    return (
      <div className="mt-3 rounded-2xl border border-white/8 bg-black/20 p-4 text-sm text-[var(--muted)]">
        {RESOLVED_LABELS[request.status] ?? request.status}
        {request.status === 'confirmed' ? `: ${request.summary}` : ''}
      </div>
    );
  }

  const isExpiring = remaining <= 10;

  return (
    <div className="mt-3 rounded-2xl border border-white/8 bg-black/20 p-4">
      <div className="flex items-start justify-between gap-4">
        <div className="flex items-start gap-2">
          <ShieldAlert className="mt-0.5 h-4 w-4 shrink-0 text-[var(--accent)]" />
          <p className="text-sm text-white">{request.summary}</p>
        </div>
        <span
          className={[
            'shrink-0 rounded-full border border-white/8 bg-white/4 px-2.5 py-1 text-[11px] tabular-nums',
            isExpiring ? 'text-red-300' : 'text-[var(--muted)]',
          ].join(' ')}
          aria-hidden="true"
        >
          {formatCountdown(remaining)}
        </span>
      </div>
      {/* A visible countdown chip updates every second above; this text keeps
          screen readers informed without re-announcing each tick. */}
      <span className="sr-only" role="status">
        {isExpiring ? `Expiring in ${remaining} seconds` : 'Awaiting confirmation'}
      </span>

      <div className="mt-3 flex flex-wrap gap-2">
        <button
          type="button"
          aria-label={`Confirm: ${request.summary}`}
          className="inline-flex h-10 items-center gap-2 rounded-full bg-[var(--accent)] px-4 text-sm font-medium text-black transition hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-60"
          disabled={isSubmitting}
          onClick={() => void handleDecision('confirm')}
        >
          <Check className="h-4 w-4" />
          Confirm
        </button>
        <button
          type="button"
          aria-label={`Cancel: ${request.summary}`}
          className="inline-flex h-10 items-center gap-2 rounded-full border border-white/8 bg-transparent px-4 text-sm text-[var(--muted-strong)] transition hover:bg-white/5 disabled:cursor-not-allowed disabled:opacity-60"
          disabled={isSubmitting}
          onClick={() => void handleDecision('cancel')}
        >
          <X className="h-4 w-4" />
          Cancel
        </button>
      </div>
    </div>
  );
}
