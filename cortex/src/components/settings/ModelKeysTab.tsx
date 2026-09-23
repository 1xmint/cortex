import { useCallback, useEffect, useState } from 'react';
import { AlertTriangle, KeyRound, Loader2, Trash2 } from 'lucide-react';
import {
  CortexApiError,
  deleteProviderKeyAllDevices,
  deleteProviderKeyDevice,
  getProviderKeys,
  saveProviderKey,
  type ProviderKeySummary,
} from '../../lib/cortexApi';
import { useAuthGate } from '../../lib/useAuthGate';
import {
  clearZenDeviceKey,
  generateZenDeviceKey,
  loadZenDeviceKey,
  saveZenDeviceKey,
} from '../../lib/zenDeviceKey';

function formatDate(ms: number | null | undefined): string {
  if (!ms) return 'never';
  return new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', year: 'numeric' }).format(new Date(ms));
}

/** `sk-abcd1234` -> `••••1234`. The API never returns more than `last4`. */
function maskedKey(last4: string): string {
  return `••••${last4}`;
}

export default function ModelKeysTab() {
  const { userId } = useAuthGate();
  const [keys, setKeys] = useState<ProviderKeySummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);

  // Form state -- only present while adding or replacing a key. The input
  // value itself is never kept anywhere else (no draft state, no localStorage).
  const [editing, setEditing] = useState(false);
  const [keyInput, setKeyInput] = useState('');
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const [confirmingDeleteDevice, setConfirmingDeleteDevice] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [confirmingDeleteAll, setConfirmingDeleteAll] = useState(false);

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

  const zenKeys = keys.filter((k) => k.provider === 'zen');
  const localDeviceKey = loadZenDeviceKey(userId);
  const thisDeviceEntry = localDeviceKey
    ? zenKeys.find((k) => k.device_id === localDeviceKey.deviceId) ?? null
    : null;
  const otherDeviceEntries = zenKeys.filter((k) => k.device_id !== localDeviceKey?.deviceId);

  async function handleSave() {
    const value = keyInput;
    setSaving(true);
    setSaveError(null);
    try {
      // Reuse this device's existing deviceId when there is one -- only the
      // secret is fresh per save. Reusing the id makes "Replace" overwrite
      // this device's row on the server instead of creating a new one,
      // which would otherwise pile up rows toward the 10-device cap. A new
      // deviceId is only generated the first time this browser saves a key.
      const existing = loadZenDeviceKey(userId);
      const fresh = generateZenDeviceKey();
      const toSave = existing ? { deviceId: existing.deviceId, secret: fresh.secret } : fresh;
      await saveProviderKey('zen', value, toSave.deviceId, toSave.secret);
      // Only persist the device key locally once the server has accepted
      // it -- a failed save must not leave this browser believing it has a
      // working key the server never stored, and must not strand the old
      // key if this was a replace.
      saveZenDeviceKey(userId, toSave);
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

  async function handleDeleteOtherDevice(deviceId: string) {
    setDeleting(true);
    setDeleteError(null);
    try {
      await deleteProviderKeyDevice('zen', deviceId);
      setConfirmingDeleteDevice(null);
      await refresh();
    } catch (err) {
      setDeleteError(err instanceof Error ? err.message : 'Could not delete key');
    } finally {
      setDeleting(false);
    }
  }

  async function handleDeleteThisDevice(deviceId: string) {
    setDeleting(true);
    setDeleteError(null);
    try {
      await deleteProviderKeyDevice('zen', deviceId);
      if (localDeviceKey?.deviceId === deviceId) clearZenDeviceKey(userId);
      setConfirmingDeleteDevice(null);
      await refresh();
    } catch (err) {
      setDeleteError(err instanceof Error ? err.message : 'Could not delete key');
    } finally {
      setDeleting(false);
    }
  }

  async function handleDeleteAll() {
    setDeleting(true);
    setDeleteError(null);
    try {
      await deleteProviderKeyAllDevices('zen');
      clearZenDeviceKey(userId);
      setConfirmingDeleteAll(false);
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
          <p className="mt-2 text-xs text-[var(--muted)]">
            Your key is locked with a code kept only in this browser. It works only on devices
            where you entered it.
          </p>

          {loading ? (
            <div className="mt-3 flex items-center gap-2 text-xs text-[var(--muted)]">
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
              Loading...
            </div>
          ) : loadError ? (
            <p className="mt-3 text-xs text-red-400" role="alert">{loadError}</p>
          ) : (
            <div className="mt-3 flex flex-col gap-3">
              {zenKeys.length > 0 && !editing ? (
                <div className="flex flex-col gap-2">
                  {thisDeviceEntry && (
                    <div className="flex flex-col gap-1 rounded-lg border border-white/8 bg-white/[0.03] px-3 py-2">
                      <p className="text-[10px] font-semibold uppercase tracking-wider text-[var(--muted)]">
                        This device
                      </p>
                      {thisDeviceEntry.status === 'rejected' ? (
                        <p className="text-xs font-medium text-red-400" role="alert">
                          Zen rejected this key
                        </p>
                      ) : (
                        <p className="text-sm text-white">
                          Zen key {maskedKey(thisDeviceEntry.last4)} · added{' '}
                          {formatDate(thisDeviceEntry.created_at)} · last used{' '}
                          {formatDate(thisDeviceEntry.last_used_at)}
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
                        {confirmingDeleteDevice !== thisDeviceEntry.device_id ? (
                          <button
                            type="button"
                            onClick={() => { setConfirmingDeleteDevice(thisDeviceEntry.device_id); setDeleteError(null); }}
                            className="inline-flex items-center gap-1.5 rounded-lg border border-red-500/40 bg-red-500/10 px-3 py-1.5 text-xs font-medium text-red-300 transition hover:bg-red-500/20 active:scale-95"
                          >
                            <Trash2 className="h-3.5 w-3.5" />
                            Remove from this device
                          </button>
                        ) : null}
                      </div>
                      {confirmingDeleteDevice === thisDeviceEntry.device_id && (
                        <div className="flex flex-col gap-2 rounded-lg border border-red-500/15 bg-red-500/5 p-3">
                          <p className="text-xs text-red-200 flex items-center gap-1.5">
                            <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
                            Remove the Zen key from this device? Chats using Zen models will stop
                            working on this device until you add a new one.
                          </p>
                          <div className="flex gap-2">
                            <button
                              type="button"
                              onClick={() => setConfirmingDeleteDevice(null)}
                              className="rounded-lg border border-white/10 px-3 py-1.5 text-xs text-[var(--muted)] transition hover:text-white"
                            >
                              Cancel
                            </button>
                            <button
                              type="button"
                              disabled={deleting}
                              onClick={() => void handleDeleteThisDevice(thisDeviceEntry.device_id)}
                              className="rounded-lg bg-red-500 px-3 py-1.5 text-xs font-medium text-white transition hover:bg-red-400 active:scale-95 disabled:opacity-30"
                            >
                              {deleting ? 'Removing...' : 'Confirm remove'}
                            </button>
                          </div>
                        </div>
                      )}
                    </div>
                  )}

                  {otherDeviceEntries.length > 0 && (
                    <div className="flex flex-col gap-1 rounded-lg border border-white/8 bg-white/[0.03] px-3 py-2">
                      <p className="text-[10px] font-semibold uppercase tracking-wider text-[var(--muted)]">
                        Other devices
                      </p>
                      {otherDeviceEntries.map((entry) => (
                        <div key={entry.device_id} className="flex items-center justify-between gap-2">
                          <p className="text-xs text-[var(--muted)]">
                            Zen key {maskedKey(entry.last4)}
                            {entry.status === 'rejected' ? ' · rejected' : ''} · added{' '}
                            {formatDate(entry.created_at)}
                          </p>
                          <button
                            type="button"
                            disabled={deleting}
                            onClick={() => void handleDeleteOtherDevice(entry.device_id)}
                            aria-label={`Remove device ${maskedKey(entry.last4)}`}
                            className="inline-flex shrink-0 items-center gap-1 rounded-lg border border-red-500/40 bg-red-500/10 px-2 py-1 text-[10px] font-medium text-red-300 transition hover:bg-red-500/20 active:scale-95 disabled:opacity-30"
                          >
                            <Trash2 className="h-3 w-3" />
                            Remove
                          </button>
                        </div>
                      ))}
                    </div>
                  )}

                  {!thisDeviceEntry && (
                    <button
                      type="button"
                      onClick={() => { setEditing(true); setSaveError(null); }}
                      className="self-start rounded-lg bg-[var(--accent)] px-3 py-1.5 text-xs font-medium text-black transition hover:brightness-110 active:scale-95"
                    >
                      Add a key on this device
                    </button>
                  )}

                  {!confirmingDeleteAll ? (
                    <button
                      type="button"
                      onClick={() => { setConfirmingDeleteAll(true); setDeleteError(null); }}
                      className="self-start text-xs text-red-300 underline underline-offset-2 hover:text-red-200"
                    >
                      Remove from all devices
                    </button>
                  ) : (
                    <div className="flex flex-col gap-2 rounded-lg border border-red-500/15 bg-red-500/5 p-3">
                      <p className="text-xs text-red-200 flex items-center gap-1.5">
                        <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
                        Remove the Zen key from every device? Chats using Zen models will stop
                        working everywhere until keys are added again.
                      </p>
                      <div className="flex gap-2">
                        <button
                          type="button"
                          onClick={() => setConfirmingDeleteAll(false)}
                          className="rounded-lg border border-white/10 px-3 py-1.5 text-xs text-[var(--muted)] transition hover:text-white"
                        >
                          Cancel
                        </button>
                        <button
                          type="button"
                          disabled={deleting}
                          onClick={() => void handleDeleteAll()}
                          className="rounded-lg bg-red-500 px-3 py-1.5 text-xs font-medium text-white transition hover:bg-red-400 active:scale-95 disabled:opacity-30"
                        >
                          {deleting ? 'Removing...' : 'Confirm remove from all devices'}
                        </button>
                      </div>
                    </div>
                  )}
                  {deleteError && <p className="text-xs text-red-400" role="alert">{deleteError}</p>}
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
                    {zenKeys.length > 0 && (
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
