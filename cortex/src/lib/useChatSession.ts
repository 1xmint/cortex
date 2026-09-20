import { useCallback, useEffect, useRef, useState } from 'react';
import type {
  ApprovalState,
  ChatMessage,
  ChatProject,
  ChatSessionControls,
  ConversationFlow,
  RunProfile,
  WorkEventItem,
} from '../types';
import type { CortexGroup } from './groups';
import { buildSovereigntyLoopState } from './sovereignty';
import {
  addMessageToConversation,
  CortexApiError,
  createConversation,
  getConversation,
  streamChat,
  updateConversationTitle,
  type ConversationMessage,
} from './cortexApi';
import { createImplementationMessage } from './projectImplementation';
// TODO: Implement workspace routing integration
// import {
//   getWorkspaceContext,
//   getWorkspaceUserId,
//   getWorkspaceRoutingPreferences,
//   shouldRouteToWorkspace,
//   getWorkspaceStatusMessage
// } from './workspaceRouting';

function createId(prefix: string) {
  return `${prefix}-${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
}

const PROJECT: ChatProject = {
  id: 'project-cortex',
  name: 'ClawNet / Cortex',
  environment: 'workspace',
  connectionStatus: 'cortex-server connected',
  environmentState: 'connected',
};

function getEmptyMessages(): ChatMessage[] {
  return [
    {
      id: 'm-init',
      role: 'assistant',
      providerLabel: 'Cortex',
      statusLabel: 'Ready',
      createdAt: new Date().toISOString(),
      content: 'What would you like to work on?',
    },
  ];
}

function formatProviderLabel(provider?: string | null) {
  return provider ? `Cortex · ${provider}` : 'Cortex';
}

function formatStatusLabel(model?: string | null) {
  return model ? `${model}` : 'Ready';
}

function mapConversationMessage(message: ConversationMessage): ChatMessage {
  return {
    id: message.id,
    role: message.role,
    content: message.content,
    createdAt: message.created_at,
    provider: message.provider ?? undefined,
    model: message.model ?? undefined,
    providerLabel: message.role === 'assistant' ? formatProviderLabel(message.provider) : undefined,
    statusLabel: message.role === 'assistant' ? formatStatusLabel(message.model) : undefined,
  };
}

function truncateTitle(text: string) {
  const normalized = text.trim().replace(/\s+/g, ' ');
  if (normalized.length <= 60) return normalized;
  return `${normalized.slice(0, 57).trimEnd()}...`;
}

function parseFlowResponse(content: string): ConversationFlow | null {
  try {
    // Look for flow options in the content
    const optionsMatch = content.match(/\*\*Available options:\*\*\n```json\n(.*?)\n```/s);
    if (!optionsMatch) return null;

    const optionsJson = optionsMatch[1];
    const options = JSON.parse(optionsJson);

    // Look for handoff information
    const handoffMatch = content.match(/\*\*Handoff to ([^*]+)\*\*\nContext: (.+)/s);
    let handoff;
    if (handoffMatch) {
      try {
        handoff = {
          target: handoffMatch[1].trim(),
          context: JSON.parse(handoffMatch[2]),
        };
      } catch {
        handoff = {
          target: handoffMatch[1].trim(),
          context: handoffMatch[2],
        };
      }
    }

    return {
      flowType: 'setup', // Default flow type, could be extracted from content if needed
      options: Array.isArray(options) ? options : [],
      flowState: {
        currentStep: 'active',
        progress: 0.5, // Default progress, could be extracted if available
        completed: false,
      },
      flowComplete: Boolean(handoff),
      handoff,
    };
  } catch {
    return null;
  }
}

function approvalStorageKey(userId: string, conversationId: string) {
  return `cortex:approvals:${userId}:${conversationId}`;
}

function isApprovalState(value: unknown): value is ApprovalState {
  return value === 'pending'
    || value === 'reviewed'
    || value === 'approved'
    || value === 'rejected';
}

function readApprovalStateMap(userId: string, conversationId: string): Record<string, ApprovalState> {
  try {
    const raw = window.localStorage.getItem(approvalStorageKey(userId, conversationId));
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== 'object') return {};

    return Object.fromEntries(
      Object.entries(parsed as Record<string, unknown>).filter((entry): entry is [string, ApprovalState] =>
        typeof entry[0] === 'string' && isApprovalState(entry[1]),
      ),
    );
  } catch {
    return {};
  }
}

function writeApprovalState(
  userId: string,
  conversationId: string,
  messageId: string,
  nextState: ApprovalState,
) {
  try {
    const current = readApprovalStateMap(userId, conversationId);
    window.localStorage.setItem(
      approvalStorageKey(userId, conversationId),
      JSON.stringify({ ...current, [messageId]: nextState }),
    );
  } catch {
    // ignore local approval persistence failures
  }
}

function applyStoredApprovals(
  messages: ChatMessage[],
  userId: string,
  conversationId: string,
): ChatMessage[] {
  const storedApprovals = readApprovalStateMap(userId, conversationId);
  if (Object.keys(storedApprovals).length === 0) return messages;

  return messages.map((message) => {
    if (!message.approvalRequest) return message;
    const storedState = storedApprovals[message.id];
    if (!storedState) return message;

    return {
      ...message,
      approvalRequest: {
        ...message.approvalRequest,
        state: storedState,
      },
    };
  });
}

interface UseChatSessionOptions {
  activeConversationId: string | null;
  userId: string;
  isSignedIn: boolean;
  group: CortexGroup;
  sessionControls: ChatSessionControls;
  runProfile: RunProfile;
  onConversationCreated: (conversationId: string) => void;
  onConversationsChanged: () => void;
}

export function useChatSession({
  activeConversationId,
  userId,
  isSignedIn,
  group,
  sessionControls,
  runProfile,
  onConversationCreated,
  onConversationsChanged,
}: UseChatSessionOptions) {
  const [messages, setMessages] = useState<ChatMessage[]>(getEmptyMessages);
  const [draft, setDraft] = useState('');
  const [isStreaming, setIsStreaming] = useState(false);
  const [isLoadingConversation, setIsLoadingConversation] = useState(false);
  const [conversationNotFound, setConversationNotFound] = useState(false);
  const [activeConversationTitle, setActiveConversationTitle] = useState<string | null>(null);
  const [workEvents, setWorkEvents] = useState<WorkEventItem[]>([]);
  const abortRef = useRef<AbortController | null>(null);
  const activeConversationIdRef = useRef<string | null>(activeConversationId);
  const messagesRef = useRef<ChatMessage[]>(messages);
  const requestVersionRef = useRef(0);
  const skipNextConversationLoadRef = useRef<string | null>(null);

  useEffect(() => {
    activeConversationIdRef.current = activeConversationId;
  }, [activeConversationId]);

  useEffect(() => {
    messagesRef.current = messages;
  }, [messages]);

  useEffect(() => {
    if (
      activeConversationId &&
      skipNextConversationLoadRef.current === activeConversationId
    ) {
      skipNextConversationLoadRef.current = null;
      return;
    }

    abortRef.current?.abort();
    abortRef.current = null;
    setIsStreaming(false);
    setIsLoadingConversation(false);
    setConversationNotFound(false);
    setWorkEvents([]);

    const requestVersion = ++requestVersionRef.current;

    if (!activeConversationId) {
      setMessages(getEmptyMessages());
      setActiveConversationTitle(null);
      return;
    }

    setMessages([]);
    setWorkEvents([]);
    setIsLoadingConversation(true);

    void (async () => {
      try {
        const conversation = await getConversation(activeConversationId, userId);
        if (requestVersionRef.current !== requestVersion) return;

        setMessages(
          conversation.messages.length > 0
            ? applyStoredApprovals(
                conversation.messages.map(mapConversationMessage),
                userId,
                activeConversationId,
              )
            : getEmptyMessages(),
        );
        setActiveConversationTitle(conversation.title);
        setIsLoadingConversation(false);
      } catch (err) {
        if (requestVersionRef.current !== requestVersion) return;
        const is404 = err instanceof CortexApiError && err.status === 404;
        setConversationNotFound(is404);
        setMessages([
          {
            id: 'm-load-error',
            role: 'assistant',
            providerLabel: 'Cortex',
            statusLabel: is404 ? 'Not found' : 'Load failed',
            createdAt: new Date().toISOString(),
            content: is404
              ? 'This conversation could not be found. It may have been deleted or the link is invalid.'
              : 'Could not load this conversation.',
          },
        ]);
        setActiveConversationTitle(null);
        setIsLoadingConversation(false);
      }
    })();
  }, [activeConversationId, userId]);

  const updateApproval = useCallback((messageId: string, nextState: ApprovalState) => {
    const conversationId = activeConversationIdRef.current;
    if (conversationId) {
      writeApprovalState(userId, conversationId, messageId, nextState);
    }

    setMessages((cur) =>
      cur.map((m) => {
        if (m.id !== messageId || !m.approvalRequest) return m;
        return { ...m, approvalRequest: { ...m.approvalRequest, state: nextState } };
      }),
    );
  }, [userId]);

  const renameConversation = useCallback(async (title: string) => {
    const conversationId = activeConversationIdRef.current;
    const nextTitle = truncateTitle(title);
    if (!conversationId || !nextTitle) return;

    const previousTitle = activeConversationTitle;
    setActiveConversationTitle(nextTitle);

    try {
      await updateConversationTitle(conversationId, nextTitle, userId);
      onConversationsChanged();
    } catch {
      setActiveConversationTitle(previousTitle);
    }
  }, [activeConversationTitle, onConversationsChanged, userId]);

  const stopStreaming = useCallback(() => {
    const streamingMessage = messagesRef.current.find(
      (message) => message.role === 'assistant' && message.isStreaming,
    );
    if (!streamingMessage) return;

    abortRef.current?.abort();
    abortRef.current = null;
    requestVersionRef.current += 1;

    const stoppedContent = streamingMessage.content
      ? `${streamingMessage.content}\n\nStopped by user.`
      : 'Stopped by user.';
    const conversationId = activeConversationIdRef.current;

    setMessages((currentMessages) =>
      currentMessages.map((message) =>
        message.id === streamingMessage.id
          ? {
              ...message,
              content: stoppedContent,
              statusLabel: 'Stopped',
              isStreaming: false,
            }
          : message,
      ),
    );
    setIsStreaming(false);
    setWorkEvents((currentEvents) => [
      {
        id: createId('work-stopped'),
        taskId: streamingMessage.id,
        title: 'Stopped',
        detail: 'User stopped the current response.',
        timestamp: new Date().toISOString(),
        state: 'done' as const,
        provider: streamingMessage.provider,
        model: streamingMessage.model,
      },
      ...currentEvents.map((workEvent) =>
        workEvent.state === 'active'
          ? { ...workEvent, state: 'done' as const }
          : workEvent,
      ),
    ].slice(0, 24));

    if (conversationId) {
      void addMessageToConversation(
        conversationId,
        'assistant',
        stoppedContent,
        streamingMessage.provider,
        streamingMessage.model,
      )
        .then(() => onConversationsChanged())
        .catch(() => {
          // keep local stopped state even if persistence fails
        });
    }
  }, [onConversationsChanged]);

  const sendMessage = useCallback(() => {
    const text = draft.trim();
    if (!text || isStreaming) return;

    const userMsg: ChatMessage = {
      id: createId('user'),
      role: 'user',
      content: text,
      createdAt: new Date().toISOString(),
    };
    const sovereignty = buildSovereigntyLoopState({
      prompt: text,
      group,
      controls: sessionControls,
      runProfile,
      userId,
      signedIn: isSignedIn,
    });

    const assistantId = createId('assistant');
    const assistantMsg: ChatMessage = {
      id: assistantId,
      role: 'assistant',
      content: '',
      createdAt: new Date().toISOString(),
      provider: 'cortex',
      providerLabel: 'Cortex',
      statusLabel: 'Routing...',
      isStreaming: true,
      sovereignty,
    };

    setDraft('');
    setIsStreaming(true);
    setMessages((cur) => [...cur, userMsg, assistantMsg]);
    setWorkEvents([
      {
        id: createId('work-seal'),
        taskId: assistantId,
        title: 'Session sealed',
        detail: `${sovereignty.seal.boundary}; ${sovereignty.seal.credentialMode.replaceAll('_', ' ')} credentials.`,
        timestamp: new Date().toISOString(),
        state: 'done',
      },
      {
        id: createId('work-context'),
        taskId: assistantId,
        title: 'Context synthesized',
        detail: [
          sovereignty.context.gene.title,
          ...sovereignty.context.liveRepo.signals.slice(0, 2),
        ].join(' · '),
        timestamp: new Date().toISOString(),
        state: 'done',
      },
      {
        id: createId('work-routing'),
        taskId: assistantId,
        title: 'Routing advised',
        detail: `${sovereignty.routing.provider} / ${sovereignty.routing.model}; ${sovereignty.routing.rationale.join(', ')}.`,
        timestamp: new Date().toISOString(),
        state: 'active',
        provider: sovereignty.routing.provider,
        model: sovereignty.routing.model,
      },
    ]);

    void (async () => {
      const streamVersion = requestVersionRef.current;
      const hadPriorUserMessage = messagesRef.current.some(
        (message) => message.role === 'user',
      );
      let conversationId = activeConversationIdRef.current;
      let assistantContent = '';
      let assistantProvider: string | undefined;
      let assistantModel: string | undefined;
      let finalized = false;

      const persistAssistant = async (content: string) => {
        if (!conversationId || finalized) return;
        finalized = true;
        try {
          await addMessageToConversation(
            conversationId,
            'assistant',
            content,
            assistantProvider,
            assistantModel,
          );
          onConversationsChanged();
        } catch {
          // keep local transcript even if persistence fails
        }
      };

      const finalizeAssistant = (next: Partial<ChatMessage>, content?: string) => {
        const finalContent = content ?? assistantContent;
        setMessages((cur) =>
          cur.map((message) =>
            message.id === assistantId
              ? {
                  ...message,
                  ...next,
                  content: finalContent,
                }
              : message,
          ),
        );
        setIsStreaming(false);
        void persistAssistant(finalContent);
      };

      try {
        if (!conversationId) {
          const created = await createConversation(userId);
          conversationId = created.id;
          activeConversationIdRef.current = created.id;
          skipNextConversationLoadRef.current = created.id;
          onConversationCreated(created.id);
          onConversationsChanged();
        }

        await addMessageToConversation(conversationId, 'user', text);

        if (!hadPriorUserMessage) {
          const nextTitle = truncateTitle(text);
          setActiveConversationTitle(nextTitle);
          void updateConversationTitle(conversationId, nextTitle, userId)
            .then(() => {
              onConversationsChanged();
            })
            .catch(() => {
              // keep chat flow moving if titling fails
            });
        }

        if (requestVersionRef.current !== streamVersion) return;

        const workspacePreferences = undefined;
        const routingContext = {
          controls: sessionControls,
          run_profile: runProfile,
          sovereignty,
          routing_preferences: workspacePreferences,
        };

        const controller = streamChat(
          text,
          [],
          routingContext,
          (event) => {
            if (requestVersionRef.current !== streamVersion) return;

            switch (event.type) {
              case 'started': {
                const stepId = event.step_id ?? event.task_id;
                assistantProvider = event.provider ?? assistantProvider;
                assistantModel = event.model ?? assistantModel;
                setWorkEvents((currentEvents) => [
                  {
                    id: createId('work-started'),
                    taskId: stepId,
                    title: 'Started',
                    detail: `${event.provider ?? 'Provider'}${event.model ? ` · ${event.model}` : ''} is working.`,
                    timestamp: new Date().toISOString(),
                    state: 'active' as const,
                    provider: event.provider,
                    model: event.model,
                  },
                  ...currentEvents.map((workEvent) =>
                    workEvent.state === 'active'
                      ? { ...workEvent, state: 'done' as const }
                      : workEvent,
                  ),
                ]);
                setMessages((cur) =>
                  cur.map((m) =>
                    m.id === assistantId
                      ? {
                          ...m,
                          provider: assistantProvider,
                          model: assistantModel,
                          providerLabel: formatProviderLabel(assistantProvider),
                          statusLabel: assistantModel ? `${assistantModel} working...` : 'Working...',
                        }
                      : m,
                  ),
                );
                break;
              }

              case 'output': {
                const stepId = event.step_id ?? event.task_id;
                assistantContent = assistantContent
                  ? `${assistantContent}\n\n${event.line ?? ''}`
                  : (event.line ?? '');

                // Detect and parse flow responses
                const flowResponse = parseFlowResponse(assistantContent);

                if (event.line) {
                  setWorkEvents((currentEvents) => [
                    {
                      id: createId('work-output'),
                      taskId: stepId,
                      title: 'Output',
                      detail: event.line ?? '',
                      timestamp: new Date().toISOString(),
                      state: 'active' as const,
                      provider: assistantProvider,
                      model: assistantModel,
                    },
                    ...currentEvents,
                  ].slice(0, 24));
                }
                setMessages((cur) =>
                  cur.map((m) =>
                    m.id === assistantId
                      ? {
                          ...m,
                          content: assistantContent,
                          conversationFlow: flowResponse ?? undefined
                        }
                      : m,
                  ),
                );
                break;
              }

              case 'completed': {
                const stepId = event.step_id ?? event.task_id;
                setWorkEvents((currentEvents) => [
                  {
                    id: createId('work-completed'),
                    taskId: stepId,
                    title: 'Completed',
                    detail: `Worker completed${typeof event.exit_code === 'number' ? ` with exit code ${event.exit_code}` : ''}.`,
                    timestamp: new Date().toISOString(),
                    state: 'done' as const,
                    provider: assistantProvider,
                    model: assistantModel,
                  },
                  ...currentEvents.map((workEvent) =>
                    workEvent.state === 'active'
                      ? { ...workEvent, state: 'done' as const }
                      : workEvent,
                  ),
                ].slice(0, 24));

                const flowResponse = parseFlowResponse(assistantContent);

                finalizeAssistant({
                  provider: assistantProvider,
                  model: assistantModel,
                  providerLabel: formatProviderLabel(assistantProvider),
                  statusLabel: 'Done',
                  isStreaming: false,
                  conversationFlow: flowResponse ?? undefined,
                });

                // Handle handoffs - if flow completed with handoff, generate implementation message
                if (flowResponse?.handoff) {
                  const implementationMessage = createImplementationMessage(flowResponse.handoff);
                  if (implementationMessage) {
                    setTimeout(() => {
                      // Add implementation message as new assistant response
                      const implId = createId('assistant');
                      const implMsg: ChatMessage = {
                        id: implId,
                        role: 'assistant',
                        content: implementationMessage,
                        createdAt: new Date().toISOString(),
                        provider: 'cortex',
                        providerLabel: 'Cortex · Project Builder',
                        statusLabel: 'Ready to build',
                      };

                      setMessages((cur) => [...cur, implMsg]);

                      // Persist the implementation message
                      if (conversationId) {
                        void addMessageToConversation(
                          conversationId,
                          'assistant',
                          implementationMessage,
                          'cortex',
                          'project_builder'
                        ).catch(() => {
                          // keep local message even if persistence fails
                        });
                      }
                    }, 1000); // Small delay for smooth UX
                  }
                }
                break;
              }

              case 'failed': {
                const stepId = event.step_id ?? event.task_id;
                const failedContent = assistantContent
                  ? `${assistantContent}\n\nError: ${event.error}`
                  : `Error: ${event.error}`;
                assistantContent = failedContent;
                setWorkEvents((currentEvents) => [
                  {
                    id: createId('work-failed'),
                    taskId: stepId,
                    title: 'Failed',
                    detail: event.error ?? 'Worker failed.',
                    timestamp: new Date().toISOString(),
                    state: 'failed' as const,
                    provider: assistantProvider,
                    model: assistantModel,
                  },
                  ...currentEvents.map((workEvent) =>
                    workEvent.state === 'active'
                      ? { ...workEvent, state: 'failed' as const }
                      : workEvent,
                  ),
                ].slice(0, 24));
                finalizeAssistant({
                  provider: assistantProvider,
                  model: assistantModel,
                  providerLabel: formatProviderLabel(assistantProvider),
                  statusLabel: 'Failed',
                  isStreaming: false,
                }, failedContent);
                break;
              }
            }
          },
          () => {
            if (requestVersionRef.current !== streamVersion || finalized) return;
            setWorkEvents((currentEvents) => [
              {
                id: createId('work-completed'),
                taskId: assistantId,
                title: 'Completed',
                detail: 'Cortex finished streaming the response.',
                timestamp: new Date().toISOString(),
                state: 'done' as const,
                provider: assistantProvider,
                model: assistantModel,
              },
              ...currentEvents.map((workEvent) =>
                workEvent.state === 'active'
                  ? { ...workEvent, state: 'done' as const }
                  : workEvent,
              ),
            ].slice(0, 24));
            finalizeAssistant({
              provider: assistantProvider,
              model: assistantModel,
              providerLabel: formatProviderLabel(assistantProvider),
              statusLabel: 'Done',
              isStreaming: false,
            });
          },
          (err) => {
            if (requestVersionRef.current !== streamVersion || finalized) return;
            const errorContent = assistantContent
              ? `${assistantContent}\n\nError: Could not reach Cortex backend: ${err.message}`
              : `Could not reach Cortex backend: ${err.message}`;
            assistantContent = errorContent;
            setWorkEvents((currentEvents) => [
              {
                id: createId('work-error'),
                taskId: assistantId,
                title: 'Connection error',
                detail: err.message,
                timestamp: new Date().toISOString(),
                state: 'failed' as const,
                provider: assistantProvider,
                model: assistantModel,
              },
              ...currentEvents.map((workEvent) =>
                workEvent.state === 'active'
                  ? { ...workEvent, state: 'failed' as const }
                  : workEvent,
              ),
            ].slice(0, 24));
            finalizeAssistant({
              provider: assistantProvider,
              model: assistantModel,
              providerLabel: formatProviderLabel(assistantProvider),
              statusLabel: 'Connection error',
              isStreaming: false,
            }, errorContent);
          },
        );

        abortRef.current = controller;
      } catch (err) {
        const message = err instanceof Error ? err.message : 'Unknown error';
        setMessages((cur) =>
          cur.map((entry) =>
            entry.id === assistantId
              ? {
                  ...entry,
                  isStreaming: false,
                  statusLabel: 'Failed',
                  content: `Could not send message: ${message}`,
                }
              : entry,
          ),
        );
        setIsStreaming(false);
        setWorkEvents((currentEvents) => [
          {
            id: createId('work-send-failed'),
            taskId: assistantId,
            title: 'Send failed',
            detail: message,
            timestamp: new Date().toISOString(),
            state: 'failed' as const,
          },
          ...currentEvents.map((workEvent) =>
            workEvent.state === 'active'
              ? { ...workEvent, state: 'failed' as const }
              : workEvent,
          ),
        ].slice(0, 24));
        if (conversationId) {
          void persistAssistant(`Could not send message: ${message}`);
        }
      }
    })();
  }, [
    draft,
    group,
    isSignedIn,
    isStreaming,
    onConversationCreated,
    onConversationsChanged,
    runProfile,
    sessionControls,
    userId,
  ]);

  return {
    project: PROJECT,
    messages,
    draft,
    isStreaming,
    isLoadingConversation,
    conversationNotFound,
    activeConversationTitle,
    workEvents,
    setDraft,
    sendMessage,
    stopStreaming,
    updateApproval,
    renameConversation,
  };
}
