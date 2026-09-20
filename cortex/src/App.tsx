import { Suspense, lazy, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { AlertTriangle, CheckCircle2, CreditCard, ExternalLink, Loader2, LogOut, Menu, Search, LayoutGrid } from 'lucide-react';
import { BrowserRouter, Navigate, Route, Routes, useNavigate, useParams } from 'react-router';
import { useClerk, useUser } from '@clerk/clerk-react';

const CLERK_ENABLED = !!import.meta.env.VITE_CLERK_PUBLISHABLE_KEY;

/**
 * Renders the signed-in user's display name/email and a logout button.
 * Must only be rendered when ClerkProvider is in the tree.
 */
function ClerkUserControls() {
  const { signOut } = useClerk();
  const { user } = useUser();
  const displayName = user?.firstName
    ?? user?.primaryEmailAddress?.emailAddress
    ?? null;

  return (
    <div className="hidden items-center gap-2 sm:flex">
      {displayName && (
        <span className="max-w-[10rem] truncate text-[11px] text-[var(--muted)]" title={displayName}>
          {displayName}
        </span>
      )}
      <button
        type="button"
        onClick={() => void signOut()}
        className="inline-flex h-9 w-9 items-center justify-center rounded-lg text-[var(--muted)] transition hover:bg-white/6 hover:text-white active:scale-95"
        aria-label="Sign out"
        title="Sign out"
      >
        <LogOut className="h-4 w-4" />
      </button>
    </div>
  );
}
import TrialBanner from './components/billing/TrialBanner';
import GroupSidebar from './components/groups/GroupSidebar';
import CommandPalette from './components/shell/CommandPalette';
import HelpMenu from './components/shell/HelpMenu';
import TaskManagerSidebar from './components/shell/TaskManagerSidebar';
import ProjectChat from './components/chat/ProjectChat';
import PersonalTaskManager from './components/personal/PersonalTaskManager';
import TaskManagerSwitcher from './components/shell/TaskManagerSwitcher';
import { useChatSession } from './lib/useChatSession';
import { useAuthGate } from './lib/useAuthGate';
import { useSomaSession } from './lib/useSomaSession';
import { useBilling } from './lib/useBilling';
import { isOnboardingComplete } from './lib/onboarding';
import SignInScreen from './components/auth/SignInScreen';
import OperationsRoom from './components/operations/OperationsRoom';
import OnboardingFlow from './components/onboarding/OnboardingFlow';
import NotFoundPage from './components/NotFoundPage';
import ProjectsView from './components/project/ProjectsView';
import MissionControl from './components/mission/MissionControl';
import { RunsPane, ReceiptsPane, LedgerPane, LeasesPane, AdminPane } from './components/mission/panes';
import {
  AUTH_CHANNEL_NAME,
  CortexApiError,
  getAdminStats,
  getDeploymentStatus,
  getUserRouting,
  listConversations,
  setAuthTokenGetter,
  type BillingAccessState,
  type ConversationSummary,
  type DeploymentStatus,
} from './lib/cortexApi';
import {
  createTeamGroup,
  DEFAULT_GROUPS,
  readGroupConversationMap,
  readGroups,
  writeGroupConversationMap,
  writeGroups,
  type CortexGroup,
} from './lib/groups';
import type { ChatSessionControls, RunProfile, TaskManagerTask } from './types';
import type { TaskManagerState } from './types';
import { openDetachedPanel } from './lib/shell/windowManager';
import { readTaskManagerState, TASK_MANAGER_CHANNEL_NAME } from './lib/taskManager';

const DEFAULT_SESSION_CONTROLS: ChatSessionControls = {
  speed: 'balanced',
  intelligence: 'balanced',
  autonomy: 'guided',
};

const SESSION_CONTROLS_STORAGE_KEY = 'cortex:session-controls';
const RUN_PROFILE_STORAGE_KEY = 'cortex:run-profile';
const FREE_TIER_ACCESS_STATES = new Set<BillingAccessState>(['needs_checkout', 'needs_phone', 'cancelled']);
type SettingsTab = 'providers' | 'integrations' | 'spend' | 'billing' | 'account';

function deploymentBadgeTitle(status: DeploymentStatus): string {
  const backend = status.commits.backend_commit_short ?? status.backend.commit_short ?? 'unknown';
  const frontend = status.commits.frontend_commit_short ?? 'unknown';
  const workflow = status.github_actions.run_id
    ? `; Deploy Production #${status.github_actions.run_number ?? status.github_actions.run_id} ${status.github_actions.conclusion ?? status.github_actions.run_status ?? 'unknown'}`
    : '';
  if (status.commits.status === 'mismatch') {
    return `Backend ${backend}; Cloudflare Pages ${frontend}${workflow}`;
  }
  if (status.commits.status === 'match') {
    return `Backend and Cloudflare Pages commit ${backend}${workflow}`;
  }
  return status.backend.commit_short ? `Live commit ${status.backend.commit_short}` : 'Deployment surfaces match';
}

function isSessionControls(value: unknown): value is ChatSessionControls {
  if (!value || typeof value !== 'object') return false;
  const candidate = value as Partial<ChatSessionControls>;
  return (
    (candidate.speed === 'steady' || candidate.speed === 'balanced' || candidate.speed === 'rapid')
    && (candidate.intelligence === 'focused'
      || candidate.intelligence === 'balanced'
      || candidate.intelligence === 'deep')
    && (candidate.autonomy === 'manual'
      || candidate.autonomy === 'guided'
      || candidate.autonomy === 'smart_auto'
      || candidate.autonomy === 'full_auto'
      || candidate.autonomy === 'custom')
  );
}

function readSessionControls(): ChatSessionControls {
  try {
    const raw = window.localStorage.getItem(SESSION_CONTROLS_STORAGE_KEY);
    if (!raw) return DEFAULT_SESSION_CONTROLS;
    const parsed = JSON.parse(raw);
    return isSessionControls(parsed) ? parsed : DEFAULT_SESSION_CONTROLS;
  } catch {
    return DEFAULT_SESSION_CONTROLS;
  }
}

function isRunProfile(value: unknown): value is RunProfile {
  return value === 'auto'
    || value === 'balanced'
    || value === 'cost_saver'
    || value === 'quality_first';
}

function readRunProfile(): RunProfile {
  try {
    const raw = window.localStorage.getItem(RUN_PROFILE_STORAGE_KEY);
    return isRunProfile(raw) ? raw : 'auto';
  } catch {
    return 'auto';
  }
}

const AdminPanel = lazy(() => import('./components/admin/AdminPanel'));
const PricingCards = lazy(() => import('./components/billing/PricingCards'));
const SettingsPanel = lazy(() => import('./components/SettingsPanel'));

function isFreeTierAccessState(accessState: BillingAccessState | undefined) {
  return Boolean(accessState && FREE_TIER_ACCESS_STATES.has(accessState));
}

function LegacyGroupRedirect() {
  const { groupId } = useParams();
  return <Navigate to={`/app/groups/${groupId ?? DEFAULT_GROUPS[0].id}/tasks`} replace />;
}

function FreeTierBanner({
  accessState,
  onOpenBilling,
}: {
  accessState: BillingAccessState;
  onOpenBilling: () => void;
}) {
  const isCancelled = accessState === 'cancelled';
  return (
    <div className="border-b border-[var(--accent)]/15 bg-[var(--accent)]/10 px-3 py-2 sm:px-4">
      <div className="mx-auto flex max-w-4xl flex-wrap items-center gap-2 sm:gap-3">
        <span className="rounded-full border border-[var(--accent)]/25 bg-black/15 px-2 py-0.5 text-[11px] font-medium text-[var(--accent)]">
          Preview
        </span>
        <p className="min-w-[12rem] flex-1 text-xs text-[var(--muted-strong)]">
          {isCancelled
            ? 'Your subscription is cancelled. Core Task Manager and routing previews remain available with free-tier limits.'
            : "You're previewing Cortex with sample data. Subscribe to connect real AI agents."}
        </p>
        <button
          type="button"
          onClick={onOpenBilling}
          className="inline-flex h-7 items-center gap-1.5 rounded-lg border border-white/10 bg-white/8 px-2.5 text-xs text-white transition hover:bg-white/12 active:scale-95"
        >
          <CreditCard className="h-3.5 w-3.5" />
          Subscribe
        </button>
      </div>
    </div>
  );
}

function PaymentIssueBanner({
  onOpenBilling,
}: {
  onOpenBilling: () => void;
}) {
  return (
    <div className="border-b border-red-400/20 bg-red-400/10 px-3 py-2 sm:px-4">
      <div className="mx-auto flex max-w-4xl flex-wrap items-center gap-2 sm:gap-3">
        <CreditCard className="h-4 w-4 shrink-0 text-red-200" />
        <p className="min-w-[12rem] flex-1 text-xs text-red-100">
          Payment failed. Your workspace is visible, but paid Cortex runtime actions need an updated payment method.
        </p>
        <button
          type="button"
          onClick={onOpenBilling}
          className="inline-flex h-7 items-center rounded-lg border border-red-200/20 bg-red-100/10 px-2.5 text-xs text-red-50 transition hover:bg-red-100/15 active:scale-95"
        >
          Fix billing
        </button>
      </div>
    </div>
  );
}

function useGroupState() {
  const [groups, setGroups] = useState<CortexGroup[]>(readGroups);
  const [conversationByGroup, setConversationByGroup] = useState<Record<string, string | null>>(
    readGroupConversationMap,
  );

  const addTeamGroup = useCallback((group: CortexGroup) => {
    setGroups((current) => {
      if (current.some((existingGroup) => existingGroup.id === group.id)) return current;
      const nextGroups = [...current, group];
      writeGroups(nextGroups);
      return nextGroups;
    });
  }, []);

  const setGroupConversation = useCallback((groupId: string, conversationId: string | null) => {
    setConversationByGroup((current) => {
      const next = { ...current, [groupId]: conversationId };
      writeGroupConversationMap(next);
      return next;
    });
  }, []);

  return {
    groups,
    addTeamGroup,
    conversationByGroup,
    setGroupConversation,
  };
}

function CortexShell() {
  const navigate = useNavigate();
  const { groupId } = useParams<{ groupId: string }>();
  const {
    groups,
    addTeamGroup,
    conversationByGroup,
    setGroupConversation,
  } = useGroupState();
  const activeGroup = useMemo(
    () => groups.find((group) => group.id === groupId) ?? DEFAULT_GROUPS[0],
    [groupId, groups],
  );
  const activeGroupId = activeGroup.id;
  const { isLoaded, isSignedIn, userId, getToken, clerkEnabled } = useAuthGate();
  // Auto-creates user's Soma identity + session-scoped delegation on sign-in
  useSomaSession(userId ?? 'anonymous', isSignedIn);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsInitialTab, setSettingsInitialTab] = useState<SettingsTab>('providers');
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const activeConversationId = conversationByGroup[activeGroupId] ?? null;
  const [renamingTitle, setRenamingTitle] = useState(false);
  const [titleDraft, setTitleDraft] = useState('');
  const [conversationListVersion, setConversationListVersion] = useState(0);
  const sessionControls = readSessionControls();
  const [runProfile, setRunProfile] = useState<RunProfile>(readRunProfile);
  const [adminOpen, setAdminOpen] = useState(false);
  const [checkoutOpen, setCheckoutOpen] = useState(false);
  const [commandPaletteOpen, setCommandPaletteOpen] = useState(false);
  const [paletteConversations, setPaletteConversations] = useState<ConversationSummary[]>([]);
  const [taskState, setTaskState] = useState<TaskManagerState | null>(null);
  const [deploymentStatus, setDeploymentStatus] = useState<DeploymentStatus | null>(null);
  const [isAdmin, setIsAdmin] = useState(false);
  const [personalTaskManagerOpen, setPersonalTaskManagerOpen] = useState(false);
  const [taskManagerSwitcherOpen, setTaskManagerSwitcherOpen] = useState(false);
  const [sessionExpired, setSessionExpired] = useState(false);
  const [isOffline, setIsOffline] = useState(!navigator.onLine);
  const billingEnabled = clerkEnabled && isSignedIn;

  const handleConversationCreated = useCallback((conversationId: string) => {
    setGroupConversation(activeGroupId, conversationId);
  }, [activeGroupId, setGroupConversation]);

  const handleConversationsChanged = useCallback(() => {
    setConversationListVersion((version) => version + 1);
  }, []);

  useEffect(() => {
    if (getToken) setAuthTokenGetter(getToken);
  }, [getToken]);

  useEffect(() => {
    let cancelled = false;
    async function refreshDeploymentStatus() {
      try {
        const status = await getDeploymentStatus();
        if (!cancelled) setDeploymentStatus(status);
      } catch {
        if (!cancelled) setDeploymentStatus(null);
      }
    }
    void refreshDeploymentStatus();
    const interval = window.setInterval(refreshDeploymentStatus, 60_000);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, []);

  useEffect(() => {
    setTaskState(readTaskManagerState(activeGroup, userId ?? 'local'));
  }, [activeGroup, userId]);

  useEffect(() => {
    const refreshConversations = () => {
      void listConversations(userId ?? 'local')
        .then(setPaletteConversations)
        .catch(() => setPaletteConversations([]));
    };
    refreshConversations();
    const interval = window.setInterval(refreshConversations, 8000);
    return () => window.clearInterval(interval);
  }, [conversationListVersion, userId]);

  useEffect(() => {
    const channel = typeof BroadcastChannel === 'undefined'
      ? null
      : new BroadcastChannel(TASK_MANAGER_CHANNEL_NAME);
    const onMessage = (event: MessageEvent<{ groupId?: string }>) => {
      if (event.data?.groupId === activeGroup.id) {
        setTaskState(readTaskManagerState(activeGroup, userId ?? 'local'));
      }
    };
    channel?.addEventListener('message', onMessage);
    const onStorage = (event: StorageEvent) => {
      if (event.key?.includes(`cortex:task-manager:${activeGroup.id}`)) {
        setTaskState(readTaskManagerState(activeGroup, userId ?? 'local'));
      }
    };
    window.addEventListener('storage', onStorage);
    return () => {
      channel?.removeEventListener('message', onMessage);
      channel?.close();
      window.removeEventListener('storage', onStorage);
    };
  }, [activeGroup, userId]);

  const groupNotFound = Boolean(groupId && !groups.some((group) => group.id === groupId));

  useEffect(() => {
    // Listen for 401 events dispatched by the API layer or other parts of the app
    function handle401(event: CustomEvent<unknown>) {
      void event;
      setSessionExpired(true);
    }
    window.addEventListener('cortex:unauthorized', handle401 as EventListener);

    // Multi-tab session sync: listen for logout broadcast from other tabs
    const authChannel = typeof BroadcastChannel !== 'undefined'
      ? new BroadcastChannel(AUTH_CHANNEL_NAME)
      : null;
    const onAuthMessage = (event: MessageEvent<{ type?: string }>) => {
      if (event.data?.type === 'logout') {
        setSessionExpired(true);
      }
    };
    authChannel?.addEventListener('message', onAuthMessage);

    return () => {
      window.removeEventListener('cortex:unauthorized', handle401 as EventListener);
      authChannel?.removeEventListener('message', onAuthMessage);
      authChannel?.close();
    };
  }, []);

  useEffect(() => {
    const goOnline = () => setIsOffline(false);
    const goOffline = () => setIsOffline(true);
    window.addEventListener('online', goOnline);
    window.addEventListener('offline', goOffline);
    return () => {
      window.removeEventListener('online', goOnline);
      window.removeEventListener('offline', goOffline);
    };
  }, []);

  const billing = useBilling(billingEnabled);

  // Deploy drift prevention: auto-retry billing if frontend detects drift
  useEffect(() => {
    if (billingEnabled && billing.error && !billing.loading) {
      const isDriftError = billing.error.includes('Billing is unavailable') ||
                          billing.error.includes('fetch') ||
                          billing.error.includes('NetworkError');
      if (isDriftError) {
        console.log('Detected potential deploy drift, retrying billing in 5s...');
        const retryTimer = setTimeout(() => {
          billing.refresh();
        }, 5000);
        return () => clearTimeout(retryTimer);
      }
    }
  }, [billing.error, billing.loading, billingEnabled, billing.refresh]);

  useEffect(() => {
    if (!isSignedIn) {
      setIsAdmin(false);
      return;
    }
    let cancelled = false;
    async function checkAdmin() {
      try {
        await getAdminStats();
        if (!cancelled) setIsAdmin(true);
      } catch (err) {
        if (!cancelled) setIsAdmin(err instanceof CortexApiError ? false : false);
      }
    }
    void checkAdmin();
    return () => { cancelled = true; };
  }, [isSignedIn]);


  useEffect(() => {
    try {
      window.localStorage.setItem(RUN_PROFILE_STORAGE_KEY, runProfile);
    } catch {
      // ignore local preference persistence failures
    }
  }, [runProfile]);

  useEffect(() => {
    let cancelled = false;

    async function loadRoutingProfile() {
      try {
        const routing = await getUserRouting();
        if (!cancelled && isRunProfile(routing.profile)) {
          setRunProfile(routing.profile);
        }
      } catch {
        // keep local profile preference when backend routing settings are unavailable
      }
    }

    void loadRoutingProfile();
    return () => {
      cancelled = true;
    };
  }, []);

  const {
    messages,
    draft,
    isStreaming,
    isLoadingConversation,
    conversationNotFound,
    activeConversationTitle,
    setDraft,
    sendMessage,
    stopStreaming,
    updateApproval,
    renameConversation,
  } = useChatSession({
    activeConversationId,
    userId: userId ?? 'local',
    isSignedIn: Boolean(isSignedIn),
    group: activeGroup,
    sessionControls,
    runProfile,
    onConversationCreated: handleConversationCreated,
    onConversationsChanged: handleConversationsChanged,
  });

  const titleInputRef = useRef<HTMLInputElement | null>(null);
  const skipTitleBlurSaveRef = useRef(false);

  const handleLaunchTaskInProjectChat = useCallback((task: TaskManagerTask) => {
    const lines = [
      `Work on task: ${task.title}`,
      task.repo ? `Repo/context: ${task.repo}` : null,
      `Task id: ${task.id}`,
      task.projectChatConversationId ? `Attached conversation: ${task.projectChatConversationId}` : null,
      '',
      'Start by confirming the goal, identifying the safest first step, and then proceed with the work.',
    ].filter(Boolean);
    setDraft(lines.join('\n'));
  }, [setDraft]);

  useEffect(() => {
    if (conversationNotFound && activeConversationId) {
      setGroupConversation(activeGroupId, null);
    }
  }, [conversationNotFound, activeConversationId, activeGroupId, setGroupConversation]);

  const handleNewChat = useCallback(() => {
    setGroupConversation(activeGroupId, null);
    setSidebarOpen(false);
  }, [activeGroupId, setGroupConversation]);

  const handleSelectConversation = useCallback((id: string) => {
    setGroupConversation(activeGroupId, id);
    setSidebarOpen(false);
  }, [activeGroupId, setGroupConversation]);

  const handleCreateGroup = useCallback(() => {
    const nextGroup = createTeamGroup(groups);
    addTeamGroup(nextGroup);
    navigate(`/app/groups/${nextGroup.id}/tasks`);
    setSidebarOpen(false);
  }, [addTeamGroup, groups, navigate]);

  const handleOpenSettings = useCallback((tab: SettingsTab = 'providers') => {
    setSettingsInitialTab(tab);
    setSettingsOpen(true);
    setSidebarOpen(false);
  }, []);

  const handleOpenAdmin = useCallback(() => {
    setAdminOpen(true);
    setSidebarOpen(false);
  }, []);

  const handleCreateTaskFromPalette = useCallback(() => {
    setDraft((currentDraft) => currentDraft.trim() ? currentDraft : 'Create task: ');
    setSidebarOpen(false);
    window.setTimeout(() => {
      document.querySelector<HTMLTextAreaElement>('textarea')?.focus();
    }, 0);
  }, [setDraft]);

  const handlePopOutTaskManager = useCallback(() => {
    openDetachedPanel('task-manager', activeGroup);
  }, [activeGroup]);



  const headerTitle = activeConversationTitle?.trim() || 'New chat';
  const accessState = billing.status?.access_state;
  const isAdminBypass = isAdmin; // Admin emails get full access without subscription
  const isFreeTier = billingEnabled && !isAdminBypass && isFreeTierAccessState(accessState);
  const runtimeLocked = billingEnabled && !isAdminBypass && accessState === 'payment_failed';
  useEffect(() => {
    if (!renamingTitle) return;
    titleInputRef.current?.focus();
    titleInputRef.current?.select();
  }, [renamingTitle]);

  const startTitleRename = useCallback(() => {
    if (!activeConversationId) return;
    setTitleDraft(headerTitle);
    setRenamingTitle(true);
  }, [activeConversationId, headerTitle]);

  const cancelTitleRename = useCallback(() => {
    skipTitleBlurSaveRef.current = true;
    setRenamingTitle(false);
    setTitleDraft('');
  }, []);

  const saveTitleRename = useCallback(() => {
    const nextTitle = titleDraft.trim();
    skipTitleBlurSaveRef.current = false;
    if (!nextTitle) {
      cancelTitleRename();
      return;
    }

    setRenamingTitle(false);
    setTitleDraft('');
    void renameConversation(nextTitle);
  }, [cancelTitleRename, renameConversation, titleDraft]);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      const isTextInput = target?.tagName === 'INPUT'
        || target?.tagName === 'TEXTAREA'
        || target?.isContentEditable;
      const hasModifier = event.metaKey || event.ctrlKey;
      const key = event.key.toLowerCase();

      if (hasModifier && key === 'k') {
        event.preventDefault();
        setCommandPaletteOpen((open) => !open);
        return;
      }

      if (event.key === 'Escape') {
        if (commandPaletteOpen) {
          setCommandPaletteOpen(false);
          return;
        }
        if (checkoutOpen) {
          setCheckoutOpen(false);
          return;
        }
        if (adminOpen) {
          setAdminOpen(false);
          return;
        }
        if (settingsOpen) {
          setSettingsOpen(false);
          return;
        }
        if (personalTaskManagerOpen) {
          setPersonalTaskManagerOpen(false);
          return;
        }
        if (taskManagerSwitcherOpen) {
          setTaskManagerSwitcherOpen(false);
          return;
        }
        if (sidebarOpen) {
          setSidebarOpen(false);
          return;
        }
        if (isStreaming) {
          stopStreaming();
        }
        return;
      }

      if (isTextInput) return;

      if (!hasModifier) return;

      if (key === 'n') {
        event.preventDefault();
        handleNewChat();
      } else if (key === 'b') {
        event.preventDefault();
        setSidebarOpen((open) => !open);
      } else if (key === ',') {
        event.preventDefault();
        handleOpenSettings();
      } else if (key === 'p') {
        event.preventDefault();
        setTaskManagerSwitcherOpen(!taskManagerSwitcherOpen);
      } else if (key === 'o' && event.shiftKey) {
        event.preventDefault();
        handlePopOutTaskManager();
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [
    adminOpen,
    checkoutOpen,
    commandPaletteOpen,
    handleNewChat,
    handleOpenSettings,
    handlePopOutTaskManager,
    isStreaming,
    personalTaskManagerOpen,
    settingsOpen,
    sidebarOpen,
    stopStreaming,
    taskManagerSwitcherOpen,
  ]);

  if (!isLoaded) {
    return (
      <div className="flex min-h-screen items-center justify-center bg-[var(--bg)]">
        <Loader2 className="h-6 w-6 animate-spin text-[var(--muted)]" />
      </div>
    );
  }

  // Auth gate: enforce sign-in and onboarding flow
  if (clerkEnabled && !isSignedIn) {
    return <SignInScreen />;
  }

  // Onboarding flow for new users
  if (clerkEnabled && isSignedIn && userId && !isOnboardingComplete(userId)) {
    return (
      <OnboardingFlow
        userId={userId}
        onComplete={() => {
          // Force re-render after onboarding completion
          window.location.reload();
        }}
      />
    );
  }

  return (
    <div className="flex h-dvh overflow-hidden bg-[var(--bg)] text-[var(--fg)]">
      {isOffline && (
        <div className="fixed top-0 left-0 right-0 z-50 bg-amber-500/90 text-black text-center text-sm py-2 font-medium">
          You're offline. Some features may be unavailable.
        </div>
      )}
      <a
        href="#main-content"
        className="sr-only focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-50 focus:bg-white focus:text-black focus:px-4 focus:py-2 focus:rounded"
      >
        Skip to content
      </a>
      <aside className="hidden h-full shrink-0 lg:block">
        <GroupSidebar
          groups={groups}
          activeGroupId={activeGroupId}
          userId={userId ?? 'local'}
          isSignedIn={Boolean(isSignedIn)}
          activeConversationId={activeConversationId}
          refreshKey={conversationListVersion}
          onCreateGroup={handleCreateGroup}
          onNewChat={handleNewChat}
          onSelectConversation={handleSelectConversation}
          onConversationsChanged={() => {
            setConversationListVersion((version) => version + 1);
          }}
          onOpenSettings={handleOpenSettings}
          onOpenAdmin={handleOpenAdmin}
          isAdmin={isAdmin}
          billing={billing.status}
        />
      </aside>

      {sidebarOpen && (
        <div className="fixed inset-0 z-40 lg:hidden">
          <button
            type="button"
            aria-label="Close conversations"
            className="absolute inset-0 bg-black/50 backdrop-blur-sm"
            onClick={() => setSidebarOpen(false)}
          />
          <div className="relative h-full w-[min(20rem,calc(100vw-3rem))] translate-x-0 border-r border-white/8 bg-[var(--bg)] shadow-2xl">
            <GroupSidebar
              groups={groups}
              activeGroupId={activeGroupId}
              userId={userId ?? 'local'}
              isSignedIn={Boolean(isSignedIn)}
              activeConversationId={activeConversationId}
              refreshKey={conversationListVersion}
              onCreateGroup={handleCreateGroup}
              onNewChat={handleNewChat}
              onSelectConversation={handleSelectConversation}
              onConversationsChanged={() => {
                setConversationListVersion((version) => version + 1);
              }}
              onOpenSettings={handleOpenSettings}
              onOpenAdmin={handleOpenAdmin}
              isAdmin={isAdmin}
              billing={billing.status}
            />
          </div>
        </div>
      )}

      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-12 shrink-0 items-center justify-between border-b border-white/6 px-3 sm:px-4">
          <div className="flex min-w-0 items-center gap-2">
            <button
              type="button"
              className="inline-flex h-9 w-9 items-center justify-center rounded-lg border border-[var(--accent)]/20 bg-[var(--accent)]/10 text-[var(--accent)] transition hover:bg-[var(--accent)]/20 hover:border-[var(--accent)]/30 active:scale-95"
              aria-label="Open Personal Task Manager"
              title="Personal Task Manager - Master overview across all teams"
              onClick={() => setTaskManagerSwitcherOpen(!taskManagerSwitcherOpen)}
            >
              <LayoutGrid className="h-4 w-4" />
            </button>
            <button
              type="button"
              className="inline-flex h-9 w-9 items-center justify-center rounded-lg text-[var(--muted)] transition hover:bg-white/6 hover:text-white active:scale-95 lg:hidden"
              aria-label="Open conversations"
              title="Open conversations (Ctrl+B)"
              onClick={() => setSidebarOpen(true)}
            >
              <Menu className="h-4 w-4" />
            </button>
            <div className="min-w-0">
              {renamingTitle ? (
                <input
                  ref={titleInputRef}
                  value={titleDraft}
                  onChange={(event) => setTitleDraft(event.target.value)}
                  onBlur={() => {
                    if (skipTitleBlurSaveRef.current) {
                      skipTitleBlurSaveRef.current = false;
                      return;
                    }
                    saveTitleRename();
                  }}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter') {
                      event.preventDefault();
                      saveTitleRename();
                    } else if (event.key === 'Escape') {
                      event.preventDefault();
                      cancelTitleRename();
                    }
                  }}
                  className="block max-w-[60vw] rounded-md border border-white/10 bg-white/8 px-2 py-1 text-sm font-medium text-white outline-none transition focus:border-white/16"
                />
              ) : (
                <button
                  type="button"
                  className="block max-w-[60vw] truncate text-left text-sm font-medium text-white transition hover:text-[var(--accent)] disabled:hover:text-white"
                  title={activeConversationId ? 'Rename conversation' : 'Start a chat to rename it'}
                  disabled={!activeConversationId}
                  onClick={startTitleRename}
                >
                  {headerTitle}
                </button>
              )}
              <div className="hidden text-[11px] text-[var(--muted)] sm:block">
                {activeGroup.name} task manager
              </div>
            </div>
          </div>
          <div className="flex items-center gap-2">
            {deploymentStatus?.frontend.drift ? (
              <span
                className="hidden items-center gap-1.5 rounded-full border border-amber-300/20 bg-amber-300/10 px-2 py-0.5 text-[11px] text-amber-100 md:inline-flex"
                title={`Live asset ${deploymentStatus.frontend.live_assets?.js ?? 'unknown'} differs from expected ${deploymentStatus.frontend.expected_assets.js ?? 'unknown'}`}
              >
                <AlertTriangle className="h-3.5 w-3.5" />
                Deploy drift
              </span>
            ) : deploymentStatus?.status === 'match' ? (
              <span
                className="hidden items-center gap-1.5 rounded-full border border-emerald-300/20 bg-emerald-300/10 px-2 py-0.5 text-[11px] text-emerald-100 md:inline-flex"
                title={deploymentBadgeTitle(deploymentStatus)}
              >
                <CheckCircle2 className="h-3.5 w-3.5" />
                Deployed
              </span>
            ) : null}
            {isStreaming && (
              <span className="flex items-center gap-1.5 rounded-full border border-emerald-500/20 bg-emerald-500/10 px-2 py-0.5 text-[11px] text-emerald-200">
                <span className="h-1.5 w-1.5 rounded-full bg-emerald-300 animate-pulse" />
                Streaming
              </span>
            )}
            <button
              type="button"
              className="hidden h-9 items-center gap-2 rounded-lg border border-white/8 bg-white/[0.03] px-2.5 text-xs text-[var(--muted)] transition hover:bg-white/6 hover:text-white active:scale-95 sm:inline-flex"
              aria-label="Open command palette"
              title="Open command palette (Cmd/Ctrl+K)"
              onClick={() => setCommandPaletteOpen(true)}
            >
              <Search className="h-3.5 w-3.5" />
              <span>Search</span>
              <kbd className="rounded border border-white/10 bg-black/20 px-1 text-[10px]">K</kbd>
            </button>
            <button
              type="button"
              className="hidden h-9 w-9 items-center justify-center rounded-lg text-[var(--muted)] transition hover:bg-white/6 hover:text-white active:scale-95 lg:inline-flex"
              aria-label="Pop out task manager"
              title="Pop out Task Manager (Cmd/Ctrl+Shift+O)"
              onClick={handlePopOutTaskManager}
            >
              <ExternalLink className="h-4 w-4" />
            </button>
            <HelpMenu />
            {CLERK_ENABLED && clerkEnabled && isSignedIn && <ClerkUserControls />}
          </div>
        </header>

        {/* Task Manager Switcher */}
        <TaskManagerSwitcher
          groups={groups}
          userId={userId ?? 'local'}
          activeGroupId={activeGroupId}
          isOpen={taskManagerSwitcherOpen}
          onClose={() => setTaskManagerSwitcherOpen(false)}
          onSwitchToGroup={(groupId) => {
            navigate(`/app/groups/${groupId}/tasks`);
          }}
          onOpenPersonalTaskManager={() => {
            setPersonalTaskManagerOpen(true);
          }}
        />

        {!isAdminBypass && (
          <TrialBanner billing={billing.status} onOpenBilling={() => handleOpenSettings('billing')} />
        )}
        {isFreeTier && accessState && !(billing.status?.trial && billing.status?.access_state === 'trial_active') && (
          <FreeTierBanner
            accessState={accessState}
            onOpenBilling={() => handleOpenSettings('billing')}
          />
        )}
        {runtimeLocked && (
          <PaymentIssueBanner onOpenBilling={() => handleOpenSettings('billing')} />
        )}
        {sessionExpired && (
          <div className="border-b border-red-400/20 bg-red-400/10 px-3 py-2 sm:px-4">
            <div className="mx-auto flex max-w-4xl flex-wrap items-center gap-2 sm:gap-3">
              <AlertTriangle className="h-4 w-4 shrink-0 text-red-200" />
              <p className="min-w-[12rem] flex-1 text-xs text-red-100">
                Your session has expired. Please sign in again to continue.
              </p>
              <a
                href="/sign-in"
                className="inline-flex h-7 items-center rounded-lg border border-red-200/20 bg-red-100/10 px-2.5 text-xs text-red-50 transition hover:bg-red-100/15 active:scale-95"
              >
                Sign in
              </a>
            </div>
          </div>
        )}
        {groupNotFound ? (
          <div className="flex flex-1 items-center justify-center">
            <div className="w-full max-w-xs rounded-2xl border border-white/8 bg-[var(--panel)] p-6 text-center">
              <div className="mx-auto flex h-10 w-10 items-center justify-center rounded-xl bg-amber-500/10 text-amber-200">
                <AlertTriangle className="h-5 w-5" />
              </div>
              <h2 className="mt-4 text-base font-semibold text-white">Group not found</h2>
              <p className="mt-2 text-sm text-[var(--muted)]">
                The group you are looking for does not exist or has been removed.
              </p>
              <a
                href={`/app/groups/${DEFAULT_GROUPS[0].id}/tasks`}
                className="mt-4 inline-flex items-center gap-2 rounded-xl bg-[var(--accent)] px-4 py-2 text-sm font-medium text-black transition hover:brightness-110 active:scale-95"
              >
                Go to default group
              </a>
            </div>
          </div>
        ) : null}

        {conversationNotFound && !groupNotFound ? (
          <div className="flex flex-1 items-center justify-center">
            <div className="w-full max-w-xs rounded-2xl border border-white/8 bg-[var(--panel)] p-6 text-center">
              <div className="mx-auto flex h-10 w-10 items-center justify-center rounded-xl bg-amber-500/10 text-amber-200">
                <AlertTriangle className="h-5 w-5" />
              </div>
              <h2 className="mt-4 text-base font-semibold text-white">Conversation not found</h2>
              <p className="mt-2 text-sm text-[var(--muted)]">
                This conversation does not exist or has been deleted.
              </p>
              <button
                type="button"
                onClick={handleNewChat}
                className="mt-4 inline-flex items-center gap-2 rounded-xl bg-[var(--accent)] px-4 py-2 text-sm font-medium text-black transition hover:brightness-110 active:scale-95"
              >
                Start a new chat
              </button>
            </div>
          </div>
        ) : null}

        <div id="main-content" className={`flex min-h-0 flex-1 overflow-hidden${groupNotFound || conversationNotFound ? ' hidden' : ''}`}>
          <ProjectChat
            group={activeGroup}
            userId={userId ?? 'local'}
            activeConversationId={activeConversationId}
            messages={messages}
            draft={draft}
            isStreaming={isStreaming}
            isLoadingConversation={isLoadingConversation}
            needsSubscription={runtimeLocked}
            isPreview={isFreeTier}
            onDraftChange={setDraft}
            onSend={sendMessage}
            onStop={isStreaming ? stopStreaming : undefined}
            onSubscribe={() => handleOpenSettings('billing')}
            onApprovalAction={updateApproval}
          />

          <div className="hidden lg:flex">
            <TaskManagerSidebar
              group={activeGroup}
              userId={userId ?? 'local'}
              activeConversationId={activeConversationId}
              messages={messages}
              draft={draft}
              isStreaming={isStreaming}
              isLoadingConversation={isLoadingConversation}
              needsSubscription={runtimeLocked}
              runProfile={runProfile}
              onDraftChange={setDraft}
              onSend={sendMessage}
              onStop={isStreaming ? stopStreaming : undefined}
              onSubscribe={() => handleOpenSettings('billing')}
              onApprovalAction={updateApproval}
              onTaskStateChange={setTaskState}
              onLaunchTaskInProjectChat={handleLaunchTaskInProjectChat}
            />
          </div>
        </div>
      </div>

      {/* WorkSurface hidden for clean interface */}

      {/* Settings modal */}
      {settingsOpen && (
        <Suspense fallback={null}>
          <SettingsPanel
            onClose={() => setSettingsOpen(false)}
            initialTab={settingsInitialTab}
            billing={billing.status}
            isAdmin={isAdmin}
          />
        </Suspense>
      )}

      {/* Admin panel */}
      {adminOpen && (
        <Suspense fallback={null}>
          <AdminPanel onClose={() => setAdminOpen(false)} />
        </Suspense>
      )}

      {/* Personal Task Manager */}
      {personalTaskManagerOpen && (
        <PersonalTaskManager
          groups={groups}
          userId={userId ?? 'local'}
          activeGroupId={activeGroupId}
          onClose={() => setPersonalTaskManagerOpen(false)}
          onSwitchToGroup={(groupId) => {
            navigate(`/app/groups/${groupId}/tasks`);
            setPersonalTaskManagerOpen(false);
          }}
        />
      )}

      {/* Checkout modal (fallback for in-app subscribe links) */}
      {checkoutOpen && (
        <Suspense fallback={null}>
          <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/80 p-3 backdrop-blur-md sm:p-6">
            <div className="relative w-full max-w-lg">
              <button
                type="button"
                onClick={() => setCheckoutOpen(false)}
                className="absolute -top-10 right-0 rounded-lg p-1.5 text-[var(--muted)] transition hover:text-white"
                aria-label="Close"
              >
                <span className="text-sm">ESC</span>
              </button>
              <PricingCards compact />
            </div>
          </div>
        </Suspense>
      )}

      <CommandPalette
        open={commandPaletteOpen}
        groups={groups}
        activeGroupId={activeGroupId}
        conversations={paletteConversations}
        taskState={taskState}
        onClose={() => setCommandPaletteOpen(false)}
        onCreateTask={handleCreateTaskFromPalette}
        onCreateGroup={handleCreateGroup}
        onNewChat={handleNewChat}
        onSelectGroup={(nextGroupId) => navigate(`/app/groups/${nextGroupId}/tasks`)}
        onSelectConversation={handleSelectConversation}
        onOpenSettings={() => handleOpenSettings()}
        onPopOutTaskManager={handlePopOutTaskManager}
      />
    </div>
  );
}

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        {/* Mission control — the six panes of cortex/plan/SURFACE.md.
            Chat is the sixth pane and a doorway; it keeps "/" until F2 moves
            the identity of the app onto runs and receipts. */}
        <Route element={<MissionControl />}>
          <Route path="/" element={<CortexShell />} />
          <Route path="/runs" element={<RunsPane />} />
          <Route path="/receipts" element={<ReceiptsPane />} />
          <Route path="/ledger" element={<LedgerPane />} />
          <Route path="/leases" element={<LeasesPane />} />
          <Route path="/admin" element={<AdminPane />} />
        </Route>
        <Route path="/app" element={<Navigate to="/" replace />} />
        <Route path="/app/groups/:groupId/tasks" element={<CortexShell />} />
        <Route path="/app/groups/:groupId/operations" element={<OperationsRoom />} />
        <Route path="/projects" element={<ProjectsView />} />
        <Route path="/projects/:projectId" element={<ProjectsView />} />
        {/* Legacy redirects */}
        <Route path="/groups/:groupId/tasks" element={<LegacyGroupRedirect />} />
        <Route path="*" element={<NotFoundPage />} />
      </Routes>
    </BrowserRouter>
  );
}
