import type { ApprovalRequest, ChatMessage, ChatProject } from '../types';

export interface MockResponseStep {
  delayMs: number;
  content: string;
}

export interface MockAssistantReply {
  providerLabel: string;
  statusLabel: string;
  steps: MockResponseStep[];
  approvalRequest?: ApprovalRequest;
}

const REPLY_TEMPLATES: MockAssistantReply[] = [
  {
    providerLabel: 'Cortex · Claude + GPT',
    statusLabel: 'Planning locally',
    steps: [
      {
        delayMs: 550,
        content: 'Routing this through the local workspace worker and keeping the current dashboard modules intact.',
      },
      {
        delayMs: 1280,
        content: 'I am shaping the chat-first shell around one project thread, mocked transport, and an approval gate that can swap to real backend events later.',
      },
      {
        delayMs: 2140,
        content: 'Next step would be wiring the same message contract to SSE or WebSocket transport once the Cortex backend exists.',
      },
    ],
  },
  {
    providerLabel: 'Cortex · provider blend',
    statusLabel: 'Reviewing patch scope',
    steps: [
      {
        delayMs: 520,
        content: 'I checked the surface area and kept changes scoped to the chat shell, shared types, and mock state.',
      },
      {
        delayMs: 1190,
        content: 'The approval card stays inline with the conversation so commit intent can be reviewed where the work was requested.',
      },
      {
        delayMs: 2050,
        content: 'This mock flow is local-only, but the component boundaries are shaped to port cleanly into the future Cortex API contract.',
      },
    ],
  },
];

const APPROVAL_TEMPLATE: Omit<ApprovalRequest, 'messageId' | 'state'> = {
  title: 'Commit request ready',
  summary: 'Chat-first panel slice prepared with mocked streaming state and inline approval controls.',
  commitMessage: 'feat(cortex): add first chat-first panel slice',
  diffSummary: 'App shell, chat components, and mock state updated; legacy dashboard modules preserved.',
  filesChanged: 8,
};

export function getMockProject(): ChatProject {
  return {
    id: 'project-clawnet',
    name: 'ClawNet / Cortex',
    environment: 'workspace',
    connectionStatus: 'guardian-vps-tail ready',
    environmentState: 'connected',
  };
}

export function getInitialMessages(): ChatMessage[] {
  return [
    {
      id: 'm-1',
      role: 'assistant',
      providerLabel: 'Cortex · Claude + GPT',
      statusLabel: 'Session ready',
      createdAt: new Date(Date.now() - 1000 * 60 * 16).toISOString(),
      content:
        'Project loaded. I can inspect the repo, draft a patch, and stage a commit approval without exposing provider-level noise.',
    },
    {
      id: 'm-2',
      role: 'user',
      createdAt: new Date(Date.now() - 1000 * 60 * 11).toISOString(),
      content: 'Prepare the first chat-first panel slice and keep the older dashboard views around for later.',
    },
    {
      id: 'm-3',
      role: 'assistant',
      providerLabel: 'Cortex · active agent haiku-router',
      statusLabel: 'Awaiting approval',
      createdAt: new Date(Date.now() - 1000 * 60 * 10).toISOString(),
      content:
        'I staged the UI slice locally and packaged the change as a mock commit request. Review it here before anything gets finalized.',
      approvalRequest: {
        messageId: 'm-3',
        state: 'pending',
        ...APPROVAL_TEMPLATE,
      },
    },
  ];
}

export function createMockAssistantReply(userText: string, messageId: string): MockAssistantReply {
  const template =
    REPLY_TEMPLATES[
      Math.abs(
        userText.split('').reduce((accumulator, character) => accumulator + character.charCodeAt(0), 0),
      ) % REPLY_TEMPLATES.length
    ];

  const shouldAttachApproval =
    /commit|approve|ship|patch|diff|review|slice|panel/i.test(userText);

  return {
    ...template,
    approvalRequest: shouldAttachApproval
      ? {
          messageId,
          state: 'pending',
          ...APPROVAL_TEMPLATE,
        }
      : undefined,
  };
}
