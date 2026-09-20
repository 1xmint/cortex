import { mkdirSync, writeFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const stateDir = join(__dirname, '..', '.dual-brain', 'state');
mkdirSync(stateDir, { recursive: true });

const now = new Date().toISOString();

writeFileSync(join(stateDir, 'providers.json'), JSON.stringify({
  timestamp: now,
  providers: {
    claude: { score: 0.92, dispatchCount: 47, degraded: false },
    openai: { score: 0.85, dispatchCount: 31, degraded: false },
  },
}, null, 2));

writeFileSync(join(stateDir, 'routing.json'), JSON.stringify({
  timestamp: now,
  totalObservations: 78,
  cells: {
    search: {
      'haiku': { ema: 0.91, observations: 22 },
      'gpt-4.1-mini': { ema: 0.84, observations: 14 },
    },
    execute: {
      'sonnet': { ema: 0.88, observations: 19 },
      'gpt-5.4': { ema: 0.82, observations: 12 },
    },
    think: {
      'opus': { ema: 0.95, observations: 7 },
      'gpt-5.5': { ema: 0.89, observations: 4 },
    },
  },
  topPerformers: [
    { cell: 'think', model: 'opus', ema: 0.95, observations: 7 },
    { cell: 'search', model: 'haiku', ema: 0.91, observations: 22 },
    { cell: 'execute', model: 'sonnet', ema: 0.88, observations: 19 },
  ],
  worstPerformers: [
    { cell: 'execute', model: 'gpt-5.4', ema: 0.82, observations: 12 },
    { cell: 'search', model: 'gpt-4.1-mini', ema: 0.84, observations: 14 },
  ],
}, null, 2));

writeFileSync(join(stateDir, 'rooms.json'), JSON.stringify({
  timestamp: now,
  rooms: [
    { id: 'rm-a1b2c3d4', project: 'clawnet-auth', status: 'active', createdAt: now, workerCount: 3 },
    { id: 'rm-e5f6g7h8', project: 'clawnet-api', status: 'active', createdAt: now, workerCount: 1 },
    { id: 'rm-i9j0k1l2', project: 'clawnet-docs', status: 'closed', createdAt: now, workerCount: 0 },
  ],
}, null, 2));

const decisions = [];
const models = ['haiku', 'sonnet', 'opus', 'gpt-4.1-mini', 'gpt-5.4', 'gpt-5.5'];
const tiers = ['search', 'execute', 'think'];
const providers = ['claude', 'openai'];
const prompts = [
  'Find all auth middleware files and check for session token storage',
  'Refactor the billing module to use the new payment provider',
  'Should we use Redis or Memcached for session caching?',
  'grep for deprecated API calls in the codebase',
  'Add rate limiting to the public API endpoints',
  'Review the migration script for the users table',
  'Update the CI pipeline to run parallel test suites',
  'Explore the logging infrastructure and find gaps',
];

for (let i = 0; i < 15; i++) {
  const t = new Date(Date.now() - (15 - i) * 120000);
  decisions.push({
    timestamp: t.toISOString(),
    promptSummary: prompts[i % prompts.length].slice(0, 100),
    provider: providers[i % 2],
    model: models[i % models.length],
    tier: tiers[i % tiers.length],
    reason: i % 3 === 0 ? 'budget balance favors this provider' : 'best EMA score for task type',
    explored: i % 5 === 0,
  });
}
writeFileSync(join(stateDir, 'decisions.json'), JSON.stringify(decisions, null, 2));

const outcomes = [];
for (let i = 0; i < 30; i++) {
  const t = new Date(Date.now() - (30 - i) * 90000);
  outcomes.push({
    timestamp: t.toISOString(),
    roomId: `rm-${String(i).padStart(8, '0')}`,
    success: Math.random() > 0.15,
    score: 0.5 + Math.random() * 0.5,
    durationMs: 2000 + Math.random() * 18000,
    provider: providers[i % 2],
    model: models[i % models.length],
  });
}
writeFileSync(join(stateDir, 'outcomes.json'), JSON.stringify(outcomes, null, 2));

writeFileSync(join(stateDir, 'costs.json'), JSON.stringify({
  timestamp: now,
  session: {
    totalTokens: 482000,
    inputTokens: 380000,
    outputTokens: 102000,
    estimatedCostUsd: 3.47,
  },
  byProvider: {
    claude: { tokens: 290000, estimatedCostUsd: 2.18 },
    openai: { tokens: 192000, estimatedCostUsd: 1.29 },
  },
}, null, 2));

console.log('Seed state written to', stateDir);
