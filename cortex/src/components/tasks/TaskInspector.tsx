import {
  AlertTriangle,
  Ban,
  CheckCircle2,
  CircleDot,
  Clock3,
  DatabaseZap,
  GitBranch,
  KeyRound,
  ListChecks,
  Loader2,
  MessageSquareText,
  Pause,
  Play,
  PlayCircle,
  RefreshCcw,
  RefreshCw,
  ShieldCheck,
  UserCircle2,
  XCircle,
} from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { TaskActivity, TaskManagerTask, TaskMember, TaskStatus } from '../../types';
import { formatTaskStatus } from '../../lib/taskManager';
import {
  checkTaskEvidence,
  getActiveConflicts,
  getRun,
  getRunEvents,
  getTaskProjection,
  resolveGroupApproval,
  type CortexApprovalRequest,
  type ResourceLeaseConflict,
  type RunOperationEvent,
  type RunStep,
  type RunSummary,
  type TaskEvidenceCheck,
  type TaskProjection,
} from '../../lib/cortexApi';
import ConflictViewer from '../resources/ConflictViewer';

interface TaskInspectorProps {
  task: TaskManagerTask | null;
  members: TaskMember[];
  activity: TaskActivity[];
  isCreatingRun?: boolean;
  runError?: string | null;
  onCreateRun?: (task: TaskManagerTask) => void;
  onSyncRunState?: (
    task: TaskManagerTask,
    status: TaskStatus,
    snapshot: Pick<TaskManagerTask, 'latestRunStatus' | 'latestRunSyncedAt' | 'latestRunStepSummary'>,
  ) => void;
  onUpdateRunSnapshot?: (
    task: TaskManagerTask,
    snapshot: Pick<TaskManagerTask, 'latestRunStatus' | 'latestRunSyncedAt' | 'latestRunStepSummary'>,
  ) => void;
  onLaunchTask?: (task: TaskManagerTask) => void;
  onPauseTask?: (taskId: string) => void;
  onResumeTask?: (taskId: string) => void;
  onRetryTask?: (taskId: string) => void;
  onCancelTask?: (taskId: string) => void;
  onMarkDone?: (taskId: string) => void;
}

const STATUS_TONE: Record<TaskStatus, string> = {
  created: 'border-white/8 bg-white/[0.04] text-[var(--muted-strong)]',
  assigned: 'border-sky-300/20 bg-sky-400/10 text-sky-100',
  'in-progress': 'border-emerald-300/20 bg-emerald-400/10 text-emerald-100',
  done: 'border-[var(--accent)]/25 bg-[var(--accent)]/10 text-[var(--accent)]',
  paused: 'border-amber-300/20 bg-amber-300/10 text-amber-100',
  cancelled: 'border-red-300/20 bg-red-400/10 text-red-100',
  queued: 'border-violet-300/20 bg-violet-400/10 text-violet-100',
};

const RUN_STATUS_TONE: Record<string, string> = {
  pending: 'border-white/8 bg-white/[0.04] text-[var(--muted)]',
  ready: 'border-white/8 bg-white/[0.04] text-[var(--muted)]',
  leased: 'border-sky-300/20 bg-sky-400/10 text-sky-100',
  running: 'border-emerald-300/20 bg-emerald-400/10 text-emerald-100',
  succeeded: 'border-[var(--accent)]/25 bg-[var(--accent)]/10 text-[var(--accent)]',
  delivered: 'border-sky-300/20 bg-sky-400/10 text-sky-100',
  verifying: 'border-sky-300/20 bg-sky-400/10 text-sky-100',
  verified: 'border-[var(--accent)]/25 bg-[var(--accent)]/10 text-[var(--accent)]',
  inconclusive: 'border-amber-300/20 bg-amber-300/10 text-amber-100',
  execution_failed: 'border-orange-300/20 bg-orange-300/10 text-orange-100',
  manual_override: 'border-amber-300/20 bg-amber-300/10 text-amber-100',
  failed: 'border-red-300/20 bg-red-400/10 text-red-100',
  recovered: 'border-amber-300/20 bg-amber-300/10 text-amber-100',
  cancelled: 'border-zinc-300/20 bg-zinc-300/10 text-zinc-100',
  orphaned: 'border-orange-300/20 bg-orange-300/10 text-orange-100',
  skipped: 'border-zinc-300/20 bg-zinc-300/10 text-zinc-100',
};

const TERMINAL_RUN_STATUSES = new Set([
  'succeeded',
  'failed',
  'cancelled',
  'recovered',
  'orphaned',
  'skipped',
]);

function formatDateTime(timestamp?: string | null) {
  if (!timestamp) return 'Not available';
  return new Date(timestamp).toLocaleString([], {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  });
}

function formatEventTime(timestamp: number) {
  return new Date(timestamp).toLocaleTimeString([], {
    hour: 'numeric',
    minute: '2-digit',
    second: '2-digit',
  });
}

function ownerLabel(task: TaskManagerTask, members: TaskMember[]) {
  if (!task.assigneeId) return 'Unassigned';
  return members.find((member) => member.id === task.assigneeId)?.name ?? 'Unknown owner';
}

function getExecutionSignal(task: TaskManagerTask) {
  if (task.status === 'done') return 'Terminal: marked done';
  if (task.latestRunId) return 'Run attached';
  if (task.projectChatConversationId || task.projectChatLaunchedAt) return 'Project Chat attached';
  if (task.status === 'in-progress') return 'Active without backend run';
  if (task.status === 'assigned') return 'Queued for owner';
  return 'Needs launch or assignment';
}

function getRiskSignal(task: TaskManagerTask) {
  if (task.status === 'in-progress' && !task.latestRunId) {
    return 'No backend run is linked yet, so progress is chat/task state only.';
  }
  if (task.priority === 'urgent' && task.status !== 'in-progress' && task.status !== 'done') {
    return 'Urgent task is not active yet.';
  }
  if (!task.assigneeId && task.status !== 'done') {
    return 'No owner assigned.';
  }
  return null;
}

function runStatusTone(status?: string) {
  return RUN_STATUS_TONE[status ?? 'pending'] ?? RUN_STATUS_TONE.pending;
}

function labelFromStatus(status?: string) {
  return status ? status.replaceAll('_', ' ') : 'pending';
}

function isTerminalRunStatus(status?: string) {
  return Boolean(status && TERMINAL_RUN_STATUSES.has(status));
}

function stepLabel(step: RunStep, index: number) {
  return step.title || step.objective || step.goal || `Step ${index + 1}`;
}

function stepDetail(step: RunStep) {
  if (step.last_error || step.error) return step.last_error || step.error;
  if (step.output_summary) return step.output_summary;
  if (step.verification_status) {
    return [step.verification_status, step.verifier_verdict].filter(Boolean).join(' · ');
  }
  if (step.latest_attempt?.error_summary) return step.latest_attempt.error_summary;
  return null;
}

function eventLabel(event: RunOperationEvent) {
  return event.event_type.replaceAll('.', ' ');
}

function eventDetail(event: RunOperationEvent, run: RunSummary | null) {
  const payload = event.payload ?? {};
  const status = typeof payload.status === 'string' ? payload.status : null;
  const worker = typeof payload.worker_id === 'string' ? payload.worker_id : null;
  const verdict = typeof payload.verdict === 'string' ? payload.verdict : null;
  const authority = payload.authority && typeof payload.authority === 'object'
    ? payload.authority as Record<string, unknown>
    : null;
  const authorityScope = typeof authority?.scope_id === 'string' ? authority.scope_id : null;
  const authorityKind = typeof authority?.scope_kind === 'string' ? authority.scope_kind : null;
  const step = event.step_id && run
    ? run.steps.find((candidate) => candidate.id === event.step_id)
    : null;
  return [
    step ? stepLabel(step, run?.steps.indexOf(step) ?? 0) : null,
    status ? labelFromStatus(status) : null,
    worker ? `worker ${worker}` : null,
    verdict,
    authorityScope ? `${authorityKind ?? 'scope'} ${authorityScope}` : null,
  ].filter(Boolean).join(' · ');
}

function buildRunSignal(run: RunSummary | null) {
  if (!run) return null;
  const statuses = run.steps.map((step) => step.status);
  const active = statuses.filter((status) => status === 'leased' || status === 'running').length;
  // Delivered work is not done. Counting it here would put a number on the
  // dashboard that says more finished than has been checked.
  const done = statuses.filter(
    (status) => status === 'verified' || status === 'manual_override' || status === 'skipped',
  ).length;
  const failed = statuses.filter((status) => status === 'failed' || status === 'orphaned').length;
  return { active, done, failed, total: statuses.length };
}

function taskStatusFromRun(run: RunSummary | null): TaskStatus | null {
  if (!run?.status) return null;
  if (run.status === 'succeeded' || run.status === 'recovered') return 'done';
  if (
    run.status === 'pending'
    || run.status === 'ready'
    || run.status === 'leased'
    || run.status === 'running'
    || run.status === 'failed'
    || run.status === 'orphaned'
  ) {
    return 'in-progress';
  }
  return null;
}

function completionTone(reason?: string) {
  if (reason === 'passed') return 'border-[var(--accent)]/25 bg-[var(--accent)]/10 text-[var(--accent)]';
  if (reason === 'no_run' || reason === 'missing_run') return 'border-white/8 bg-white/[0.04] text-[var(--muted)]';
  return 'border-amber-300/20 bg-amber-300/10 text-amber-100';
}

function completionLabel(reason?: string) {
  if (!reason) return 'No completion evidence';
  return reason.replaceAll('_', ' ');
}

function eventAuthorityLabel(event: RunOperationEvent) {
  const payload = event.payload ?? {};
  const authority = payload.authority && typeof payload.authority === 'object'
    ? payload.authority as Record<string, unknown>
    : null;
  const scopeId = event.scope_id ?? (typeof authority?.scope_id === 'string' ? authority.scope_id : null);
  const role = typeof authority?.role === 'string' ? authority.role : null;
  if (!scopeId && !event.actor_user_id) return null;
  return [scopeId, role ? `role ${role}` : null, event.actor_user_id ? `actor ${event.actor_user_id}` : null]
    .filter(Boolean)
    .join(' · ');
}

export default function TaskInspector({
  task,
  members,
  activity,
  isCreatingRun = false,
  runError,
  onCreateRun,
  onSyncRunState,
  onUpdateRunSnapshot,
  onLaunchTask,
  onPauseTask,
  onResumeTask,
  onRetryTask,
  onCancelTask,
  onMarkDone,
}: TaskInspectorProps) {
  const [run, setRun] = useState<RunSummary | null>(null);
  const [events, setEvents] = useState<RunOperationEvent[]>([]);
  const [projection, setProjection] = useState<TaskProjection | null>(null);
  const [isLoadingRun, setIsLoadingRun] = useState(false);
  const [isLoadingProjection, setIsLoadingProjection] = useState(false);
  const [resolvingApprovalId, setResolvingApprovalId] = useState<string | null>(null);
  const [runLoadError, setRunLoadError] = useState<string | null>(null);
  const [projectionError, setProjectionError] = useState<string | null>(null);
  const [approvalActionError, setApprovalActionError] = useState<string | null>(null);
  const [evidenceCheck, setEvidenceCheck] = useState<TaskEvidenceCheck | null>(null);
  const [conflicts, setConflicts] = useState<ResourceLeaseConflict[]>([]);
  const latestRunId = task?.latestRunId
    ?? projection?.task.latest_run_id
    ?? projection?.runs[0]?.id
    ?? null;
  const taskRef = useRef(task);
  const updateRunSnapshotRef = useRef(onUpdateRunSnapshot);
  const isRefreshingRunRef = useRef(false);
  const isRefreshingProjectionRef = useRef(false);

  useEffect(() => {
    taskRef.current = task;
  }, [task]);

  useEffect(() => {
    updateRunSnapshotRef.current = onUpdateRunSnapshot;
  }, [onUpdateRunSnapshot]);

  const taskGroupId = task?.groupId ?? null;
  const taskId = task?.id ?? null;

  const refreshTaskProjection = useCallback(async () => {
    if (!taskGroupId || !taskId) {
      setProjection(null);
      setProjectionError(null);
      return;
    }
    if (isRefreshingProjectionRef.current) return;
    isRefreshingProjectionRef.current = true;
    setIsLoadingProjection(true);
    try {
      const nextProjection = await getTaskProjection(taskGroupId, taskId);
      setProjection(nextProjection);
      setProjectionError(null);
    } catch (error) {
      setProjection(null);
      setProjectionError(error instanceof Error ? error.message : 'Could not load task projection.');
    } finally {
      isRefreshingProjectionRef.current = false;
      setIsLoadingProjection(false);
    }
  }, [taskGroupId, taskId]);

  useEffect(() => {
    void refreshTaskProjection();
  }, [refreshTaskProjection]);

  useEffect(() => {
    if (!taskGroupId || !taskId) {
      setEvidenceCheck(null);
      return;
    }
    void checkTaskEvidence(taskGroupId, taskId)
      .then(setEvidenceCheck)
      .catch(() => setEvidenceCheck(null));
  }, [taskGroupId, taskId]);


  const refreshRunProjection = useCallback(async () => {
    if (!latestRunId) {
      setRun(null);
      setEvents([]);
      setConflicts([]);
      setRunLoadError(null);
      return;
    }
    if (isRefreshingRunRef.current) return;
    isRefreshingRunRef.current = true;
    setIsLoadingRun(true);
    try {
      const [nextRun, nextEvents, nextConflicts] = await Promise.all([
        getRun(latestRunId),
        getRunEvents(latestRunId, 50),
        getActiveConflicts(latestRunId).catch(() => [] as ResourceLeaseConflict[]),
      ]);
      setRun(nextRun);
      setEvents(nextEvents.events);
      setConflicts(nextConflicts);
      const currentTask = taskRef.current;
      if (currentTask) {
        updateRunSnapshotRef.current?.(currentTask, {
          latestRunStatus: nextRun.status ?? null,
          latestRunSyncedAt: new Date().toISOString(),
          latestRunStepSummary: buildRunSignal(nextRun),
        });
      }
      setRunLoadError(null);
    } catch (error) {
      setRun(null);
      setEvents([]);
      setConflicts([]);
      setRunLoadError(error instanceof Error ? error.message : 'Could not load linked run.');
    } finally {
      isRefreshingRunRef.current = false;
      setIsLoadingRun(false);
    }
  }, [latestRunId]);

  const resolvePendingApproval = useCallback(async (
    approval: CortexApprovalRequest,
    status: 'approved' | 'rejected',
  ) => {
    if (!taskGroupId) return;
    setResolvingApprovalId(approval.id);
    setApprovalActionError(null);
    try {
      const resolved = await resolveGroupApproval(taskGroupId, approval.id, status, {
        source: 'task_inspector',
        task_id: approval.task_id ?? taskId,
        run_id: approval.run_id ?? null,
        step_id: approval.step_id ?? null,
      });
      setProjection((current) => {
        if (!current) return current;
        return {
          ...current,
          approvals: current.approvals.map((candidate) => (
            candidate.id === resolved.id ? resolved : candidate
          )),
        };
      });
      await refreshTaskProjection();
      await refreshRunProjection();
    } catch (error) {
      setApprovalActionError(error instanceof Error ? error.message : 'Could not resolve approval.');
    } finally {
      setResolvingApprovalId(null);
    }
  }, [refreshRunProjection, refreshTaskProjection, taskGroupId, taskId]);

  useEffect(() => {
    void refreshRunProjection();
  }, [refreshRunProjection]);

  useEffect(() => {
    if (!latestRunId || isTerminalRunStatus(run?.status)) return;
    const interval = window.setInterval(() => {
      void refreshRunProjection();
    }, 5000);
    return () => window.clearInterval(interval);
  }, [latestRunId, refreshRunProjection, run?.status]);

  const runSignal = useMemo(() => buildRunSignal(run), [run]);
  const suggestedTaskStatus = useMemo(() => taskStatusFromRun(run), [run]);

  if (!task) {
    return (
      <section className="rounded-lg border border-dashed border-white/10 bg-white/[0.02] p-3">
        <div className="flex items-center gap-2">
          <CircleDot className="h-4 w-4 text-[var(--muted)]" />
          <h2 className="text-sm font-semibold text-white">Task Inspector</h2>
        </div>
        <p className="mt-2 text-xs leading-5 text-[var(--muted)]">
          Select a task to inspect ownership, execution state, evidence, blockers, and recent movement.
        </p>
      </section>
    );
  }

  const taskActivity = activity.filter((item) => item.taskId === task.id).slice(0, 5);
  const riskSignal = getRiskSignal(task);
  const projectionRuns = projection?.runs ?? [];
  const projectionChats = projection?.chats ?? [];
  const projectionEvents = projection?.events ?? [];
  const displayedEvents = projectionEvents.length > 0
    ? projectionEvents.slice(0, 5)
    : events.slice(-5).reverse();
  const completion = projection?.task.completion;
  const projectedRunStatus = projection?.task.completion?.run_status || task.latestRunStatus || undefined;
  const pendingApprovals = projection?.approvals.filter((approval) => approval.status === 'pending') ?? [];
  const approvedApprovals = projection?.approvals.filter((approval) => approval.status === 'approved') ?? [];
  const rejectedApprovals = projection?.approvals.filter((approval) => approval.status === 'rejected') ?? [];
  const cancelledApprovals = projection?.approvals.filter((approval) => approval.status === 'cancelled') ?? [];
  const primaryPendingApproval = pendingApprovals[0] ?? null;
  const isResolvingPrimaryApproval = Boolean(primaryPendingApproval && resolvingApprovalId === primaryPendingApproval.id);
  const latestAuthorityEvent = projectionEvents.find((event) => eventAuthorityLabel(event));

  return (
    <section className="rounded-lg border border-white/8 bg-white/[0.03] p-3">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <CircleDot className="h-4 w-4 shrink-0 text-[var(--muted)]" />
            <h2 className="truncate text-sm font-semibold text-white">Task Inspector</h2>
          </div>
          <p className="mt-2 line-clamp-3 text-sm font-medium leading-5 text-white">{task.title}</p>
          {task.description && (
            <p className="mt-1 line-clamp-3 text-xs leading-5 text-[var(--muted)]">{task.description}</p>
          )}
        </div>
        <span className={`shrink-0 rounded-full border px-2 py-0.5 text-[10px] ${STATUS_TONE[task.status]}`}>
          {formatTaskStatus(task.status)}
        </span>
      </div>

      <div className="mt-3 grid grid-cols-2 gap-2">
        <div className="min-w-0 rounded-md border border-white/8 bg-black/10 p-2">
          <div className="flex items-center gap-1.5 text-[10px] uppercase text-[var(--muted)]">
            <UserCircle2 className="h-3.5 w-3.5" />
            Owner
          </div>
          <p className="mt-1 truncate text-xs text-[var(--muted-strong)]">{ownerLabel(task, members)}</p>
        </div>
        <div className="min-w-0 rounded-md border border-white/8 bg-black/10 p-2">
          <div className="flex items-center gap-1.5 text-[10px] uppercase text-[var(--muted)]">
            <GitBranch className="h-3.5 w-3.5" />
            Repo
          </div>
          <p className="mt-1 truncate text-xs text-[var(--muted-strong)]">{task.repo || 'No repo set'}</p>
        </div>
        <div className="min-w-0 rounded-md border border-white/8 bg-black/10 p-2">
          <div className="flex items-center gap-1.5 text-[10px] uppercase text-[var(--muted)]">
            <PlayCircle className="h-3.5 w-3.5" />
            Execution
          </div>
          <p className="mt-1 truncate text-xs text-[var(--muted-strong)]">{getExecutionSignal(task)}</p>
        </div>
        <div className="min-w-0 rounded-md border border-white/8 bg-black/10 p-2">
          <div className="flex items-center gap-1.5 text-[10px] uppercase text-[var(--muted)]">
            <Clock3 className="h-3.5 w-3.5" />
            Updated
          </div>
          <p className="mt-1 truncate text-xs text-[var(--muted-strong)]">{formatDateTime(task.updatedAt)}</p>
        </div>
      </div>

      <div className="mt-3 flex items-center gap-1.5">
        {task.status === 'in-progress' && onPauseTask && (
          <button type="button" aria-label="Pause task" onClick={() => onPauseTask(task.id)} className="inline-flex h-7 flex-1 items-center justify-center gap-1.5 rounded-md border border-amber-300/20 bg-amber-300/10 text-xs text-amber-100 transition hover:bg-amber-300/20 active:scale-[0.98]">
            <Pause className="h-3.5 w-3.5" /> Pause
          </button>
        )}
        {task.status === 'paused' && onResumeTask && (
          <button type="button" aria-label="Resume task" onClick={() => onResumeTask(task.id)} className="inline-flex h-7 flex-1 items-center justify-center gap-1.5 rounded-md border border-emerald-300/20 bg-emerald-400/10 text-xs text-emerald-100 transition hover:bg-emerald-400/20 active:scale-[0.98]">
            <Play className="h-3.5 w-3.5" /> Resume
          </button>
        )}
        {(task.latestRunStatus === 'failed' || task.status === 'cancelled') && onRetryTask && (
          <button type="button" aria-label="Retry task" onClick={() => onRetryTask(task.id)} className="inline-flex h-7 flex-1 items-center justify-center gap-1.5 rounded-md border border-sky-300/20 bg-sky-400/10 text-xs text-sky-100 transition hover:bg-sky-400/20 active:scale-[0.98]">
            <RefreshCw className="h-3.5 w-3.5" /> Retry
          </button>
        )}
        {task.status !== 'done' && task.status !== 'cancelled' && onMarkDone && (() => {
          const evidenceBlocked = evidenceCheck !== null && !evidenceCheck.completion_gate.gated_done;
          const blockReason = evidenceBlocked
            ? `Evidence requirements not met: ${evidenceCheck!.completion_gate.steps.unverified} steps unverified`
            : undefined;
          return (
            <button
              type="button"
              aria-label="Mark done"
              title={blockReason}
              disabled={evidenceBlocked}
              onClick={() => onMarkDone(task.id)}
              className="inline-flex h-7 flex-1 items-center justify-center gap-1.5 rounded-md border border-[var(--accent)]/20 bg-[var(--accent)]/10 text-xs text-[var(--accent)] transition hover:bg-[var(--accent)]/15 active:scale-[0.98] disabled:cursor-not-allowed disabled:opacity-50"
            >
              <CheckCircle2 className="h-3.5 w-3.5" /> Mark Done
            </button>
          );
        })()}
        {task.status !== 'done' && task.status !== 'cancelled' && onCancelTask && (
          <button type="button" aria-label="Cancel task" onClick={() => onCancelTask(task.id)} className="inline-flex h-7 items-center justify-center gap-1.5 rounded-md border border-red-300/15 bg-red-400/[0.06] px-3 text-xs text-[var(--muted)] transition hover:bg-red-400/15 hover:text-red-100 active:scale-[0.98]">
            <Ban className="h-3.5 w-3.5" /> Cancel
          </button>
        )}
        {pendingApprovals.length > 0 && (
          <button
            type="button"
            disabled={Boolean(resolvingApprovalId)}
            onClick={() => {
              if (primaryPendingApproval) {
                void resolvePendingApproval(primaryPendingApproval, 'approved');
              }
            }}
            className="inline-flex h-7 flex-1 items-center justify-center gap-1.5 rounded-md border border-[var(--accent)]/20 bg-[var(--accent)]/10 text-xs text-[var(--accent)] transition hover:bg-[var(--accent)]/15 active:scale-[0.98] disabled:cursor-not-allowed disabled:opacity-50"
          >
            <CheckCircle2 className="h-3.5 w-3.5" /> Approve
          </button>
        )}
      </div>

      <div className="mt-3 space-y-2">
        {riskSignal ? (
          <div className="rounded-md border border-amber-300/20 bg-amber-300/10 px-2.5 py-2">
            <div className="flex items-start gap-2">
              <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-amber-200" />
              <p className="text-xs leading-5 text-amber-100">{riskSignal}</p>
            </div>
          </div>
        ) : (
          <div className="rounded-md border border-[var(--accent)]/20 bg-[var(--accent)]/10 px-2.5 py-2">
            <div className="flex items-start gap-2">
              <CheckCircle2 className="mt-0.5 h-3.5 w-3.5 shrink-0 text-[var(--accent)]" />
              <p className="text-xs leading-5 text-[var(--muted-strong)]">No obvious task-level blocker from current state.</p>
            </div>
          </div>
        )}

        <div className="rounded-md border border-white/8 bg-black/10 p-2.5">
          <div className="flex items-center gap-1.5 text-xs font-medium text-[var(--muted-strong)]">
            <ShieldCheck className="h-3.5 w-3.5 text-[var(--muted)]" />
            Evidence
          </div>
          {completion ? (
            <div className="mt-2 space-y-2">
              <div className={`rounded-md border px-2 py-1.5 text-[11px] capitalize ${completionTone(completion.reason)}`}>
                {completion.gated_done ? 'Evidence-backed done' : completionLabel(completion.reason)}
              </div>
              <div className="grid grid-cols-4 gap-1.5">
                <div className="rounded-md border border-white/8 bg-white/[0.02] px-2 py-1.5">
                  <p className="text-[9px] uppercase text-[var(--muted)]">Steps</p>
                  <p className="text-xs font-semibold text-white">{completion.steps.total}</p>
                </div>
                <div className="rounded-md border border-white/8 bg-white/[0.02] px-2 py-1.5">
                  <p className="text-[9px] uppercase text-[var(--muted)]">Verified</p>
                  <p className="text-xs font-semibold text-[var(--accent)]">{completion.steps.verified_pass}</p>
                </div>
                <div className="rounded-md border border-white/8 bg-white/[0.02] px-2 py-1.5">
                  <p className="text-[9px] uppercase text-[var(--muted)]">Failed</p>
                  <p className="text-xs font-semibold text-rose-200">{completion.steps.failed}</p>
                </div>
                <div className="rounded-md border border-white/8 bg-white/[0.02] px-2 py-1.5">
                  <p className="text-[9px] uppercase text-[var(--muted)]">Open</p>
                  <p className="text-xs font-semibold text-amber-100">{completion.steps.unverified}</p>
                </div>
              </div>
            </div>
          ) : (
            <div className="mt-2 rounded-md border border-dashed border-white/10 px-2 py-1.5 text-[11px] text-[var(--muted)]">
              Completion evidence has not been projected yet.
            </div>
          )}
          <div className="mt-2 space-y-1 text-[11px] leading-4 text-[var(--muted)]">
            <p>Project Chat: {task.projectChatLaunchedAt ? formatDateTime(task.projectChatLaunchedAt) : 'Not launched'}</p>
            <p>Conversation: {projection?.task.conversation_id || task.projectChatConversationId || 'Not attached'}</p>
            <p>Latest run: {projection?.task.latest_run_id || task.latestRunId || 'Not linked'}</p>
            {projectedRunStatus && (
              <p>Run status: {labelFromStatus(projectedRunStatus)}</p>
            )}
            {task.latestRunSyncedAt && <p>Synced: {formatDateTime(task.latestRunSyncedAt)}</p>}
          </div>
        </div>

        <div className="rounded-md border border-white/8 bg-black/10 p-2.5">
          <div className="flex items-center gap-1.5 text-xs font-medium text-[var(--muted-strong)]">
            <ListChecks className="h-3.5 w-3.5 text-[var(--muted)]" />
            Approvals
          </div>
          {projection ? (
            <div className="mt-2 space-y-2">
              <div className="grid grid-cols-4 gap-1.5">
                <div className="rounded-md border border-white/8 bg-white/[0.02] px-2 py-1.5">
                  <p className="text-[9px] uppercase text-[var(--muted)]">Pending</p>
                  <p className="text-xs font-semibold text-amber-100">{pendingApprovals.length}</p>
                </div>
                <div className="rounded-md border border-white/8 bg-white/[0.02] px-2 py-1.5">
                  <p className="text-[9px] uppercase text-[var(--muted)]">Approved</p>
                  <p className="text-xs font-semibold text-[var(--accent)]">{approvedApprovals.length}</p>
                </div>
                <div className="rounded-md border border-white/8 bg-white/[0.02] px-2 py-1.5">
                  <p className="text-[9px] uppercase text-[var(--muted)]">Rejected</p>
                  <p className="text-xs font-semibold text-rose-200">{rejectedApprovals.length}</p>
                </div>
                <div className="rounded-md border border-white/8 bg-white/[0.02] px-2 py-1.5">
                  <p className="text-[9px] uppercase text-[var(--muted)]">Cancelled</p>
                  <p className="text-xs font-semibold text-[var(--muted-strong)]">{cancelledApprovals.length}</p>
                </div>
              </div>
              {primaryPendingApproval && (
                <div className="rounded-md border border-amber-300/20 bg-amber-300/10 px-2 py-1.5">
                  <p className="truncate text-[11px] font-medium text-amber-100">{primaryPendingApproval.title}</p>
                  <p className="mt-0.5 truncate text-[10px] capitalize text-amber-200/80">
                    {primaryPendingApproval.ask_type} · {primaryPendingApproval.priority}
                  </p>
                  {primaryPendingApproval.body && (
                    <p className="mt-1 line-clamp-2 text-[10px] leading-4 text-amber-100/80">
                      {primaryPendingApproval.body}
                    </p>
                  )}
                  <div className="mt-2 grid grid-cols-2 gap-1.5">
                    <button
                      type="button"
                      disabled={Boolean(resolvingApprovalId)}
                      onClick={() => void resolvePendingApproval(primaryPendingApproval, 'approved')}
                      className="inline-flex h-7 items-center justify-center gap-1 rounded-md bg-[var(--accent)] px-2 text-[10px] font-medium text-black transition hover:brightness-110 active:scale-[0.98] disabled:cursor-not-allowed disabled:opacity-50"
                    >
                      {isResolvingPrimaryApproval ? (
                        <Loader2 className="h-3 w-3 animate-spin" />
                      ) : (
                        <CheckCircle2 className="h-3 w-3" />
                      )}
                      Approve
                    </button>
                    <button
                      type="button"
                      disabled={Boolean(resolvingApprovalId)}
                      onClick={() => void resolvePendingApproval(primaryPendingApproval, 'rejected')}
                      className="inline-flex h-7 items-center justify-center gap-1 rounded-md border border-white/8 bg-black/15 px-2 text-[10px] font-medium text-[var(--muted-strong)] transition hover:bg-white/[0.06] hover:text-white active:scale-[0.98] disabled:cursor-not-allowed disabled:opacity-50"
                    >
                      {isResolvingPrimaryApproval ? (
                        <Loader2 className="h-3 w-3 animate-spin" />
                      ) : (
                        <XCircle className="h-3 w-3" />
                      )}
                      Reject
                    </button>
                  </div>
                </div>
              )}
              {approvalActionError && (
                <div className="rounded-md border border-red-300/20 bg-red-400/10 px-2 py-1.5 text-[11px] leading-4 text-red-100">
                  {approvalActionError}
                </div>
              )}
            </div>
          ) : (
            <div className="mt-2 rounded-md border border-dashed border-white/10 px-2 py-1.5 text-[11px] text-[var(--muted)]">
              Approval state loads with the backend projection.
            </div>
          )}
        </div>

        <div className="rounded-md border border-white/8 bg-black/10 p-2.5">
          <div className="flex items-center gap-1.5 text-xs font-medium text-[var(--muted-strong)]">
            <KeyRound className="h-3.5 w-3.5 text-[var(--muted)]" />
            Authority
          </div>
          {latestAuthorityEvent ? (
            <div className="mt-2 space-y-1 text-[11px] leading-4 text-[var(--muted)]">
              <p>{eventAuthorityLabel(latestAuthorityEvent)}</p>
              <p className="capitalize">{eventLabel(latestAuthorityEvent)} · {formatEventTime(latestAuthorityEvent.created_at)}</p>
            </div>
          ) : (
            <div className="mt-2 rounded-md border border-dashed border-white/10 px-2 py-1.5 text-[11px] text-[var(--muted)]">
              No authority event has been recorded for this task yet.
            </div>
          )}
        </div>

        {conflicts.length > 0 && <ConflictViewer conflicts={conflicts} />}

        <div className="rounded-md border border-white/8 bg-black/10 p-2.5">
          <div className="flex items-center justify-between gap-2">
            <div className="flex min-w-0 items-center gap-1.5 text-xs font-medium text-[var(--muted-strong)]">
              <DatabaseZap className="h-3.5 w-3.5 shrink-0 text-[var(--muted)]" />
              <span className="truncate">Backend Projection</span>
            </div>
            <button
              type="button"
              disabled={isLoadingProjection}
              onClick={() => void refreshTaskProjection()}
              className="shrink-0 rounded-md p-1 text-[var(--muted)] transition hover:bg-white/6 hover:text-white active:scale-95 disabled:opacity-50"
              aria-label="Refresh task projection"
              title="Refresh task projection"
            >
              <RefreshCcw className={`h-3.5 w-3.5 ${isLoadingProjection ? 'animate-spin' : ''}`} />
            </button>
          </div>

          {projectionError ? (
            <div className="mt-2 rounded-md border border-amber-300/20 bg-amber-300/10 px-2 py-1.5 text-[11px] leading-4 text-amber-100">
              {projectionError}
            </div>
          ) : projection ? (
            <div className="mt-2 grid grid-cols-4 gap-1.5">
              <div className="rounded-md border border-white/8 bg-white/[0.02] px-2 py-1.5">
                <p className="text-[10px] uppercase text-[var(--muted)]">Runs</p>
                <p className="mt-0.5 text-sm font-semibold text-white">{projectionRuns.length}</p>
              </div>
              <div className="rounded-md border border-white/8 bg-white/[0.02] px-2 py-1.5">
                <p className="text-[10px] uppercase text-[var(--muted)]">Chats</p>
                <p className="mt-0.5 text-sm font-semibold text-white">{projectionChats.length}</p>
              </div>
              <div className="rounded-md border border-white/8 bg-white/[0.02] px-2 py-1.5">
                <p className="text-[10px] uppercase text-[var(--muted)]">Events</p>
                <p className="mt-0.5 text-sm font-semibold text-white">{projectionEvents.length}</p>
              </div>
              <div className="rounded-md border border-white/8 bg-white/[0.02] px-2 py-1.5">
                <p className="text-[10px] uppercase text-[var(--muted)]">Asks</p>
                <p className="mt-0.5 text-sm font-semibold text-white">{projection.approvals.length}</p>
              </div>
            </div>
          ) : (
            <div className="mt-2 rounded-md border border-dashed border-white/10 px-2 py-1.5 text-[11px] text-[var(--muted)]">
              {isLoadingProjection ? 'Loading task projection.' : 'No backend projection loaded.'}
            </div>
          )}

          {projectionChats.length > 0 && (
            <div className="mt-2 max-h-20 space-y-1.5 overflow-y-auto pr-1">
              {projectionChats.slice(0, 3).map((chat) => (
                <div key={chat.id} className="rounded-md border border-white/8 bg-white/[0.02] px-2 py-1.5">
                  <div className="flex items-center justify-between gap-2">
                    <span className="truncate text-[11px] font-medium text-[var(--muted-strong)]">
                      {chat.title || 'Project Chat'}
                    </span>
                    <span className="shrink-0 text-[10px] text-[var(--muted)]">{formatDateTime(chat.attached_at)}</span>
                  </div>
                  <p className="mt-0.5 truncate text-[10px] text-[var(--muted)]">{chat.id}</p>
                </div>
              ))}
            </div>
          )}
          {projectionRuns.length > 0 && (
            <div className="mt-2 max-h-24 space-y-1.5 overflow-y-auto pr-1">
              {projectionRuns.slice(0, 3).map((projectionRun) => (
                <div key={projectionRun.id} className="rounded-md border border-white/8 bg-white/[0.02] px-2 py-1.5">
                  <div className="flex items-center justify-between gap-2">
                    <span className="truncate text-[11px] font-medium text-[var(--muted-strong)]">
                      {projectionRun.goal}
                    </span>
                    <span className={`shrink-0 rounded-full border px-1.5 py-0.5 text-[9px] capitalize ${runStatusTone(projectionRun.status)}`}>
                      {labelFromStatus(projectionRun.status)}
                    </span>
                  </div>
                  <p className="mt-0.5 truncate text-[10px] text-[var(--muted)]">{projectionRun.id}</p>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>

      {latestRunId && (
        <div className="mt-3 rounded-md border border-white/8 bg-black/10 p-2.5">
          <div className="flex items-center justify-between gap-2">
            <div className="min-w-0">
              <div className="flex items-center gap-1.5 text-xs font-medium text-[var(--muted-strong)]">
                <PlayCircle className="h-3.5 w-3.5 text-[var(--muted)]" />
                Linked Run
              </div>
              <p className="mt-0.5 truncate text-[10px] text-[var(--muted)]">{latestRunId}</p>
            </div>
            <button
              type="button"
              disabled={isLoadingRun}
              onClick={() => void refreshRunProjection()}
              className="shrink-0 rounded-md p-1 text-[var(--muted)] transition hover:bg-white/6 hover:text-white active:scale-95 disabled:opacity-50"
              aria-label="Refresh linked run"
              title="Refresh linked run"
            >
              <RefreshCcw className={`h-3.5 w-3.5 ${isLoadingRun ? 'animate-spin' : ''}`} />
            </button>
          </div>

          {runLoadError ? (
            <div className="mt-2 rounded-md border border-amber-300/20 bg-amber-300/10 px-2 py-1.5 text-[11px] leading-4 text-amber-100">
              {runLoadError}
            </div>
          ) : run ? (
            <div className="mt-2 space-y-2">
              <div className="flex items-center justify-between gap-2">
                <span className={`shrink-0 rounded-full border px-2 py-0.5 text-[10px] capitalize ${runStatusTone(run.status)}`}>
                  {labelFromStatus(run.status)}
                </span>
                <span className="truncate text-[10px] text-[var(--muted)]">
                  {runSignal
                    ? `${runSignal.done}/${runSignal.total} done · ${runSignal.active} active · ${runSignal.failed} failed`
                    : 'No steps yet'}
                </span>
              </div>
              {suggestedTaskStatus && suggestedTaskStatus !== task.status && (
                <button
                  type="button"
                  onClick={() => onSyncRunState?.(task, suggestedTaskStatus, {
                    latestRunStatus: run.status ?? null,
                    latestRunSyncedAt: new Date().toISOString(),
                    latestRunStepSummary: runSignal,
                  })}
                  className="inline-flex h-7 w-full items-center justify-center gap-1.5 rounded-md border border-white/8 bg-white/[0.03] text-[11px] text-[var(--muted-strong)] transition hover:bg-white/[0.07] hover:text-white active:scale-[0.99]"
                >
                  <CheckCircle2 className="h-3.5 w-3.5" />
                  Sync task to {formatTaskStatus(suggestedTaskStatus)}
                </button>
              )}

              {run.steps.length === 0 ? (
                <div className="rounded-md border border-dashed border-white/10 px-2 py-1.5 text-[11px] text-[var(--muted)]">
                  Waiting for scheduler steps.
                </div>
              ) : (
                <div className="max-h-36 space-y-1.5 overflow-y-auto pr-1">
                  {run.steps.slice(0, 6).map((step, index) => {
                    const detail = stepDetail(step);
                    return (
                      <div key={step.id} className="rounded-md border border-white/8 bg-white/[0.02] px-2 py-1.5">
                        <div className="flex items-center justify-between gap-2">
                          <span className="min-w-0 truncate text-[11px] font-medium text-white">
                            {stepLabel(step, index)}
                          </span>
                          <span className={`shrink-0 rounded-full border px-1.5 py-0.5 text-[9px] capitalize ${runStatusTone(step.status)}`}>
                            {labelFromStatus(step.status)}
                          </span>
                        </div>
                        {detail && (
                          <p className="mt-1 line-clamp-2 text-[10px] leading-4 text-[var(--muted)]">{detail}</p>
                        )}
                      </div>
                    );
                  })}
                </div>
              )}

              {displayedEvents.length > 0 && (
                <div>
                  <div className="mb-1.5 flex items-center justify-between">
                    <h3 className="text-[11px] font-medium text-[var(--muted-strong)]">Backend Events</h3>
                    <span className="text-[10px] text-[var(--muted)]">{displayedEvents.length}</span>
                  </div>
                  <div className="max-h-28 space-y-1.5 overflow-y-auto pr-1">
                    {displayedEvents.map((event) => {
                      const detail = eventDetail(event, run);
                      return (
                        <div key={event.id} className="rounded-md border border-white/8 bg-white/[0.02] px-2 py-1.5">
                          <div className="flex items-center justify-between gap-2">
                            <span className="truncate text-[11px] font-medium capitalize text-[var(--muted-strong)]">
                              {eventLabel(event)}
                            </span>
                            <span className="shrink-0 text-[10px] text-[var(--muted)]">
                              {formatEventTime(event.created_at)}
                            </span>
                          </div>
                          {detail && (
                            <p className="mt-0.5 truncate text-[10px] text-[var(--muted)]">{detail}</p>
                          )}
                          {eventAuthorityLabel(event) && (
                            <p className="mt-0.5 truncate text-[10px] text-[var(--muted)]">
                              {eventAuthorityLabel(event)}
                            </p>
                          )}
                        </div>
                      );
                    })}
                  </div>
                </div>
              )}
            </div>
          ) : (
            <div className="mt-2 rounded-md border border-dashed border-white/10 px-2 py-1.5 text-[11px] text-[var(--muted)]">
              {isLoadingRun ? 'Loading linked run.' : 'Run projection unavailable.'}
            </div>
          )}
        </div>
      )}

      {runError && (
        <div className="mt-3 rounded-md border border-red-300/20 bg-red-400/10 px-2.5 py-2 text-xs leading-5 text-red-100">
          {runError}
        </div>
      )}

      <div className="mt-3">
        <div className="mb-2 flex items-center justify-between gap-2">
          <h3 className="text-xs font-medium text-[var(--muted-strong)]">Recent Movement</h3>
          <span className="text-[10px] text-[var(--muted)]">{taskActivity.length}</span>
        </div>
        {taskActivity.length === 0 ? (
          <div className="rounded-md border border-dashed border-white/10 px-2.5 py-2 text-[11px] text-[var(--muted)]">
            No task-specific activity yet.
          </div>
        ) : (
          <div className="max-h-32 space-y-1.5 overflow-y-auto pr-1">
            {taskActivity.map((item) => (
              <div key={item.id} className="rounded-md border border-white/8 bg-black/10 px-2 py-1.5">
                <div className="flex items-center justify-between gap-2">
                  <span className="truncate text-[11px] font-medium text-[var(--muted-strong)]">{item.actor}</span>
                  <span className="shrink-0 text-[10px] text-[var(--muted)]">{formatDateTime(item.createdAt)}</span>
                </div>
                <p className="mt-0.5 line-clamp-2 text-[11px] leading-4 text-[var(--muted)]">{item.summary}</p>
              </div>
            ))}
          </div>
        )}
      </div>

      <div className="mt-3 grid grid-cols-2 gap-2">
        <button
          type="button"
          onClick={() => onLaunchTask?.(task)}
          className="inline-flex h-8 items-center justify-center gap-1.5 rounded-md border border-white/8 bg-white/[0.03] text-xs text-[var(--muted-strong)] transition hover:bg-white/[0.07] hover:text-white active:scale-[0.99]"
        >
          <MessageSquareText className="h-3.5 w-3.5" />
          Chat
        </button>
        <button
          type="button"
          disabled={isCreatingRun}
          onClick={() => onCreateRun?.(task)}
          className="inline-flex h-8 items-center justify-center gap-1.5 rounded-md border border-[var(--accent)]/20 bg-[var(--accent)]/10 text-xs text-[var(--accent)] transition hover:bg-[var(--accent)]/15 active:scale-[0.99] disabled:cursor-not-allowed disabled:opacity-50"
        >
          {isCreatingRun ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <PlayCircle className="h-3.5 w-3.5" />}
          Run
        </button>
      </div>
    </section>
  );
}
