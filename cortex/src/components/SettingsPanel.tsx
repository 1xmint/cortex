import { useState } from 'react';
import { AlertTriangle, User, X } from 'lucide-react';
import { useUser } from '@clerk/clerk-react';
import {
  SOMA_API_ENABLED,
  type BillingStatus,
} from '../lib/cortexApi';
import BillingPage from './billing/BillingPage';
import SpendDashboard from './spend/SpendDashboard';
import IntegrationSetup from './integrations/IntegrationSetup';
import type { RunProfile } from '../types';

type SettingsTab = 'integrations' | 'spend' | 'billing' | 'notifications' | 'account';

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

function AccountTab() {
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

interface SettingsPanelProps {
  onClose: () => void;
  initialTab?: SettingsTab;
  billing: BillingStatus | null;
  isAdmin?: boolean;
}


export default function SettingsPanel({
  onClose,
  initialTab = 'account',
  billing,
}: SettingsPanelProps) {
  const [tab, setTab] = useState<SettingsTab>(initialTab);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-3 backdrop-blur-sm sm:p-6">
      <div className={`relative flex max-h-[min(44rem,calc(100dvh-1.5rem))] w-full flex-col overflow-hidden rounded-2xl border border-white/8 bg-[var(--panel)] animate-scale-in shadow-2xl sm:max-h-[min(44rem,calc(100dvh-3rem))] ${tab === 'billing' ? 'max-w-2xl' : 'max-w-lg'}`}>
        {/* Header */}
        <div className="shrink-0 border-b border-white/6 px-5 py-4">
          <div className="flex items-center justify-between gap-4">
            <div>
              <h2 className="text-base font-semibold text-white">Settings</h2>
              <p className="mt-0.5 text-xs text-[var(--muted)]">Account, billing, and integration settings.</p>
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
        </div>

        {/* Content */}
        <div className="min-h-0 flex-1 overflow-y-auto p-5">
          {tab === 'billing' ? (
            <BillingPage billing={billing} />
          ) : tab === 'integrations' ? (
            <IntegrationSetup />
          ) : tab === 'spend' ? (
            SOMA_API_ENABLED ? <SpendDashboard /> : null
          ) : tab === 'notifications' ? (
            <NotificationsTab />
          ) : (
            <AccountTab />
          )}
        </div>
      </div>
    </div>
  );
}
