export interface ProviderInfo {
  score: number;
  cooldownUntil?: number;
  lastError?: string;
  degraded?: boolean;
  dispatchCount: number;
}

export interface ProvidersState {
  timestamp: string;
  providers: Record<string, ProviderInfo>;
}

export interface RoutingCell {
  ema: number;
  observations: number;
}

export interface TopPerformer {
  cell: string;
  model: string;
  ema: number;
  observations: number;
}

export interface RoutingState {
  timestamp: string;
  totalObservations: number;
  cells: Record<string, Record<string, RoutingCell>>;
  topPerformers: TopPerformer[];
  worstPerformers: TopPerformer[];
}

export interface Room {
  id: string;
  project: string;
  status: 'active' | 'closed';
  createdAt: string;
  workerCount: number;
}

export interface RoomsState {
  timestamp: string;
  rooms: Room[];
}

export interface Decision {
  timestamp: string;
  promptSummary: string;
  provider: string;
  model: string;
  tier: string;
  reason: string;
  explored: boolean;
}

export interface Outcome {
  timestamp: string;
  roomId: string;
  success: boolean;
  score: number;
  durationMs: number;
  provider: string;
  model: string;
}

export interface CostsState {
  timestamp: string;
  session: {
    totalTokens: number;
    inputTokens: number;
    outputTokens: number;
    estimatedCostUsd: number;
  };
  byProvider: Record<
    string,
    {
      tokens: number;
      estimatedCostUsd: number;
    }
  >;
}

export interface CortexState {
  providers: ProvidersState | null;
  routing: RoutingState | null;
  rooms: RoomsState | null;
  decisions: Decision[];
  outcomes: Outcome[];
  costs: CostsState | null;
  lastUpdated: string | null;
}

export type ChatRole = 'user' | 'assistant';
export type ApprovalState = 'pending' | 'reviewed' | 'approved' | 'rejected';
export type EnvironmentState = 'connected' | 'degraded' | 'offline';

export interface ChatProject {
  id: string;
  name: string;
  environment: string;
  connectionStatus: string;
  environmentState: EnvironmentState;
}

export interface ProviderStatus {
  id: string;
  label: string;
  status: 'active' | 'standby';
}

export interface AgentStatus {
  name: string;
  detail: string;
}

export interface ApprovalRequest {
  messageId: string;
  title: string;
  summary: string;
  commitMessage: string;
  diffSummary: string;
  filesChanged: number;
  state: ApprovalState;
}

export interface FlowOption {
  id: string;
  label: string;
  description?: string;
  category: string;
}

export interface FlowState {
  currentStep: string;
  progress: number;
  completed: boolean;
}

export interface ConversationFlow {
  flowType: string;
  options: FlowOption[];
  flowState: FlowState;
  flowComplete: boolean;
  handoff?: {
    target: string;
    context: unknown;
  };
}

export interface ChatMessage {
  id: string;
  role: ChatRole;
  content: string;
  createdAt: string;
  provider?: string;
  model?: string;
  providerLabel?: string;
  statusLabel?: string;
  isStreaming?: boolean;
  approvalRequest?: ApprovalRequest;
  sovereignty?: SovereigntyLoopState;
  conversationFlow?: ConversationFlow;
}

export type SessionSpeed = 'steady' | 'balanced' | 'rapid';
export type SessionIntelligence = 'focused' | 'balanced' | 'deep';
export type SessionAutonomy =
  | 'manual'
  | 'guided'
  | 'smart_auto'
  | 'full_auto'
  | 'custom';

export interface ChatSessionControls {
  speed: SessionSpeed;
  intelligence: SessionIntelligence;
  autonomy: SessionAutonomy;
}

export type RunProfile = 'auto' | 'balanced' | 'cost_saver' | 'quality_first';

export interface ContextSynthesis {
  gene: {
    title: string;
    invariants: string[];
  };
  liveRepo: {
    branch: string;
    signals: string[];
  };
  sessionMemory: string[];
}

export interface RoutingRecommendation {
  provider: string;
  model: string;
  mode: 'advisory' | 'manual' | 'auto';
  confidence: number;
  rationale: string[];
  tradeoffs: string[];
  alternatives: Array<{
    provider: string;
    model: string;
    reason: string;
  }>;
}

export interface SessionSeal {
  id: string;
  boundary: string;
  credentialMode: 'user_owned' | 'delegated' | 'not_connected';
  receipts: string[];
}

export interface SovereigntyLoopState {
  context: ContextSynthesis;
  routing: RoutingRecommendation;
  seal: SessionSeal;
  controls: ChatSessionControls;
  runProfile: RunProfile;
}

export type WorkEventState = 'active' | 'done' | 'waiting' | 'failed';

export interface WorkEventItem {
  id: string;
  taskId?: string;
  title: string;
  detail: string;
  timestamp: string;
  state: WorkEventState;
  provider?: string;
  model?: string;
}

export type TaskStatus = 'created' | 'assigned' | 'in-progress' | 'done' | 'paused' | 'cancelled' | 'queued';
export type TaskPriority = 'normal' | 'high' | 'urgent';
export type TaskMemberStatus = 'online' | 'working' | 'away';

export interface TaskMember {
  id: string;
  name: string;
  initials: string;
  status: TaskMemberStatus;
  currentTaskId?: string | null;
  color: string;
}

export interface TaskManagerTask {
  id: string;
  groupId: string;
  title: string;
  description?: string;
  status: TaskStatus;
  assigneeId?: string | null;
  repo?: string | null;
  priority: TaskPriority;
  createdAt: string;
  updatedAt: string;
  createdBy: string;
  sourceMessageId?: string;
  projectChatConversationId?: string | null;
  projectChatLaunchedAt?: string | null;
  latestRunId?: string | null;
  latestRunStatus?: string | null;
  latestRunSyncedAt?: string | null;
  latestRunStepSummary?: {
    total: number;
    active: number;
    done: number;
    failed: number;
  } | null;
  evidenceStatus?: {
    hasEvidence: boolean;
    gateddone: boolean;
    reason: string;
    totalSteps: number;
    verifiedSteps: number;
    failedSteps: number;
    unverifiedSteps: number;
  } | null;
}

export type TaskActivityKind = 'created' | 'assigned' | 'status' | 'handoff' | 'linked' | 'note' | 'paused' | 'resumed' | 'retried' | 'cancelled' | 'prioritized';

export interface TaskActivity {
  id: string;
  groupId: string;
  taskId?: string;
  kind: TaskActivityKind;
  actor: string;
  summary: string;
  createdAt: string;
}

export interface TaskManagerState {
  tasks: TaskManagerTask[];
  members: TaskMember[];
  activity: TaskActivity[];
  updatedAt: string;
}

export interface TaskCommandAction {
  type: 'task' | 'status' | 'handoff' | 'note' | 'pause' | 'resume' | 'retry' | 'cancel' | 'prioritize';
  title: string;
  assigneeId?: string | null;
  status?: TaskStatus;
  priority?: TaskPriority;
  targetTaskId?: string;
  summary: string;
}
