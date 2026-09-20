import {
  Activity,
  Command,
  LayoutGrid,
  ListPlus,
  MessageSquareText,
  PanelRight,
  Plus,
  RefreshCcw,
  Users,
  Brain,
  Lightbulb,
  FileText,
} from 'lucide-react';
import { useCallback, useEffect, useMemo, useState } from 'react';
import ChatComposer from '../chat/ChatComposer';
import ChatTimeline from '../chat/ChatTimeline';
import ResizablePanels from '../shell/ResizablePanels';
import TaskBoard from './TaskBoard';
import TaskInspector from './TaskInspector';
import { parseTaskCommand, useTaskManager } from '../../lib/taskManager';
import {
  CortexApiError,
  createRun,
  repoKeyFromLabel,
  processMemoryEnhancedChat,
  applyMemorySuggestion,
  createFromAutoCapture,
  MEMORY_API_ENABLED,
  type MemoryEnhancedChatResponse,
  type LiveMemorySuggestion,
  type AutoCaptureOpportunity,
} from '../../lib/cortexApi';
import type { CortexGroup } from '../../lib/groups';
import type {
  ApprovalState,
  ChatMessage,
  RunProfile,
  TaskManagerTask,
  TaskMember,
} from '../../types';

interface TaskManagerChatProps {
  group: CortexGroup;
  userId: string;
  activeConversationId: string | null;
  messages: ChatMessage[];
  draft: string;
  isStreaming: boolean;
  isLoadingConversation: boolean;
  needsSubscription: boolean;
  runProfile: RunProfile;
  sidebarMode?: boolean;
  onDraftChange: (value: string) => void;
  onSend: () => void;
  onStop?: () => void;
  onSubscribe: () => void;
  onApprovalAction: (messageId: string, nextState: ApprovalState) => void;
  onTaskStateChange?: (state: ReturnType<typeof useTaskManager>['state']) => void;
  onLaunchTaskInProjectChat?: (task: TaskManagerTask) => void;
}

type MobilePanel = 'chat' | 'board' | 'team';

function formatTime(timestamp: string) {
  return new Date(timestamp).toLocaleTimeString([], {
    hour: 'numeric',
    minute: '2-digit',
  });
}

function statusTone(member: TaskMember) {
  if (member.status === 'working') return 'bg-emerald-300';
  if (member.status === 'away') return 'bg-amber-300';
  return 'bg-[var(--accent)]';
}

function currentTask(tasks: TaskManagerTask[], member: TaskMember) {
  if (!member.currentTaskId) return null;
  return tasks.find((task) => task.id === member.currentTaskId) ?? null;
}

/// `declaredMembers` is the group's own headcount; `members` is who this
/// client actually has data for. When the two disagree the difference is
/// rendered as a count, never as invented people — see `defaultMembers`.
function TeamPanel({
  members,
  tasks,
  declaredMembers,
}: {
  members: TaskMember[];
  tasks: TaskManagerTask[];
  declaredMembers: number;
}) {
  const knownCount = members.length;
  const totalCount = Math.max(declaredMembers, knownCount);
  const unknownCount = totalCount - knownCount;
  return (
    <section className="flex min-h-0 flex-col">
      <div className="mb-3 flex items-center justify-between">
        <div className="flex items-center gap-2">
          <Users className="h-4 w-4 text-[var(--muted)]" />
          <h2 className="text-sm font-semibold text-white">Team</h2>
        </div>
        <span className="text-[11px] text-[var(--muted)]">
          {totalCount} {totalCount === 1 ? 'member' : 'members'}
        </span>
      </div>
      <div className="space-y-2">
        {members.map((member) => {
          const task = currentTask(tasks, member);
          const assignedCount = tasks.filter((item) => item.assigneeId === member.id && item.status !== 'done').length;
          return (
            <div key={member.id} className="rounded-lg border border-white/8 bg-white/[0.03] px-3 py-2.5">
              <div className="flex items-center gap-2">
                <span
                  className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full text-xs font-semibold text-black"
                  style={{ backgroundColor: member.color }}
                >
                  {member.initials}
                </span>
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-1.5">
                    <p className="truncate text-sm font-medium text-white">{member.name}</p>
                    <span className={`h-1.5 w-1.5 rounded-full ${statusTone(member)}`} />
                  </div>
                  <p className="truncate text-xs text-[var(--muted)]">
                    {task ? task.title : member.status === 'working' ? 'Working' : 'Available'}
                  </p>
                </div>
                <span className="rounded-full border border-white/8 bg-white/[0.03] px-2 py-0.5 text-[11px] text-[var(--muted-strong)]">
                  {assignedCount}
                </span>
              </div>
            </div>
          );
        })}
        {unknownCount > 0 && (
          <div className="rounded-lg border border-dashed border-white/8 px-3 py-2.5 text-xs text-[var(--muted)]">
            {unknownCount} other {unknownCount === 1 ? 'member' : 'members'} in this
            group. Their details have not loaded yet.
          </div>
        )}
      </div>
    </section>
  );
}

function StructuredTaskForm({
  members,
  onCreateTask,
}: {
  members: TaskMember[];
  onCreateTask: (title: string, assigneeId?: string | null, repo?: string | null) => void;
}) {
  const [title, setTitle] = useState('');
  const [assigneeId, setAssigneeId] = useState('');
  const [repo, setRepo] = useState('');

  function submit() {
    const trimmed = title.trim();
    if (!trimmed) return;
    onCreateTask(trimmed, assigneeId || null, repo.trim() || null);
    setTitle('');
    setAssigneeId('');
    setRepo('');
  }

  return (
    <section className="rounded-lg border border-white/8 bg-white/[0.03] p-3">
      <div className="mb-3 flex items-center gap-2">
        <ListPlus className="h-4 w-4 text-[var(--muted)]" />
        <h2 className="text-sm font-semibold text-white">Create Task</h2>
      </div>
      <div className="space-y-2">
        <input
          value={title}
          onChange={(event) => setTitle(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') submit();
          }}
          placeholder="Task title"
          className="h-10 w-full rounded-lg border border-white/8 bg-black/20 px-3 text-sm text-white outline-none transition placeholder:text-[var(--muted)] focus:border-[var(--accent)]/40"
        />
        <div className="grid grid-cols-2 gap-2">
          <select
            value={assigneeId}
            onChange={(event) => setAssigneeId(event.target.value)}
            className="h-10 rounded-lg border border-white/8 bg-black/20 px-3 text-sm text-white outline-none transition focus:border-[var(--accent)]/40"
          >
            <option value="">Unassigned</option>
            {members.map((member) => (
              <option key={member.id} value={member.id}>{member.name}</option>
            ))}
          </select>
          <input
            value={repo}
            onChange={(event) => setRepo(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') submit();
            }}
            placeholder="Repo"
            className="h-10 rounded-lg border border-white/8 bg-black/20 px-3 text-sm text-white outline-none transition placeholder:text-[var(--muted)] focus:border-[var(--accent)]/40"
          />
        </div>
        <button
          type="button"
          onClick={submit}
          disabled={!title.trim()}
          className="inline-flex h-10 w-full items-center justify-center gap-2 rounded-lg bg-[var(--accent)] px-3 text-sm font-medium text-black transition hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-50 active:scale-[0.99]"
        >
          <Plus className="h-4 w-4" />
          Add task
        </button>
      </div>
    </section>
  );
}

function ActivityPanel({
  activity,
}: {
  activity: ReturnType<typeof useTaskManager>['state']['activity'];
}) {
  return (
    <section className="min-h-0">
      <div className="mb-3 flex items-center gap-2">
        <Activity className="h-4 w-4 text-[var(--muted)]" />
        <h2 className="text-sm font-semibold text-white">History</h2>
      </div>
      <div className="max-h-72 space-y-2 overflow-y-auto pr-1">
        {activity.slice(0, 12).map((item) => (
          <div key={item.id} className="rounded-lg border border-white/8 bg-white/[0.025] px-3 py-2">
            <div className="flex items-center justify-between gap-2">
              <span className="truncate text-xs font-medium text-[var(--muted-strong)]">{item.actor}</span>
              <span className="shrink-0 text-[11px] text-[var(--muted)]">{formatTime(item.createdAt)}</span>
            </div>
            <p className="mt-1 text-xs leading-5 text-[var(--muted)]">{item.summary}</p>
          </div>
        ))}
      </div>
    </section>
  );
}

function MemoryPanel({
  memoryData,
  onApplySuggestion,
  onCreateCapture,
  onClose,
}: {
  memoryData: MemoryEnhancedChatResponse;
  onApplySuggestion: (suggestion: LiveMemorySuggestion) => void;
  onCreateCapture: (opportunity: AutoCaptureOpportunity) => void;
  onClose: () => void;
}) {
  return (
    <section className="rounded-lg border border-[var(--accent)]/20 bg-[var(--accent)]/5 p-3">
      <div className="mb-3 flex items-center justify-between">
        <div className="flex items-center gap-2">
          <Brain className="h-4 w-4 text-[var(--accent)]" />
          <h2 className="text-sm font-semibold text-white">Memory Intelligence</h2>
        </div>
        <button
          onClick={onClose}
          className="text-xs text-[var(--muted)] hover:text-white"
        >
          ×
        </button>
      </div>

      {memoryData.relevant_memories.length > 0 && (
        <div className="mb-3">
          <div className="mb-2 flex items-center gap-1">
            <FileText className="h-3 w-3 text-green-400" />
            <span className="text-xs font-medium text-green-400">Relevant Knowledge</span>
          </div>
          <div className="space-y-2 max-h-32 overflow-y-auto">
            {memoryData.relevant_memories.slice(0, 3).map((match, index) => (
              <div key={index} className="rounded border border-white/8 bg-white/[0.03] p-2">
                <div className="flex items-center gap-1 mb-1">
                  <span className="text-xs font-medium text-[var(--accent)]">
                    {match.memory.importance}
                  </span>
                  <span className="text-xs text-[var(--muted)]">
                    ({Math.round(match.relevance_score * 100)}% match)
                  </span>
                </div>
                <p className="text-xs text-[var(--muted-strong)] leading-relaxed">
                  {match.memory.content.slice(0, 100)}...
                </p>
              </div>
            ))}
          </div>
        </div>
      )}

      {memoryData.live_suggestions.length > 0 && (
        <div className="mb-3">
          <div className="mb-2 flex items-center gap-1">
            <Lightbulb className="h-3 w-3 text-yellow-400" />
            <span className="text-xs font-medium text-yellow-400">Smart Suggestions</span>
          </div>
          <div className="space-y-1">
            {memoryData.live_suggestions.slice(0, 2).map((suggestion, index) => (
              <button
                key={index}
                onClick={() => onApplySuggestion(suggestion)}
                className="w-full text-left text-xs p-2 rounded border border-white/8 bg-white/[0.03] hover:bg-white/8 transition-colors"
              >
                <div className="font-medium text-[var(--accent)] mb-1">
                  {suggestion.suggestion_type}
                </div>
                <div className="text-[var(--muted-strong)]">
                  {suggestion.suggestion.suggested_content.slice(0, 80)}...
                </div>
              </button>
            ))}
          </div>
        </div>
      )}

      {memoryData.auto_capture && memoryData.auto_capture.confidence > 0.6 && (
        <div className="p-2 rounded border border-blue-400/20 bg-blue-400/10">
          <div className="flex items-center justify-between mb-1">
            <span className="text-xs font-medium text-blue-400">Worth Remembering</span>
            <span className="text-xs text-[var(--muted)]">
              {Math.round(memoryData.auto_capture.confidence * 100)}% confidence
            </span>
          </div>
          <p className="text-xs text-[var(--muted-strong)] mb-2">
            {memoryData.auto_capture.rationale}
          </p>
          <button
            onClick={() => onCreateCapture(memoryData.auto_capture!)}
            className="text-xs px-2 py-1 rounded bg-blue-400/20 text-blue-400 hover:bg-blue-400/30 transition-colors"
          >
            Save as {memoryData.auto_capture.suggested_importance}
          </button>
        </div>
      )}

      <div className="mt-3 pt-2 border-t border-white/8">
        <div className="text-xs text-[var(--muted)] space-y-1">
          <div>💾 {memoryData.memory_stats.total_memories} memories</div>
          <div>🎯 {Math.round(memoryData.memory_stats.avg_effectiveness * 100)}% avg effectiveness</div>
          <div>⚡ {memoryData.memory_stats.recent_activity} recent activity</div>
        </div>
      </div>
    </section>
  );
}

export default function TaskManagerChat({
  group,
  userId,
  activeConversationId,
  messages,
  draft,
  isStreaming,
  isLoadingConversation,
  needsSubscription,
  runProfile,
  sidebarMode = false,
  onDraftChange,
  onSend,
  onStop,
  onSubscribe,
  onApprovalAction,
  onTaskStateChange,
  onLaunchTaskInProjectChat,
}: TaskManagerChatProps) {
  const taskManager = useTaskManager(group, userId);
  const [mobilePanel, setMobilePanel] = useState<MobilePanel>('chat');
  const [rightPanelOpen, setRightPanelOpen] = useState(false);
  const [memoryData, setMemoryData] = useState<MemoryEnhancedChatResponse | null>(null);
  const [showMemoryPanel, setShowMemoryPanel] = useState(false);
  const [isProcessingMemory, setIsProcessingMemory] = useState(false);
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [creatingRunTaskId, setCreatingRunTaskId] = useState<string | null>(null);
  const [runError, setRunError] = useState<string | null>(null);

  const parsedPreview = useMemo(
    () => parseTaskCommand(draft, taskManager.state),
    [draft, taskManager.state],
  );
  const selectedTask = useMemo(
    () => taskManager.state.tasks.find((task) => task.id === selectedTaskId) ?? taskManager.state.tasks[0] ?? null,
    [selectedTaskId, taskManager.state.tasks],
  );

  // Process message through memory system
  const processMemory = useCallback(async (message: string) => {
    if (!MEMORY_API_ENABLED || !message.trim() || isProcessingMemory) return;

    try {
      setIsProcessingMemory(true);

      // Create task-specific workspace ID
      const taskWorkspaceId = `task_manager_${group.id}`;

      // Extract current files from task context
      const currentFiles = taskManager.state.tasks
        .filter(task => task.status === 'in-progress')
        .map(task => task.repo || 'unknown')
        .filter(Boolean);

      // Process through memory system
      const response = await processMemoryEnhancedChat({
        message,
        files: currentFiles,
        workspaceId: taskWorkspaceId,
        conversationId: activeConversationId || 'default',
        teamMembers: taskManager.state.members.map(m => m.name),
        projectPhase: 'task_management',
        activeTopics: ['tasks', 'coordination', 'assignments'],
      });

      setMemoryData(response);

      // Show memory panel if we have relevant data
      if (response.relevant_memories.length > 0 || response.live_suggestions.length > 0) {
        setShowMemoryPanel(true);
      }
    } catch (error) {
      console.error('Memory processing failed:', error);
    } finally {
      setIsProcessingMemory(false);
    }
  }, [group.id, taskManager.state, activeConversationId, isProcessingMemory]);

  // Memory interaction handlers
  const handleApplyMemorySuggestion = useCallback(async (suggestion: LiveMemorySuggestion) => {
    try {
      await applyMemorySuggestion(suggestion.suggestion_id);
      // Apply the suggested content to draft
      onDraftChange(suggestion.suggestion.suggested_content);
    } catch (error) {
      console.error('Failed to apply memory suggestion:', error);
    }
  }, [onDraftChange]);

  const handleCreateMemoryCapture = useCallback(async (opportunity: AutoCaptureOpportunity) => {
    try {
      const taskWorkspaceId = `task_manager_${group.id}`;
      await createFromAutoCapture(opportunity, taskWorkspaceId);
      // Hide the auto-capture after creation
      setMemoryData(prev => prev ? { ...prev, auto_capture: undefined } : null);
    } catch (error) {
      console.error('Failed to create memory capture:', error);
    }
  }, [group.id]);

  const handleSend = useCallback(async () => {
    // Process through memory system first
    await processMemory(draft);

    // Apply task command
    taskManager.applyTextCommand(draft);

    // Send the message
    onSend();
  }, [draft, onSend, taskManager, processMemory]);

  const handleLaunchTaskInProjectChat = useCallback(async (task: TaskManagerTask) => {
    const linkedTask = await taskManager.launchTaskInProjectChat(task.id, activeConversationId);
    setSelectedTaskId(task.id);
    onLaunchTaskInProjectChat?.(linkedTask ?? task);
  }, [activeConversationId, onLaunchTaskInProjectChat, taskManager]);

  const handleSelectTask = useCallback((task: TaskManagerTask) => {
    setSelectedTaskId(task.id);
  }, []);

  const handleCreateTaskRun = useCallback(async (task: TaskManagerTask) => {
    if (creatingRunTaskId) return;
    setSelectedTaskId(task.id);
    setCreatingRunTaskId(task.id);
    setRunError(null);
    const goal = [
      `Work on task: ${task.title}`,
      task.repo ? `Repo/context: ${task.repo}` : null,
      `Task id: ${task.id}`,
      task.description ? `Details: ${task.description}` : null,
      'Create a safe decomposed run plan, execute the work, verify it, and preserve evidence for review.',
    ].filter(Boolean).join('\n');

    try {
      const created = await createRun(goal, runProfile, [], {
        repoKey: repoKeyFromLabel(task.repo),
        taskId: task.id,
        groupId: group.id,
        conversationId: activeConversationId,
      });
      taskManager.updateTask(task.id, {
        status: task.status === 'done' ? task.status : 'in-progress',
        latestRunId: created.run_id,
      });
    } catch (error) {
      if (error instanceof CortexApiError && error.status === 503) {
        setRunError('Runtime is starting up. Try again in a moment.');
      } else {
        setRunError(error instanceof Error ? error.message : 'Could not create backend run.');
      }
    } finally {
      setCreatingRunTaskId(null);
    }
  }, [activeConversationId, creatingRunTaskId, group.id, runProfile, taskManager]);

  const handleSyncRunState = useCallback((
    task: TaskManagerTask,
    status: TaskManagerTask['status'],
    snapshot: Pick<TaskManagerTask, 'latestRunStatus' | 'latestRunSyncedAt' | 'latestRunStepSummary'>,
  ) => {
    taskManager.updateTask(task.id, { status, ...snapshot });
  }, [taskManager]);

  const handleUpdateRunSnapshot = useCallback((
    task: TaskManagerTask,
    snapshot: Pick<TaskManagerTask, 'latestRunStatus' | 'latestRunSyncedAt' | 'latestRunStepSummary'>,
  ) => {
    taskManager.updateTaskRunSnapshot(task.id, snapshot);
  }, [taskManager]);

  useEffect(() => {
    onTaskStateChange?.(taskManager.state);
  }, [onTaskStateChange, taskManager.state]);

  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      const isTextInput = target?.tagName === 'INPUT'
        || target?.tagName === 'TEXTAREA'
        || target?.tagName === 'SELECT'
        || target?.isContentEditable;
      if (isTextInput) return;
      if (!event.metaKey && !event.ctrlKey && event.key.toLowerCase() === 'g') {
        const nextHandler = (nextEvent: KeyboardEvent) => {
          const nextKey = nextEvent.key.toLowerCase();
          if (nextKey === 'b') {
            nextEvent.preventDefault();
            setMobilePanel('board');
          } else if (nextKey === 't') {
            nextEvent.preventDefault();
            setMobilePanel('team');
          }
          window.removeEventListener('keydown', nextHandler, true);
        };
        window.addEventListener('keydown', nextHandler, true);
        return;
      }
      if (!(event.metaKey || event.ctrlKey)) return;
      const key = event.key.toLowerCase();
      if (key === '1') {
        event.preventDefault();
        setMobilePanel('chat');
      } else if (key === '2') {
        event.preventDefault();
        setMobilePanel('board');
      } else if (key === '3') {
        event.preventDefault();
        setMobilePanel('team');
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, []);

  const boardHeader = (
    <div className="flex items-center justify-between gap-3">
      <div className="min-w-0">
        <div className="flex items-center gap-2">
          <LayoutGrid className="h-4 w-4 text-[var(--muted)]" />
          <h2 className="text-sm font-semibold text-white">Task Board</h2>
        </div>
        <p className="mt-0.5 text-xs text-[var(--muted)]">
          {taskManager.summary.open} open · {taskManager.summary.inProgress} active · {taskManager.summary.done} done
        </p>
      </div>
      <button
        type="button"
        title="Reset task state"
        aria-label="Reset task state"
        onClick={taskManager.resetTasks}
        className="inline-flex h-8 w-8 items-center justify-center rounded-lg text-[var(--muted)] transition hover:bg-white/6 hover:text-white active:scale-95"
      >
        <RefreshCcw className="h-4 w-4" />
      </button>
    </div>
  );

  // Simplified sidebar mode
  if (sidebarMode) {
    return (
      <div className="flex h-full min-h-0 flex-col overflow-hidden">
        <ChatTimeline
          messages={messages}
          isLoading={isLoadingConversation}
          showStarters={!activeConversationId && !isStreaming}
          onSelectStarter={needsSubscription ? undefined : onDraftChange}
          onApprovalAction={onApprovalAction}
        />

        {memoryData && showMemoryPanel && (
          <div className="border-t border-white/6 px-3 py-2">
            <MemoryPanel
              memoryData={memoryData}
              onApplySuggestion={handleApplyMemorySuggestion}
              onCreateCapture={handleCreateMemoryCapture}
              onClose={() => setShowMemoryPanel(false)}
            />
          </div>
        )}

        {parsedPreview.length > 0 && (
          <div className="border-t border-white/6 px-3 py-2">
            <div className="flex items-start gap-2 rounded-lg border border-[var(--accent)]/20 bg-[var(--accent)]/10 px-3 py-2">
              <Command className="mt-0.5 h-4 w-4 shrink-0 text-[var(--accent)]" />
              <div className="min-w-0 flex-1">
                <p className="text-xs font-medium text-white">
                  Cortex will update {parsedPreview.length} task {parsedPreview.length === 1 ? 'item' : 'items'}
                </p>
                <p className="mt-0.5 truncate text-xs text-[var(--muted-strong)]">
                  {parsedPreview.map((action) => action.summary).join(' ')}
                </p>
              </div>
            </div>
          </div>
        )}

        <ChatComposer
          draft={draft}
          disabled={isStreaming}
          locked={needsSubscription}
          placeholder={`Coordinate ${group.name} work, assign tasks, manage priorities...`}
          onDraftChange={onDraftChange}
          onSend={handleSend}
          onStop={onStop}
          onSubscribe={onSubscribe}
        />
      </div>
    );
  }

  return (
    <main className="flex min-h-0 flex-1 flex-col overflow-hidden">
      <div className="border-b border-white/6 px-3 py-2 lg:hidden">
        <div className="grid grid-cols-3 gap-1 rounded-lg border border-white/8 bg-white/[0.03] p-1">
          {[
            ['chat', MessageSquareText, 'Chat'],
            ['board', LayoutGrid, 'Board'],
            ['team', Users, 'Team'],
          ].map(([key, Icon, label]) => (
            <button
              key={key as string}
              type="button"
              onClick={() => setMobilePanel(key as MobilePanel)}
              className={[
                'inline-flex h-9 items-center justify-center gap-1.5 rounded-md text-xs transition',
                mobilePanel === key ? 'bg-white/10 text-white' : 'text-[var(--muted)] hover:text-white',
              ].join(' ')}
            >
              <Icon className="h-4 w-4" />
              {label as string}
            </button>
          ))}
        </div>
      </div>

      {rightPanelOpen ? (
        <ResizablePanels
          storageKey="cortex:task-manager:panel-ratio"
          className="task-manager-panels"
          minLeft={460}
          minRight={390}
          left={(
          <section className={`${mobilePanel === 'chat' ? 'flex' : 'hidden'} h-full min-h-0 flex-col border-r border-white/6 lg:flex`}>
          <div className="border-b border-white/6 px-4 py-3">
            <div className="flex items-center justify-between gap-3">
              <div className="min-w-0">
                <div className="flex items-center gap-2">
                  <MessageSquareText className="h-4 w-4 text-[var(--muted)]" />
                  <h1 className="truncate text-sm font-semibold text-white">Task Manager Chat</h1>
                </div>
                <p className="mt-0.5 truncate text-xs text-[var(--muted)]">
                  Coordinate assignments, handoffs, repo work, and live priorities for {group.name}.
                </p>
              </div>
              <div className="flex items-center gap-2">
                {memoryData && (
                  <button
                    type="button"
                    title={showMemoryPanel ? "Hide memory panel" : "Show memory intelligence"}
                    onClick={() => setShowMemoryPanel(!showMemoryPanel)}
                    className={`inline-flex h-8 w-8 items-center justify-center rounded-lg transition active:scale-95 ${
                      showMemoryPanel ? 'bg-[var(--accent)]/20 text-[var(--accent)]' : 'text-[var(--muted)] hover:bg-white/6 hover:text-white'
                    }`}
                  >
                    <Brain className="h-4 w-4" />
                  </button>
                )}
                <button
                  type="button"
                  title={rightPanelOpen ? "Hide task board" : "Show task board"}
                  onClick={() => setRightPanelOpen(!rightPanelOpen)}
                  className="inline-flex h-8 w-8 items-center justify-center rounded-lg text-[var(--muted)] transition hover:bg-white/6 hover:text-white active:scale-95 lg:flex hidden"
                >
                  <LayoutGrid className="h-4 w-4" />
                </button>
                <span className="hidden rounded-full border border-[var(--accent)]/20 bg-[var(--accent)]/10 px-2.5 py-1 text-[11px] text-[var(--muted-strong)] sm:inline">
                  {group.kind === 'personal' ? 'Personal' : 'Team'} brain
                </span>
              </div>
            </div>
          </div>

          <ChatTimeline
            messages={messages}
            isLoading={isLoadingConversation}
            showStarters={!activeConversationId && !isStreaming}
            onSelectStarter={needsSubscription ? undefined : onDraftChange}
            onApprovalAction={onApprovalAction}
          />

          {parsedPreview.length > 0 && (
            <div className="border-t border-white/6 px-3 py-2 sm:px-4">
              <div className="flex items-start gap-2 rounded-lg border border-[var(--accent)]/20 bg-[var(--accent)]/10 px-3 py-2">
                <Command className="mt-0.5 h-4 w-4 shrink-0 text-[var(--accent)]" />
                <div className="min-w-0 flex-1">
                  <p className="text-xs font-medium text-white">
                    Cortex will update {parsedPreview.length} task {parsedPreview.length === 1 ? 'item' : 'items'}
                  </p>
                  <p className="mt-0.5 truncate text-xs text-[var(--muted-strong)]">
                    {parsedPreview.map((action) => action.summary).join(' ')}
                  </p>
                </div>
              </div>
            </div>
          )}
          <ChatComposer
            draft={draft}
            disabled={isStreaming}
            locked={needsSubscription}
            onDraftChange={onDraftChange}
            onSend={handleSend}
            onStop={onStop}
            onSubscribe={onSubscribe}
          />
        </section>
        )}
        right={(

        <aside className={`${mobilePanel === 'chat' ? 'hidden' : 'flex'} h-full min-h-0 flex-col ${rightPanelOpen ? 'lg:flex' : 'lg:hidden'}`}>
          <div className="hidden border-b border-white/6 px-4 py-3 lg:block">
            {boardHeader}
          </div>
          <div className={`${mobilePanel === 'board' ? 'flex' : 'hidden'} min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-3 lg:flex lg:p-4`}>
            <div className="lg:hidden">{boardHeader}</div>
            <StructuredTaskForm
              members={taskManager.state.members}
              onCreateTask={taskManager.createTask}
            />
            <TaskBoard
              tasks={taskManager.state.tasks}
              members={taskManager.state.members}
              groupId={group.id}
              compact
              showBackendSignals
              selectedTaskId={selectedTask?.id ?? null}
              onUpdateTask={taskManager.updateTask}
              onSelectTask={handleSelectTask}
              onLaunchTask={handleLaunchTaskInProjectChat}
            />
            <TaskInspector
              task={selectedTask}
              members={taskManager.state.members}
              activity={taskManager.state.activity}
              isCreatingRun={creatingRunTaskId === selectedTask?.id}
              runError={runError}
              onCreateRun={handleCreateTaskRun}
              onSyncRunState={handleSyncRunState}
              onUpdateRunSnapshot={handleUpdateRunSnapshot}
              onLaunchTask={handleLaunchTaskInProjectChat}
            />
          </div>
          <div className={`${mobilePanel === 'team' ? 'flex' : 'hidden'} min-h-0 flex-1 flex-col gap-5 overflow-y-auto p-3 lg:flex lg:p-4`}>
            <TeamPanel
              members={taskManager.state.members}
              tasks={taskManager.state.tasks}
              declaredMembers={group.members}
            />
            {memoryData && showMemoryPanel && (
              <MemoryPanel
                memoryData={memoryData}
                onApplySuggestion={handleApplyMemorySuggestion}
                onCreateCapture={handleCreateMemoryCapture}
                onClose={() => setShowMemoryPanel(false)}
              />
            )}
            <ActivityPanel activity={taskManager.state.activity} />
            <div className="rounded-lg border border-white/8 bg-white/[0.03] p-3">
              <div className="mb-2 flex items-center gap-2">
                <PanelRight className="h-4 w-4 text-[var(--muted)]" />
                <h2 className="text-sm font-semibold text-white">Shortcuts</h2>
              </div>
              <div className="grid gap-1.5 text-xs text-[var(--muted)]">
                <p><span className="text-[var(--muted-strong)]">Enter</span> sends chat commands.</p>
                <p><span className="text-[var(--muted-strong)]">Shift Enter</span> adds a line.</p>
                <p><span className="text-[var(--muted-strong)]">Ctrl 1/2/3</span> switches mobile panels.</p>
              </div>
            </div>
          </div>
        </aside>
        )}
      />
      ) : (
        <section className={`${mobilePanel === 'chat' ? 'flex' : 'hidden'} h-full min-h-0 flex-col lg:flex`}>
          <div className="border-b border-white/6 px-4 py-3">
            <div className="flex items-center justify-between gap-3">
              <div className="min-w-0">
                <div className="flex items-center gap-2">
                  <MessageSquareText className="h-4 w-4 text-[var(--muted)]" />
                  <h1 className="truncate text-sm font-semibold text-white">Task Manager Chat</h1>
                </div>
                <p className="mt-0.5 truncate text-xs text-[var(--muted)]">
                  Coordinate assignments, handoffs, repo work, and live priorities for {group.name}.
                </p>
              </div>
              <div className="flex items-center gap-2">
                {memoryData && (
                  <button
                    type="button"
                    title={showMemoryPanel ? "Hide memory panel" : "Show memory intelligence"}
                    onClick={() => setShowMemoryPanel(!showMemoryPanel)}
                    className={`inline-flex h-8 w-8 items-center justify-center rounded-lg transition active:scale-95 ${
                      showMemoryPanel ? 'bg-[var(--accent)]/20 text-[var(--accent)]' : 'text-[var(--muted)] hover:bg-white/6 hover:text-white'
                    }`}
                  >
                    <Brain className="h-4 w-4" />
                  </button>
                )}
                <button
                  type="button"
                  title={rightPanelOpen ? "Hide task board" : "Show task board"}
                  onClick={() => setRightPanelOpen(!rightPanelOpen)}
                  className="inline-flex h-8 w-8 items-center justify-center rounded-lg text-[var(--muted)] transition hover:bg-white/6 hover:text-white active:scale-95 lg:flex hidden"
                >
                  <LayoutGrid className="h-4 w-4" />
                </button>
                <span className="hidden rounded-full border border-[var(--accent)]/20 bg-[var(--accent)]/10 px-2.5 py-1 text-[11px] text-[var(--muted-strong)] sm:inline">
                  {group.kind === 'personal' ? 'Personal' : 'Team'} brain
                </span>
              </div>
            </div>
          </div>

          <ChatTimeline
            messages={messages}
            isLoading={isLoadingConversation}
            showStarters={!activeConversationId && !isStreaming}
            onSelectStarter={needsSubscription ? undefined : onDraftChange}
            onApprovalAction={onApprovalAction}
          />

          {parsedPreview.length > 0 && (
            <div className="border-t border-white/6 px-3 py-2 sm:px-4">
              <div className="flex items-start gap-2 rounded-lg border border-[var(--accent)]/20 bg-[var(--accent)]/10 px-3 py-2">
                <Command className="mt-0.5 h-4 w-4 shrink-0 text-[var(--accent)]" />
                <div className="min-w-0 flex-1">
                  <p className="text-xs font-medium text-white">
                    Cortex will update {parsedPreview.length} task {parsedPreview.length === 1 ? 'item' : 'items'}
                  </p>
                  <p className="mt-0.5 truncate text-xs text-[var(--muted-strong)]">
                    {parsedPreview.map((action) => action.summary).join(' ')}
                  </p>
                </div>
              </div>
            </div>
          )}

          <ChatComposer
            draft={draft}
            disabled={isStreaming}
            locked={needsSubscription}
            onDraftChange={onDraftChange}
            onSend={handleSend}
            onStop={onStop}
            onSubscribe={onSubscribe}
          />
        </section>
      )}
    </main>
  );
}
