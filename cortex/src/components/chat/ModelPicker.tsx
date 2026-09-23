import { useCallback, useEffect, useState } from 'react';
import { ChevronDown, Lock } from 'lucide-react';
import { getChatModels, type ChatModelEntry } from '../../lib/cortexApi';
import { useAuthGate } from '../../lib/useAuthGate';
import { loadZenDeviceKey } from '../../lib/zenDeviceKey';

/** `unavailable_reason` -> the short copy shown next to a disabled Zen entry. */
function reasonLabel(reason: string | null): string {
  switch (reason) {
    case 'needs_key':
      return 'add a key';
    case 'key_rejected':
      return 'key rejected';
    default:
      return 'unavailable';
  }
}

interface ModelPickerProps {
  /** `undefined` selects the default Claude tier (no `model` sent to `/api/chat`). */
  selectedModel: string | undefined;
  onSelect: (model: string | undefined) => void;
  /** Opens Settings → Model keys. Used by disabled Zen entries and the 409 banner. */
  onOpenModelSettings?: () => void;
  /**
   * The server's own message from a 409 `zen_key_required` response on the
   * turn just sent, if any. Shown inline; this component never reacts to it
   * by switching `selectedModel` or re-sending on its own.
   */
  zenKeyError?: string | null;
}

export default function ModelPicker({ selectedModel, onSelect, onOpenModelSettings, zenKeyError }: ModelPickerProps) {
  const { userId } = useAuthGate();
  const [models, setModels] = useState<ChatModelEntry[]>([]);
  const [open, setOpen] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      // Only send this device's id -- never its secret -- and only when a
      // local device key exists, so the server can report whether *this*
      // device's Zen key is usable.
      const deviceId = loadZenDeviceKey(userId)?.deviceId;
      const { models: next } = await getChatModels(deviceId);
      setModels(next);
      setLoadError(null);
    } catch (err) {
      setLoadError(err instanceof Error ? err.message : 'Could not load models');
    }
  }, [userId]);

  useEffect(() => {
    void load();
  }, [load]);

  const claudeModels = models.filter((m) => m.provider === 'claude');
  const zenModels = models.filter((m) => m.provider === 'zen');
  const current = models.find((m) => m.model === selectedModel);
  const currentLabel = current?.label ?? (selectedModel ? selectedModel : 'Cortex credits');

  function choose(entry: ChatModelEntry) {
    if (!entry.available) {
      onOpenModelSettings?.();
      return;
    }
    setOpen(false);
    onSelect(entry.provider === 'zen' ? entry.model : undefined);
  }

  return (
    <div className="relative">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-haspopup="listbox"
        aria-expanded={open}
        className="inline-flex items-center gap-1.5 rounded-full border border-white/10 bg-white/8 px-3 py-1.5 text-xs font-medium text-white transition hover:bg-white/12"
      >
        {currentLabel}
        <ChevronDown className="h-3.5 w-3.5" />
      </button>

      {open && (
        <div role="listbox" className="absolute bottom-full left-0 z-10 mb-2 w-64 rounded-xl border border-white/10 bg-[var(--panel)] p-2 shadow-2xl">
          {loadError && <p className="px-2 py-1 text-xs text-red-400" role="alert">{loadError}</p>}

          {claudeModels.length > 0 && (
            <div className="mb-2">
              <p className="px-2 py-1 text-[10px] font-semibold uppercase tracking-wider text-[var(--muted)]">
                Cortex credits
              </p>
              {claudeModels.map((entry) => (
                <button
                  key={entry.model}
                  type="button"
                  role="option"
                  aria-selected={selectedModel === undefined ? entry.provider === 'claude' && current === undefined : selectedModel === entry.model}
                  onClick={() => choose(entry)}
                  className="flex w-full items-center justify-between rounded-lg px-2 py-1.5 text-left text-sm text-white transition hover:bg-white/8"
                >
                  {entry.label}
                </button>
              ))}
            </div>
          )}

          {zenModels.length > 0 && (
            <div>
              <p className="px-2 py-1 text-[10px] font-semibold uppercase tracking-wider text-[var(--muted)]">
                Your Zen key
              </p>
              {zenModels.map((entry) => (
                <button
                  key={entry.model}
                  type="button"
                  role="option"
                  aria-selected={selectedModel === entry.model}
                  aria-disabled={!entry.available}
                  data-disabled={!entry.available ? 'true' : undefined}
                  onClick={() => choose(entry)}
                  className={`flex w-full items-center justify-between rounded-lg px-2 py-1.5 text-left text-sm transition ${
                    entry.available ? 'text-white hover:bg-white/8' : 'text-[var(--muted)] hover:bg-white/4'
                  }`}
                >
                  <span>{entry.label}</span>
                  {!entry.available && (
                    <span className="inline-flex items-center gap-1 text-[10px] text-[var(--muted)]">
                      <Lock className="h-3 w-3" />
                      {reasonLabel(entry.unavailable_reason)}
                    </span>
                  )}
                </button>
              ))}
            </div>
          )}
        </div>
      )}

      {zenKeyError && (
        <p className="mt-1.5 flex items-center gap-1 text-xs text-red-400" role="alert">
          {zenKeyError}{' '}
          {onOpenModelSettings && (
            <button
              type="button"
              onClick={onOpenModelSettings}
              className="font-medium text-red-300 underline underline-offset-2 hover:text-red-200"
            >
              Open Settings
            </button>
          )}
        </p>
      )}
    </div>
  );
}
