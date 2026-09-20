import { useCallback, useEffect, useState } from 'react';
import {
  AlertTriangle,
  Check,
  Copy,
  Loader2,
  Plus,
  ShieldCheck,
  ToggleLeft,
  ToggleRight,
  Trash2,
  Users,
  X,
} from 'lucide-react';
import {
  CortexApiError,
  createAdminPromoCode,
  deleteAdminPromoCode,
  getAdminPromoCodes,
  getAdminRedemptions,
  updateAdminPromoCode,
  type CodeRedemption,
  type DiscountOption,
  type PromoCode,
} from '../../lib/cortexApi';

type Tab = 'codes' | 'redemptions';

const DISCOUNT_TYPE_LABELS: Record<string, string> = {
  trial_extension: 'Trial extension',
  percent_off: 'Percent off',
  free_trial: 'Free trial',
};

function formatDate(iso: string | null) {
  if (!iso) return '--';
  return new Date(iso).toLocaleDateString('en-US', {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
  });
}

function formatDiscountValue(type: string, value: number) {
  if (type === 'percent_off') return `${value}%`;
  if (type === 'trial_extension') return `${value} days`;
  if (type === 'free_trial') return `${value} days`;
  return String(value);
}

export default function PromoCodeManager() {
  const [tab, setTab] = useState<Tab>('codes');
  const [codes, setCodes] = useState<PromoCode[]>([]);
  const [redemptions, setRedemptions] = useState<CodeRedemption[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showCreate, setShowCreate] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [redemptionFilter, setRedemptionFilter] = useState('');
  const [copiedCode, setCopiedCode] = useState<string | null>(null);

  const fetchCodes = useCallback(async () => {
    try {
      const result = await getAdminPromoCodes();
      setCodes(result.codes);
      setError(null);
    } catch (err) {
      if (err instanceof CortexApiError && (err.status === 403 || err.status === 401)) {
        setError('Admin access required');
      } else {
        setError(err instanceof Error ? err.message : 'Failed to load promo codes');
      }
    } finally {
      setLoading(false);
    }
  }, []);

  const fetchRedemptions = useCallback(async () => {
    try {
      const result = await getAdminRedemptions(redemptionFilter || undefined);
      setRedemptions(result.redemptions);
    } catch {
      // silent
    }
  }, [redemptionFilter]);

  useEffect(() => {
    void fetchCodes();
  }, [fetchCodes]);

  useEffect(() => {
    if (tab === 'redemptions') void fetchRedemptions();
  }, [tab, fetchRedemptions]);

  const handleToggleActive = async (code: PromoCode) => {
    await updateAdminPromoCode(code.id, { active: !code.active });
    setCodes((prev) =>
      prev.map((c) => (c.id === code.id ? { ...c, active: !c.active } : c)),
    );
  };

  const handleDelete = async (id: string) => {
    await deleteAdminPromoCode(id);
    setCodes((prev) => prev.filter((c) => c.id !== id));
    setConfirmDelete(null);
  };

  const handleCopyCode = (code: string) => {
    navigator.clipboard.writeText(code);
    setCopiedCode(code);
    setTimeout(() => setCopiedCode(null), 2000);
  };

  if (loading) {
    return (
      <div className="flex items-center justify-center py-12">
        <Loader2 className="h-5 w-5 animate-spin text-[var(--muted)]" />
      </div>
    );
  }

  if (error) {
    return (
      <div className="rounded-xl border border-red-400/15 bg-red-400/8 p-4">
        <div className="flex items-center gap-2 text-sm text-red-100">
          <AlertTriangle className="h-4 w-4" />
          {error}
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          <ShieldCheck className="h-5 w-5 text-[var(--accent)]" />
          <h2 className="text-base font-semibold text-white">Promo Codes</h2>
        </div>
        <button
          type="button"
          onClick={() => setShowCreate(true)}
          className="flex items-center gap-1.5 rounded-lg bg-[var(--accent)] px-3 py-1.5 text-xs font-semibold text-black transition hover:brightness-110 active:scale-95"
        >
          <Plus className="h-3.5 w-3.5" />
          Create code
        </button>
      </div>

      {/* Tabs */}
      <div className="flex gap-1 rounded-xl border border-white/8 bg-white/[0.03] p-1">
        {(['codes', 'redemptions'] as Tab[]).map((t) => (
          <button
            key={t}
            type="button"
            onClick={() => setTab(t)}
            className={`flex-1 rounded-lg px-3 py-1.5 text-xs font-medium capitalize transition ${
              tab === t
                ? 'bg-white/10 text-white'
                : 'text-[var(--muted)] hover:text-white'
            }`}
          >
            {t}
          </button>
        ))}
      </div>

      {showCreate && (
        <CreateCodeForm
          onCreated={(code) => {
            setCodes((prev) => [code, ...prev]);
            setShowCreate(false);
          }}
          onCancel={() => setShowCreate(false)}
        />
      )}

      {tab === 'codes' && (
        <div className="space-y-2">
          {codes.length === 0 ? (
            <p className="py-8 text-center text-sm text-[var(--muted)]">
              No promo codes yet. Create your first one above.
            </p>
          ) : (
            codes.map((code) => (
              <div
                key={code.id}
                className={`rounded-xl border bg-white/[0.02] p-3 transition ${
                  code.active ? 'border-white/8' : 'border-white/5 opacity-60'
                }`}
              >
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <button
                        type="button"
                        onClick={() => handleCopyCode(code.code)}
                        className="flex items-center gap-1.5 rounded-lg bg-white/8 px-2.5 py-1 font-mono text-sm font-bold text-white transition hover:bg-white/12"
                      >
                        {code.code}
                        {copiedCode === code.code ? (
                          <Check className="h-3 w-3 text-emerald-300" />
                        ) : (
                          <Copy className="h-3 w-3 text-[var(--muted)]" />
                        )}
                      </button>
                      <span
                        className={`rounded-full px-2 py-0.5 text-[10px] font-medium uppercase ${
                          code.active
                            ? 'bg-emerald-500/15 text-emerald-300'
                            : 'bg-white/8 text-[var(--muted)]'
                        }`}
                      >
                        {code.active ? 'Active' : 'Disabled'}
                      </span>
                    </div>
                    <div className="mt-2 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-[var(--muted)]">
                      <span>{DISCOUNT_TYPE_LABELS[code.discount_type] ?? code.discount_type}</span>
                      <span className="font-medium text-[var(--muted-strong)]">
                        {formatDiscountValue(code.discount_type, code.discount_value)}
                      </span>
                      <span className="flex items-center gap-1">
                        <Users className="h-3 w-3" />
                        {code.current_uses}/{code.max_uses} used
                      </span>
                      {code.expires_at && (
                        <span>Expires {formatDate(code.expires_at)}</span>
                      )}
                    </div>
                    {code.description && (
                      <p className="mt-1.5 text-xs text-[var(--muted)]">{code.description}</p>
                    )}
                  </div>
                  <div className="flex shrink-0 items-center gap-1">
                    <button
                      type="button"
                      onClick={() => void handleToggleActive(code)}
                      title={code.active ? 'Disable' : 'Enable'}
                      className="rounded-lg p-1.5 text-[var(--muted)] transition hover:bg-white/8 hover:text-white"
                    >
                      {code.active ? (
                        <ToggleRight className="h-4 w-4 text-emerald-300" />
                      ) : (
                        <ToggleLeft className="h-4 w-4" />
                      )}
                    </button>
                    {confirmDelete === code.id ? (
                      <div className="flex items-center gap-1">
                        <button
                          type="button"
                          onClick={() => void handleDelete(code.id)}
                          className="rounded-lg bg-red-500/20 px-2 py-1 text-[10px] font-medium text-red-200 transition hover:bg-red-500/30"
                        >
                          Confirm
                        </button>
                        <button
                          type="button"
                          onClick={() => setConfirmDelete(null)}
                          className="rounded-lg p-1 text-[var(--muted)] transition hover:bg-white/8"
                        >
                          <X className="h-3.5 w-3.5" />
                        </button>
                      </div>
                    ) : (
                      <button
                        type="button"
                        onClick={() => setConfirmDelete(code.id)}
                        className="rounded-lg p-1.5 text-[var(--muted)] transition hover:bg-red-500/12 hover:text-red-300"
                      >
                        <Trash2 className="h-4 w-4" />
                      </button>
                    )}
                  </div>
                </div>
              </div>
            ))
          )}
        </div>
      )}

      {tab === 'redemptions' && (
        <div className="space-y-3">
          <input
            value={redemptionFilter}
            onChange={(e) => setRedemptionFilter(e.target.value.toUpperCase())}
            placeholder="Filter by code..."
            className="w-full rounded-lg border border-white/8 bg-[var(--composer)] px-3 py-2 text-sm text-white outline-none transition focus:border-[var(--accent)]/50"
          />
          {redemptions.length === 0 ? (
            <p className="py-8 text-center text-sm text-[var(--muted)]">
              No redemptions{redemptionFilter ? ` for "${redemptionFilter}"` : ' yet'}.
            </p>
          ) : (
            <div className="divide-y divide-white/5 rounded-xl border border-white/8 bg-white/[0.02]">
              {redemptions.map((r) => (
                <div key={r.id} className="flex items-center justify-between gap-3 px-3 py-2.5">
                  <div className="min-w-0">
                    <span className="rounded bg-white/8 px-1.5 py-0.5 font-mono text-xs font-medium text-white">
                      {r.code}
                    </span>
                    <span className="ml-2 text-xs text-[var(--muted)]">
                      {r.user_id.slice(0, 16)}...
                    </span>
                  </div>
                  <span className="shrink-0 text-xs text-[var(--muted)]">
                    {formatDate(r.redeemed_at)}
                  </span>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

interface TagDraft {
  type: string;
  value: number;
}

const AVAILABLE_TYPES = [
  { key: 'trial_extension', label: 'Trial extension', unit: 'days', defaultValue: 14 },
  { key: 'percent_off', label: 'Percent off', unit: '%', defaultValue: 25 },
  { key: 'free_trial', label: 'Free trial', unit: 'days', defaultValue: 14 },
] as const;

function CreateCodeForm({
  onCreated,
  onCancel,
}: {
  onCreated: (code: PromoCode) => void;
  onCancel: () => void;
}) {
  const [code, setCode] = useState('');
  const [tags, setTags] = useState<TagDraft[]>([{ type: 'trial_extension', value: 14 }]);
  const [maxUses, setMaxUses] = useState(25);
  const [expiresAt, setExpiresAt] = useState('');
  const [description, setDescription] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  function addTag(typeKey: string) {
    const def = AVAILABLE_TYPES.find((t) => t.key === typeKey);
    if (!def) return;
    if (tags.some((t) => t.type === typeKey)) return;
    setTags((prev) => [...prev, { type: typeKey, value: def.defaultValue }]);
  }

  function removeTag(typeKey: string) {
    setTags((prev) => prev.filter((t) => t.type !== typeKey));
  }

  function updateTagValue(typeKey: string, value: number) {
    setTags((prev) => prev.map((t) => t.type === typeKey ? { ...t, value } : t));
  }

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (tags.length === 0) { setError('Add at least one discount type'); return; }
    setSaving(true);
    setError(null);
    try {
      const primary = tags[0];
      const discountOptions: DiscountOption[] | undefined = tags.length > 1
        ? tags.map((t) => {
            const def = AVAILABLE_TYPES.find((a) => a.key === t.type);
            return { label: `${t.value}${def?.unit === '%' ? '%' : ` ${def?.unit ?? ''}`} ${def?.label?.toLowerCase() ?? t.type}`, discount_type: t.type, discount_value: t.value };
          })
        : undefined;

      const result = await createAdminPromoCode({
        code: code.trim().toUpperCase(),
        discount_type: primary.type,
        discount_value: primary.value,
        max_uses: maxUses,
        expires_at: expiresAt || undefined,
        description: description.trim() || undefined,
        discount_options: discountOptions,
      });
      onCreated(result);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to create code');
    } finally {
      setSaving(false);
    }
  };

  const unusedTypes = AVAILABLE_TYPES.filter((t) => !tags.some((tag) => tag.type === t.key));

  return (
    <form
      onSubmit={(e) => void handleSubmit(e)}
      className="space-y-3 rounded-xl border border-[var(--accent)]/20 bg-[var(--accent)]/5 p-4"
    >
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-medium text-white">New promo code</h3>
        <button
          type="button"
          onClick={onCancel}
          className="rounded-lg p-1 text-[var(--muted)] transition hover:bg-white/8 hover:text-white"
        >
          <X className="h-4 w-4" />
        </button>
      </div>

      <div className="grid gap-3 sm:grid-cols-2">
        <div>
          <label className="mb-1 block text-[11px] font-medium uppercase tracking-wider text-[var(--muted)]">
            Code
          </label>
          <input
            value={code}
            onChange={(e) => setCode(e.target.value.toUpperCase().replace(/[^A-Z0-9_-]/g, ''))}
            placeholder="e.g. LAUNCH2025"
            maxLength={32}
            required
            className="w-full rounded-lg border border-white/8 bg-[var(--composer)] px-3 py-2 font-mono text-sm text-white outline-none transition focus:border-[var(--accent)]/50"
          />
        </div>
        <div>
          <label className="mb-1 block text-[11px] font-medium uppercase tracking-wider text-[var(--muted)]">
            Max uses
          </label>
          <input
            type="number"
            value={maxUses}
            onChange={(e) => setMaxUses(Number(e.target.value))}
            min={1}
            max={10000}
            required
            className="w-full rounded-lg border border-white/8 bg-[var(--composer)] px-3 py-2 text-sm text-white outline-none transition focus:border-[var(--accent)]/50"
          />
        </div>
        <div>
          <label className="mb-1 block text-[11px] font-medium uppercase tracking-wider text-[var(--muted)]">
            Expires (optional)
          </label>
          <input
            type="date"
            value={expiresAt}
            onChange={(e) => setExpiresAt(e.target.value)}
            className="w-full rounded-lg border border-white/8 bg-[var(--composer)] px-3 py-2 text-sm text-white outline-none transition focus:border-[var(--accent)]/50"
          />
        </div>
        <div>
          <label className="mb-1 block text-[11px] font-medium uppercase tracking-wider text-[var(--muted)]">
            Description (optional)
          </label>
          <input
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="Launch promo"
            className="w-full rounded-lg border border-white/8 bg-[var(--composer)] px-3 py-2 text-sm text-white outline-none transition focus:border-[var(--accent)]/50"
          />
        </div>
      </div>

      {/* Discount type tags */}
      <div>
        <label className="mb-2 block text-[11px] font-medium uppercase tracking-wider text-[var(--muted)]">
          Discount types
        </label>
        <div className="space-y-2">
          {tags.map((tag) => {
            const def = AVAILABLE_TYPES.find((t) => t.key === tag.type);
            return (
              <div
                key={tag.type}
                className="flex items-center gap-2 rounded-lg border border-[var(--accent)]/20 bg-[var(--accent)]/8 px-3 py-2"
              >
                <span className="text-xs font-medium text-white">{def?.label ?? tag.type}</span>
                <input
                  type="number"
                  value={tag.value}
                  onChange={(e) => updateTagValue(tag.type, Number(e.target.value))}
                  min={1}
                  max={tag.type === 'percent_off' ? 100 : 365}
                  className="w-16 rounded border border-white/10 bg-[var(--composer)] px-2 py-1 text-xs text-white outline-none transition focus:border-[var(--accent)]/50"
                />
                <span className="text-[10px] text-[var(--muted)]">{def?.unit}</span>
                <button
                  type="button"
                  onClick={() => removeTag(tag.type)}
                  className="ml-auto rounded p-1 text-[var(--muted)] transition hover:bg-red-500/12 hover:text-red-300"
                >
                  <X className="h-3 w-3" />
                </button>
              </div>
            );
          })}
        </div>
        {unusedTypes.length > 0 && (
          <div className="mt-2 flex flex-wrap gap-1.5">
            {unusedTypes.map((t) => (
              <button
                key={t.key}
                type="button"
                onClick={() => addTag(t.key)}
                className="flex items-center gap-1 rounded-full border border-dashed border-white/12 px-2.5 py-1 text-[10px] text-[var(--muted)] transition hover:border-white/25 hover:text-white"
              >
                <Plus className="h-2.5 w-2.5" /> {t.label}
              </button>
            ))}
          </div>
        )}
      </div>

      {error && (
        <p className="rounded-lg border border-red-400/15 bg-red-400/8 px-3 py-2 text-xs text-red-100">
          {error}
        </p>
      )}

      <div className="flex justify-end gap-2">
        <button
          type="button"
          onClick={onCancel}
          className="rounded-lg border border-white/8 px-3 py-1.5 text-xs text-[var(--muted-strong)] transition hover:bg-white/6"
        >
          Cancel
        </button>
        <button
          type="submit"
          disabled={saving || !code.trim()}
          className="flex items-center gap-1.5 rounded-lg bg-[var(--accent)] px-4 py-1.5 text-xs font-semibold text-black transition hover:brightness-110 active:scale-95 disabled:opacity-50"
        >
          {saving ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Plus className="h-3.5 w-3.5" />}
          Create
        </button>
      </div>
    </form>
  );
}

