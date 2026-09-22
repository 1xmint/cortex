import { useCallback, useEffect, useState } from 'react';
import { AlertTriangle, KeyRound, Loader2, Trash2 } from 'lucide-react';
import {
  CortexApiError,
  deleteProviderKey,
  getProviderKeys,
  saveProviderKey,
  type ProviderKeySummary,
} from '../../lib/cortexApi';

function formatDate(ms: number | null | undefined): string {
  if (!ms) return 'never';
  return new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', year: 'numeric' }).format(new Date(ms));
}

/** `sk-abcd1234` -> `••••1234`. The API never returns more than `last4`. */
function maskedKey(last4: string): string {
  return `••••${last4}`;
}

export default function ModelKeysTab() {
  const [keys, setKeys] = useState<ProviderKeySummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);

  // Form state -- only present while adding or replacing a key. The input
  // value itself is never kept anywhere else (no draft state, no localStorage).
  const [editing, setEditing] = useState(false);
  const [keyInput, setKeyInput] = useState('');
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setLoadError(null);
    try {
      const next = await getProviderKeys();
      setKeys(next);
    } catch (err) {
      setLoadError(err instanceof Error ? err.message : 'Could not load saved keys');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const zenKey = keys.find((k) => k.provider === 'zen') ?? null;

  async function handleSave() {
    const value = keyInput;
    setSaving(true);
    setSaveError(null);
    try {
      await saveProviderKey('zen', value);
      setKeyInput('');
      setEditing(false);
      await refresh();
    } catch (err) {
      setKeyInput('');
      setSaveError(
        err instanceof CortexApiError ? err.message : err instanceof Error ? err.message : 'Could not save key',
      );
    } finally {
      setSaving(false);
    }
  }

  async function handleDelete() {
    setDeleting(true);
    setDeleteError(null);
    try {
      await deleteProviderKey('zen');
      setConfirmingDelete(false);
      await refresh();
    } catch (err) {
      setDeleteError(err instanceof Error ? err.message : 'Could not delete key');
    } finally {
      setDeleting(false);
    }
  }

  return (
    <div className="flex flex-col gap-5">
      <section className="flex flex-col gap-3">
        <h3 className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-[var(--muted)]">
          <KeyRound className="h-3.5 w-3.5" />
          Model keys
        </h3>

        <div className="rounded-xl border border-white/8 bg-white/[0.02] px-4 py-3">
          <p className="text-xs text-[var(--muted)]">
            OpenCode Zen: runs GLM, Kimi, DeepSeek and MiniMax on your own Zen account. Cortex
            does not charge credits for these; Zen bills you directly.
          </p>

          {loading ? (
            <div className="mt-3 flex items-center gap-2 text-xs text-[var(--muted)]">
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
              Loading...
            </div>
          ) : loadError ? (
            <p className="mt-3 text-xs text-red-400" role="alert">{loadError}</p>
          ) : (
            <div className="mt-3">
              {zenKey && !editing ? (
                <div className="flex flex-col gap-2">
                  {zenKey.status === 'rejected' ? (
                    <p className="text-xs font-medium text-red-400" role="alert">
                      Zen rejected this key
                    </p>
                  ) : (
                    <p className="text-sm text-white">
                      Zen key {maskedKey(zenKey.last4)} · added {formatDate(zenKey.created_at)} · last used{' '}
                      {formatDate(zenKey.last_used_at)}
                    </p>
                  )}
                  <div className="flex items-center gap-2">
                    <button
                      type="button"
                      onClick={() => { setEditing(true); setSaveError(null); }}
                      className="rounded-lg border border-white/10 bg-white/8 px-3 py-1.5 text-xs font-medium text-white transition hover:bg-white/12 active:scale-95"
                    >
                      Replace
                    </button>
                    {!confirmingDelete ? (
                      <button
                        type="button"
                        onClick={() => { setConfirmingDelete(true); setDeleteError(null); }}
                        className="inline-flex items-center gap-1.5 rounded-lg border border-red-500/40 bg-red-500/10 px-3 py-1.5 text-xs font-medium text-red-300 transition hover:bg-red-500/20 active:scale-95"
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                        Delete
                      </button>
                    ) : null}
                  </div>
                  {confirmingDelete && (
                    <div className="flex flex-col gap-2 rounded-lg border border-red-500/15 bg-red-500/5 p-3">
                      <p className="text-xs text-red-200 flex items-center gap-1.5">
                        <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
                        Delete your saved Zen key? Chats using Zen models will stop working until you add a new one.
                      </p>
                      <div className="flex gap-2">
                        <button
                          type="button"
                          onClick={() => setConfirmingDelete(false)}
                          className="rounded-lg border border-white/10 px-3 py-1.5 text-xs text-[var(--muted)] transition hover:text-white"
                        >
                          Cancel
                        </button>
                        <button
                          type="button"
                          disabled={deleting}
                          onClick={() => void handleDelete()}
                          className="rounded-lg bg-red-500 px-3 py-1.5 text-xs font-medium text-white transition hover:bg-red-400 active:scale-95 disabled:opacity-30"
                        >
                          {deleting ? 'Deleting...' : 'Confirm delete'}
                        </button>
                      </div>
                      {deleteError && <p className="text-xs text-red-400" role="alert">{deleteError}</p>}
                    </div>
                  )}
                </div>
              ) : (
                <div className="flex flex-col gap-2">
                  <label className="text-xs text-[var(--muted)]" htmlFor="zen-api-key">
                    OpenCode Zen API key
                  </label>
                  <input
                    id="zen-api-key"
                    type="password"
                    autoComplete="off"
                    spellCheck={false}
                    value={keyInput}
                    onChange={(e) => setKeyInput(e.target.value)}
                    placeholder="Paste your Zen key"
                    className="rounded-lg border border-white/10 bg-[var(--composer)] px-3 py-2 font-mono text-sm text-white placeholder:text-white/20 focus:border-[var(--accent)]/50 focus:outline-none"
                  />
                  <div className="flex items-center gap-2">
                    <button
                      type="button"
                      disabled={saving || keyInput.trim().length === 0}
                      onClick={() => void handleSave()}
                      className="rounded-lg bg-[var(--accent)] px-3 py-1.5 text-xs font-medium text-black transition hover:brightness-110 active:scale-95 disabled:opacity-30"
                    >
                      {saving ? 'Saving...' : 'Save key'}
                    </button>
                    {zenKey && (
                      <button
                        type="button"
                        disabled={saving}
                        onClick={() => { setEditing(false); setKeyInput(''); setSaveError(null); }}
                        className="rounded-lg border border-white/10 px-3 py-1.5 text-xs text-[var(--muted)] transition hover:text-white"
                      >
                        Cancel
                      </button>
                    )}
                  </div>
                  {saveError && <p className="text-xs text-red-400" role="alert">{saveError}</p>}
                </div>
              )}
            </div>
          )}
        </div>
      </section>
    </div>
  );
}
