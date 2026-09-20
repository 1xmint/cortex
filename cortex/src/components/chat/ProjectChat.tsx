import { useCallback, useState } from 'react';
import ChatComposer from './ChatComposer';
import ChatTimeline from './ChatTimeline';
import MemoryStatus from './MemoryStatus';
import MemoryManagement from './MemoryManagement';
import ContextMemoryPrompts from './ContextMemoryPrompts';
import { Brain, Settings, Lightbulb, FileText } from 'lucide-react';
import {
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
} from '../../types';

interface ProjectChatProps {
  group: CortexGroup;
  userId: string;
  activeConversationId: string | null;
  messages: ChatMessage[];
  draft: string;
  isStreaming: boolean;
  isLoadingConversation: boolean;
  needsSubscription: boolean;
  isPreview?: boolean;
  onDraftChange: (value: string) => void;
  onSend: () => void;
  onStop?: () => void;
  onSubscribe: () => void;
  onApprovalAction: (messageId: string, nextState: ApprovalState) => void;
}

const PROJECT_STARTER_PROMPTS = [
  'I want to build a habit tracker app',
  'Create a new task manager project',
  'Build a REST API service',
  'Help me debug this API call that\'s returning 500 errors.',
  'Review this code for potential security vulnerabilities.',
  'Set up a new React project',
];

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
    <div className="px-4 pb-2">
      <section className="rounded-lg border border-[var(--accent)]/20 bg-[var(--accent)]/5 p-3">
        <div className="mb-3 flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Brain className="h-4 w-4 text-[var(--accent)]" />
            <h2 className="text-sm font-semibold text-white">Memory Intelligence</h2>
          </div>
          <button
            onClick={onClose}
            aria-label="Close memory panel"
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
    </div>
  );
}

export default function ProjectChat({
  group,
  activeConversationId,
  messages,
  draft,
  isStreaming,
  isLoadingConversation,
  needsSubscription,
  isPreview = false,
  onDraftChange,
  onSend,
  onStop,
  onSubscribe,
  onApprovalAction,
}: ProjectChatProps) {
  const [showMemoryManagement, setShowMemoryManagement] = useState(false);
  const [memoryData, setMemoryData] = useState<MemoryEnhancedChatResponse | null>(null);
  const [showMemoryPanel, setShowMemoryPanel] = useState(false);
  const [isProcessingMemory, setIsProcessingMemory] = useState(false);

  // Extract current files from messages context
  const currentFiles = messages
    .slice(-5) // Look at recent messages
    .flatMap(msg => {
      const fileMatches = msg.content.match(/`([^`]*\.(rs|ts|tsx|js|jsx|py|go|java|cpp|c|h))`/g);
      return fileMatches ? fileMatches.map(match => match.replace(/`/g, '')) : [];
    })
    .filter((file, index, arr) => arr.indexOf(file) === index); // Unique files

  const recentMessages = messages.slice(-3).map(msg => msg.content);

  // Count messages sent by the user (not system/assistant) for preview prompt
  const userMessageCount = messages.filter(msg => msg.role === 'user').length;

  // Process message through memory system
  const processMemory = useCallback(async (message: string) => {
    if (!MEMORY_API_ENABLED || !message.trim() || isProcessingMemory) return;

    try {
      setIsProcessingMemory(true);

      // Use group id as workspace for project-specific memory
      const projectWorkspaceId = `project_${group.id}`;

      // Process through memory system
      const response = await processMemoryEnhancedChat({
        message,
        files: currentFiles,
        workspaceId: projectWorkspaceId,
        conversationId: activeConversationId || 'default',
        teamMembers: [], // Projects don't have explicit team members like tasks
        projectPhase: 'development',
        activeTopics: ['code', 'implementation', 'debugging'],
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
  }, [group.id, currentFiles, activeConversationId, isProcessingMemory]);

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
      const projectWorkspaceId = `project_${group.id}`;
      await createFromAutoCapture(opportunity, projectWorkspaceId);
      // Hide the auto-capture after creation
      setMemoryData(prev => prev ? { ...prev, auto_capture: undefined } : null);
    } catch (error) {
      console.error('Failed to create memory capture:', error);
    }
  }, [group.id]);

  const handleSend = useCallback(async () => {
    // Process through memory system first
    await processMemory(draft);

    // Send the message
    onSend();
  }, [draft, onSend, processMemory]);

  const handleSelectFlowOption = useCallback((optionId: string) => {
    // Send the selected option as a message
    onDraftChange(optionId);
    setTimeout(() => onSend(), 0); // Send after state update
  }, [onDraftChange, onSend]);

  // Show context prompts when user starts typing (but not memory commands)
  const shouldShowContextPrompts = draft.length > 10 &&
    !draft.toLowerCase().match(/\b(remember|recall|forget|store|save)\b/) &&
    !isStreaming &&
    !needsSubscription;

  const handleContextPromptSelect = useCallback((prompt: string) => {
    onDraftChange(prompt);
  }, [onDraftChange]);

  return (
    <main className="flex h-full min-h-0 flex-1 flex-col overflow-hidden" aria-busy={isStreaming}>
      <div className="border-b border-white/6 px-4 py-2">
        <div className="flex items-center justify-between gap-3">
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <h1 className="truncate text-sm font-semibold text-white">Project Chat</h1>
              <span className="truncate text-xs text-[var(--muted)]">{group.name}</span>
            </div>
          </div>

          <div className="flex items-center gap-2">
            {/* Memory Status Compact */}
            {MEMORY_API_ENABLED && <MemoryStatus workspaceId={`project_${group.id}`} compact />}

            {memoryData && (
              <button
                type="button"
                title={showMemoryPanel ? "Hide memory panel" : "Show memory intelligence"}
                aria-label={showMemoryPanel ? "Hide memory panel" : "Show memory intelligence"}
                onClick={() => setShowMemoryPanel(!showMemoryPanel)}
                className={`inline-flex h-8 w-8 items-center justify-center rounded-lg transition active:scale-95 ${
                  showMemoryPanel ? 'bg-[var(--accent)]/20 text-[var(--accent)]' : 'text-[var(--muted)] hover:bg-white/6 hover:text-white'
                }`}
              >
                <Brain className="h-4 w-4" />
              </button>
            )}

            {MEMORY_API_ENABLED && (
              <button
                onClick={() => setShowMemoryManagement(true)}
                className="flex items-center gap-1 px-2 py-1 text-xs bg-white/5 border border-white/10 rounded-md hover:bg-white/10 transition-colors"
                title="Manage memories"
                aria-label="Manage memories"
              >
                <Brain className="h-3 w-3" />
                <Settings className="h-3 w-3" />
              </button>
            )}
          </div>
        </div>
      </div>

      <ChatTimeline
        messages={messages}
        isLoading={isLoadingConversation}
        showStarters={!activeConversationId && !isStreaming}
        onSelectStarter={needsSubscription ? undefined : onDraftChange}
        onApprovalAction={onApprovalAction}
        onSelectFlowOption={needsSubscription ? undefined : handleSelectFlowOption}
        flowOptionsDisabled={isStreaming}
        starterPrompts={PROJECT_STARTER_PROMPTS}
      />

      {/* Memory Intelligence Panel */}
      {memoryData && showMemoryPanel && (
        <MemoryPanel
          memoryData={memoryData}
          onApplySuggestion={handleApplyMemorySuggestion}
          onCreateCapture={handleCreateMemoryCapture}
          onClose={() => setShowMemoryPanel(false)}
        />
      )}

      {/* Context-aware memory prompts */}
      {MEMORY_API_ENABLED && shouldShowContextPrompts && (
        <div className="px-4 pb-2">
          <ContextMemoryPrompts
            context={{
              message: draft,
              files: currentFiles,
              recentMessages,
              workspaceId: group.id,
            }}
            onPromptSelect={handleContextPromptSelect}
            visible={shouldShowContextPrompts}
          />
        </div>
      )}

      {/* Soft subscription prompt for preview users who have sent 5+ messages */}
      {isPreview && !needsSubscription && userMessageCount >= 5 && (
        <div className="mx-4 mb-3 rounded-xl border border-[var(--accent)]/20 bg-[var(--accent)]/8 p-3">
          <p className="text-xs text-[var(--muted-strong)]">
            You're exploring Cortex in preview mode with sample data.
            <button
              type="button"
              onClick={onSubscribe}
              className="ml-1 font-medium text-[var(--accent)] hover:underline"
            >
              Subscribe to Cortex Pro
            </button>
            {' '}to connect real AI agents and unlock full capabilities.
          </p>
        </div>
      )}

      <ChatComposer
        draft={draft}
        disabled={isStreaming}
        locked={needsSubscription}
        placeholder="Ask me to help debug, refactor, review, or build anything..."
        onDraftChange={onDraftChange}
        onSend={handleSend}
        onStop={onStop}
        onSubscribe={onSubscribe}
        currentFiles={currentFiles}
        recentMessages={recentMessages}
      />

      {/* Memory Management Modal */}
      {MEMORY_API_ENABLED && showMemoryManagement && (
        <MemoryManagement
          workspaceId={`project_${group.id}`}
          onClose={() => setShowMemoryManagement(false)}
        />
      )}
    </main>
  );
}
