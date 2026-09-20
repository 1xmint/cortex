import { useCallback, useEffect, useState } from 'react';
import {
  ChevronDown,
  ChevronRight,
  Loader2,
  RefreshCw,
  Send,
  Shield,
  ShieldAlert,
  ShieldCheck,
  X,
} from 'lucide-react';
import {
  delegateAuthority,
  getAuthorityScopes,
  type CortexAuthorityScope,
} from '../../lib/cortexApi';

const ROLE_COLORS: Record<string, string> = {
  owner: 'border-amber-300/30 bg-amber-400/15 text-amber-100',
  admin: 'border-sky-300/30 bg-sky-400/15 text-sky-100',
  member: 'border-emerald-300/30 bg-emerald-400/15 text-emerald-100',
  viewer: 'border-zinc-400/30 bg-zinc-500/15 text-zinc-200',
};

const ACCESS_COLORS: Record<string, string> = {
  admin: 'text-amber-200',
  write: 'text-sky-200',
  read: 'text-zinc-300',
};

function RoleBadge({ role }: { role: string }) {
  const colors = ROLE_COLORS[role] ?? ROLE_COLORS.viewer;
  return (
    <span className={`inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-[10px] font-medium ${colors}`}>
      {role === 'owner' ? <ShieldAlert className="h-2.5 w-2.5" /> : <Shield className="h-2.5 w-2.5" />}
      {role}
    </span>
  );
}

function AccessBadge({ access }: { access: string }) {
  const color = ACCESS_COLORS[access] ?? 'text-zinc-400';
  return (
    <span className={`text-[10px] font-medium ${color}`}>{access}</span>
  );
}

function DelegationForm({
  scopeId,
  onClose,
  onSuccess,
}: {
  scopeId: string;
  onClose: () => void;
  onSuccess: () => void;
}) {
  const [subjectDid, setSubjectDid] = useState('');
  const [role, setRole] = useState<'admin' | 'member' | 'viewer'>('member');
  const [reason, setReason] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!subjectDid.trim()) return;
    setSubmitting(true);
    setError(null);
    try {
      await delegateAuthority({
        scope_id: scopeId,
        subject_did: subjectDid.trim(),
        role,
        reason: reason.trim() || undefined,
      });
      onSuccess();
      onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Delegation failed');
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <form onSubmit={(e) => void handleSubmit(e)} className="mt-3 space-y-3 rounded-lg border border-sky-300/15 bg-sky-400/[0.04] p-3">
      <div className="flex items-center justify-between">
        <p className="text-xs font-medium text-sky-100">Delegate Authority</p>
        <button
          type="button"
          onClick={onClose}
          className="rounded p-0.5 text-zinc-400 transition hover:text-white"
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </div>

      <div>
        <label className="block text-[10px] uppercase tracking-wider text-zinc-400">Subject DID</label>
        <input
          type="text"
          value={subjectDid}
          onChange={(e) => setSubjectDid(e.target.value)}
          placeholder="did:soma:..."
          className="mt-1 w-full rounded-md border border-zinc-700 bg-zinc-900 px-3 py-1.5 text-xs text-zinc-100 placeholder-zinc-500 outline-none focus:border-sky-500"
        />
      </div>

      <div>
        <label className="block text-[10px] uppercase tracking-wider text-zinc-400">Role</label>
        <select
          value={role}
          onChange={(e) => setRole(e.target.value as 'admin' | 'member' | 'viewer')}
          className="mt-1 w-full rounded-md border border-zinc-700 bg-zinc-900 px-3 py-1.5 text-xs text-zinc-100 outline-none focus:border-sky-500"
        >
          <option value="admin">Admin</option>
          <option value="member">Member</option>
          <option value="viewer">Viewer</option>
        </select>
      </div>

      <div>
        <label className="block text-[10px] uppercase tracking-wider text-zinc-400">Reason (optional)</label>
        <input
          type="text"
          value={reason}
          onChange={(e) => setReason(e.target.value)}
          placeholder="Why this delegation?"
          className="mt-1 w-full rounded-md border border-zinc-700 bg-zinc-900 px-3 py-1.5 text-xs text-zinc-100 placeholder-zinc-500 outline-none focus:border-sky-500"
        />
      </div>

      {error && (
        <p className="text-[11px] text-red-300">{error}</p>
      )}

      <button
        type="submit"
        disabled={submitting || !subjectDid.trim()}
        className="inline-flex items-center gap-1.5 rounded-md border border-sky-400/30 bg-sky-500/20 px-3 py-1.5 text-xs font-medium text-sky-100 transition hover:bg-sky-500/30 disabled:opacity-50"
      >
        {submitting ? <Loader2 className="h-3 w-3 animate-spin" /> : <Send className="h-3 w-3" />}
        Delegate
      </button>
    </form>
  );
}

function ScopeCard({
  scope,
  onDelegated,
}: {
  scope: CortexAuthorityScope;
  onDelegated: () => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const [showDelegate, setShowDelegate] = useState(false);

  return (
    <div className="rounded-lg border border-zinc-700 bg-zinc-900 px-4 py-3">
      <div className="flex items-start justify-between gap-3">
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          className="flex min-w-0 flex-1 items-start gap-2 text-left"
        >
          <div className="mt-0.5 shrink-0 text-zinc-400">
            {expanded ? <ChevronDown className="h-3.5 w-3.5" /> : <ChevronRight className="h-3.5 w-3.5" />}
          </div>
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <p className="truncate text-sm font-medium text-zinc-100">{scope.name}</p>
              <RoleBadge role={scope.role} />
              <span className={`rounded-full border px-1.5 py-0.5 text-[9px] ${
                scope.status === 'active'
                  ? 'border-emerald-300/20 bg-emerald-400/10 text-emerald-200'
                  : 'border-zinc-600 bg-zinc-800 text-zinc-400'
              }`}>
                {scope.status}
              </span>
            </div>
            <p className="mt-0.5 text-[11px] text-zinc-400">{scope.description}</p>
            <p className="mt-0.5 text-[10px] text-zinc-500">
              {scope.kind} scope · {scope.resources.length} resource{scope.resources.length === 1 ? '' : 's'}
            </p>
          </div>
        </button>
        <button
          type="button"
          onClick={() => setShowDelegate((v) => !v)}
          className="inline-flex shrink-0 items-center gap-1 rounded-md border border-zinc-700 bg-zinc-800 px-2.5 py-1 text-[11px] font-medium text-zinc-300 transition hover:border-sky-500/40 hover:bg-sky-500/10 hover:text-sky-200"
        >
          <ShieldCheck className="h-3 w-3" />
          Delegate
        </button>
      </div>

      {expanded && scope.resources.length > 0 && (
        <div className="mt-3 space-y-1.5 border-t border-zinc-800 pt-3">
          <p className="text-[10px] uppercase tracking-wider text-zinc-500">Resources</p>
          {scope.resources.map((resource) => (
            <div
              key={resource.id}
              className="flex items-center justify-between rounded-md border border-zinc-800 bg-zinc-950 px-3 py-1.5"
            >
              <div className="min-w-0">
                <p className="truncate text-xs text-zinc-200">
                  <span className="text-zinc-500">{resource.resource_type}:</span>{' '}
                  {resource.resource_key}
                </p>
              </div>
              <AccessBadge access={resource.access} />
            </div>
          ))}
        </div>
      )}

      {showDelegate && (
        <DelegationForm
          scopeId={scope.id}
          onClose={() => setShowDelegate(false)}
          onSuccess={onDelegated}
        />
      )}
    </div>
  );
}

export default function AuthorityScopesPanel() {
  const [scopes, setScopes] = useState<CortexAuthorityScope[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const response = await getAuthorityScopes();
      setScopes(response.scopes);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load authority scopes');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return (
    <div>
      <div className="mb-3 flex items-center justify-between">
        <button
          type="button"
          onClick={() => setCollapsed((v) => !v)}
          className="flex items-center gap-2"
        >
          <div className="text-zinc-400">
            {collapsed ? <ChevronRight className="h-4 w-4" /> : <ChevronDown className="h-4 w-4" />}
          </div>
          <Shield className="h-4 w-4 text-sky-300" />
          <h2 className="text-sm font-semibold text-white">
            Authority Scopes ({scopes.length})
          </h2>
        </button>
        <button
          type="button"
          onClick={() => void refresh()}
          className="inline-flex h-7 w-7 items-center justify-center rounded-md border border-zinc-700 bg-zinc-800 text-zinc-400 transition hover:text-white"
          aria-label="Refresh scopes"
        >
          <RefreshCw className={`h-3 w-3 ${loading ? 'animate-spin' : ''}`} />
        </button>
      </div>

      {collapsed ? null : loading && scopes.length === 0 ? (
        <div className="flex h-24 items-center justify-center rounded-lg border border-zinc-700 bg-zinc-900/50 text-sm text-zinc-400">
          <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          Loading scopes...
        </div>
      ) : error && scopes.length === 0 ? (
        <div className="flex h-24 items-center justify-center rounded-lg border border-zinc-700 bg-zinc-900/50 text-xs text-zinc-400">
          {error}
        </div>
      ) : scopes.length === 0 ? (
        <div className="flex h-24 items-center justify-center rounded-lg border border-zinc-700 bg-zinc-900/50 text-sm text-zinc-400">
          No authority scopes configured
        </div>
      ) : (
        <div className="space-y-2">
          {scopes.map((scope) => (
            <ScopeCard
              key={scope.id}
              scope={scope}
              onDelegated={() => void refresh()}
            />
          ))}
        </div>
      )}
    </div>
  );
}
