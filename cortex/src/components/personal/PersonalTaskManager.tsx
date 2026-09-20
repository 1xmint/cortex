import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useNavigate } from 'react-router';
import {
  AlertTriangle,
  ArrowRight,
  CheckCircle2,
  Expand,
  GitBranch,
  Loader2,
  Pause,
  Shield,
  ShieldCheck,
  X,
  XCircle,
  Users,
  LayoutGrid,
  Zap,
} from 'lucide-react';
import { buildTaskSummary, readTaskManagerState, useTaskManager } from '../../lib/taskManager';
import type { CortexGroup } from '../../lib/groups';
import { getPersonalOperationsSummary, type PersonalOperationsGroupSummary, type PersonalOperationsSummary, type RunOperationEvent } from '../../lib/cortexApi';

interface PersonalTaskManagerProps {
  groups: CortexGroup[];
  userId: string;
  activeGroupId: string;
  onClose: () => void;
  onSwitchToGroup: (groupId: string) => void;
}

type OverviewTab = 'overview' | 'groups';

function localPersonalSummary(groups: CortexGroup[], userId: string) {
  return groups.reduce((acc, group) => {
    const summary = buildTaskSummary(readTaskManagerState(group, userId));
    return {
      open: acc.open + summary.open,
      inProgress: acc.inProgress + summary.inProgress,
      done: acc.done + summary.done,
    };
  }, { open: 0, inProgress: 0, done: 0 });
}

function countNonApprovalAttention(items: { kind?: string | null }[] | undefined) {
  return items?.filter((item) => item.kind !== 'approval_pending').length ?? 0;
}

function usePersonalOperationsSummary() {
  const [summary, setSummary] = useState<PersonalOperationsSummary | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const next = await getPersonalOperationsSummary();
      setSummary(next);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Operations summary unavailable');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    refresh();
    const interval = window.setInterval(refresh, 20_000);
    return () => window.clearInterval(interval);
  }, [refresh]);

  return { summary, loading, error, refresh };
}

function GroupOverview({
  group,
  userId,
  operations,
  isActive,
  onClick,
  onOpenOperationsRoom,
}: {
  group: CortexGroup;
  userId: string;
  operations?: PersonalOperationsGroupSummary;
  isActive: boolean;
  onClick: () => void;
  onOpenOperationsRoom?: () => void;
}) {
  const taskManager = useTaskManager(group, userId);
  const localSummary = taskManager.summary;
  const open = operations?.tasks.open ?? localSummary.open;
  const active = operations?.tasks.active ?? localSummary.inProgress;
  const done = operations?.tasks.completion.gated_done ?? operations?.tasks.done_raw ?? localSummary.done;
  const needsAttention = countNonApprovalAttention(operations?.attention);
  const pendingApprovals = operations?.approvals.pending ?? 0;
  const activeLeases = operations?.resource_leases.active ?? 0;

  return (
    <div
      className={`rounded-xl border p-4 transition-all cursor-pointer ${
        isActive
          ? 'border-[var(--accent)]/40 bg-[var(--accent)]/10'
          : 'border-white/8 bg-white/[0.03] hover:bg-white/[0.06] hover:border-white/12'
      }`}
      onClick={onClick}
    >
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <div
              className="h-3 w-3 rounded-full"
              style={{ backgroundColor: group.accent }}
            />
            <h3 className="font-medium text-white">{group.name}</h3>
            {group.kind === 'team' && (
              <span className="text-xs text-[var(--muted)]">
                {group.members} members
              </span>
            )}
          </div>
          <p className="mt-1 text-xs text-[var(--muted)]">
            {group.description}
          </p>
        </div>
        {isActive && (
          <div className="rounded-full bg-[var(--accent)]/20 px-2 py-1 text-[10px] font-medium text-[var(--accent)]">
            Active
          </div>
        )}
      </div>

      <div className="mt-4 grid grid-cols-4 gap-2">
        <div className="rounded-lg border border-white/8 bg-white/[0.03] px-3 py-2 text-center">
          <div className="text-lg font-semibold text-white">{open}</div>
          <div className="text-[10px] uppercase tracking-wide text-[var(--muted)]">Open</div>
        </div>
        <div className="rounded-lg border border-white/8 bg-white/[0.03] px-3 py-2 text-center">
          <div className="text-lg font-semibold text-emerald-300">{active}</div>
          <div className="text-[10px] uppercase tracking-wide text-[var(--muted)]">Active</div>
        </div>
        <div className="rounded-lg border border-white/8 bg-white/[0.03] px-3 py-2 text-center">
          <div className="text-lg font-semibold text-blue-300">{done}</div>
          <div className="text-[10px] uppercase tracking-wide text-[var(--muted)]">Done</div>
        </div>
        <div className="rounded-lg border border-white/8 bg-white/[0.03] px-3 py-2 text-center">
          <div className="flex items-center justify-center gap-1">
            <div className="text-lg font-semibold text-violet-300">{operations?.runs.active ?? 0}</div>
            {(operations?.runs.active ?? 0) > 0 && (
              <span className="h-2 w-2 animate-pulse rounded-full bg-violet-400" />
            )}
          </div>
          <div className="text-[10px] uppercase tracking-wide text-[var(--muted)]">Runs</div>
        </div>
      </div>

      {operations && (needsAttention > 0 || pendingApprovals > 0 || activeLeases > 0 || operations.steps.failed > 0) && (
        <div className="mt-3 flex flex-wrap gap-2 text-[11px]">
          {needsAttention > 0 && (
            <span className="rounded-full border border-amber-300/20 bg-amber-300/10 px-2 py-1 text-amber-200">
              {needsAttention} need attention
            </span>
          )}
          {pendingApprovals > 0 && (
            <span className="rounded-full border border-sky-300/20 bg-sky-300/10 px-2 py-1 text-sky-200">
              {pendingApprovals} approval{pendingApprovals === 1 ? '' : 's'}
            </span>
          )}
          {activeLeases > 0 && (
            <span className="rounded-full border border-sky-300/20 bg-sky-300/10 px-2 py-1 text-sky-200">
              {activeLeases} active leases
            </span>
          )}
          {operations.steps.failed > 0 && (
            <span className="rounded-full border border-rose-300/20 bg-rose-300/10 px-2 py-1 text-rose-200">
              {operations.steps.failed} failed steps
            </span>
          )}
        </div>
      )}
      {onOpenOperationsRoom && (
        <button
          type="button"
          onClick={(e) => { e.stopPropagation(); onOpenOperationsRoom(); }}
          className="mt-3 inline-flex h-8 w-full items-center justify-center gap-2 rounded-lg border border-white/8 bg-white/[0.03] text-xs text-[var(--muted-strong)] transition hover:bg-white/[0.07] hover:text-white active:scale-[0.99]"
        >
          <Expand className="h-3.5 w-3.5" />
          Open Operations Room
        </button>
      )}
    </div>
  );
}

interface ActionItem {
  id: string;
  label: string;
  detail: string;
  groupId: string;
  groupName: string;
  urgency: 'critical' | 'high' | 'normal';
  kind: 'approval' | 'failure' | 'attention' | 'blocker' | 'paused';
}

function buildActionItems(
  operations: PersonalOperationsSummary | null,
  groups: CortexGroup[],
): ActionItem[] {
  if (!operations) return [];
  const items: ActionItem[] = [];
  const groupMap = new Map(groups.map((g) => [g.id, g.name]));

  for (const group of operations.groups) {
    const groupName = groupMap.get(group.group_id) ?? group.group_id;

    if (group.approvals.pending > 0) {
      items.push({
        id: `approval-${group.group_id}`,
        label: `${group.approvals.pending} pending approval${group.approvals.pending === 1 ? '' : 's'}`,
        detail: groupName,
        groupId: group.group_id,
        groupName,
        urgency: 'high',
        kind: 'approval',
      });
    }

    if (group.runs.failed > 0) {
      items.push({
        id: `failed-${group.group_id}`,
        label: `${group.runs.failed} failed run${group.runs.failed === 1 ? '' : 's'}`,
        detail: groupName,
        groupId: group.group_id,
        groupName,
        urgency: 'critical',
        kind: 'failure',
      });
    }

    if (group.steps.failed > 0) {
      items.push({
        id: `steps-failed-${group.group_id}`,
        label: `${group.steps.failed} failed step${group.steps.failed === 1 ? '' : 's'}`,
        detail: groupName,
        groupId: group.group_id,
        groupName,
        urgency: 'high',
        kind: 'failure',
      });
    }

    // Blocker: tasks with failed runs and no active runs (not retried)
    if (group.runs.failed > 0 && group.runs.active === 0) {
      items.push({
        id: `blocker-${group.group_id}`,
        label: `${group.runs.failed} blocked task${group.runs.failed === 1 ? '' : 's'} (failed, not retried)`,
        detail: groupName,
        groupId: group.group_id,
        groupName,
        urgency: 'critical',
        kind: 'blocker',
      });
    }

    // Paused tasks: attention items with kind 'paused' or status 'paused'
    const pausedItems = group.attention?.filter((a) => a.kind === 'paused' || a.status === 'paused') ?? [];
    if (pausedItems.length > 0) {
      items.push({
        id: `paused-${group.group_id}`,
        label: `${pausedItems.length} paused task${pausedItems.length === 1 ? '' : 's'}`,
        detail: groupName,
        groupId: group.group_id,
        groupName,
        urgency: 'normal',
        kind: 'paused',
      });
    }

    const nonApprovalAttention = group.attention?.filter((a) => a.kind !== 'approval_pending' && a.kind !== 'paused' && a.status !== 'paused') ?? [];
    if (nonApprovalAttention.length > 0) {
      items.push({
        id: `attention-${group.group_id}`,
        label: `${nonApprovalAttention.length} signal${nonApprovalAttention.length === 1 ? '' : 's'} need attention`,
        detail: groupName,
        groupId: group.group_id,
        groupName,
        urgency: 'normal',
        kind: 'attention',
      });
    }
  }

  items.sort((a, b) => {
    const urgencyOrder = { critical: 0, high: 1, normal: 2 };
    return urgencyOrder[a.urgency] - urgencyOrder[b.urgency];
  });

  return items;
}

function ActionItemCard({
  item,
  onNavigate,
}: {
  item: ActionItem;
  onNavigate: (groupId: string) => void;
}) {
  const border = item.urgency === 'critical' ? 'border-red-300/20' : item.urgency === 'high' ? 'border-amber-300/20' : 'border-white/8';
  const bg = item.urgency === 'critical' ? 'bg-red-400/[0.06]' : item.urgency === 'high' ? 'bg-amber-300/[0.06]' : 'bg-white/[0.03]';
  const Icon = item.kind === 'failure' ? XCircle : item.kind === 'approval' ? Shield : item.kind === 'paused' ? Pause : AlertTriangle;
  const iconColor = item.urgency === 'critical' ? 'text-red-300' : item.urgency === 'high' ? 'text-amber-200' : 'text-[var(--muted)]';

  return (
    <button
      type="button"
      onClick={() => onNavigate(item.groupId)}
      className={`flex w-full items-center gap-3 rounded-lg border ${border} ${bg} px-4 py-3 text-left transition hover:brightness-110 active:scale-[0.99]`}
    >
      <Icon className={`h-4 w-4 shrink-0 ${iconColor}`} />
      <div className="min-w-0 flex-1">
        <p className="text-sm font-medium text-white">{item.label}</p>
        <p className="text-[11px] text-[var(--muted)]">{item.detail}</p>
      </div>
      <ArrowRight className="h-4 w-4 shrink-0 text-[var(--muted)]" />
    </button>
  );
}

function WhatNeedsMeNow({
  items,
  onNavigate,
}: {
  items: ActionItem[];
  onNavigate: (groupId: string) => void;
}) {
  if (items.length === 0) {
    return (
      <div className="rounded-xl border border-emerald-300/15 bg-emerald-400/[0.04] px-6 py-8 text-center">
        <CheckCircle2 className="mx-auto h-8 w-8 text-emerald-300" />
        <p className="mt-3 text-sm font-medium text-white">All clear</p>
        <p className="mt-1 text-xs text-[var(--muted)]">Nothing needs your attention right now</p>
      </div>
    );
  }

  return (
    <div className="space-y-2">
      {items.map((item) => (
        <ActionItemCard key={item.id} item={item} onNavigate={onNavigate} />
      ))}
    </div>
  );
}

function RecentActivityFeed({ events }: { events: RunOperationEvent[] }) {
  if (events.length === 0) return null;
  return (
    <div className="max-h-[280px] space-y-1 overflow-y-auto rounded-lg border border-white/8 bg-black/15 p-3">
      {events.slice(0, 20).map((event) => (
        <div key={event.id} className="flex items-center gap-3 rounded-md px-2 py-1.5">
          <div className="shrink-0">
            {event.event_type.includes('failed') ? (
              <XCircle className="h-3 w-3 text-red-300" />
            ) : event.event_type.includes('completed') ? (
              <CheckCircle2 className="h-3 w-3 text-emerald-300" />
            ) : (
              <GitBranch className="h-3 w-3 text-[var(--muted)]" />
            )}
          </div>
          <span className="min-w-0 flex-1 truncate text-[11px] text-[var(--muted-strong)]">
            {event.event_type.replaceAll('.', ' ').replaceAll('_', ' ')}
          </span>
          <span className="shrink-0 text-[10px] text-[var(--muted)]">
            {new Date(event.created_at).toLocaleTimeString()}
          </span>
        </div>
      ))}
    </div>
  );
}

function MasterOverview({
  groups,
  userId,
  operations,
  loading,
  error,
  activeGroupId,
  onSwitchToGroup,
  onOpenOperationsRoom,
}: {
  groups: CortexGroup[];
  userId: string;
  operations: PersonalOperationsSummary | null;
  loading: boolean;
  error: string | null;
  activeGroupId: string;
  onSwitchToGroup: (groupId: string) => void;
  onOpenOperationsRoom: (groupId: string) => void;
}) {
  const fallbackStats = useMemo(() => localPersonalSummary(groups, userId), [groups, userId]);
  const groupOperations = useMemo(() => {
    const byId = new Map<string, PersonalOperationsGroupSummary>();
    operations?.groups.forEach((group) => byId.set(group.group_id, group));
    return byId;
  }, [operations]);
  const open = operations?.tasks.open ?? fallbackStats.open;
  const active = operations?.tasks.active ?? fallbackStats.inProgress;
  const done = operations?.tasks.completion.gated_done ?? operations?.tasks.done_raw ?? fallbackStats.done;
  const attention = countNonApprovalAttention(operations?.attention);
  const pendingApprovals = operations?.approvals.pending ?? 0;
  const activeLeases = operations?.resource_leases.active ?? 0;
  const failedSteps = operations?.steps.failed ?? 0;
  const actionItems = useMemo(() => buildActionItems(operations, groups), [operations, groups]);

  return (
    <div className="space-y-6">
      {/* Master Stats */}
      <div>
        <div className="mb-3 flex items-center justify-between gap-3">
          <div>
            <h2 className="text-lg font-semibold text-white">Master Overview</h2>
            <p className="mt-0.5 text-xs text-[var(--muted)]">
              {operations
                ? `Operations refreshed ${new Date(operations.generated_at).toLocaleTimeString()}`
                : 'Live operations are loading'}
            </p>
          </div>
          {loading ? (
            <div className="inline-flex items-center gap-2 rounded-full border border-white/8 bg-white/[0.03] px-3 py-1 text-xs text-[var(--muted)]">
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
              Syncing
            </div>
          ) : error ? (
            <div className="inline-flex items-center gap-2 rounded-full border border-amber-300/20 bg-amber-300/10 px-3 py-1 text-xs text-amber-200">
              <AlertTriangle className="h-3.5 w-3.5" />
              Local view
            </div>
          ) : (
            <div className="inline-flex items-center gap-2 rounded-full border border-emerald-300/20 bg-emerald-300/10 px-3 py-1 text-xs text-emerald-200">
              <ShieldCheck className="h-3.5 w-3.5" />
              Live source
            </div>
          )}
        </div>
        <div className="grid grid-cols-4 gap-3">
          <div className="rounded-xl border border-white/8 bg-white/[0.03] px-4 py-3 text-center">
            <div className="text-2xl font-bold text-white">{operations?.groups_total ?? groups.length}</div>
            <div className="text-xs text-[var(--muted)]">Scopes</div>
          </div>
          <div className="rounded-xl border border-white/8 bg-white/[0.03] px-4 py-3 text-center">
            <div className="text-2xl font-bold text-white">{open}</div>
            <div className="text-xs text-[var(--muted)]">Open</div>
          </div>
          <div className="rounded-xl border border-white/8 bg-white/[0.03] px-4 py-3 text-center">
            <div className="text-2xl font-bold text-emerald-300">{active}</div>
            <div className="text-xs text-[var(--muted)]">Active</div>
          </div>
          <div className="rounded-xl border border-white/8 bg-white/[0.03] px-4 py-3 text-center">
            <div className="text-2xl font-bold text-blue-300">{done}</div>
            <div className="text-xs text-[var(--muted)]">Completed</div>
          </div>
        </div>
        {operations && (
          <div className="mt-3 grid gap-2 md:grid-cols-4">
            <div className="rounded-lg border border-white/8 bg-white/[0.03] px-3 py-2">
              <div className="text-xs text-[var(--muted)]">Needs attention</div>
              <div className="mt-1 text-lg font-semibold text-amber-200">{attention}</div>
              {pendingApprovals > 0 && (
                <div className="mt-0.5 text-[10px] text-sky-200">
                  {pendingApprovals} pending approval{pendingApprovals === 1 ? '' : 's'}
                </div>
              )}
            </div>
            <div className="rounded-lg border border-white/8 bg-white/[0.03] px-3 py-2">
              <div className="text-xs text-[var(--muted)]">Active leases</div>
              <div className="mt-1 text-lg font-semibold text-sky-200">{activeLeases}</div>
            </div>
            <div className="rounded-lg border border-white/8 bg-white/[0.03] px-3 py-2">
              <div className="text-xs text-[var(--muted)]">Failed steps</div>
              <div className="mt-1 text-lg font-semibold text-rose-200">{failedSteps}</div>
            </div>
            <div className="rounded-lg border border-white/8 bg-white/[0.03] px-3 py-2">
              <div className="text-xs text-[var(--muted)]">Without runs</div>
              <div className="mt-1 text-lg font-semibold text-white">{operations.tasks.without_run}</div>
            </div>
          </div>
        )}
      </div>

      {/* Active Runs */}
      {operations && operations.runs.active > 0 && (
        <div className="rounded-xl border border-violet-300/15 bg-violet-400/[0.04] px-5 py-4">
          <div className="flex items-center gap-3">
            <div className="flex items-center gap-2">
              <GitBranch className="h-4 w-4 text-violet-300" />
              <h3 className="text-sm font-semibold text-white">Active Runs</h3>
            </div>
            <div className="flex items-center gap-1.5 rounded-full border border-violet-300/20 bg-violet-300/10 px-2 py-0.5">
              <span className="h-2 w-2 animate-pulse rounded-full bg-violet-400" />
              <span className="text-xs font-medium text-violet-200">{operations.runs.active} executing</span>
            </div>
          </div>
          <p className="mt-1.5 text-xs text-[var(--muted)]">
            {operations.runs.total} total runs · {operations.runs.failed} failed · {operations.steps.active} active steps
          </p>
        </div>
      )}

      {/* What needs me now */}
      {operations && (
        <div>
          <div className="mb-3 flex items-center gap-2">
            <Zap className="h-4 w-4 text-amber-200" />
            <h3 className="text-sm font-semibold text-white">What needs me now</h3>
          </div>
          <WhatNeedsMeNow items={actionItems} onNavigate={onSwitchToGroup} />
        </div>
      )}

      {/* Activity + Groups side by side */}
      <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
        <div>
          <h3 className="mb-3 text-sm font-medium text-white">Team Coordination</h3>
          <div className="grid gap-3">
            {groups.map((group) => (
              <GroupOverview
                key={group.id}
                group={group}
                userId={userId}
                operations={groupOperations.get(group.id)}
                isActive={group.id === activeGroupId}
                onClick={() => onSwitchToGroup(group.id)}
                onOpenOperationsRoom={() => onOpenOperationsRoom(group.id)}
              />
            ))}
          </div>
        </div>
        <div>
          <h3 className="mb-3 text-sm font-medium text-white">Recent Activity</h3>
          {operations?.recent_events && operations.recent_events.length > 0 ? (
            <RecentActivityFeed events={operations.recent_events} />
          ) : (
            <div className="flex h-32 items-center justify-center rounded-lg border border-white/8 bg-black/15 text-sm text-[var(--muted)]">
              No recent events
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

export default function PersonalTaskManager({
  groups,
  userId,
  activeGroupId,
  onClose,
  onSwitchToGroup,
}: PersonalTaskManagerProps) {
  const navigate = useNavigate();
  const [activeTab, setActiveTab] = useState<OverviewTab>('overview');
  const { summary, loading, error } = usePersonalOperationsSummary();
  const modalRef = useRef<HTMLDivElement>(null);

  // Focus trap: lock keyboard focus inside the modal while open
  useEffect(() => {
    const modal = modalRef.current;
    if (!modal) return;

    const focusableSelector =
      'a[href], button:not([disabled]), textarea:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])';

    // Focus the first focusable element on mount
    const firstFocusable = modal.querySelector<HTMLElement>(focusableSelector);
    firstFocusable?.focus();

    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === 'Escape') {
        event.preventDefault();
        onClose();
        return;
      }

      if (event.key !== 'Tab') return;

      const focusableElements = modal!.querySelectorAll<HTMLElement>(focusableSelector);
      if (focusableElements.length === 0) return;

      const first = focusableElements[0];
      const last = focusableElements[focusableElements.length - 1];

      if (event.shiftKey) {
        if (document.activeElement === first) {
          event.preventDefault();
          last.focus();
        }
      } else {
        if (document.activeElement === last) {
          event.preventDefault();
          first.focus();
        }
      }
    }

    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [onClose]);

  const handleOpenOperationsRoom = useCallback((groupId: string) => {
    onClose();
    navigate(`/app/groups/${groupId}/operations`);
  }, [navigate, onClose]);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/80 p-0 sm:p-4 backdrop-blur-md">
      <div ref={modalRef} role="dialog" aria-modal="true" aria-label="Personal Task Manager" className="relative flex h-full w-full flex-col rounded-none border-0 bg-[var(--bg)] shadow-2xl sm:h-[90vh] sm:max-w-6xl sm:rounded-2xl sm:border sm:border-white/10">
        {/* Header */}
        <div className="border-b border-white/6 px-6 py-4">
          <div className="flex items-center justify-between">
            <div>
              <h1 className="text-xl font-semibold text-white">Personal Task Manager</h1>
              <p className="mt-1 text-sm text-[var(--muted)]">
                Master overview of all your team coordination and work
              </p>
            </div>
            <button
              onClick={onClose}
              className="inline-flex h-10 w-10 items-center justify-center rounded-lg text-[var(--muted)] transition hover:bg-white/6 hover:text-white active:scale-95"
              aria-label="Close Personal Task Manager"
            >
              <X className="h-5 w-5" />
            </button>
          </div>

          {/* Tabs */}
          <div className="mt-4 flex gap-1 rounded-lg border border-white/8 bg-white/[0.03] p-1">
            <button
              onClick={() => setActiveTab('overview')}
              className={[
                'inline-flex h-9 items-center gap-2 rounded-md px-3 text-sm font-medium transition',
                activeTab === 'overview'
                  ? 'bg-white/10 text-white'
                  : 'text-[var(--muted)] hover:text-white',
              ].join(' ')}
            >
              <LayoutGrid className="h-4 w-4" />
              Overview
            </button>
            <button
              onClick={() => setActiveTab('groups')}
              className={[
                'inline-flex h-9 items-center gap-2 rounded-md px-3 text-sm font-medium transition',
                activeTab === 'groups'
                  ? 'bg-white/10 text-white'
                  : 'text-[var(--muted)] hover:text-white',
              ].join(' ')}
            >
              <Users className="h-4 w-4" />
              All Teams
            </button>
          </div>
        </div>

        {/* Content */}
        <div className="flex-1 overflow-y-auto px-4 py-4 sm:px-6 sm:py-6" style={{ height: undefined }}>
          {activeTab === 'overview' ? (
            <MasterOverview
              groups={groups}
              userId={userId}
              operations={summary}
              loading={loading}
              error={error}
              activeGroupId={activeGroupId}
              onSwitchToGroup={onSwitchToGroup}
              onOpenOperationsRoom={handleOpenOperationsRoom}
            />
          ) : (
            <div className="space-y-4">
              <h2 className="text-lg font-semibold text-white">All Teams</h2>
              <div className="grid gap-4 sm:grid-cols-2">
                {groups.map((group) => (
                  <GroupOverview
                    key={group.id}
                    group={group}
                    userId={userId}
                    operations={summary?.groups.find((item) => item.group_id === group.id)}
                    isActive={group.id === activeGroupId}
                    onClick={() => onSwitchToGroup(group.id)}
                    onOpenOperationsRoom={() => handleOpenOperationsRoom(group.id)}
                  />
                ))}
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
