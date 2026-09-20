import {
  Ban,
  CheckCircle2,
  Circle,
  Clock,
  GripVertical,
  ListTodo,
  MessageSquareText,
  Pause,
  Play,
  PlayCircle,
  RefreshCw,
  UserPlus,
  XCircle,
} from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState, type DragEvent } from 'react';
import type { TaskManagerTask, TaskMember, TaskPriority, TaskStatus } from '../../types';
import { getGroupOperationsSummary, type GroupOperationsSummary } from '../../lib/cortexApi';
import { formatTaskStatus } from '../../lib/taskManager';
import BottomSheet from '../ui/BottomSheet';

interface TaskBoardProps {
  tasks: TaskManagerTask[];
  members: TaskMember[];
  groupId?: string;
  compact?: boolean;
  showBackendSignals?: boolean;
  selectedTaskId?: string | null;
  onUpdateTask: (
    taskId: string,
    patch: Partial<Pick<TaskManagerTask, 'assigneeId' | 'status' | 'title' | 'repo' | 'priority' | 'projectChatConversationId' | 'projectChatLaunchedAt' | 'latestRunId' | 'latestRunStatus' | 'latestRunSyncedAt' | 'latestRunStepSummary'>>,
  ) => void;
  onSelectTask?: (task: TaskManagerTask) => void;
  onLaunchTask?: (task: TaskManagerTask) => void;
  onPauseTask?: (taskId: string) => void;
  onResumeTask?: (taskId: string) => void;
  onRetryTask?: (taskId: string) => void;
  onCancelTask?: (taskId: string) => void;
}

const BOARD_STATUSES: TaskStatus[] = ['created', 'assigned', 'in-progress', 'done'];

const STATUS_ICON: Record<TaskStatus, typeof Circle> = {
  created: Circle,
  assigned: UserPlus,
  'in-progress': PlayCircle,
  done: CheckCircle2,
  paused: Pause,
  cancelled: XCircle,
  queued: Clock,
};

const PRIORITY_STYLE: Record<TaskPriority, string> = {
  normal: 'border-white/8 bg-white/[0.03] text-[var(--muted)]',
  high: 'border-amber-300/20 bg-amber-300/10 text-amber-100',
  urgent: 'border-red-300/25 bg-red-400/10 text-red-100',
};

interface TaskBackendSignal {
  attentionCount: number;
  eventCount: number;
  latestAttentionKind?: string | null;
  latestAttentionStatus?: string | null;
  latestRunStatus?: string | null;
  latestEventAt?: number | null;
  tone: 'quiet' | 'attached' | 'active' | 'stale' | 'failed';
}

interface BoardBackendSummary {
  open: number;
  active: number;
  rawDone: number;
  attention: number;
  failedRuns: number;
  failedSteps: number;
  orphanedSteps: number;
  gatedDoneAvailable: boolean;
  gatedDone: number;
  doneWithoutEvidence: number;
  pendingApprovals: number;
  activeLeases: number;
  pathLeases: number;
  taskLeases: number;
}

function formatAge(timestamp: string) {
  const ageMs = Date.now() - new Date(timestamp).getTime();
  const minutes = Math.max(Math.floor(ageMs / 60000), 0);
  if (minutes < 1) return 'now';
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}

function getMember(members: TaskMember[], memberId?: string | null) {
  if (!memberId) return null;
  return members.find((member) => member.id === memberId) ?? null;
}

function formatRunStatus(status?: string | null) {
  return status ? status.replaceAll('_', ' ') : 'run linked';
}

function statusTone(status?: string | null): TaskBackendSignal['tone'] {
  if (!status) return 'attached';
  if (status === 'failed' || status === 'orphaned' || status === 'cancelled') return 'failed';
  if (status === 'pending' || status === 'ready' || status === 'leased' || status === 'running') return 'active';
  return 'attached';
}

function formatAttentionKind(kind?: string | null) {
  if (!kind) return 'Backend signal';
  return kind.replaceAll('_', ' ');
}

function countNonApprovalAttention(summary: GroupOperationsSummary) {
  return summary.attention.filter((item) => item.kind !== 'approval_pending').length;
}

function backendSignalsFromSummary(summary: GroupOperationsSummary): Record<string, TaskBackendSignal> {
  const signals: Record<string, TaskBackendSignal> = {};

  for (const event of summary.recent_events) {
    if (!event.task_id) continue;
    const signal = signals[event.task_id] ?? {
      attentionCount: 0,
      eventCount: 0,
      latestAttentionKind: null,
      latestAttentionStatus: null,
      latestRunStatus: null,
      latestEventAt: null,
      tone: 'attached' as const,
    };
    signal.eventCount += 1;
    signal.latestEventAt = Math.max(signal.latestEventAt ?? 0, event.created_at);
    signals[event.task_id] = signal;
  }

  for (const item of summary.attention) {
    if (!item.task_id) continue;
    const signal = signals[item.task_id] ?? {
      attentionCount: 0,
      eventCount: 0,
      latestAttentionKind: null,
      latestAttentionStatus: null,
      latestRunStatus: null,
      latestEventAt: null,
      tone: 'attached' as const,
    };
    signal.attentionCount += 1;
    signal.latestAttentionKind ??= item.kind;
    signal.latestAttentionStatus ??= item.status ?? null;
    if (item.status) signal.latestRunStatus ??= item.status;
    signal.latestEventAt = Math.max(signal.latestEventAt ?? 0, item.created_at ?? 0) || signal.latestEventAt;
    signal.tone = item.kind.includes('failed') || item.kind.includes('orphaned') ? 'failed' : 'active';
    signals[item.task_id] = signal;
  }

  for (const signal of Object.values(signals)) {
    const quietMs = signal.latestEventAt ? Date.now() - signal.latestEventAt : null;
    if (signal.tone === 'active' && quietMs !== null && quietMs > 5 * 60 * 1000) {
      signal.tone = 'stale';
    }
    if (signal.attentionCount === 0 && signal.eventCount === 0) {
      signal.tone = 'quiet';
    }
  }

  return signals;
}

function backendSignalFromTask(task: TaskManagerTask): TaskBackendSignal | null {
  if (!task.latestRunId && !task.latestRunStatus) return null;
  let tone = statusTone(task.latestRunStatus);
  const quietMs = task.latestRunSyncedAt ? Date.now() - new Date(task.latestRunSyncedAt).getTime() : null;
  if (tone === 'active' && quietMs !== null && quietMs > 5 * 60 * 1000) {
    tone = 'stale';
  }
  return {
    attentionCount: 0,
    eventCount: 0,
    latestAttentionKind: null,
    latestAttentionStatus: null,
    latestRunStatus: task.latestRunStatus ?? null,
    latestEventAt: task.latestRunSyncedAt ? new Date(task.latestRunSyncedAt).getTime() : null,
    tone,
  };
}

function boardSummaryFromOperations(summary: GroupOperationsSummary): BoardBackendSummary {
  return {
    open: summary.tasks.open,
    active: summary.tasks.active + summary.runs.active,
    rawDone: summary.tasks.done_raw,
    attention: countNonApprovalAttention(summary),
    failedRuns: summary.runs.failed,
    failedSteps: summary.steps.failed,
    orphanedSteps: summary.steps.orphaned,
    gatedDoneAvailable: summary.tasks.completion.gated_done_available,
    gatedDone: summary.tasks.completion.gated_done ?? 0,
    doneWithoutEvidence: summary.tasks.completion.done_without_evidence ?? 0,
    pendingApprovals: summary.approvals?.pending ?? 0,
    activeLeases: summary.resource_leases?.active ?? 0,
    pathLeases: summary.resource_leases?.by_type.path ?? 0,
    taskLeases: summary.resource_leases?.by_type.task ?? 0,
  };
}

function backendSignalLabel(signal: TaskBackendSignal) {
  if (signal.tone === 'failed') return 'Needs attention';
  if (signal.tone === 'stale') return 'Stale';
  if (signal.latestAttentionKind) return formatAttentionKind(signal.latestAttentionKind);
  if (signal.tone === 'active') return formatRunStatus(signal.latestRunStatus);
  if (signal.tone === 'attached') return 'Attached';
  return 'No backend work';
}

function backendSignalStyle(tone: TaskBackendSignal['tone']) {
  if (tone === 'failed') return 'border-red-300/20 bg-red-400/10 text-red-100';
  if (tone === 'stale') return 'border-amber-300/20 bg-amber-300/10 text-amber-100';
  if (tone === 'active') return 'border-emerald-300/20 bg-emerald-400/10 text-emerald-100';
  if (tone === 'attached') return 'border-sky-300/20 bg-sky-400/10 text-sky-100';
  return 'border-white/8 bg-white/[0.03] text-[var(--muted)]';
}

function getRunSnapshotAge(timestamp?: string | null) {
  if (!timestamp) return null;
  const ageMs = Date.now() - new Date(timestamp).getTime();
  const minutes = Math.max(Math.floor(ageMs / 60000), 0);
  return {
    label: minutes < 1 ? 'fresh' : `${formatAge(timestamp)} ago`,
    stale: minutes >= 5,
  };
}

function TaskCard({
  task,
  assignee,
  compact,
  selected,
  onUpdateTask,
  onSelectTask,
  onLaunchTask,
  onPauseTask,
  onResumeTask,
  onRetryTask,
  onCancelTask,
  backendSignal,
}: {
  task: TaskManagerTask;
  assignee: TaskMember | null;
  compact?: boolean;
  selected?: boolean;
  onUpdateTask: TaskBoardProps['onUpdateTask'];
  onSelectTask?: TaskBoardProps['onSelectTask'];
  onLaunchTask?: TaskBoardProps['onLaunchTask'];
  onPauseTask?: TaskBoardProps['onPauseTask'];
  onResumeTask?: TaskBoardProps['onResumeTask'];
  onRetryTask?: TaskBoardProps['onRetryTask'];
  onCancelTask?: TaskBoardProps['onCancelTask'];
  backendSignal?: TaskBackendSignal | null;
}) {
  const runSnapshotAge = getRunSnapshotAge(task.latestRunSyncedAt);
  const [mobileActionsOpen, setMobileActionsOpen] = useState(false);

  function onDragStart(event: DragEvent<HTMLDivElement>) {
    event.dataTransfer.setData('application/cortex-task-id', task.id);
    event.dataTransfer.effectAllowed = 'move';
  }

  return (
    <div
      draggable
      onDragStart={onDragStart}
      onClick={() => onSelectTask?.(task)}
      role={onSelectTask ? 'button' : undefined}
      tabIndex={onSelectTask ? 0 : undefined}
      onKeyDown={(event) => {
        if (!onSelectTask) return;
        if (event.key === 'Enter' || event.key === ' ') {
          event.preventDefault();
          onSelectTask(task);
        }
      }}
      className={[
        'group rounded-lg border bg-[var(--panel)] p-3 text-left shadow-[0_1px_0_rgba(255,255,255,0.02)] transition hover:border-white/14 hover:bg-white/[0.05]',
        selected ? 'border-[var(--accent)]/40 bg-[var(--accent)]/10' : 'border-white/8',
        onSelectTask ? 'cursor-pointer' : '',
      ].join(' ')}
    >
      <div className="flex items-start gap-2">
        <GripVertical className="mt-0.5 h-4 w-4 shrink-0 text-[var(--muted)] opacity-60" />
        <div className="min-w-0 flex-1">
          <div className="flex items-start justify-between gap-2">
            <p className="line-clamp-2 text-sm font-medium leading-5 text-white">{task.title}</p>
            <span className={`shrink-0 rounded-full border px-2 py-0.5 text-[10px] ${PRIORITY_STYLE[task.priority]}`}>
              {task.priority}
            </span>
          </div>
          {task.repo && (
            <p className="mt-1 truncate text-[11px] text-[var(--muted)]">{task.repo}</p>
          )}
          {task.projectChatLaunchedAt && (
            <p className="mt-1 truncate text-[11px] text-[var(--accent)]">Attached to Project Chat</p>
          )}
          {task.latestRunId && (
            <p className="mt-1 truncate text-[11px] text-[var(--muted-strong)]">
              Run: {formatRunStatus(task.latestRunStatus)}
              {task.latestRunStepSummary ? ` · ${task.latestRunStepSummary.done}/${task.latestRunStepSummary.total} done` : ''}
              {runSnapshotAge ? ` · ${runSnapshotAge.label}` : ''}
            </p>
          )}
          {runSnapshotAge?.stale && (
            <p className="mt-1 truncate text-[10px] text-amber-200">Select to refresh backend signal</p>
          )}
          {backendSignal && (
            <div className="mt-2 flex items-center justify-between gap-2 rounded-md border border-white/8 bg-black/10 px-2 py-1.5">
              <span className={`shrink-0 rounded-full border px-1.5 py-0.5 text-[9px] capitalize ${backendSignalStyle(backendSignal.tone)}`}>
                {backendSignalLabel(backendSignal)}
              </span>
              <span className="min-w-0 truncate text-[10px] text-[var(--muted)]">
                {backendSignal.attentionCount} attention
                {' · '}
                {backendSignal.eventCount} event{backendSignal.eventCount === 1 ? '' : 's'}
              </span>
            </div>
          )}
          <div className="mt-3 flex items-center justify-between gap-2">
            {assignee ? (
              <div className="flex min-w-0 items-center gap-2">
                <span
                  className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full text-[10px] font-semibold text-black"
                  style={{ backgroundColor: assignee.color }}
                >
                  {assignee.initials}
                </span>
                <span className="truncate text-xs text-[var(--muted-strong)]">{assignee.name}</span>
              </div>
            ) : (
              <span className="text-xs text-[var(--muted)]">Unassigned</span>
            )}
            <span className="shrink-0 text-[11px] text-[var(--muted)]">{formatAge(task.updatedAt)}</span>
          </div>
          {!compact && (
            <>
              {/* Desktop action buttons */}
              <div className="mt-3 hidden items-center gap-1.5 sm:flex">
                {BOARD_STATUSES.map((status) => (
                  <button
                    key={status}
                    type="button"
                    title={formatTaskStatus(status)}
                    aria-label={`Move to ${formatTaskStatus(status)}`}
                    onClick={(event) => {
                      event.stopPropagation();
                      onUpdateTask(task.id, { status });
                    }}
                    className={[
                      'h-6 flex-1 rounded-md border text-[10px] transition active:scale-95',
                      task.status === status
                        ? 'border-[var(--accent)]/40 bg-[var(--accent)]/20 text-white'
                        : 'border-white/8 bg-white/[0.02] text-[var(--muted)] hover:bg-white/[0.06] hover:text-white',
                    ].join(' ')}
                  >
                    {status === 'in-progress' ? 'Active' : formatTaskStatus(status)}
                  </button>
                ))}
                <button
                  type="button"
                  title="Open in Project Chat"
                  aria-label="Open task in Project Chat"
                  onClick={(event) => {
                    event.stopPropagation();
                    onLaunchTask?.(task);
                  }}
                  className="inline-flex h-6 w-8 shrink-0 items-center justify-center rounded-md border border-white/8 bg-white/[0.02] text-[var(--muted)] transition hover:bg-white/[0.06] hover:text-white active:scale-95"
                >
                  <MessageSquareText className="h-3.5 w-3.5" />
                </button>
              </div>
              <div className="mt-1.5 hidden items-center gap-1 sm:flex">
                {task.status === 'created' && (
                  <button type="button" title="Queue" aria-label="Queue task" onClick={(e) => { e.stopPropagation(); onUpdateTask(task.id, { status: 'queued' }); }} className="inline-flex h-6 flex-1 items-center justify-center gap-1 rounded-md border border-violet-300/20 bg-violet-400/10 text-[10px] text-violet-100 transition hover:bg-violet-400/20 active:scale-95">
                    <Clock className="h-3 w-3" /> Queue
                  </button>
                )}
                {task.status === 'in-progress' && onPauseTask && (
                  <button type="button" title="Pause" aria-label="Pause task" onClick={(e) => { e.stopPropagation(); onPauseTask(task.id); }} className="inline-flex h-6 flex-1 items-center justify-center gap-1 rounded-md border border-amber-300/20 bg-amber-300/10 text-[10px] text-amber-100 transition hover:bg-amber-300/20 active:scale-95">
                    <Pause className="h-3 w-3" /> Pause
                  </button>
                )}
                {task.status === 'paused' && onResumeTask && (
                  <button type="button" title="Resume" aria-label="Resume task" onClick={(e) => { e.stopPropagation(); onResumeTask(task.id); }} className="inline-flex h-6 flex-1 items-center justify-center gap-1 rounded-md border border-emerald-300/20 bg-emerald-400/10 text-[10px] text-emerald-100 transition hover:bg-emerald-400/20 active:scale-95">
                    <Play className="h-3 w-3" /> Resume
                  </button>
                )}
                {(task.latestRunStatus === 'failed' || task.status === 'cancelled') && onRetryTask && (
                  <button type="button" title="Retry" aria-label="Retry task" onClick={(e) => { e.stopPropagation(); onRetryTask(task.id); }} className="inline-flex h-6 flex-1 items-center justify-center gap-1 rounded-md border border-sky-300/20 bg-sky-400/10 text-[10px] text-sky-100 transition hover:bg-sky-400/20 active:scale-95">
                    <RefreshCw className="h-3 w-3" /> Retry
                  </button>
                )}
                {task.status !== 'done' && task.status !== 'cancelled' && onCancelTask && (
                  <button type="button" title="Cancel" aria-label="Cancel task" onClick={(e) => { e.stopPropagation(); onCancelTask(task.id); }} className="inline-flex h-6 w-8 shrink-0 items-center justify-center rounded-md border border-red-300/15 bg-red-400/[0.06] text-[var(--muted)] transition hover:bg-red-400/15 hover:text-red-100 active:scale-95">
                    <Ban className="h-3 w-3" />
                  </button>
                )}
              </div>

              {/* Mobile: open bottom sheet for actions */}
              <button
                type="button"
                aria-label="Open task actions"
                onClick={(e) => { e.stopPropagation(); setMobileActionsOpen(true); }}
                className="mt-3 inline-flex h-9 w-full items-center justify-center gap-1.5 rounded-md border border-white/8 bg-white/[0.03] text-xs text-[var(--muted-strong)] transition hover:bg-white/[0.07] active:scale-[0.99] sm:hidden"
              >
                Actions
              </button>
              <BottomSheet open={mobileActionsOpen} onClose={() => setMobileActionsOpen(false)}>
                <p className="mb-3 truncate text-sm font-medium text-white">{task.title}</p>
                <div className="flex flex-col gap-2">
                  {BOARD_STATUSES.map((status) => (
                    <button
                      key={status}
                      type="button"
                      onClick={() => { onUpdateTask(task.id, { status }); setMobileActionsOpen(false); }}
                      className={[
                        'h-11 w-full rounded-lg border text-sm transition active:scale-[0.98]',
                        task.status === status
                          ? 'border-[var(--accent)]/40 bg-[var(--accent)]/20 font-medium text-white'
                          : 'border-white/8 bg-white/[0.04] text-[var(--muted-strong)]',
                      ].join(' ')}
                    >
                      {status === 'in-progress' ? 'Active' : formatTaskStatus(status)}
                    </button>
                  ))}
                  <button
                    type="button"
                    onClick={() => { onLaunchTask?.(task); setMobileActionsOpen(false); }}
                    className="flex h-11 w-full items-center justify-center gap-2 rounded-lg border border-white/8 bg-white/[0.04] text-sm text-[var(--muted-strong)] transition active:scale-[0.98]"
                  >
                    <MessageSquareText className="h-4 w-4" /> Open in Project Chat
                  </button>
                  {task.status === 'in-progress' && onPauseTask && (
                    <button type="button" onClick={() => { onPauseTask(task.id); setMobileActionsOpen(false); }} className="flex h-11 w-full items-center justify-center gap-2 rounded-lg border border-amber-300/20 bg-amber-300/10 text-sm text-amber-100 transition active:scale-[0.98]">
                      <Pause className="h-4 w-4" /> Pause
                    </button>
                  )}
                  {task.status === 'paused' && onResumeTask && (
                    <button type="button" onClick={() => { onResumeTask(task.id); setMobileActionsOpen(false); }} className="flex h-11 w-full items-center justify-center gap-2 rounded-lg border border-emerald-300/20 bg-emerald-400/10 text-sm text-emerald-100 transition active:scale-[0.98]">
                      <Play className="h-4 w-4" /> Resume
                    </button>
                  )}
                  {(task.latestRunStatus === 'failed' || task.status === 'cancelled') && onRetryTask && (
                    <button type="button" onClick={() => { onRetryTask(task.id); setMobileActionsOpen(false); }} className="flex h-11 w-full items-center justify-center gap-2 rounded-lg border border-sky-300/20 bg-sky-400/10 text-sm text-sky-100 transition active:scale-[0.98]">
                      <RefreshCw className="h-4 w-4" /> Retry
                    </button>
                  )}
                  {task.status !== 'done' && task.status !== 'cancelled' && onCancelTask && (
                    <button type="button" onClick={() => { onCancelTask(task.id); setMobileActionsOpen(false); }} className="flex h-11 w-full items-center justify-center gap-2 rounded-lg border border-red-300/15 bg-red-400/[0.06] text-sm text-red-100 transition active:scale-[0.98]">
                      <Ban className="h-4 w-4" /> Cancel
                    </button>
                  )}
                </div>
              </BottomSheet>
            </>
          )}
          {compact && (
            <button
              type="button"
              onClick={(event) => {
                event.stopPropagation();
                onLaunchTask?.(task);
              }}
              className="mt-3 inline-flex h-7 w-full items-center justify-center gap-1.5 rounded-md border border-white/8 bg-white/[0.03] text-[11px] text-[var(--muted-strong)] transition hover:bg-white/[0.07] hover:text-white active:scale-[0.99]"
            >
              <MessageSquareText className="h-3.5 w-3.5" />
              Open in Project Chat
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

function DropColumn({
  status,
  tasks,
  members,
  compact,
  selectedTaskId,
  onUpdateTask,
  onSelectTask,
  onLaunchTask,
  onPauseTask,
  onResumeTask,
  onRetryTask,
  onCancelTask,
  backendSignals,
}: {
  status: TaskStatus;
  tasks: TaskManagerTask[];
  members: TaskMember[];
  compact?: boolean;
  selectedTaskId?: string | null;
  onUpdateTask: TaskBoardProps['onUpdateTask'];
  onSelectTask?: TaskBoardProps['onSelectTask'];
  onLaunchTask?: TaskBoardProps['onLaunchTask'];
  onPauseTask?: TaskBoardProps['onPauseTask'];
  onResumeTask?: TaskBoardProps['onResumeTask'];
  onRetryTask?: TaskBoardProps['onRetryTask'];
  onCancelTask?: TaskBoardProps['onCancelTask'];
  backendSignals?: Record<string, TaskBackendSignal>;
}) {
  const Icon = STATUS_ICON[status];
  const [scrollTop, setScrollTop] = useState(0);
  const shouldVirtualize = tasks.length > 60;
  const estimatedRowHeight = compact ? 122 : 154;
  const virtualWindow = useMemo(() => {
    if (!shouldVirtualize) {
      return {
        items: tasks,
        top: 0,
        bottom: 0,
      };
    }
    const start = Math.max(0, Math.floor(scrollTop / estimatedRowHeight) - 6);
    const visibleCount = 18;
    const end = Math.min(tasks.length, start + visibleCount);
    return {
      items: tasks.slice(start, end),
      top: start * estimatedRowHeight,
      bottom: Math.max(0, (tasks.length - end) * estimatedRowHeight),
    };
  }, [estimatedRowHeight, scrollTop, shouldVirtualize, tasks]);

  function onDrop(event: DragEvent<HTMLDivElement>) {
    event.preventDefault();
    const taskId = event.dataTransfer.getData('application/cortex-task-id');
    if (taskId) onUpdateTask(taskId, { status });
  }

  return (
    <section
      onDragOver={(event) => event.preventDefault()}
      onDrop={onDrop}
      className="flex min-h-44 flex-col rounded-lg border border-white/8 bg-white/[0.025]"
    >
      <div className="flex h-10 items-center justify-between border-b border-white/6 px-3">
        <div className="flex min-w-0 items-center gap-2">
          <Icon className="h-4 w-4 text-[var(--muted)]" />
          <h3 className="truncate text-xs font-semibold uppercase text-[var(--muted-strong)]">
            {formatTaskStatus(status)}
          </h3>
        </div>
        <span className="rounded-full bg-white/6 px-2 py-0.5 text-[11px] text-[var(--muted)]">
          {tasks.length}
        </span>
      </div>
      <div
        className="flex flex-1 flex-col gap-2 overflow-y-auto p-2"
        onScroll={(event) => {
          if (shouldVirtualize) setScrollTop(event.currentTarget.scrollTop);
        }}
      >
        {tasks.length === 0 ? (
          <div className="flex flex-1 items-center justify-center rounded-md border border-dashed border-white/8 px-3 py-6 text-center text-xs leading-5 text-[var(--muted)]">
            Drop tasks here
          </div>
        ) : (
          <>
            {virtualWindow.top > 0 && <div style={{ height: virtualWindow.top }} aria-hidden="true" />}
            {virtualWindow.items.map((task) => (
              <TaskCard
                key={task.id}
                task={task}
                assignee={getMember(members, task.assigneeId)}
                compact={compact}
                selected={task.id === selectedTaskId}
                onUpdateTask={onUpdateTask}
                onSelectTask={onSelectTask}
                onLaunchTask={onLaunchTask}
                onPauseTask={onPauseTask}
                onResumeTask={onResumeTask}
                onRetryTask={onRetryTask}
                onCancelTask={onCancelTask}
                backendSignal={backendSignals?.[task.id] ?? null}
              />
            ))}
            {virtualWindow.bottom > 0 && <div style={{ height: virtualWindow.bottom }} aria-hidden="true" />}
          </>
        )}
      </div>
    </section>
  );
}

export default function TaskBoard({
  tasks,
  members,
  groupId,
  compact = false,
  showBackendSignals = false,
  selectedTaskId,
  onUpdateTask,
  onSelectTask,
  onLaunchTask,
  onPauseTask,
  onResumeTask,
  onRetryTask,
  onCancelTask,
}: TaskBoardProps) {
  const [liveAnnouncement, setLiveAnnouncement] = useState('');
  const prevTaskStatusRef = useRef<Record<string, TaskStatus>>({});

  // Track task status changes and announce them via ARIA live region
  useEffect(() => {
    const prevStatuses = prevTaskStatusRef.current;
    const announcements: string[] = [];

    for (const task of tasks) {
      const prev = prevStatuses[task.id];
      if (prev && prev !== task.status) {
        announcements.push(`Task moved to ${formatTaskStatus(task.status)}`);
      }
    }

    // Update the ref with current statuses
    const nextStatuses: Record<string, TaskStatus> = {};
    for (const task of tasks) {
      nextStatuses[task.id] = task.status;
    }
    prevTaskStatusRef.current = nextStatuses;

    if (announcements.length > 0) {
      setLiveAnnouncement(announcements.join('. '));
    }
  }, [tasks]);

  const handleUpdateTaskWithAnnouncement = useCallback(
    (taskId: string, patch: Parameters<TaskBoardProps['onUpdateTask']>[1]) => {
      if (patch.status) {
        const task = tasks.find((t) => t.id === taskId);
        const statusLabel = formatTaskStatus(patch.status);
        if (task && patch.status === 'done') {
          setLiveAnnouncement('Task completed');
        } else if (task) {
          setLiveAnnouncement(`Task moved to ${statusLabel}`);
        }
      }
      onUpdateTask(taskId, patch);
    },
    [onUpdateTask, tasks],
  );

  const hasTasks = tasks.length > 0;
  const hasPaused = tasks.some((t) => t.status === 'paused');
  const hasCancelled = tasks.some((t) => t.status === 'cancelled');
  const hasQueued = tasks.some((t) => t.status === 'queued');
  const activeStatuses = compact
    ? BOARD_STATUSES.filter((status) => status !== 'done')
    : [
        ...(hasQueued ? ['queued' as TaskStatus] : []),
        ...BOARD_STATUSES,
        ...(hasPaused ? ['paused' as TaskStatus] : []),
        ...(hasCancelled ? ['cancelled' as TaskStatus] : []),
      ];
  const [backendSignals, setBackendSignals] = useState<Record<string, TaskBackendSignal>>({});
  const [boardSummary, setBoardSummary] = useState<BoardBackendSummary | null>(null);
  const summaryGroupIds = useMemo(() => {
    if (!showBackendSignals) return [];
    if (groupId) return [groupId];
    return Array.from(new Set(tasks.map((task) => task.groupId))).sort();
  }, [groupId, showBackendSignals, tasks]);

  useEffect(() => {
    if (!showBackendSignals || summaryGroupIds.length === 0) {
      setBackendSignals({});
      setBoardSummary(null);
      return undefined;
    }
    let cancelled = false;
    const loadSignals = async () => {
      const nextSignals: Record<string, TaskBackendSignal> = {};
      let nextBoardSummary: BoardBackendSummary | null = null;
      await Promise.all(summaryGroupIds.map(async (groupId) => {
        try {
          const summary = await getGroupOperationsSummary(groupId);
          Object.assign(nextSignals, backendSignalsFromSummary(summary));
          if (!nextBoardSummary) {
            nextBoardSummary = boardSummaryFromOperations(summary);
          } else {
            nextBoardSummary = {
              open: nextBoardSummary.open + summary.tasks.open,
              active: nextBoardSummary.active + summary.tasks.active + summary.runs.active,
              rawDone: nextBoardSummary.rawDone + summary.tasks.done_raw,
              attention: nextBoardSummary.attention + countNonApprovalAttention(summary),
              failedRuns: nextBoardSummary.failedRuns + summary.runs.failed,
              failedSteps: nextBoardSummary.failedSteps + summary.steps.failed,
              orphanedSteps: nextBoardSummary.orphanedSteps + summary.steps.orphaned,
              gatedDoneAvailable: nextBoardSummary.gatedDoneAvailable && summary.tasks.completion.gated_done_available,
              gatedDone: nextBoardSummary.gatedDone + (summary.tasks.completion.gated_done ?? 0),
              doneWithoutEvidence: nextBoardSummary.doneWithoutEvidence
                + (summary.tasks.completion.done_without_evidence ?? 0),
              pendingApprovals: nextBoardSummary.pendingApprovals + (summary.approvals?.pending ?? 0),
              activeLeases: nextBoardSummary.activeLeases + (summary.resource_leases?.active ?? 0),
              pathLeases: nextBoardSummary.pathLeases + (summary.resource_leases?.by_type.path ?? 0),
              taskLeases: nextBoardSummary.taskLeases + (summary.resource_leases?.by_type.task ?? 0),
            };
          }
        } catch {
          // Summary gaps stay quiet on the map; the inspector shows detailed projection errors.
        }
      }));
      for (const task of tasks) {
        if (nextSignals[task.id]) continue;
        const taskSignal = backendSignalFromTask(task);
        if (taskSignal) nextSignals[task.id] = taskSignal;
      }
      if (!cancelled) {
        setBackendSignals(nextSignals);
        setBoardSummary(nextBoardSummary);
      }
    };

    void loadSignals();
    const interval = window.setInterval(() => {
      void loadSignals();
    }, 20000);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [showBackendSignals, summaryGroupIds, tasks]);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="sr-only" aria-live="polite" aria-atomic="true">
        {liveAnnouncement}
      </div>
      {!hasTasks ? (
        <div className="flex min-h-64 flex-1 flex-col items-center justify-center rounded-lg border border-dashed border-white/10 bg-white/[0.02] px-5 text-center">
          <ListTodo className="h-8 w-8 text-[var(--muted)]" />
          <p className="mt-3 text-sm font-medium text-white">No coordinated tasks yet</p>
          <p className="mt-1 max-w-72 text-xs leading-5 text-[var(--muted)]">
            Ask Cortex to assign work or create a task from the form.
          </p>
        </div>
      ) : (
        <>
          {showBackendSignals && boardSummary && (
            <div className="mb-3 grid gap-2 sm:grid-cols-4">
              <div className="rounded-lg border border-white/8 bg-white/[0.03] px-3 py-2">
                <p className="text-[10px] uppercase text-[var(--muted)]">Queue</p>
                <p className="mt-0.5 text-sm font-semibold text-white">{boardSummary.open} open · {boardSummary.active} active</p>
              </div>
              <div className="rounded-lg border border-white/8 bg-white/[0.03] px-3 py-2">
                <p className="text-[10px] uppercase text-[var(--muted)]">Completion</p>
                <p className="mt-0.5 text-sm font-semibold text-white">
                  {boardSummary.gatedDoneAvailable
                    ? `${boardSummary.gatedDone}/${boardSummary.rawDone} verified`
                    : `${boardSummary.rawDone} raw done · ungated`}
                </p>
                {boardSummary.doneWithoutEvidence > 0 && (
                  <p className="mt-0.5 text-[10px] text-amber-100">
                    {boardSummary.doneWithoutEvidence} need evidence
                  </p>
                )}
                {boardSummary.pendingApprovals > 0 && (
                  <p className="mt-0.5 text-[10px] text-sky-100">
                    {boardSummary.pendingApprovals} awaiting approval
                  </p>
                )}
              </div>
              <div className="rounded-lg border border-red-300/15 bg-red-400/[0.06] px-3 py-2">
                <p className="text-[10px] uppercase text-[var(--muted)]">Attention</p>
                <p className="mt-0.5 text-sm font-semibold text-white">{boardSummary.attention} signal{boardSummary.attention === 1 ? '' : 's'}</p>
              </div>
              <div className="rounded-lg border border-white/8 bg-white/[0.03] px-3 py-2">
                <p className="text-[10px] uppercase text-[var(--muted)]">Execution</p>
                <p className="mt-0.5 text-sm font-semibold text-white">
                  {boardSummary.activeLeases} lock{boardSummary.activeLeases === 1 ? '' : 's'} · {boardSummary.failedSteps + boardSummary.orphanedSteps} step issue
                </p>
                {boardSummary.activeLeases > 0 && (
                  <p className="mt-0.5 text-[10px] text-[var(--muted)]">
                    {boardSummary.pathLeases} path · {boardSummary.taskLeases} task
                  </p>
                )}
                {boardSummary.failedRuns > 0 && (
                  <p className="mt-0.5 text-[10px] text-red-100">
                    {boardSummary.failedRuns} failed run{boardSummary.failedRuns === 1 ? '' : 's'}
                  </p>
                )}
              </div>
            </div>
          )}
          <div className={compact ? 'grid gap-3' : `grid min-h-0 flex-1 gap-3 xl:grid-cols-${Math.min(activeStatuses.length, 6)}`} style={!compact && activeStatuses.length > 4 ? { gridTemplateColumns: `repeat(${activeStatuses.length}, minmax(0, 1fr))` } : undefined}>
            {activeStatuses.map((status) => (
              <DropColumn
                key={status}
                status={status}
                tasks={tasks.filter((task) => task.status === status)}
                members={members}
                compact={compact}
                selectedTaskId={selectedTaskId}
                onUpdateTask={handleUpdateTaskWithAnnouncement}
                onSelectTask={onSelectTask}
                onLaunchTask={onLaunchTask}
                onPauseTask={onPauseTask}
                onResumeTask={onResumeTask}
                onRetryTask={onRetryTask}
                onCancelTask={onCancelTask}
                backendSignals={backendSignals}
              />
            ))}
          </div>
        </>
      )}
    </div>
  );
}
