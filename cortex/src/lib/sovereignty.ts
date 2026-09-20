import type {
  ChatSessionControls,
  ContextSynthesis,
  RoutingRecommendation,
  RunProfile,
  SessionSeal,
  SovereigntyLoopState,
} from '../types';
import type { CortexGroup } from './groups';

const ROUTING_PREFERENCE_KEY = 'cortex:routing-preferences';

interface RoutingPreferenceMemory {
  qualityBias: number;
  costBias: number;
  manualBias: number;
  lastUpdated: string;
}

const DEFAULT_MEMORY: RoutingPreferenceMemory = {
  qualityBias: 0,
  costBias: 0,
  manualBias: 0,
  lastUpdated: new Date(0).toISOString(),
};

function readPreferenceMemory(): RoutingPreferenceMemory {
  try {
    const raw = window.localStorage.getItem(ROUTING_PREFERENCE_KEY);
    if (!raw) return DEFAULT_MEMORY;
    const parsed = JSON.parse(raw) as Partial<RoutingPreferenceMemory>;
    return {
      qualityBias: Number(parsed.qualityBias ?? 0),
      costBias: Number(parsed.costBias ?? 0),
      manualBias: Number(parsed.manualBias ?? 0),
      lastUpdated: typeof parsed.lastUpdated === 'string'
        ? parsed.lastUpdated
        : new Date().toISOString(),
    };
  } catch {
    return DEFAULT_MEMORY;
  }
}

function writePreferenceMemory(memory: RoutingPreferenceMemory) {
  try {
    window.localStorage.setItem(ROUTING_PREFERENCE_KEY, JSON.stringify(memory));
  } catch {
    // Preference learning is local and best effort.
  }
}

export type RoutingPreferenceSignal = 'quality' | 'cost' | 'manual';

export function recordRoutingPreference(signal: RoutingPreferenceSignal) {
  const current = readPreferenceMemory();
  const next = {
    ...current,
    qualityBias: current.qualityBias + (signal === 'quality' ? 1 : 0),
    costBias: current.costBias + (signal === 'cost' ? 1 : 0),
    manualBias: current.manualBias + (signal === 'manual' ? 1 : 0),
    lastUpdated: new Date().toISOString(),
  };
  writePreferenceMemory(next);
}

function detectIntent(text: string) {
  const lower = text.toLowerCase();
  return {
    codeChange: /\b(implement|fix|patch|build|edit|refactor|change)\b/.test(lower),
    review: /\b(review|audit|risk|inspect|check)\b/.test(lower),
    docs: /\b(doc|proposal|adr|readme|explain)\b/.test(lower),
    urgent: /\b(urgent|quick|fast|now|broken|hotfix)\b/.test(lower),
    highRisk: /\b(auth|credential|payment|billing|deploy|production|security|soma|ledger)\b/.test(lower),
  };
}

export function synthesizeContext(
  prompt: string,
  group: CortexGroup,
  controls: ChatSessionControls,
): ContextSynthesis {
  const intent = detectIntent(prompt);
  const invariants = [
    'Cortex keeps execution inside a sealed room owned by the user.',
    'Routing is advisory and visible before work is trusted.',
    'Soma receipts record boundary, delegation, and spend-relevant decisions.',
  ];

  if (intent.highRisk) {
    invariants.push('Security, billing, credential, and deploy changes require explicit boundary evidence.');
  }

  return {
    gene: {
      title: 'Sovereignty-first coding loop',
      invariants,
    },
    liveRepo: {
      branch: 'current workspace',
      signals: [
        `${group.name} ${group.kind === 'personal' ? 'personal' : 'team'} room`,
        intent.codeChange ? 'Likely code-writing request' : 'Likely advisory request',
        intent.review ? 'Review and risk scan requested' : 'Implementation path can be proposed',
        controls.autonomy === 'manual' ? 'Manual approval preferred' : 'Guided execution permitted',
      ],
    },
    sessionMemory: [
      `${controls.speed} speed`,
      `${controls.intelligence} reasoning`,
      `${controls.autonomy} autonomy`,
    ],
  };
}

export function recommendRoute(
  prompt: string,
  controls: ChatSessionControls,
  runProfile: RunProfile,
): RoutingRecommendation {
  const intent = detectIntent(prompt);
  const memory = readPreferenceMemory();
  const qualityScore = (controls.intelligence === 'deep' ? 2 : 0)
    + (runProfile === 'quality_first' ? 2 : 0)
    + memory.qualityBias;
  const costScore = (runProfile === 'cost_saver' ? 2 : 0)
    + (controls.speed === 'rapid' ? 1 : 0)
    + memory.costBias;
  const manualScore = (controls.autonomy === 'manual' ? 2 : 0)
    + (intent.highRisk ? 2 : 0)
    + memory.manualBias;

  const provider = qualityScore >= costScore ? 'Cortex worker' : 'Cortex fast lane';
  const model = qualityScore >= costScore ? 'deep-coding' : 'balanced-coding';
  const mode = manualScore >= 2
    ? 'manual'
    : controls.autonomy === 'full_auto' || controls.autonomy === 'smart_auto'
      ? 'auto'
      : 'advisory';

  const rationale = [
    intent.highRisk ? 'high-risk boundary detected' : 'standard coding boundary',
    intent.review ? 'review context requested' : 'implementation context requested',
    runProfile === 'cost_saver' ? 'cost saver profile' : runProfile === 'quality_first' ? 'quality first profile' : 'adaptive profile',
    `${controls.autonomy.replaceAll('_', ' ')} autonomy`,
  ];

  return {
    provider,
    model,
    mode,
    confidence: Math.min(0.92, 0.62 + Math.max(qualityScore, costScore, manualScore) * 0.05),
    rationale,
    tradeoffs: [
      qualityScore >= costScore ? 'More context budget for safer edits' : 'Lower latency and spend, less exhaustive search',
      mode === 'manual' ? 'User approval required before credentialed execution' : 'Worker may proceed inside current room limits',
    ],
    alternatives: [
      { provider: 'Cortex worker', model: 'deep-coding', reason: 'Best for risky multi-file changes' },
      { provider: 'Cortex fast lane', model: 'balanced-coding', reason: 'Best for small or time-sensitive edits' },
      { provider: 'Cortex review lane', model: 'critic-coding', reason: 'Best when the next action is review' },
    ],
  };
}

export function createSessionSeal(userId: string, signedIn: boolean): SessionSeal {
  return {
    id: `seal-${userId}-${Date.now().toString(36)}`,
    boundary: 'single workspace room',
    credentialMode: signedIn ? 'delegated' : 'not_connected',
    receipts: [
      'context_synthesized',
      'routing_recommendation_visible',
      signedIn ? 'user_delegation_available' : 'user_delegation_missing',
    ],
  };
}

export function buildSovereigntyLoopState(args: {
  prompt: string;
  group: CortexGroup;
  controls: ChatSessionControls;
  runProfile: RunProfile;
  userId: string;
  signedIn: boolean;
}): SovereigntyLoopState {
  return {
    context: synthesizeContext(args.prompt, args.group, args.controls),
    routing: recommendRoute(args.prompt, args.controls, args.runProfile),
    seal: createSessionSeal(args.userId, args.signedIn),
    controls: args.controls,
    runProfile: args.runProfile,
  };
}
