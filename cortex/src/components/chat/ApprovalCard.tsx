import { Check, Eye, GitCommitHorizontal, X } from 'lucide-react';
import type { ApprovalRequest, ApprovalState } from '../../types';

interface ApprovalCardProps {
  request: ApprovalRequest;
  onAction: (messageId: string, nextState: ApprovalState) => void;
}

const STATE_LABELS: Record<ApprovalState, string> = {
  pending: 'Awaiting review',
  reviewed: 'Reviewed',
  approved: 'Approved',
  rejected: 'Rejected',
};

export default function ApprovalCard({ request, onAction }: ApprovalCardProps) {
  const isResolved = request.state === 'approved' || request.state === 'rejected';

  return (
    <div className="mt-3 rounded-2xl border border-white/8 bg-black/20 p-4">
      <div className="flex items-start justify-between gap-4">
        <div>
          <div className="flex items-center gap-2 text-sm font-medium text-white">
            <GitCommitHorizontal className="h-4 w-4 text-[var(--accent)]" />
            {request.title}
          </div>
          <p className="mt-1 text-sm text-[var(--muted)]">{request.summary}</p>
        </div>
        <span className="rounded-full border border-white/8 bg-white/4 px-2.5 py-1 text-[11px] text-[var(--muted)]">
          {STATE_LABELS[request.state]}
        </span>
      </div>

      <div className="mt-3 grid gap-2 rounded-xl border border-white/6 bg-white/[0.03] p-3 text-sm text-[var(--muted)] sm:grid-cols-[1fr_auto] sm:items-center">
        <div>
          <div className="font-medium text-white">{request.commitMessage}</div>
          <div className="mt-1">{request.filesChanged} files changed</div>
        </div>
        <div className="text-left text-xs text-[var(--muted)] sm:text-right">
          {request.diffSummary}
        </div>
      </div>

      <div className="mt-3 flex flex-wrap gap-2">
        <button
          type="button"
          className="inline-flex h-10 items-center gap-2 rounded-full border border-white/8 bg-white/4 px-4 text-sm text-white transition hover:bg-white/8"
          disabled={isResolved}
          onClick={() => onAction(request.messageId, 'reviewed')}
        >
          <Eye className="h-4 w-4" />
          Review
        </button>
        <button
          type="button"
          className="inline-flex h-10 items-center gap-2 rounded-full bg-[var(--accent)] px-4 text-sm font-medium text-black transition hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-60"
          disabled={isResolved}
          onClick={() => onAction(request.messageId, 'approved')}
        >
          <Check className="h-4 w-4" />
          Approve
        </button>
        <button
          type="button"
          className="inline-flex h-10 items-center gap-2 rounded-full border border-white/8 bg-transparent px-4 text-sm text-[var(--muted-strong)] transition hover:bg-white/5 disabled:cursor-not-allowed disabled:opacity-60"
          disabled={isResolved}
          onClick={() => onAction(request.messageId, 'rejected')}
        >
          <X className="h-4 w-4" />
          Reject
        </button>
      </div>
    </div>
  );
}
