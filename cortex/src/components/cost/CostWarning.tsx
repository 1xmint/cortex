import { useState } from 'react';
import { AlertTriangle, X, AlertCircle, Info } from 'lucide-react';
import type { CostWarning } from '../../lib/cortexApi';

interface CostWarningComponentProps {
  warnings: CostWarning[];
  onAcknowledge: (warningId: string) => void;
  onCancel?: () => void;
}

interface CostBlockingModalProps {
  warning: CostWarning;
  onContinue: () => void;
  onCancel: () => void;
}

function formatCurrency(amount: number): string {
  return new Intl.NumberFormat('en-US', {
    style: 'currency',
    currency: 'USD',
    maximumFractionDigits: 2,
  }).format(amount);
}

function getWarningIcon(warning: CostWarning) {
  switch (warning.level) {
    case 'error':
      return <AlertTriangle className="h-4 w-4 text-red-400" />;
    case 'warning':
      return <AlertCircle className="h-4 w-4 text-amber-400" />;
    default:
      return <Info className="h-4 w-4 text-blue-400" />;
  }
}

function getWarningStyles(warning: CostWarning): string {
  switch (warning.level) {
    case 'error':
      return 'border-red-400/20 bg-red-400/8 text-red-100';
    case 'warning':
      return 'border-amber-400/20 bg-amber-400/8 text-amber-100';
    default:
      return 'border-blue-400/20 bg-blue-400/8 text-blue-100';
  }
}

function CostBlockingModal({ warning, onContinue, onCancel }: CostBlockingModalProps) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4 backdrop-blur-sm">
      <div className="max-w-md rounded-xl border border-red-400/20 bg-[var(--panel)] p-6 shadow-2xl">
        <div className="flex items-start gap-3">
          <div className="flex h-8 w-8 items-center justify-center rounded-full bg-red-400/15">
            <AlertTriangle className="h-4 w-4 text-red-400" />
          </div>
          <div className="flex-1">
            <h3 className="text-base font-semibold text-white">{warning.title}</h3>
            <p className="mt-1 text-sm text-[var(--muted)]">{warning.message}</p>

            {warning.current_usage && warning.limit && (
              <div className="mt-3 rounded-lg border border-red-400/15 bg-red-400/5 p-3">
                <div className="flex items-center justify-between text-sm">
                  <span className="text-red-200">Current usage:</span>
                  <span className="font-medium text-white">
                    {formatCurrency(warning.current_usage)}
                  </span>
                </div>
                <div className="flex items-center justify-between text-sm">
                  <span className="text-red-200">{warning.budget_type} limit:</span>
                  <span className="font-medium text-white">
                    {formatCurrency(warning.limit)}
                  </span>
                </div>
              </div>
            )}
          </div>
        </div>

        <div className="mt-6 flex gap-3">
          <button
            onClick={onCancel}
            className="flex-1 rounded-lg border border-white/10 px-3 py-2 text-sm font-medium text-white transition hover:bg-white/5 active:scale-95"
          >
            Cancel
          </button>
          <button
            onClick={onContinue}
            className="flex-1 rounded-lg bg-red-500 px-3 py-2 text-sm font-medium text-white transition hover:bg-red-400 active:scale-95"
          >
            Continue anyway
          </button>
        </div>
      </div>
    </div>
  );
}

function InlineWarning({ warning, onAcknowledge }: { warning: CostWarning; onAcknowledge: (id: string) => void }) {
  const [dismissed, setDismissed] = useState(false);

  if (dismissed) return null;

  return (
    <div role="status" className={`rounded-xl border p-4 ${getWarningStyles(warning)}`}>
      <div className="flex items-start gap-3">
        {getWarningIcon(warning)}
        <div className="flex-1 min-w-0">
          <div className="flex items-center justify-between gap-2">
            <h4 className="font-medium text-white">{warning.title}</h4>
            <button
              onClick={() => setDismissed(true)}
              aria-label="Dismiss warning"
              className="rounded-lg p-1 text-white/60 transition hover:bg-white/10 hover:text-white"
            >
              <X className="h-4 w-4" />
            </button>
          </div>
          <p className="mt-1 text-sm opacity-90">{warning.message}</p>

          {warning.current_usage && warning.limit && (
            <div className="mt-2 flex items-center gap-4 text-sm">
              <span>
                Usage: <span className="font-medium">{formatCurrency(warning.current_usage)}</span>
              </span>
              <span>
                Limit: <span className="font-medium">{formatCurrency(warning.limit)}</span>
              </span>
              <span>
                ({warning.limit > 0 ? Math.round((warning.current_usage / warning.limit) * 100) : 0}%)
              </span>
            </div>
          )}

          {!warning.acknowledged && (
            <button
              onClick={() => onAcknowledge(warning.id)}
              className="mt-3 inline-flex items-center gap-1.5 rounded-lg bg-white/10 px-3 py-1.5 text-sm font-medium transition hover:bg-white/15 active:scale-95"
            >
              Acknowledge
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

export default function CostWarningComponent({ warnings, onAcknowledge, onCancel }: CostWarningComponentProps) {
  const blockingWarning = warnings.find(w => w.action_required && w.level === 'error');
  const inlineWarnings = warnings.filter(w => !w.action_required || w.level !== 'error');

  return (
    <>
      {/* Blocking modal for critical cost issues */}
      {blockingWarning && onCancel && (
        <CostBlockingModal
          warning={blockingWarning}
          onContinue={() => onAcknowledge(blockingWarning.id)}
          onCancel={onCancel}
        />
      )}

      {/* Inline warnings */}
      {inlineWarnings.length > 0 && (
        <div className="space-y-3">
          {inlineWarnings.map((warning) => (
            <InlineWarning
              key={warning.id}
              warning={warning}
              onAcknowledge={onAcknowledge}
            />
          ))}
        </div>
      )}
    </>
  );
}

export { CostBlockingModal };
export type { CostWarningComponentProps };