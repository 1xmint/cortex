import { useCallback, useEffect, useState } from 'react';
import { AlertTriangle, CheckCircle, ExternalLink, Key, Loader2, Star, Trash2, User, X, XCircle } from 'lucide-react';
import { useUser } from '@clerk/clerk-react';
import {
  getAuthStatus,
  startAuth,
  submitAuthCode,
  deleteCredential,
  setDefaultCredential,
  assignCredential,
  listCredentialAssignments,
  removeCredentialAssignment,
  SOMA_API_ENABLED,
  type BillingStatus,
  type ProviderAuthInfo,
  type CredentialAssignment,
} from '../lib/cortexApi';
import BillingPage from './billing/BillingPage';
import SpendDashboard from './spend/SpendDashboard';
import IntegrationSetup from './integrations/IntegrationSetup';
import BudgetSettings from './settings/BudgetSettings';
import type { RunProfile } from '../types';
import { getBudgetSettings, updateBudgetSettings, getCurrentUsage, type BudgetSettings as BudgetSettingsType, type UsageData } from '../lib/cortexApi';

type SettingsTab = 'providers' | 'integrations' | 'spend' | 'billing' | 'budget' | 'notifications' | 'account' | 'apikeys' | 'credentials';

const RUN_PROFILE_LABELS: Record<RunProfile, string> = {
  auto: 'Auto (adaptive)',
  balanced: 'Balanced',
  cost_saver: 'Cost saver',
  quality_first: 'Quality first',
};

const RUN_PROFILE_DESCRIPTIONS: Record<RunProfile, string> = {
  auto: 'Adapts routing based on task risk, provider health, and past outcomes. Recommended for most users.',
  balanced: 'Uses the best model per tier with normal budgets. Good general-purpose option.',
  cost_saver: 'Prefers cheaper models and lower budgets. Skips GPT for non-critical tasks.',
  quality_first: 'Uses dual-brain review for medium+ risk tasks. Higher budgets, stricter quality gates.',
};

const WORKSPACE_PROFILE_KEY = 'cortex:default-run-profile';

function readDefaultProfile(): RunProfile {
  try {
    const raw = window.localStorage.getItem(WORKSPACE_PROFILE_KEY);
    if (raw === 'auto' || raw === 'balanced' || raw === 'cost_saver' || raw === 'quality_first') {
      return raw;
    }
  } catch { /* ignore */ }
  return 'auto';
}

function saveDefaultProfile(profile: RunProfile) {
  try {
    window.localStorage.setItem(WORKSPACE_PROFILE_KEY, profile);
  } catch { /* ignore */ }
}

// TODO: Replace with real DELETE /api/account endpoint when backend supports it.
// For now, direct users to support for GDPR-compliant account deletion.

interface NotificationPrefs {
  taskCompletions: boolean;
  runFailures: boolean;
  teamActivity: boolean;
  systemUpdates: boolean;
}

const NOTIFICATION_PREFS_KEY = 'cortex:notification-prefs';

const DEFAULT_NOTIFICATION_PREFS: NotificationPrefs = {
  taskCompletions: true,
  runFailures: true,
  teamActivity: true,
  systemUpdates: false,
};

function readNotificationPrefs(): NotificationPrefs {
  try {
    const raw = window.localStorage.getItem(NOTIFICATION_PREFS_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as Partial<NotificationPrefs>;
      return { ...DEFAULT_NOTIFICATION_PREFS, ...parsed };
    }
  } catch { /* ignore */ }
  return { ...DEFAULT_NOTIFICATION_PREFS };
}

function saveNotificationPrefs(prefs: NotificationPrefs) {
  try {
    window.localStorage.setItem(NOTIFICATION_PREFS_KEY, JSON.stringify(prefs));
  } catch { /* ignore */ }
}

function NotificationsTab() {
  const [prefs, setPrefs] = useState<NotificationPrefs>(readNotificationPrefs);

  const toggle = (key: keyof NotificationPrefs) => {
    const updated = { ...prefs, [key]: !prefs[key] };
    setPrefs(updated);
    saveNotificationPrefs(updated);
  };

  const categories: { key: keyof NotificationPrefs; label: string }[] = [
    { key: 'taskCompletions', label: 'Task completions' },
    { key: 'runFailures', label: 'Run failures' },
    { key: 'teamActivity', label: 'Team activity' },
    { key: 'systemUpdates', label: 'System updates' },
  ];

  return (
    <div className="flex flex-col gap-5">
      <section className="flex flex-col gap-3">
        <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--muted)]">
          Notification preferences
        </h3>
        <div className="rounded-xl border border-white/8 bg-white/[0.02]">
          {categories.map((cat, idx) => (
            <div
              key={cat.key}
              className={`flex items-center justify-between px-4 py-3 ${idx < categories.length - 1 ? 'border-b border-white/6' : ''}`}
            >
              <span className="text-sm text-white">{cat.label}</span>
              <button
                type="button"
                role="switch"
                aria-checked={prefs[cat.key]}
                onClick={() => toggle(cat.key)}
                className={`relative inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full transition-colors duration-200 ${
                  prefs[cat.key] ? 'bg-[var(--accent)]' : 'bg-white/10'
                }`}
              >
                <span
                  className={`inline-block h-3.5 w-3.5 rounded-full bg-white shadow transition-transform duration-200 ${
                    prefs[cat.key] ? 'translate-x-[1.125rem]' : 'translate-x-[0.1875rem]'
                  }`}
                />
              </button>
            </div>
          ))}
        </div>
        <p className="text-[10px] text-[var(--muted)]">
          Preferences are stored locally. Notification delivery depends on backend availability.
        </p>
      </section>
    </div>
  );
}

function AccountTab({ providers }: { providers: ProviderAuthInfo[] }) {
  const { user } = useUser();
  const [defaultProfile, setDefaultProfile] = useState<RunProfile>(readDefaultProfile);
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
  const [deleteInput, setDeleteInput] = useState('');
  const [deleteSubmitted, setDeleteSubmitted] = useState(false);

  const handleProfileChange = (profile: RunProfile) => {
    setDefaultProfile(profile);
    saveDefaultProfile(profile);
  };

  const displayName = user?.fullName ?? user?.username ?? '—';
  const email = user?.primaryEmailAddress?.emailAddress ?? '—';

  return (
    <div className="flex flex-col gap-5">
      {/* Profile section */}
      <section className="flex flex-col gap-3">
        <h3 className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-[var(--muted)]">
          <User className="h-3.5 w-3.5" />
          Profile
        </h3>
        <div className="rounded-xl border border-white/8 bg-white/[0.02] px-4 py-3">
          <div className="flex flex-col gap-3">
            <div>
              <label className="mb-1 block text-xs text-[var(--muted)]">Display name</label>
              <div className="rounded-lg border border-white/8 bg-white/4 px-3 py-2 text-sm text-white">
                {displayName}
              </div>
            </div>
            <div>
              <label className="mb-1 block text-xs text-[var(--muted)]">Email</label>
              <div className="rounded-lg border border-white/8 bg-white/4 px-3 py-2 text-sm text-white">
                {email}
              </div>
            </div>
            <p className="text-[10px] text-[var(--muted)]">
              Profile details are managed by Clerk. Visit your Clerk dashboard to make changes.
            </p>
          </div>
        </div>
      </section>

      {/* Workspace preferences */}
      <section className="flex flex-col gap-3">
        <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--muted)]">
          Workspace preferences
        </h3>
        <div className="rounded-xl border border-white/8 bg-white/[0.02] px-4 py-3">
          <label className="mb-1.5 block text-xs text-[var(--muted)]">Default run profile</label>
          <select
            value={defaultProfile}
            onChange={(e) => handleProfileChange(e.target.value as RunProfile)}
            title="Controls how tasks are routed between AI providers and quality gates"
            className="w-full rounded-lg border border-white/10 bg-[var(--composer)] px-3 py-2 text-sm text-white focus:border-[var(--accent)]/50 focus:outline-none"
          >
            {(Object.keys(RUN_PROFILE_LABELS) as RunProfile[]).map((profile) => (
              <option key={profile} value={profile}>
                {RUN_PROFILE_LABELS[profile]}
              </option>
            ))}
          </select>
          <p className="mt-1.5 text-xs text-zinc-500">
            {RUN_PROFILE_DESCRIPTIONS[defaultProfile]}
          </p>
          <p className="mt-1 text-[10px] text-[var(--muted)]">
            Sets the initial routing profile for new sessions.
          </p>
        </div>
      </section>

      {/* Connected accounts */}
      <section className="flex flex-col gap-3">
        <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--muted)]">
          Connected accounts
        </h3>
        <div className="rounded-xl border border-white/8 bg-white/[0.02]">
          {providers.length === 0 ? (
            <p className="px-4 py-3 text-sm text-[var(--muted)]">No providers connected yet.</p>
          ) : (
            providers.map((p, idx) => (
              <div
                key={p.provider}
                className={`flex items-center justify-between px-4 py-3 ${idx < providers.length - 1 ? 'border-b border-white/6' : ''}`}
              >
                <div className="flex items-center gap-2">
                  <div className="flex h-7 w-7 items-center justify-center rounded-md bg-white/6 text-[10px] font-bold uppercase text-[var(--muted-strong)]">
                    {p.provider === 'claude' ? 'CL' : 'OA'}
                  </div>
                  <span className="text-sm text-white">
                    {p.provider === 'claude' ? 'Claude (Anthropic)' : 'OpenAI (Codex)'}
                  </span>
                </div>
                {p.authenticated ? (
                  <div className="flex items-center gap-1 text-xs text-emerald-300">
                    <CheckCircle className="h-3 w-3" />
                    Connected
                  </div>
                ) : (
                  <div className="flex items-center gap-1 text-xs text-[var(--muted)]">
                    <XCircle className="h-3 w-3" />
                    Not connected
                  </div>
                )}
              </div>
            ))
          )}
        </div>
      </section>

      {/* Danger zone */}
      <section className="flex flex-col gap-3">
        <h3 className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-red-400">
          <AlertTriangle className="h-3.5 w-3.5" />
          Danger zone
        </h3>
        <div className="rounded-xl border border-red-500/20 bg-red-500/5 px-4 py-3">
          <div className="flex flex-col gap-3">
            <div className="flex items-center justify-between gap-4">
              <div>
                <p className="text-sm font-medium text-white">Delete account</p>
                <p className="mt-0.5 text-xs text-[var(--muted)]">
                  Permanently delete your account and all associated data.
                </p>
              </div>
              {!showDeleteConfirm && !deleteSubmitted && (
                <button
                  type="button"
                  onClick={() => { setShowDeleteConfirm(true); setDeleteInput(''); }}
                  className="shrink-0 rounded-lg border border-red-500/40 bg-red-500/10 px-3 py-1.5 text-xs font-medium text-red-300 transition hover:bg-red-500/20 active:scale-95"
                >
                  Delete account
                </button>
              )}
            </div>
            {showDeleteConfirm && !deleteSubmitted && (
              <div className="flex flex-col gap-2 rounded-lg border border-red-500/15 bg-red-500/5 p-3">
                <p className="text-xs text-red-200">
                  Type <span className="font-mono font-bold">DELETE</span> to confirm account deletion:
                </p>
                <div className="flex gap-2">
                  <input
                    type="text"
                    value={deleteInput}
                    onChange={(e) => setDeleteInput(e.target.value)}
                    placeholder="Type DELETE"
                    className="flex-1 rounded-lg border border-white/10 bg-[var(--composer)] px-3 py-2 font-mono text-sm text-white placeholder:text-white/20 focus:border-red-400/50 focus:outline-none"
                    autoComplete="off"
                    spellCheck={false}
                  />
                  <button
                    type="button"
                    onClick={() => setShowDeleteConfirm(false)}
                    className="rounded-lg border border-white/10 px-3 py-1.5 text-xs text-[var(--muted)] transition hover:text-white"
                  >
                    Cancel
                  </button>
                  <button
                    type="button"
                    disabled={deleteInput !== 'DELETE'}
                    onClick={() => { setShowDeleteConfirm(false); setDeleteSubmitted(true); }}
                    className="rounded-lg bg-red-500 px-3 py-1.5 text-xs font-medium text-white transition hover:bg-red-400 active:scale-95 disabled:opacity-30"
                  >
                    Confirm
                  </button>
                </div>
              </div>
            )}
            {deleteSubmitted && (
              <div className="rounded-lg border border-amber-400/20 bg-amber-400/8 p-3">
                <p className="text-xs text-amber-200">
                  Account deletion is not yet available as a self-service action.
                  Please contact <a href="mailto:support@heyvera.org" className="font-medium underline">support@heyvera.org</a> to
                  request account deletion. We will process your request within 30 days per GDPR requirements.
                </p>
              </div>
            )}
          </div>
        </div>
      </section>
    </div>
  );
}

function CredentialsTab({ isAdmin }: { isAdmin?: boolean }) {
  const [credentials, setCredentials] = useState<ProviderAuthInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [addingProvider, setAddingProvider] = useState<string | null>(null);
  // Operator-funded model: credentials are operator API keys. Subscription
  // (OAuth) credentials are not a supported path — Anthropic's Feb 2026 terms
  // update prohibits subscription tokens in third-party tools (enforced from
  // 2026-04-04). See cortex/plan/PLAN-2026-08.md §3.
  const addType: 'subscription' | 'api_key' = 'api_key';
  const [codeInput, setCodeInput] = useState('');
  const [labelInput, setLabelInput] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [authInfo, setAuthInfo] = useState<{ auth_url?: string; message?: string } | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Assignments state
  const [assignments, setAssignments] = useState<CredentialAssignment[]>([]);
  const [assignmentsLoading, setAssignmentsLoading] = useState(true);
  const [showAssignForm, setShowAssignForm] = useState(false);
  const [assignCredId, setAssignCredId] = useState('');
  const [assignTargetType, setAssignTargetType] = useState('project');
  const [assignTargetId, setAssignTargetId] = useState('');
  const [assignSubmitting, setAssignSubmitting] = useState(false);

  const fetchCredentials = useCallback(async () => {
    try {
      const creds = await getAuthStatus();
      setCredentials(creds);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(`Failed to load credentials: ${msg}`);
    } finally {
      setLoading(false);
    }
  }, []);

  const fetchAssignments = useCallback(async () => {
    setAssignmentsLoading(true);
    try {
      const list = await listCredentialAssignments();
      setAssignments(list);
    } catch {
      // silent — assignments are optional
    } finally {
      setAssignmentsLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchCredentials();
    fetchAssignments();
  }, [fetchCredentials, fetchAssignments]);

  const handleDelete = async (credentialId: string) => {
    try {
      await deleteCredential(credentialId);
      await fetchCredentials();
    } catch {
      setError('Failed to delete credential');
    }
  };

  const handleSetDefault = async (credentialId: string) => {
    try {
      await setDefaultCredential(credentialId);
      await fetchCredentials();
    } catch {
      setError('Failed to set default');
    }
  };

  const handleStartAdd = async (provider: string) => {
    setAddingProvider(provider);
    setError(null);
    setAuthInfo(null);
    setCodeInput('');
    setLabelInput('');
    try {
      const result = await startAuth(provider, addType);
      setAuthInfo({ auth_url: result.auth_url ?? undefined, message: result.message });
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(`Failed to start auth flow: ${msg}`);
      setAddingProvider(null);
    }
  };

  const handleSubmitCode = async () => {
    if (!addingProvider || !codeInput.trim()) return;
    setSubmitting(true);
    setError(null);
    try {
      const result = await submitAuthCode(
        addingProvider,
        codeInput.trim(),
        labelInput.trim() || undefined,
        addType,
      );
      if (result.success) {
        setAddingProvider(null);
        setCodeInput('');
        setLabelInput('');
        setAuthInfo(null);
        await fetchCredentials();
      } else {
        setError(result.message);
      }
    } catch {
      setError('Failed to submit credential');
    } finally {
      setSubmitting(false);
    }
  };

  const handleCancelAdd = () => {
    setAddingProvider(null);
    setCodeInput('');
    setLabelInput('');
    setAuthInfo(null);
    setError(null);
  };

  const handleAssign = async () => {
    if (!assignCredId) return;
    setAssignSubmitting(true);
    try {
      await assignCredential(assignCredId, assignTargetType, assignTargetId.trim() || undefined);
      setShowAssignForm(false);
      setAssignCredId('');
      setAssignTargetType('project');
      setAssignTargetId('');
      await fetchAssignments();
    } catch {
      setError('Failed to create assignment');
    } finally {
      setAssignSubmitting(false);
    }
  };

  const handleRemoveAssignment = async (assignmentId: string) => {
    try {
      await removeCredentialAssignment(assignmentId);
      await fetchAssignments();
    } catch {
      setError('Failed to remove assignment');
    }
  };

  if (loading) {
    return (
      <div className="flex flex-col gap-2 py-4">
        {[0, 1, 2].map((i) => (
          <div key={i} className="rounded-lg border border-white/8 bg-white/[0.02] p-4 flex items-center gap-3">
            <div className="h-8 w-8 shrink-0 rounded-lg bg-white/6 animate-pulse" />
            <div className="flex flex-col gap-2">
              <div className="h-3 w-32 rounded bg-white/8 animate-pulse" />
              <div className="h-3 w-20 rounded bg-white/8 animate-pulse" />
            </div>
          </div>
        ))}
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-4">
      <div>
        <h3 className="text-sm font-medium text-white mb-1">Your Credentials</h3>
        <p className="text-xs text-[var(--muted)]">
          Manage your AI provider subscriptions and API keys. You can add multiple credentials per provider.
        </p>
      </div>

      {error && (
        <div className="rounded-lg bg-red-500/10 border border-red-500/20 px-3 py-2 text-xs text-red-300">
          {error}
        </div>
      )}

      {credentials.length === 0 ? (
        <div className="rounded-lg border border-white/8 bg-white/[0.02] px-4 py-6 text-center text-sm text-[var(--muted)]">
          <Key className="text-[var(--muted)] opacity-40 h-8 w-8 mx-auto mb-2" />
          No credentials yet. Add a subscription or API key below.
        </div>
      ) : (
        <div className="flex flex-col gap-2">
          {credentials.map((cred) => (
            <div
              key={cred.credential_id}
              className="flex items-center justify-between rounded-lg border border-white/8 bg-white/[0.02] px-4 py-3"
            >
              <div className="flex items-center gap-3">
                <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-white/6 text-xs font-bold uppercase text-[var(--muted-strong)]">
                  {cred.provider === 'claude' ? 'CL' : 'OA'}
                </div>
                <div>
                  <div className="flex items-center gap-2">
                    <span className="text-sm font-medium text-white">
                      {cred.label || `${cred.provider} ${cred.credential_type}`}
                    </span>
                    <span className="rounded bg-white/8 px-1.5 py-0.5 text-[10px] font-medium text-[var(--muted)]">
                      {cred.credential_type}
                    </span>
                    {cred.is_default && (
                      <span className="rounded bg-emerald-500/20 px-1.5 py-0.5 text-[10px] font-medium text-emerald-300">
                        default
                      </span>
                    )}
                    <span className={`rounded px-1.5 py-0.5 text-[10px] font-medium ${
                      cred.status === 'active' ? 'bg-emerald-500/10 text-emerald-400' :
                      cred.status === 'expired' ? 'bg-amber-500/10 text-amber-400' :
                      'bg-red-500/10 text-red-400'
                    }`}>
                      {cred.status}
                    </span>
                  </div>
                  {cred.email && (
                    <div className="text-xs text-[var(--muted)]">{cred.email}</div>
                  )}
                </div>
              </div>

              <div className="flex items-center gap-1">
                {!cred.is_default && cred.status === 'active' && (
                  <button
                    onClick={() => handleSetDefault(cred.credential_id)}
                    className="rounded p-1.5 text-[var(--muted)] hover:bg-white/8 hover:text-amber-300 transition"
                    title="Set as default"
                  >
                    <Star className="h-3.5 w-3.5" />
                  </button>
                )}
                <button
                  onClick={() => handleDelete(cred.credential_id)}
                  className="rounded p-1.5 text-[var(--muted)] hover:bg-white/8 hover:text-red-400 transition"
                  title="Remove credential"
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </button>
              </div>
            </div>
          ))}
        </div>
      )}

      {/* Add credential section */}
      <div className="border-t border-white/8 pt-4">
        <h4 className="text-sm font-medium text-white mb-3">Add Credential</h4>

        {!addingProvider ? (
          <div className="flex flex-col gap-3">
            {isAdmin && (
              <p className="mb-2 text-xs text-[var(--muted)]">
                Operator API keys. Subscription credentials are not supported —
                provider terms prohibit subscription tokens in third-party tools.
              </p>
            )}
            <div className="flex gap-2">
              <button
                onClick={() => handleStartAdd('claude')}
                className="flex-1 rounded-lg border border-white/8 bg-white/[0.02] px-4 py-3 text-sm text-white hover:bg-white/6 transition"
              >
                + Claude (Anthropic)
              </button>
              <button
                onClick={() => handleStartAdd('openai')}
                className="flex-1 rounded-lg border border-white/8 bg-white/[0.02] px-4 py-3 text-sm text-white hover:bg-white/6 transition"
              >
                + OpenAI
              </button>
            </div>
          </div>
        ) : (
          <div className="rounded-lg border border-white/8 bg-white/[0.02] p-4">
            <div className="flex items-center justify-between mb-3">
              <span className="text-sm font-medium text-white">
                {`Add ${addingProvider === 'claude' ? 'Claude' : 'OpenAI'} API Key`}
              </span>
              <button onClick={handleCancelAdd} className="text-xs text-[var(--muted)] hover:text-white">
                Cancel
              </button>
            </div>

            {!authInfo && (
              <div className="flex items-center gap-2 py-4 text-sm text-[var(--muted)]">
                <Loader2 className="h-4 w-4 animate-spin" />
                Setting up container...
              </div>
            )}

            {authInfo?.auth_url && (
              <div className="mb-4">
                <a
                  href={authInfo.auth_url}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="inline-flex items-center gap-2 rounded-lg bg-[var(--accent)] px-4 py-2.5 text-sm font-medium text-white hover:opacity-90 transition"
                >
                  {`Open ${addingProvider === 'claude' ? 'Anthropic Console' : 'OpenAI Platform'}`}
                  <ExternalLink className="h-3.5 w-3.5" />
                </a>
              </div>
            )}

            {authInfo?.message && (
              <p className="text-xs text-[var(--muted)] mb-3 whitespace-pre-line">{authInfo.message}</p>
            )}

            {authInfo && (
              <div className="flex flex-col gap-2">
                <input
                  type="text"
                  value={labelInput}
                  onChange={(e) => setLabelInput(e.target.value)}
                  placeholder="Label (optional, e.g. 'Work Claude')"
                  className="rounded-lg border border-white/10 bg-white/5 px-3 py-2 text-sm text-white placeholder:text-[var(--muted)] focus:border-[var(--accent)] focus:outline-none"
                />
                <input
                  type="password"
                  value={codeInput}
                  onChange={(e) => setCodeInput(e.target.value)}
                  placeholder={addType === 'api_key' ? 'Paste API key...' : 'Paste auth token or code...'}
                  className="rounded-lg border border-white/10 bg-white/5 px-3 py-2 text-sm text-white placeholder:text-[var(--muted)] focus:border-[var(--accent)] focus:outline-none"
                  onKeyDown={(e) => e.key === 'Enter' && handleSubmitCode()}
                />
                <button
                  onClick={handleSubmitCode}
                  disabled={submitting || !codeInput.trim()}
                  className="btn-primary rounded-lg px-4 py-2 text-sm disabled:opacity-50"
                >
                  {submitting ? 'Connecting...' : addType === 'api_key' ? 'Save API Key' : 'Connect Subscription'}
                </button>
              </div>
            )}
          </div>
        )}
      </div>

      {/* Assignments section */}
      <div className="border-t border-white/8 pt-4">
        <div className="flex items-center justify-between mb-3">
          <h4 className="text-sm font-medium text-white">Assignments</h4>
          {!showAssignForm && credentials.length > 0 && (
            <button
              onClick={() => {
                setShowAssignForm(true);
                setAssignCredId(credentials[0]?.credential_id ?? '');
              }}
              className="rounded px-2 py-1 text-xs bg-white/6 text-[var(--muted)] hover:text-white transition"
            >
              + Assign
            </button>
          )}
        </div>
        <p className="text-xs text-[var(--muted)] mb-3">
          Assign credentials to projects or teams so they can use a specific provider account.
        </p>

        {showAssignForm && (
          <div className="rounded-lg border border-white/8 bg-white/[0.02] p-3 mb-3 flex flex-col gap-2">
            <div className="flex items-center justify-between">
              <span className="text-xs font-medium text-white">New assignment</span>
              <button
                onClick={() => setShowAssignForm(false)}
                className="text-xs text-[var(--muted)] hover:text-white"
              >
                Cancel
              </button>
            </div>
            <select
              value={assignCredId}
              onChange={(e) => setAssignCredId(e.target.value)}
              className="w-full rounded-lg border border-white/10 bg-[var(--composer)] px-3 py-2 text-sm text-white focus:border-[var(--accent)]/50 focus:outline-none"
            >
              {credentials.map((c) => (
                <option key={c.credential_id} value={c.credential_id}>
                  {c.label || `${c.provider} ${c.credential_type}`}
                </option>
              ))}
            </select>
            <select
              value={assignTargetType}
              onChange={(e) => setAssignTargetType(e.target.value)}
              className="w-full rounded-lg border border-white/10 bg-[var(--composer)] px-3 py-2 text-sm text-white focus:border-[var(--accent)]/50 focus:outline-none"
            >
              <option value="project">Project</option>
              <option value="team">Team</option>
              <option value="global">Global (all)</option>
            </select>
            {assignTargetType !== 'global' && (
              <input
                type="text"
                value={assignTargetId}
                onChange={(e) => setAssignTargetId(e.target.value)}
                placeholder={`${assignTargetType === 'project' ? 'Project' : 'Team'} ID`}
                className="rounded-lg border border-white/10 bg-white/5 px-3 py-2 text-sm text-white placeholder:text-[var(--muted)] focus:border-[var(--accent)] focus:outline-none"
              />
            )}
            <button
              onClick={handleAssign}
              disabled={assignSubmitting || !assignCredId || (assignTargetType !== 'global' && !assignTargetId.trim())}
              className="rounded-lg bg-[var(--accent)] px-4 py-2 text-sm font-medium text-white hover:opacity-90 transition disabled:opacity-50"
            >
              {assignSubmitting ? 'Assigning...' : 'Save Assignment'}
            </button>
          </div>
        )}

        {assignmentsLoading ? (
          <div className="flex items-center gap-2 py-4 text-xs text-[var(--muted)]">
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
            Loading assignments...
          </div>
        ) : assignments.length === 0 ? (
          <div className="rounded-lg border border-white/8 bg-white/[0.02] px-4 py-4 text-center text-xs text-[var(--muted)]">
            No assignments yet.
          </div>
        ) : (
          <div className="flex flex-col gap-2">
            {assignments.map((a) => {
              const cred = credentials.find((c) => c.credential_id === a.credential_id);
              const credLabel = cred ? (cred.label || `${cred.provider} ${cred.credential_type}`) : a.credential_id.slice(0, 8);
              return (
                <div
                  key={a.id}
                  className="flex items-center justify-between rounded-lg border border-white/8 bg-white/[0.02] px-4 py-2.5"
                >
                  <div className="flex flex-col gap-0.5">
                    <span className="text-sm text-white">{credLabel}</span>
                    <span className="text-xs text-[var(--muted)]">
                      {a.target_type}{a.target_id ? ` — ${a.target_id}` : ''}
                    </span>
                  </div>
                  <button
                    onClick={() => handleRemoveAssignment(a.id)}
                    className="rounded p-1.5 text-[var(--muted)] hover:bg-white/8 hover:text-red-400 transition"
                    title="Remove assignment"
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </button>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}

interface SettingsPanelProps {
  onClose: () => void;
  initialTab?: SettingsTab;
  billing: BillingStatus | null;
  isAdmin?: boolean;
}


export default function SettingsPanel({
  onClose,
  initialTab = 'providers',
  billing,
  isAdmin,
}: SettingsPanelProps) {
  const [tab, setTab] = useState<SettingsTab>(initialTab);
  const [providers, setProviders] = useState<ProviderAuthInfo[]>([]);

  // Budget settings state
  const [budgetSettings, setBudgetSettings] = useState<BudgetSettingsType | null>(null);
  const [budgetUsage, setBudgetUsage] = useState<UsageData | null>(null);
  const [budgetLoading, setBudgetLoading] = useState(false);
  const [budgetError, setBudgetError] = useState<string | null>(null);

  const fetchStatus = useCallback(async () => {
    try {
      const status = await getAuthStatus();
      setProviders(status);
    } catch {
      // silent
    }
  }, []);

  useEffect(() => { fetchStatus(); }, [fetchStatus]);

  const fetchBudgetData = useCallback(async () => {
    setBudgetLoading(true);
    setBudgetError(null);
    try {
      const [settings, usage] = await Promise.all([
        getBudgetSettings(),
        getCurrentUsage(),
      ]);
      setBudgetSettings(settings);
      setBudgetUsage(usage);
    } catch (err) {
      setBudgetError(err instanceof Error ? err.message : 'Failed to load budget data');
    } finally {
      setBudgetLoading(false);
    }
  }, []);

  const handleSaveBudgetSettings = useCallback(async (settings: BudgetSettingsType) => {
    try {
      const updated = await updateBudgetSettings(settings);
      setBudgetSettings(updated);
      // Refresh usage data after updating settings
      const usage = await getCurrentUsage();
      setBudgetUsage(usage);
    } catch (err) {
      throw new Error(err instanceof Error ? err.message : 'Failed to save budget settings', {
        cause: err,
      });
    }
  }, []);

  // Fetch budget data when budget tab is selected
  useEffect(() => {
    if (tab === 'budget') {
      fetchBudgetData();
    }
  }, [tab, fetchBudgetData]);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-3 backdrop-blur-sm sm:p-6">
      <div className={`relative flex max-h-[min(44rem,calc(100dvh-1.5rem))] w-full flex-col overflow-hidden rounded-2xl border border-white/8 bg-[var(--panel)] animate-scale-in shadow-2xl sm:max-h-[min(44rem,calc(100dvh-3rem))] ${tab === 'billing' || tab === 'budget' ? 'max-w-2xl' : 'max-w-lg'}`}>
        {/* Header */}
        <div className="shrink-0 border-b border-white/6 px-5 py-4">
          <div className="flex items-center justify-between gap-4">
            <div>
              <h2 className="text-base font-semibold text-white">Settings</h2>
              <p className="mt-0.5 text-xs text-[var(--muted)]">Provider subscriptions and account connections.</p>
            </div>
            <button
              onClick={onClose}
              className="rounded-lg p-1.5 text-[var(--muted)] transition hover:bg-white/8 hover:text-white active:scale-95"
              aria-label="Close settings"
            >
              <X className="h-4 w-4" />
            </button>
          </div>
        </div>

        {/* Tabs */}
        <div className="flex shrink-0 gap-4 border-b border-white/6 px-5">
          <button
            onClick={() => setTab('providers')}
            className={`border-b-2 px-1 py-2.5 text-sm font-medium transition ${
              tab === 'providers' ? 'border-[var(--accent)] text-white' : 'border-transparent text-[var(--muted)] hover:text-white'
            }`}
          >
            Subscriptions
          </button>
          <button
            onClick={() => setTab('integrations')}
            className={`border-b-2 px-1 py-2.5 text-sm font-medium transition ${
              tab === 'integrations' ? 'border-[var(--accent)] text-white' : 'border-transparent text-[var(--muted)] hover:text-white'
            }`}
          >
            Integrations
          </button>
          {/* Soma is fenced off by default; the tab is hidden rather than
              removed, because the feature returns. See SOMA_API_ENABLED. */}
          {SOMA_API_ENABLED && (
          <button
            onClick={() => setTab('spend')}
            className={`border-b-2 px-1 py-2.5 text-sm font-medium transition ${
              tab === 'spend' ? 'border-[var(--accent)] text-white' : 'border-transparent text-[var(--muted)] hover:text-white'
            }`}
          >
            Soma spend
          </button>
          )}
          <button
            onClick={() => setTab('billing')}
            className={`border-b-2 px-1 py-2.5 text-sm font-medium transition ${
              tab === 'billing' ? 'border-[var(--accent)] text-white' : 'border-transparent text-[var(--muted)] hover:text-white'
            }`}
          >
            Billing
          </button>
          <button
            onClick={() => setTab('budget')}
            className={`border-b-2 px-1 py-2.5 text-sm font-medium transition ${
              tab === 'budget' ? 'border-[var(--accent)] text-white' : 'border-transparent text-[var(--muted)] hover:text-white'
            }`}
          >
            Budget & Costs
          </button>
          <button
            onClick={() => setTab('notifications')}
            className={`border-b-2 px-1 py-2.5 text-sm font-medium transition ${
              tab === 'notifications' ? 'border-[var(--accent)] text-white' : 'border-transparent text-[var(--muted)] hover:text-white'
            }`}
          >
            Notifications
          </button>
          <button
            onClick={() => setTab('account')}
            className={`border-b-2 px-1 py-2.5 text-sm font-medium transition ${
              tab === 'account' ? 'border-[var(--accent)] text-white' : 'border-transparent text-[var(--muted)] hover:text-white'
            }`}
          >
            Account
          </button>
          <button
            onClick={() => setTab('credentials')}
            className={`border-b-2 px-1 py-2.5 text-sm font-medium transition ${
              tab === 'credentials' ? 'border-[var(--accent)] text-white' : 'border-transparent text-[var(--muted)] hover:text-white'
            }`}
          >
            Credentials
          </button>
        </div>

        {/* Content */}
        <div className="min-h-0 flex-1 overflow-y-auto p-5">
          {tab === 'billing' ? (
            <BillingPage billing={billing} />
          ) : tab === 'integrations' ? (
            <IntegrationSetup />
          ) : tab === 'spend' ? (
            SOMA_API_ENABLED ? <SpendDashboard /> : null
          ) : tab === 'budget' ? (
            <BudgetSettings
              settings={budgetSettings}
              usage={budgetUsage}
              onSave={handleSaveBudgetSettings}
              loading={budgetLoading}
              error={budgetError || undefined}
            />
          ) : tab === 'notifications' ? (
            <NotificationsTab />
          ) : tab === 'credentials' ? (
            <CredentialsTab isAdmin={isAdmin} />
          ) : tab === 'account' ? (
            <AccountTab providers={providers} />
          ) : (
            <CredentialsTab isAdmin={isAdmin} />
          )}
        </div>
      </div>
    </div>
  );
}
