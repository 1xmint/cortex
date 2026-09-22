import type {
  ChatSessionControls,
  RunProfile,
  SovereigntyLoopState,
  TaskCommandAction,
  TaskManagerState,
} from '../types';

const CONFIGURED_API_BASE = import.meta.env.VITE_CORTEX_API as string | undefined;
// Relative URLs by default. In dev the Vite proxy sends /api → localhost:3001;
// in production Caddy serves the SPA and the API from the same host, so the
// same relative path works and there is no cross-origin hop to configure.
const BASE_URL = CONFIGURED_API_BASE ?? '';

/**
 * The frontend half of the Soma fence.
 *
 * The backend fences `/api/soma/*` behind a cargo feature that is off by
 * default, so in every deployed build those six endpoints 404. The frontend was
 * still calling them: a failed session POST on every sign-in, and a spend tab
 * that could only ever render an error.
 *
 * This is off by default and matches the backend's default. It must NOT be
 * turned on to make the tab work — turning the backend feature on in production
 * is what the fence exists to prevent. It is here so the surface is hidden
 * rather than deleted, because Soma returns later; the day it does, this flag
 * and the cargo feature go on together.
 *
 * See docs/adr/ADR-0003-soma-feature-fence.md.
 */
export const SOMA_API_ENABLED = import.meta.env.VITE_CORTEX_SOMA_ENABLED === 'true';

/** Returns true when the browser believes it has no network connectivity. */
export function isOffline(): boolean {
  return typeof navigator !== 'undefined' && !navigator.onLine;
}

const RETRY_DELAYS_MS = [1000, 2000, 4000];
const STREAM_RECONNECT_DELAYS_MS = [1000, 2000, 4000];
const RETRYABLE_STATUSES = new Set([503]);

/**
 * Wraps a fetch call with exponential-backoff retry logic.
 * Retries on 503 responses, network errors, and AbortError-free timeouts.
 * Attempts: up to 3 total (initial + 2 retries), delays: 1s / 2s / 4s.
 */
async function fetchWithRetry(
  url: string,
  init?: RequestInit,
): Promise<Response> {
  let lastError: unknown;
  for (let attempt = 0; attempt <= RETRY_DELAYS_MS.length; attempt++) {
    try {
      const res = await fetch(url, init);
      if (!RETRYABLE_STATUSES.has(res.status) || attempt === RETRY_DELAYS_MS.length) {
        return res;
      }
      // retryable status — fall through to wait
      lastError = new Error(`HTTP ${res.status}`);
    } catch (err) {
      if (err instanceof Error && err.name === 'AbortError') throw err;
      lastError = err;
    }
    if (attempt < RETRY_DELAYS_MS.length) {
      await new Promise<void>((resolve) => {
        window.setTimeout(resolve, RETRY_DELAYS_MS[attempt]);
      });
    }
  }
  throw lastError;
}

export function apiUrl(path: string) {
  const base = BASE_URL.replace(/\/$/, '');
  if (!base) return path;
  if (base.endsWith('/api') && path.startsWith('/api/')) {
    return `${base}${path.slice(4)}`;
  }
  return `${base}${path}`;
}

let _tokenGetter: (() => Promise<string | null>) | null = null;
let _somaDelegation: SomaDelegation | null = null;

export function setAuthTokenGetter(getter: () => Promise<string | null>) {
  _tokenGetter = getter;
}

export function setSomaDelegation(delegation: SomaDelegation | null) {
  // Guarded here rather than only at the call site because of what a delegation
  // does downstream: `authedFetch` sends `Authorization: Soma <json>` *instead
  // of* the Clerk bearer token whenever one is set. With the backend fence on,
  // the `Soma ` scheme is refused — so a delegation arriving from anywhere would
  // not degrade the Soma features, it would unauthenticate the entire app.
  if (!SOMA_API_ENABLED) return;
  _somaDelegation = delegation;
}

/**
 * Exposed (not just `authedFetch`-internal) for callers that need the raw
 * bearer token themselves -- e.g. a `fetch(..., { keepalive: true })` sent
 * from a `pagehide` handler, where there is no time left to route through
 * the retry/JSON plumbing below.
 */
export async function getAuthToken(): Promise<string | null> {
  if (!_tokenGetter) return null;
  return _tokenGetter();
}

async function authedFetch(url: string, init?: RequestInit): Promise<Response> {
  const headers = new Headers(init?.headers);
  if (_somaDelegation) {
    headers.set('Authorization', `Soma ${JSON.stringify(_somaDelegation)}`);
  } else {
    const token = await getAuthToken();
    if (token) headers.set('Authorization', `Bearer ${token}`);
  }
  if (!headers.has('Content-Type') && init?.method && init.method !== 'GET') {
    headers.set('Content-Type', 'application/json');
  }
  return fetchWithRetry(url, { ...init, headers });
}

async function bearerFetch(url: string, init?: RequestInit): Promise<Response> {
  const headers = new Headers(init?.headers);
  const token = await getAuthToken();
  if (token) headers.set('Authorization', `Bearer ${token}`);
  if (!headers.has('Content-Type') && init?.method && init.method !== 'GET') {
    headers.set('Content-Type', 'application/json');
  }
  return fetchWithRetry(url, { ...init, headers });
}

export class CortexApiError extends Error {
  status: number;
  retryAfter: string | null;

  constructor(status: number, message: string, retryAfter: string | null = null) {
    super(message);
    this.name = 'CortexApiError';
    this.status = status;
    this.retryAfter = retryAfter;
  }
}

async function readErrorMessage(res: Response): Promise<string> {
  const fallback = res.status === 503
    ? 'Starting up...'
    : res.status === 429
      ? 'Rate limit reached. Try again shortly.'
      : `Cortex API ${res.status}`;
  try {
    const body = await res.json();
    return typeof body?.error === 'string' ? body.error : fallback;
  } catch {
    return fallback;
  }
}

/** BroadcastChannel name used for cross-tab auth sync. */
export const AUTH_CHANNEL_NAME = 'cortex-auth';

function dispatchUnauthorized() {
  try {
    window.dispatchEvent(new CustomEvent('cortex:unauthorized'));
  } catch {
    // ignore in non-browser environments
  }
  // Notify other tabs so they also sign out
  try {
    if (typeof BroadcastChannel !== 'undefined') {
      const ch = new BroadcastChannel(AUTH_CHANNEL_NAME);
      ch.postMessage({ type: 'logout' });
      ch.close();
    }
  } catch {
    // ignore in non-browser environments
  }
}

export async function requestJson<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await authedFetch(apiUrl(path), init);
  if (!res.ok) {
    if (res.status === 401) dispatchUnauthorized();
    throw new CortexApiError(res.status, await readErrorMessage(res), res.headers.get('Retry-After'));
  }
  return res.json() as Promise<T>;
}

async function requestBillingJson<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await bearerFetch(apiUrl(path), init);
  if (!res.ok) {
    if (res.status === 401) dispatchUnauthorized();
    throw new CortexApiError(res.status, await readErrorMessage(res), res.headers.get('Retry-After'));
  }
  return res.json() as Promise<T>;
}

export interface WorkerEvent {
  type: 'started' | 'output' | 'completed' | 'failed' | 'tool_activity' | 'confirm_required';
  task_id?: string;
  step_id?: string;
  provider?: string;
  model?: string;
  line?: string;
  exit_code?: number;
  error?: string;
  // Only present for `tool_activity`: which read-only tool the agent used
  // and whether it succeeded. Never carries the tool's arguments or result.
  tool_name?: string;
  ok?: boolean;
  // Only present for `confirm_required`: a risky action awaiting tap-to-confirm.
  // `expires_at` is RFC 3339. The nonce round-trips through the confirm/cancel
  // calls below and is never logged or displayed.
  action_id?: string;
  nonce?: string;
  summary?: string;
  expires_at?: string;
}

export interface ConfirmAgentActionResponse {
  status: string;
  result?: unknown;
}

/**
 * Tap-to-confirm for a risky agent action raised via a `confirm_required`
 * stream event. A 404 or 409 means the action is gone -- expired or already
 * used -- and callers should treat that as "no longer available" rather than
 * retrying.
 */
export async function confirmAgentAction(
  actionId: string,
  nonce: string,
): Promise<ConfirmAgentActionResponse> {
  return requestJson<ConfirmAgentActionResponse>(
    `/api/agent/actions/${encodeURIComponent(actionId)}/confirm`,
    {
      method: 'POST',
      body: JSON.stringify({ nonce }),
    },
  );
}

export async function cancelAgentAction(
  actionId: string,
  nonce: string,
): Promise<ConfirmAgentActionResponse> {
  return requestJson<ConfirmAgentActionResponse>(
    `/api/agent/actions/${encodeURIComponent(actionId)}/cancel`,
    {
      method: 'POST',
      body: JSON.stringify({ nonce }),
    },
  );
}

export function streamChat(
  message: string,
  filePaths: string[],
  routingContext: {
    controls: ChatSessionControls;
    run_profile: RunProfile;
    sovereignty: SovereigntyLoopState;
    // Forwarded verbatim into the request body; nothing here reads its
    // fields, and the only producer currently passes `undefined`. Structural
    // rather than `any` so it cannot silently absorb a mistake.
    routing_preferences?: Record<string, unknown>;
  } | null,
  onEvent: (event: WorkerEvent) => void,
  onDone: () => void,
  onError: (err: Error) => void,
): AbortController {
  const controller = new AbortController();

  (async () => {
    for (let attempt = 0; attempt <= STREAM_RECONNECT_DELAYS_MS.length; attempt++) {
      try {
        const res = await authedFetch(apiUrl('/api/chat'), {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            message,
            file_paths: filePaths,
            routing_context: routingContext?.routing_preferences ? {
              ...routingContext,
              routing_preferences: routingContext.routing_preferences,
            } : routingContext,
          }),
          signal: controller.signal,
        });

        if (!res.ok) {
          const body = await res.text();
          const status = res.status;
          // Non-retryable client errors
          if (status >= 400 && status < 500) {
            throw new CortexApiError(status, body);
          }
          throw new Error(`Cortex API ${status}: ${body}`);
        }

        const reader = res.body?.getReader();
        if (!reader) throw new Error('No response body');

        const decoder = new TextDecoder();
        let buffer = '';
        let sawCompletion = false;

        while (true) {
          const { done, value } = await reader.read();
          if (done) break;

          buffer += decoder.decode(value, { stream: true });
          const lines = buffer.split('\n');
          buffer = lines.pop() ?? '';

          for (const line of lines) {
            if (!line.startsWith('data: ')) continue;
            const json = line.slice(6).trim();
            if (!json) continue;
            try {
              const event = JSON.parse(json) as WorkerEvent;
              onEvent(event);
              if (event.type === 'completed' || event.type === 'failed') {
                sawCompletion = true;
              }
            } catch {
              // skip malformed lines
            }
          }
        }

        // Stream ended -- if we saw a terminal event or stream closed normally, we're done
        if (sawCompletion) {
          onDone();
          return;
        }
        // Server closed without terminal event -- treat as normal completion on first attempt
        onDone();
        return;
      } catch (err) {
        if (controller.signal.aborted) return;
        if (err instanceof Error && err.name === 'AbortError') return;
        // Non-retryable HTTP errors
        if (err instanceof CortexApiError && err.status >= 400 && err.status < 500) {
          onError(err);
          return;
        }
        if (attempt === STREAM_RECONNECT_DELAYS_MS.length) {
          if (err instanceof Error) onError(err);
          return;
        }
      }

      // Wait before reconnect
      if (attempt < STREAM_RECONNECT_DELAYS_MS.length) {
        await new Promise<void>((resolve) => {
          window.setTimeout(resolve, STREAM_RECONNECT_DELAYS_MS[attempt]);
        });
        if (controller.signal.aborted) return;
      }
    }
  })();

  return controller;
}

export async function getProviders() {
  return requestJson('/api/providers');
}

export async function getHealth() {
  return requestJson('/api/health');
}

export interface FrontendAssets {
  js: string | null;
  css: string | null;
}

export interface DeploymentStatus {
  status: 'match' | 'drift' | 'unknown';
  service: string;
  observed_at: string;
  commits: {
    status: 'match' | 'mismatch' | 'unknown';
    backend_commit: string | null;
    backend_commit_short: string | null;
    frontend_commit: string | null;
    frontend_commit_short: string | null;
    backend_branch: string | null;
    frontend_branch: string | null;
    branch_match: boolean | null;
  };
  backend: {
    service: string;
    version: string;
    commit: string | null;
    commit_short: string | null;
    branch: string | null;
    deployed_at: string | null;
  };
  frontend: {
    public_url: string;
    local_root: string;
    expected_source: string;
    expected_assets: FrontendAssets;
    live_assets: FrontendAssets | null;
    status: 'match' | 'drift' | 'unknown';
    drift: boolean;
    checked_at: string;
    error: string | null;
    cloudflare_pages: {
      configured: boolean;
      project: string;
      status: 'match' | 'drift' | 'unknown';
      deployment_id: string | null;
      environment: string | null;
      branch: string | null;
      commit: string | null;
      url: string | null;
      error: string | null;
    };
  };
  github_actions: {
    configured: boolean;
    source: string;
    owner: string;
    repo: string;
    workflow: string;
    workflow_id: number | null;
    workflow_name: string | null;
    workflow_path: string | null;
    branch: string;
    status: 'match' | 'drift' | 'unknown';
    run_id: number | null;
    run_number: number | null;
    run_attempt: number | null;
    run_status: string | null;
    conclusion: string | null;
    event: string | null;
    head_branch: string | null;
    head_sha: string | null;
    head_sha_short: string | null;
    html_url: string | null;
    created_at: string | null;
    updated_at: string | null;
    run_started_at: string | null;
    error: string | null;
  };
}

export async function getDeploymentStatus(): Promise<DeploymentStatus> {
  return requestJson<DeploymentStatus>('/api/deployment/status');
}

export interface SomaIdentity {
  did: string;
  genome?: unknown;
  protocol: string;
  heartbeats: number;
  head_hash?: string;
  capabilities?: unknown;
}

export async function getSomaIdentity(): Promise<SomaIdentity> {
  return requestJson<SomaIdentity>('/api/soma/identity');
}

export interface SomaDelegation {
  id: string;
  issuer_did: string;
  subject_did: string;
  capabilities: string[];
  caveats?: Array<{ type?: string; timestamp?: number; [key: string]: unknown }>;
  issued_at: number;
  signature: string;
}

export interface SomaSession {
  delegation: SomaDelegation;
  user_identity: { did: string; public_key: string; created_at: number };
  cortex_did: string;
  root_did: string | null;
}

export interface SomaUserIdentity {
  did: string;
  public_key: string;
  has_delegation: boolean;
}

export async function createSomaSession(): Promise<SomaSession> {
  return requestJson<SomaSession>('/api/soma/session', { method: 'POST' });
}

export async function getSomaMe(): Promise<SomaUserIdentity> {
  return requestJson<SomaUserIdentity>('/api/soma/me');
}

export interface SomaSpendDelegation {
  delegation_id: string;
  subject_did: string;
  cumulative_spend: number;
  receipt_count: number;
  last_activity_ms: number | null;
  capabilities: string[];
}

export interface SomaSpendSummary {
  delegations: SomaSpendDelegation[];
  total_spend: number;
}

export interface SomaSpendReceipt {
  amount: number;
  cumulative: number;
  capability: string;
  timestamp: number;
}

export interface SomaDelegationSpend {
  delegation_id: string;
  receipts: SomaSpendReceipt[];
  cumulative: number;
}

export async function getSomaSpend(): Promise<SomaSpendSummary> {
  return requestJson<SomaSpendSummary>('/api/soma/spend');
}

export async function getSomaDelegationSpend(delegationId: string): Promise<SomaDelegationSpend> {
  return requestJson<SomaDelegationSpend>(`/api/soma/spend/${encodeURIComponent(delegationId)}`);
}

export async function revokeSomaDelegation(delegationId: string, subjectDid: string): Promise<{ revoked: boolean }> {
  return requestJson<{ revoked: boolean }>('/api/soma/revoke', {
    method: 'POST',
    body: JSON.stringify({ delegation_id: delegationId, subject_did: subjectDid }),
  });
}

export type BillingAccessState =
  | 'signed_out'
  | 'needs_phone'
  | 'needs_checkout'
  | 'trial_active'
  | 'active'
  | 'payment_failed'
  | 'cancelled';

export interface BillingStatus {
  access_state: BillingAccessState;
  plan: {
    plan_type: 'monthly' | 'annual';
    status: 'trialing' | 'active' | 'past_due' | 'cancelled' | 'paused';
    billing_period_end: string;
    next_charge_amount_cents: number | null;
    next_charge_date: string | null;
    started_at: string;
  } | null;
  trial: {
    trial_end: string;
    days_remaining: number;
    auto_charge_amount_cents: number;
    auto_charge_plan: 'monthly' | 'annual';
  } | null;
  delegation: {
    status: 'active' | 'pending' | 'expired' | 'revoked' | 'not_issued';
    budget_enforced: boolean;
    delegation_id: string | null;
    expires_at: string | null;
  };
  payment_method: {
    last4: string;
    brand: string;
    exp_month: number;
    exp_year: number;
  } | null;
  referral: {
    code: string;
    uses_remaining: number;
    total_uses: number;
    weeks_earned: number;
  } | null;
}

export interface CheckoutResponse {
  checkout_url: string;
  session_id: string;
}

export interface DiscountOption {
  label: string;
  discount_type: string;
  discount_value: number;
}

export interface ReferralValidateResponse {
  valid: boolean;
  discount_type: string | null;
  discount_value: number | null;
  description: string | null;
  options: DiscountOption[];
  uses_remaining: number | null;
  error: string | null;
}

export interface BillingHistoryEntry {
  date: string;
  amount_cents: number;
  description: string;
  status: string;
}

export async function getBillingStatus(): Promise<BillingStatus> {
  return requestBillingJson<BillingStatus>('/api/billing/status');
}

export async function createBillingCheckout(body: {
  plan: 'monthly' | 'annual';
  email?: string;
  referral_code?: string;
  referral_choice?: string;
}): Promise<CheckoutResponse> {
  return requestBillingJson<CheckoutResponse>('/api/billing/checkout', {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

export async function createBillingPortal(): Promise<{ portal_url: string }> {
  return requestBillingJson<{ portal_url: string }>('/api/billing/portal', { method: 'POST' });
}

export async function validateReferralCode(code: string): Promise<ReferralValidateResponse> {
  return requestBillingJson<ReferralValidateResponse>('/api/billing/referral/validate', {
    method: 'POST',
    body: JSON.stringify({ code }),
  });
}

export async function getBillingHistory(): Promise<BillingHistoryEntry[]> {
  return requestBillingJson<BillingHistoryEntry[]>('/api/billing/history');
}

export interface GitHubStatus {
  linked: boolean;
  username: string | null;
  repos: GitHubRepo[];
}

export interface GitHubRepo {
  id: number;
  full_name: string;
  description: string | null;
  html_url: string;
  private: boolean;
  language: string | null;
  default_branch?: string | null;
  owner?: {
    login: string;
    type: string;
  } | null;
  permissions?: {
    admin?: boolean;
    maintain?: boolean;
    push?: boolean;
    triage?: boolean;
    pull?: boolean;
  } | null;
}

export async function getGitHubStatus(): Promise<GitHubStatus> {
  return requestJson<GitHubStatus>('/api/user/github/status');
}

export async function selectRepos(repoIds: number[]): Promise<void> {
  const res = await authedFetch(apiUrl('/api/user/repos/select'), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ repo_ids: repoIds }),
  });
  if (!res.ok) {
    if (res.status === 401) dispatchUnauthorized();
    throw new CortexApiError(res.status, await readErrorMessage(res), res.headers.get('Retry-After'));
  }
}

// --- GitHub repo import + sync ---

export interface GitHubReposResponse {
  linked: boolean;
  repos: GitHubRepo[];
}

export interface GitHubImportResponse {
  import_id: string;
  status: string;
  clone_path: string;
}

export interface GitHubImportStatus {
  import_id: string;
  repo_full_name: string;
  status: 'pending' | 'importing' | 'ready' | 'failed';
  progress: number;
  stage: string;
  clone_path: string;
  error: string | null;
  last_synced_at: number | null;
  head_commit: string | null;
}

export interface GitHubSyncResponse {
  status: string;
  committed: boolean;
  pushed: boolean;
  conflict: boolean;
  head_commit: string | null;
  detail: string;
}

/** List the user's GitHub repositories available to import. */
export async function listGitHubRepos(): Promise<GitHubReposResponse> {
  return requestJson<GitHubReposResponse>('/api/github/repos');
}

/** List the user's imported repos. */
export async function listGitHubImports(): Promise<GitHubImportStatus[]> {
  return requestJson<GitHubImportStatus[]>('/api/github/imports');
}

/** Start importing (cloning) a repo into the user's container. */
export async function importGitHubRepo(repoFullName: string): Promise<GitHubImportResponse> {
  return requestJson<GitHubImportResponse>('/api/github/import', {
    method: 'POST',
    body: JSON.stringify({ repo_full_name: repoFullName }),
  });
}

/** Poll the status of an in-progress or completed import. */
export async function getGitHubImportStatus(importId: string): Promise<GitHubImportStatus> {
  return requestJson<GitHubImportStatus>(`/api/github/status/${encodeURIComponent(importId)}`);
}

/** Bidirectionally sync an imported repo with GitHub. */
export async function syncGitHubRepo(
  importId: string,
  opts?: { message?: string; push?: boolean },
): Promise<GitHubSyncResponse> {
  return requestJson<GitHubSyncResponse>(`/api/github/sync/${encodeURIComponent(importId)}`, {
    method: 'POST',
    body: JSON.stringify(opts ?? {}),
  });
}

export async function getUserProfile() {
  return requestJson('/api/user/profile');
}

export interface UserRoutingSettings {
  profile: string;
  auto_mode?: string;
  pressure?: unknown;
}

export async function getUserRouting(): Promise<UserRoutingSettings> {
  return requestJson<UserRoutingSettings>('/api/user/routing');
}

export async function updateUserRouting(profile: string): Promise<UserRoutingSettings> {
  return requestJson<UserRoutingSettings>('/api/user/routing', {
    method: 'POST',
    body: JSON.stringify({ profile }),
  });
}

export async function getGroupTaskManagerState(groupId: string): Promise<TaskManagerState> {
  return requestJson<TaskManagerState>(`/api/groups/${encodeURIComponent(groupId)}/tasks`);
}

export async function updateGroupTaskManagerState(
  groupId: string,
  state: TaskManagerState,
): Promise<TaskManagerState> {
  return requestJson<TaskManagerState>(`/api/groups/${encodeURIComponent(groupId)}/tasks`, {
    method: 'PUT',
    body: JSON.stringify(state),
  });
}

export async function createGroupTaskManagerTask(
  groupId: string,
  task: TaskManagerState['tasks'][number],
): Promise<TaskManagerState> {
  return requestJson<TaskManagerState>(`/api/groups/${encodeURIComponent(groupId)}/tasks`, {
    method: 'POST',
    body: JSON.stringify(task),
  });
}

export async function applyGroupTaskManagerActions(
  groupId: string,
  actions: TaskCommandAction[],
  actor: string,
): Promise<TaskManagerState> {
  return requestJson<TaskManagerState>(`/api/groups/${encodeURIComponent(groupId)}/tasks/actions`, {
    method: 'POST',
    body: JSON.stringify({ actions, actor }),
  });
}

export async function patchGroupTaskManagerTask(
  groupId: string,
  taskId: string,
  patch: Partial<TaskManagerState['tasks'][number]>,
): Promise<TaskManagerState> {
  return requestJson<TaskManagerState>(
    `/api/groups/${encodeURIComponent(groupId)}/tasks/${encodeURIComponent(taskId)}`,
    {
      method: 'PATCH',
      body: JSON.stringify(patch),
    },
  );
}

export interface TaskEvidenceCheck {
  task_id: string;
  has_evidence_backed_completion: boolean;
  completion_gate: {
    gated_done: boolean;
    raw_done: boolean;
    reason: string;
    run_id?: string | null;
    run_status?: string | null;
    steps: {
      total: number;
      verified_pass: number;
      failed: number;
      unverified: number;
    };
  };
}

/**
 * `/api/groups/{id}/tasks/{id}/evidence` is not served. The backend offers
 * `/chats` and `/projection` on that task and nothing else.
 *
 * This one deserves care, because "was this task really finished?" is the
 * question the whole product exists to answer. Reporting *no* evidence is the
 * safe direction to be wrong in: the gate stays shut and the task is not shown
 * as done. Reporting evidence that was never checked would be the bug that
 * matters.
 */
export async function checkTaskEvidence(
  groupId: string,
  taskId: string,
): Promise<TaskEvidenceCheck> {
  void groupId;
  return {
    task_id: taskId,
    has_evidence_backed_completion: false,
    completion_gate: {
      gated_done: false,
      raw_done: false,
      reason: 'Evidence checking is not available: the backend does not serve this route.',
      steps: { total: 0, verified_pass: 0, failed: 0, unverified: 0 },
    },
  };
}

export interface IntegrationConnection {
  id: string;
  provider: 'slack' | 'replit' | string;
  external_id: string | null;
  display_name: string;
  status: 'connected' | 'needs_config' | 'degraded' | string;
  scopes: string[];
  metadata: Record<string, unknown>;
  last_sync_at: string | null;
  last_error: string | null;
  created_at: string;
  updated_at: string;
}

export interface IntegrationMapping {
  id: string;
  provider: 'slack' | 'replit' | string;
  group_id: string;
  external_id: string;
  external_name: string;
  mapping_type: string;
  metadata: Record<string, unknown>;
  created_at: string;
  updated_at: string;
}

export interface IntegrationStatus {
  connections: IntegrationConnection[];
  mappings: IntegrationMapping[];
  slack_configured: boolean;
  replit_configured: boolean;
}

export interface SlackChannel {
  id: string;
  name: string;
  is_private: boolean;
  member_count: number;
}

export interface ReplitWorkspace {
  id: string;
  title: string;
  language: string;
  url: string | null;
}

export async function getIntegrationStatus(): Promise<IntegrationStatus> {
  return requestJson<IntegrationStatus>('/api/integrations/status');
}

export async function startSlackOAuth(): Promise<{ auth_url: string; state: string }> {
  return requestJson<{ auth_url: string; state: string }>('/api/integrations/slack/oauth/start', {
    method: 'POST',
    body: JSON.stringify({ redirect_after: window.location.pathname }),
  });
}

export async function getSlackChannels(): Promise<SlackChannel[]> {
  return requestJson<SlackChannel[]>('/api/integrations/slack/channels');
}

export async function importSlackChannels(channels: SlackChannel[]): Promise<{ groups: unknown[]; mappings: IntegrationMapping[] }> {
  return requestJson<{ groups: unknown[]; mappings: IntegrationMapping[] }>('/api/integrations/slack/import-channels', {
    method: 'POST',
    body: JSON.stringify({ channels }),
  });
}

export async function getReplitWorkspaces(): Promise<ReplitWorkspace[]> {
  return requestJson<ReplitWorkspace[]>('/api/integrations/replit/workspaces');
}

export async function importReplitWorkspace(workspace: ReplitWorkspace): Promise<{ groups: unknown[]; mappings: IntegrationMapping[] }> {
  return requestJson<{ groups: unknown[]; mappings: IntegrationMapping[] }>('/api/integrations/replit/import', {
    method: 'POST',
    body: JSON.stringify({
      workspace_id: workspace.id,
      title: workspace.title,
      language: workspace.language,
    }),
  });
}

export interface UsageSummary {
  last_24h?: unknown;
  last_30d?: unknown;
  gate?: unknown;
}

export interface DailyUsage {
  date: string;
  tokens_in?: number;
  tokens_out?: number;
  cost?: number;
  steps?: number;
}

export async function getUsage(): Promise<UsageSummary> {
  return requestJson<UsageSummary>('/api/usage');
}

export async function getDailyUsage(days = 30): Promise<DailyUsage[]> {
  return requestJson<DailyUsage[]>(`/api/usage/daily?days=${days}`);
}

export interface AdminStats {
  runs?: unknown;
  steps?: unknown;
  workers?: unknown;
  users?: unknown;
  decisions?: unknown;
}

export interface AdminWorkers {
  connected?: unknown[];
  all?: unknown[];
  count?: number;
}

export async function getAdminStats(): Promise<AdminStats> {
  return requestJson<AdminStats>('/api/admin/stats');
}

export async function getAdminWorkers(): Promise<AdminWorkers> {
  return requestJson<AdminWorkers>('/api/admin/workers');
}

// Admin — Promo Codes

export interface PromoCode {
  id: string;
  code: string;
  discount_type: 'trial_extension' | 'percent_off' | 'free_trial';
  discount_value: number;
  max_uses: number;
  current_uses: number;
  expires_at: string | null;
  active: boolean;
  created_by: string;
  created_at: string;
  description: string | null;
}

export interface CreatePromoCodeRequest {
  code: string;
  discount_type: string;
  discount_value: number;
  max_uses?: number;
  expires_at?: string;
  description?: string;
  discount_options?: DiscountOption[];
}

export interface UpdatePromoCodeRequest {
  active?: boolean;
  max_uses?: number;
  expires_at?: string | null;
  description?: string | null;
}

export interface CodeRedemption {
  id: string;
  promo_code_id: string;
  code: string;
  user_id: string;
  redeemed_at: string;
}

export async function getAdminPromoCodes(): Promise<{ codes: PromoCode[]; total: number }> {
  return requestJson<{ codes: PromoCode[]; total: number }>('/api/admin/codes');
}

export async function createAdminPromoCode(req: CreatePromoCodeRequest): Promise<PromoCode> {
  return requestJson<PromoCode>('/api/admin/codes', {
    method: 'POST',
    body: JSON.stringify(req),
  });
}

export async function updateAdminPromoCode(id: string, req: UpdatePromoCodeRequest): Promise<{ updated: boolean }> {
  return requestJson<{ updated: boolean }>(`/api/admin/codes/${encodeURIComponent(id)}`, {
    method: 'PATCH',
    body: JSON.stringify(req),
  });
}

export async function deleteAdminPromoCode(id: string): Promise<{ deleted: boolean }> {
  return requestJson<{ deleted: boolean }>(`/api/admin/codes/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });
}

export async function getAdminRedemptions(code?: string): Promise<{ redemptions: CodeRedemption[]; total: number }> {
  const query = code ? `?code=${encodeURIComponent(code)}` : '';
  return requestJson<{ redemptions: CodeRedemption[]; total: number }>(`/api/admin/redemptions${query}`);
}

// Decision ledger

export interface LedgerEntry {
  id: string;
  timestamp: string;
  event: {
    type: string;
    task_id?: string;
    provider?: string;
    model?: string;
    tier?: string;
    risk?: string;
    rationale?: string[];
    score?: number;
    alternatives_considered?: Array<{
      provider?: string;
      model?: string;
      tier?: string;
      score?: number;
    }>;
    status?: string;
    duration_ms?: number;
    files_changed?: number;
    tests_passed?: boolean | null;
    authenticated?: boolean;
    pressure?: number;
  };
}

export async function getLedger(): Promise<LedgerEntry[]> {
  return requestJson<LedgerEntry[]>('/api/ledger');
}

// Runs

/**
 * Where a step is in its life.
 *
 * `delivered` means a worker handed back a commit and nothing has checked it.
 * `verified` means Cortex executed the frozen checks and they passed. There is
 * no `succeeded` — it used to mean both at once, which is how a worker's
 * opinion became the product's truth. See docs/adr/ADR-0001-step-truth-model.md.
 */
export type RunStepStatus =
  | 'pending'
  | 'ready'
  | 'leased'
  | 'running'
  | 'delivered'
  | 'verifying'
  | 'verified'
  | 'failed'
  | 'inconclusive'
  | 'execution_failed'
  | 'manual_override'
  | 'recovered'
  | 'cancelled'
  | 'orphaned'
  | 'skipped'
  | string;

/** Statuses a step will not leave on its own. */
export const TERMINAL_STEP_STATUSES: readonly RunStepStatus[] = [
  'verified',
  'manual_override',
  'failed',
  'execution_failed',
  'recovered',
  'cancelled',
  'skipped',
];

/** Statuses that mean the work was accepted: we verified it, or a human did. */
export const ACCEPTED_STEP_STATUSES: readonly RunStepStatus[] = ['verified', 'manual_override'];

/** The work exists but nothing has graded it. Never render these as done. */
export const UNVERIFIED_STEP_STATUSES: readonly RunStepStatus[] = ['delivered', 'verifying'];

export interface RunStep {
  id: string;
  status: RunStepStatus;
  goal?: string;
  title?: string;
  kind?: string;
  work_kind?: string;
  tier?: string;
  risk?: string;
  objective?: string;
  attempt_count?: number;
  max_attempts?: number;
  lease_gen?: number;
  lease_deadline?: number | null;
  assigned_worker?: string | null;
  lease_stale?: boolean;
  health?: string;
  blocked_by?: Array<{ id?: string; status?: string; edge_type?: string }>;
  latest_attempt?: {
    attempt_number?: number;
    worker_id?: string | null;
    lease_gen?: number;
    status?: string;
    provider?: string | null;
    model?: string | null;
    started_at?: number;
    finished_at?: number | null;
    failure_kind?: string | null;
    error_summary?: string | null;
  };
  error?: string | null;
  last_error?: string | null;
  output_summary?: string | null;
  files_changed?: string[] | null;
  verification_status?: string | null;
  verifier_verdict?: string | null;
  verifier_report_id?: string | null;
  recipe_seed?: unknown;
  work_recipe?: {
    version?: number;
    kind?: string;
    target_paths?: string[];
    acceptance?: Array<{ id?: string; text?: string; verification?: unknown }>;
    constraints?: string[];
    required_checks?: Array<{ name?: string; command?: string; required?: boolean }>;
  } | null;
  acceptance_criteria?: string[];
  required_checks?: Array<{ name?: string; command?: string; required?: boolean }>;
  predecessors?: string[];
  parent_id?: string | null;
}

export interface RunGraphNode {
  id: string;
  label: string;
  status?: string | null;
  kind?: string | null;
  work_kind?: string | null;
  tier?: string | null;
  risk?: string | null;
  verification_status?: string | null;
}

export interface RunGraphEdge {
  from: string;
  to: string;
  edge_type: 'success_required' | 'completion_required' | string;
}

export interface RunGraph {
  nodes: RunGraphNode[];
  edges: RunGraphEdge[];
}

export interface RunSummary {
  id: string;
  goal: string;
  status?: string;
  profile?: string;
  created_at?: string;
  task_id?: string | null;
  group_id?: string | null;
  conversation_id?: string | null;
  steps: RunStep[];
  graph?: RunGraph;
}

export interface RunOperationEvent {
  id: string;
  created_at: number;
  actor_user_id?: string | null;
  scope_id?: string | null;
  project_id?: string | null;
  task_id?: string | null;
  run_id?: string | null;
  step_id?: string | null;
  attempt_id?: string | null;
  event_type: string;
  entity_type: string;
  entity_id: string;
  payload?: Record<string, unknown>;
}

export interface RunEventsResponse {
  run_id: string;
  events: RunOperationEvent[];
}

export interface TaskProjectionRun {
  id: string;
  goal: string;
  status: string;
  profile: string;
  created_at: number;
  updated_at: number;
  started_at?: number | null;
  finished_at?: number | null;
  heal_attempts?: number;
  task_id?: string | null;
  group_id?: string | null;
  conversation_id?: string | null;
}

export interface TaskProjectionChat {
  id: string;
  title?: string | null;
  created_at: string;
  updated_at: string;
  attached_at: string;
}

export interface CortexApprovalRequest {
  id: string;
  group_id: string;
  task_id?: string | null;
  step_id?: string | null;
  conversation_id?: string | null;
  run_id?: string | null;
  ask_type: string;
  status: 'pending' | 'approved' | 'rejected' | 'cancelled' | string;
  title: string;
  body: string;
  priority: 'normal' | 'high' | 'urgent' | string;
  requested_by: string;
  decision?: unknown;
  created_at: number;
  updated_at: number;
  resolved_at?: number | null;
}

export interface TaskProjection {
  task: {
    id: string;
    group_id: string;
    title: string;
    status: string;
    priority: string;
    conversation_id?: string | null;
    latest_run_id?: string | null;
    source?: Record<string, unknown>;
    created_at: string;
    updated_at: string;
    version: number;
    completion?: {
      gated_done: boolean;
      raw_done: boolean;
      reason: string;
      run_id?: string | null;
      run_status?: string | null;
      steps: {
        total: number;
        verified_pass: number;
        failed: number;
        unverified: number;
      };
    };
  };
  runs: TaskProjectionRun[];
  chats: TaskProjectionChat[];
  approvals: CortexApprovalRequest[];
  events: RunOperationEvent[];
}

export interface GroupOperationsAttentionItem {
  kind: string;
  approval_id?: string | null;
  task_id?: string | null;
  run_id?: string | null;
  step_id?: string | null;
  title?: string | null;
  status?: string | null;
  priority?: string | null;
  reason?: string | null;
  created_at?: number | null;
  updated_at?: string | null;
}

export interface GroupOperationsResourceLease {
  id: string;
  group_id?: string | null;
  task_id?: string | null;
  run_id: string;
  step_id?: string | null;
  holder_type: string;
  resource_type: string;
  repo_key: string;
  resource_key: string;
  mode: string;
  lease_gen: number;
  acquired_at: number;
  expires_at: number;
  seconds_until_expiry: number;
  reason?: string | null;
  metadata?: Record<string, unknown>;
}

export interface GroupOperationsSummary {
  group_id: string;
  scope: 'group';
  generated_at: number;
  tasks: {
    total: number;
    open: number;
    active: number;
    done_raw: number;
    urgent: number;
    unassigned: number;
    without_run: number;
    by_status: Record<string, number>;
    completion: {
      gated_done_available: boolean;
      raw_done: number;
      gated_done?: number;
      done_without_evidence?: number;
    };
  };
  runs: {
    total: number;
    active: number;
    failed: number;
    succeeded: number;
    latest_run_id?: string | null;
  };
  steps: {
    total: number;
    active: number;
    failed: number;
    orphaned: number;
    verified_pass: number;
    verified_fail: number;
  };
  approvals?: {
    total: number;
    pending: number;
    approved: number;
    rejected: number;
    cancelled?: number;
  };
  resource_leases?: {
    active: number;
    by_type: Record<string, number>;
    by_mode: Record<string, number>;
    leases: GroupOperationsResourceLease[];
  };
  attention: GroupOperationsAttentionItem[];
  recent_events: RunOperationEvent[];
}

export type OperationsGraphNodeType =
  | 'task'
  | 'run'
  | 'step'
  | 'chat'
  | 'attempt'
  | 'evidence'
  | 'approval'
  | 'resource_lease'
  | string;

export interface OperationsGraphNode {
  id: string;
  type: OperationsGraphNodeType;
  entity_id: string;
  group_id?: string | null;
  task_id?: string | null;
  run_id?: string | null;
  step_id?: string | null;
  conversation_id?: string | null;
  label?: string | null;
  status?: string | null;
  priority?: string | null;
  kind?: string | null;
  work_kind?: string | null;
  risk?: string | null;
  tier?: string | null;
  verification_status?: string | null;
  verifier_report_id?: string | null;
  verdict?: string | null;
  lease_stale?: boolean;
  [key: string]: unknown;
}

export interface OperationsGraphEdge {
  id: string;
  from: string;
  to: string;
  type: string;
  edge_type?: string;
  [key: string]: unknown;
}

export interface GroupOperationsGraph {
  group_id: string;
  scope: 'group';
  generated_at: number;
  nodes: OperationsGraphNode[];
  edges: OperationsGraphEdge[];
  recent_events: RunOperationEvent[];
}

export interface PersonalOperationsGroupSummary {
  group_id: string;
  name: string;
  kind: string;
  source: string;
  accent: string;
  active: number;
  tasks: GroupOperationsSummary['tasks'];
  runs: GroupOperationsSummary['runs'];
  steps: GroupOperationsSummary['steps'];
  approvals: NonNullable<GroupOperationsSummary['approvals']>;
  resource_leases: NonNullable<GroupOperationsSummary['resource_leases']>;
  attention: GroupOperationsAttentionItem[];
}

export interface PersonalOperationsSummary {
  scope: 'personal';
  generated_at: number;
  groups_total: number;
  active_groups: number;
  tasks: GroupOperationsSummary['tasks'];
  runs: Omit<GroupOperationsSummary['runs'], 'latest_run_id'>;
  steps: GroupOperationsSummary['steps'];
  approvals: NonNullable<GroupOperationsSummary['approvals']>;
  resource_leases: NonNullable<GroupOperationsSummary['resource_leases']>;
  attention: GroupOperationsAttentionItem[];
  recent_events: RunOperationEvent[];
  groups: PersonalOperationsGroupSummary[];
}

export type CortexAuthorityScopeKind = 'personal' | 'team' | 'org' | 'company' | string;

export interface CortexAuthorityResource {
  id: string;
  scope_id: string;
  resource_type: string;
  resource_key: string;
  access: 'read' | 'write' | 'admin' | string;
  policy: Record<string, unknown>;
  created_at: number;
  updated_at: number;
}

export interface CortexAuthorityScope {
  id: string;
  kind: CortexAuthorityScopeKind;
  name: string;
  description: string;
  source: string;
  external_id?: string | null;
  status: string;
  role: 'owner' | 'admin' | 'member' | 'viewer' | string;
  policy: Record<string, unknown>;
  created_at: number;
  updated_at: number;
  resources: CortexAuthorityResource[];
}

export interface CortexAuthorityScopesResponse {
  scopes: CortexAuthorityScope[];
}

export interface RunListItem {
  id: string;
  goal: string;
  status: string;
  profile: string;
  created_at: string;
  task_id?: string | null;
  group_id?: string | null;
  conversation_id?: string | null;
}

export interface CreateRunResponse {
  run_id: string;
  steps: number;
  authority_scope_id?: string | null;
}

export interface CreateRunOptions {
  repoKey?: string | null;
  taskId?: string | null;
  groupId?: string | null;
  conversationId?: string | null;
  authorityScopeId?: string | null;
  authorityHandoffId?: string | null;
  authorityReason?: string | null;
}

export function repoKeyFromLabel(repo?: string | null): string | undefined {
  const raw = repo?.trim();
  if (!raw) return undefined;
  if (/^[a-z][a-z0-9+.-]*:/i.test(raw)) return raw;

  const githubMatch = raw.match(/github\.com[:/]([^/\s]+)\/([^/\s#?]+?)(?:\.git)?(?:[/?#\s]|$)/i);
  if (githubMatch) {
    return `github:${githubMatch[1]}/${githubMatch[2]}`;
  }

  const ownerRepoMatch = raw.match(/^([A-Za-z0-9_.-]+)\/([A-Za-z0-9_.-]+)$/);
  if (ownerRepoMatch) {
    return `github:${ownerRepoMatch[1]}/${ownerRepoMatch[2]}`;
  }

  return raw;
}

export interface RunStreamEvent {
  type: 'run_update' | 'run_complete';
  run_id: string;
  steps?: RunStep[];
  graph?: RunGraph;
  status?: string;
}

export async function createRun(
  goal: string,
  profile: string,
  filePaths: string[] = [],
  options: CreateRunOptions = {},
): Promise<CreateRunResponse> {
  return requestJson<CreateRunResponse>('/api/runs', {
    method: 'POST',
    body: JSON.stringify({
      goal,
      file_paths: filePaths,
      repo_key: options.repoKey ?? undefined,
      profile,
      task_id: options.taskId ?? undefined,
      group_id: options.groupId ?? undefined,
      conversation_id: options.conversationId ?? undefined,
      authority_scope_id: options.authorityScopeId ?? undefined,
      authority_handoff_id: options.authorityHandoffId ?? undefined,
      authority_reason: options.authorityReason ?? undefined,
    }),
  });
}

export async function getRun(runId: string): Promise<RunSummary> {
  return requestJson<RunSummary>(`/api/runs/${runId}`);
}

export async function getRunEvents(runId: string, limit = 200): Promise<RunEventsResponse> {
  return requestJson<RunEventsResponse>(`/api/runs/${runId}/events?limit=${limit}`);
}

// --- Verifier reports (receipts) -----------------------------------------
//
// GET /api/runs/{run}/steps/{step}/verifier-report/{id} returns the stored
// verifier_reports row plus its evidence JSON. Two evidence generations
// exist:
//
//  - Today ("engine_verifier"): the server-side verifier evaluated evidence
//    the worker reported about itself. Real, but not independent.
//  - V6 (migration v61, in flight): checks Cortex re-executed in an isolated
//    runner — `gate` + `executions`, mirroring cortex_core::verification.
//
// The renderer distinguishes the two honestly; nothing here converts one
// into the other.

/** The verifier_reports row, as stored. */
export interface VerifierReportRow {
  id: string;
  step_id: string;
  run_id: string;
  lease_gen: number;
  worker_id: string | null;
  verifier: string;
  status: string;
  verdict: string;
  created_at: number;
  updated_at: number;
}

/** One check as the worker reported it (engine-verifier generation). */
export interface EngineCheckEvidence {
  name: string;
  status: 'passed' | 'failed' | 'skipped' | 'unknown' | string;
  summary?: string | null;
}

/** crates/engine verifier.rs VerifierReport, as serialized into evidence. */
export interface EngineVerifierReport {
  verdict: 'success' | 'needs_evidence' | 'failed' | 'blocked' | string;
  risk?: string;
  evidence_floor?: { satisfied: boolean; signals_met: string[]; missing: string[] };
  allowed_path_violations?: Array<{ path: string; reason: string }>;
  command_summary?: {
    total: number;
    succeeded: number;
    failed: number;
    unknown: number;
    failures: Array<{ command: string; exit_code?: number | null; summary?: string | null }>;
  };
  check_summary?: {
    total: number;
    passed: number;
    failed: number;
    skipped: number;
    unknown: number;
    failures: Array<{ name: string; summary?: string | null }>;
  };
  required_check_summary?: {
    total: number;
    satisfied: number;
    expected: EngineCheckEvidence[];
    missing: EngineCheckEvidence[];
    failed: EngineCheckEvidence[];
    skipped: EngineCheckEvidence[];
    unknown: EngineCheckEvidence[];
  };
  acceptance_coverage?: {
    evaluated: boolean;
    total: number;
    covered: number;
    uncovered: string[];
    note: string;
  };
  stale_base_notes?: Array<{
    expected_base_commit?: string | null;
    observed_base_commit?: string | null;
    note: string;
  }>;
  next_action?: string;
}

/** V6 wire shapes (cortex_core::verification), present once v61 persists them. */
export interface CheckExecutionWire {
  spec_id: string;
  exit_code: number | null;
  outcome: 'passed' | 'failed' | 'timed_out' | 'not_executed';
  duration_ms: number;
  output_digest: string;
  output_tail: string;
  runner_image: string;
}

export interface VerdictReportWire {
  verdict: 'verified' | 'failed' | 'inconclusive' | 'unverified';
  required_total: number;
  required_passed: number;
  failed: string[];
  not_executed: string[];
}

export interface VerifierEvidence {
  source?: string;
  message_id?: string;
  worker_completed?: {
    exit_code?: number | null;
    summary?: string | null;
    files_changed?: string[];
    base_commit?: string | null;
    head_commit?: string | null;
    branch?: string | null;
  };
  verifier_input?: {
    evidence?: {
      checks?: EngineCheckEvidence[];
      files_changed?: string[];
    };
  };
  verifier_report?: EngineVerifierReport;
  // V6 generation:
  gate?: VerdictReportWire;
  executions?: CheckExecutionWire[];
  tree_hash?: string;
  attempt?: number;
  verification_id?: string;
}

export interface VerifierReportPayload {
  report: VerifierReportRow;
  evidence: VerifierEvidence;
}

export async function getVerifierReport(
  runId: string,
  stepId: string,
  reportId: string,
): Promise<VerifierReportPayload> {
  return requestJson<VerifierReportPayload>(
    `/api/runs/${encodeURIComponent(runId)}/steps/${encodeURIComponent(stepId)}/verifier-report/${encodeURIComponent(reportId)}`,
  );
}

// --- V3 receipts: checks Cortex executed itself -------------------------
//
// Distinct from getVerifierReport above, which serves worker-reported
// evidence. This is the artifact a charge is bound to, and the only one that
// can honestly be called independent.

/** One check as Cortex executed it. Mirrors `cortex_core::verification`. */
export interface ReceiptCheckExecution {
  spec_id: string;
  exit_code: number | null;
  outcome: 'passed' | 'failed' | 'timed_out' | 'not_executed';
  duration_ms: number;
  output_digest: string;
  output_tail: string;
  runner_image: string;
}

/** The gate, recomputed server-side from the frozen specs and executions. */
export interface ReceiptVerdictReport {
  verdict: 'verified' | 'failed' | 'inconclusive' | 'unverified';
  required_total: number;
  required_passed: number;
  failed: string[];
  not_executed: string[];
}

/** `crates/api/src/db.rs::Receipt`, as served. */
export interface ReceiptPayload {
  verification_id: string;
  run_id: string;
  step_id: string;
  attempt: number;
  tree_hash: string;
  gate: ReceiptVerdictReport;
  executions: ReceiptCheckExecution[];
}

/**
 * The receipt for a step, or `null` when no verification exists for it.
 *
 * A 404 is the normal answer for any step verified before V3 shipped, or one
 * whose verification has not finished. It is not an error state and must not
 * be rendered as one — the caller falls back to the legacy report.
 */
export async function getReceipt(
  runId: string,
  stepId: string,
): Promise<ReceiptPayload | null> {
  try {
    return await requestJson<ReceiptPayload>(
      `/api/runs/${encodeURIComponent(runId)}/steps/${encodeURIComponent(stepId)}/receipt`,
    );
  } catch (err) {
    if (err instanceof CortexApiError && err.status === 404) return null;
    throw err;
  }
}

export async function getTaskProjection(groupId: string, taskId: string): Promise<TaskProjection> {
  return requestJson<TaskProjection>(
    `/api/groups/${encodeURIComponent(groupId)}/tasks/${encodeURIComponent(taskId)}/projection`,
  );
}

export async function getGroupOperationsSummary(groupId: string): Promise<GroupOperationsSummary> {
  return requestJson<GroupOperationsSummary>(
    `/api/groups/${encodeURIComponent(groupId)}/operations/summary`,
  );
}

export async function getGroupOperationsGraph(groupId: string): Promise<GroupOperationsGraph> {
  return requestJson<GroupOperationsGraph>(
    `/api/groups/${encodeURIComponent(groupId)}/operations/graph`,
  );
}

export async function getPersonalOperationsSummary(): Promise<PersonalOperationsSummary> {
  return requestJson<PersonalOperationsSummary>('/api/operations/summary');
}

export async function getAuthorityScopes(): Promise<CortexAuthorityScopesResponse> {
  return requestJson<CortexAuthorityScopesResponse>('/api/authority/scopes');
}

export interface CreateAuthorityScopeRequest {
  name: string;
  description: string;
  kind: CortexAuthorityScopeKind;
  role: 'owner' | 'admin' | 'member' | 'viewer';
}

export interface DelegateAuthorityRequest {
  scope_id: string;
  subject_did: string;
  role: 'admin' | 'member' | 'viewer';
  reason?: string;
}

export interface DelegateAuthorityResponse {
  delegation_id: string;
  scope_id: string;
  subject_did: string;
  role: string;
  created_at: number;
}

export async function createAuthorityScope(
  request: CreateAuthorityScopeRequest,
): Promise<CortexAuthorityScope> {
  return requestJson<CortexAuthorityScope>('/api/authority/scopes', {
    method: 'POST',
    body: JSON.stringify(request),
  });
}

/**
 * `/api/authority/delegate` is not served. Its neighbours are: the backend has
 * `/api/authority/scopes` and `DELETE /api/authority/delegations/{id}`, so you
 * can create a scope and revoke a delegation, but not make one.
 *
 * Handing one account the right to act for another is not something to fake a
 * success for, so this refuses out loud rather than returning a neutral value.
 */
export async function delegateAuthority(
  request: DelegateAuthorityRequest,
): Promise<DelegateAuthorityResponse> {
  void request;
  throw new CortexApiError(501, 'Delegating authority is not available yet.');
}

export async function revokeAuthorityDelegation(
  delegationId: string,
): Promise<{ revoked: boolean }> {
  return requestJson<{ revoked: boolean }>(`/api/authority/delegations/${encodeURIComponent(delegationId)}`, {
    method: 'DELETE',
  });
}

export async function listGroupApprovals(
  groupId: string,
  status?: string,
): Promise<CortexApprovalRequest[]> {
  const query = status ? `?status=${encodeURIComponent(status)}` : '';
  return requestJson<CortexApprovalRequest[]>(
    `/api/groups/${encodeURIComponent(groupId)}/approvals${query}`,
  );
}

export async function createGroupApproval(
  groupId: string,
  request: {
    title: string;
    body?: string;
    task_id?: string | null;
    conversation_id?: string | null;
    run_id?: string | null;
    priority?: string;
    requested_by?: string;
  },
): Promise<CortexApprovalRequest> {
  return requestJson<CortexApprovalRequest>(`/api/groups/${encodeURIComponent(groupId)}/approvals`, {
    method: 'POST',
    body: JSON.stringify(request),
  });
}

export async function resolveGroupApproval(
  groupId: string,
  requestId: string,
  status: 'approved' | 'rejected' | 'cancelled',
  decision: unknown = {},
): Promise<CortexApprovalRequest> {
  return requestJson<CortexApprovalRequest>(
    `/api/groups/${encodeURIComponent(groupId)}/approvals/${encodeURIComponent(requestId)}`,
    {
      method: 'PATCH',
      body: JSON.stringify({ status, decision }),
    },
  );
}

export async function listRuns(limit = 10, offset = 0): Promise<RunListItem[]> {
  return requestJson<RunListItem[]>(`/api/runs?limit=${limit}&offset=${offset}`);
}

export async function createRunPullRequest(
  runId: string,
  body: { title?: string; base?: string } = {},
): Promise<{ pr_url: string; branch: string }> {
  return requestJson<{ pr_url: string; branch: string }>(`/api/runs/${runId}/pr`, {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

export function streamRun(
  runId: string,
  onEvent: (event: RunStreamEvent) => void,
  onError: (err: Error) => void,
): AbortController {
  const controller = new AbortController();

  (async () => {
    for (let attempt = 0; attempt <= STREAM_RECONNECT_DELAYS_MS.length; attempt++) {
      try {
        const res = await authedFetch(apiUrl(`/api/runs/${runId}/stream`), {
          signal: controller.signal,
        });

        if (!res.ok) {
          throw new CortexApiError(res.status, await readErrorMessage(res), res.headers.get('Retry-After'));
        }

        const reader = res.body?.getReader();
        if (!reader) throw new Error('No response body');

        const decoder = new TextDecoder();
        let buffer = '';
        let completedNormally = false;

        while (true) {
          const { done, value } = await reader.read();
          if (done) {
            completedNormally = true;
            break;
          }

          buffer += decoder.decode(value, { stream: true });
          const lines = buffer.split('\n');
          buffer = lines.pop() ?? '';

          for (const line of lines) {
            if (!line.startsWith('data: ')) continue;
            const json = line.slice(6).trim();
            if (!json) continue;
            try {
              const event = JSON.parse(json) as RunStreamEvent;
              onEvent(event);
              // If the server signalled completion, no need to reconnect
              if (event.type === 'run_complete') return;
            } catch {
              // skip malformed lines
            }
          }
        }

        // Stream ended normally (server closed) -- no reconnect needed
        if (completedNormally) return;
      } catch (err) {
        if (controller.signal.aborted) return;
        if (err instanceof Error && err.name === 'AbortError') return;
        // Non-retryable HTTP errors (4xx)
        if (err instanceof CortexApiError && err.status >= 400 && err.status < 500) {
          onError(err);
          return;
        }
        // Last attempt -- surface the error
        if (attempt === STREAM_RECONNECT_DELAYS_MS.length) {
          if (err instanceof Error) onError(err);
          return;
        }
      }

      // Wait before reconnect
      if (attempt < STREAM_RECONNECT_DELAYS_MS.length) {
        await new Promise<void>((resolve) => {
          window.setTimeout(resolve, STREAM_RECONNECT_DELAYS_MS[attempt]);
        });
        if (controller.signal.aborted) return;
      }
    }
  })();

  return controller;
}

// Resource conflict types (mirrors crates/api/src/db.rs ResourceLeaseConflict)

export interface ResourceLeaseConflict {
  lease_id: string;
  run_id: string;
  step_id: string | null;
  holder_type: string;
  resource_type: string;
  repo_key: string;
  resource_key: string;
  mode: string;
  expires_at: number;
}

/**
 * Fetch active resource conflicts blocking a given run.
 * Stub: the backend endpoint is not yet wired, so this returns an empty array.
 * When the endpoint lands, replace with a real `requestJson` call.
 */
export async function getActiveConflicts(runId: string): Promise<ResourceLeaseConflict[]> {
  void runId;
  return Promise.resolve([]);
}

// Conversations

export interface ConversationSummary {
  id: string;
  title: string | null;
  updated_at: string;
  message_count: number;
  last_message_preview: string | null;
}

export interface ConversationMessage {
  id: string;
  conversation_id: string;
  role: 'user' | 'assistant';
  content: string;
  provider: string | null;
  model: string | null;
  created_at: string;
}

export interface ConversationWithMessages {
  id: string;
  user_id: string;
  title: string | null;
  created_at: string;
  updated_at: string;
  messages: ConversationMessage[];
}

export async function listConversations(_userId = 'local'): Promise<ConversationSummary[]> {
  void _userId;
  const conversations = await requestJson<unknown>('/api/conversations');
  return Array.isArray(conversations)
    ? conversations.flatMap((conversation) => {
      if (!conversation || typeof conversation !== 'object') return [];
      const record = conversation as Partial<ConversationSummary>;
      if (typeof record.id !== 'string') return [];
      return [{
        id: record.id,
        title: typeof record.title === 'string' ? record.title : null,
        updated_at: typeof record.updated_at === 'string' ? record.updated_at : new Date(0).toISOString(),
        message_count: typeof record.message_count === 'number' ? record.message_count : 0,
        last_message_preview: typeof record.last_message_preview === 'string'
          ? record.last_message_preview
          : null,
      }];
    })
    : [];
}

export async function createConversation(_userId = 'local', title?: string): Promise<{ id: string }> {
  void _userId;
  return requestJson<{ id: string }>('/api/conversations', {
    method: 'POST',
    body: JSON.stringify({ title }),
  });
}

export async function getConversation(id: string, _userId = 'local'): Promise<ConversationWithMessages> {
  void _userId;
  return requestJson<ConversationWithMessages>(`/api/conversations/${id}`);
}

export async function deleteConversation(id: string, _userId = 'local'): Promise<void> {
  void _userId;
  await requestJson<unknown>(`/api/conversations/${id}`, { method: 'DELETE' });
}

export async function updateConversationTitle(id: string, title: string, _userId = 'local'): Promise<void> {
  void _userId;
  await requestJson<unknown>(`/api/conversations/${id}`, {
    method: 'PATCH',
    body: JSON.stringify({ title }),
  });
}

export async function addMessageToConversation(
  conversationId: string,
  role: string,
  content: string,
  provider?: string,
  model?: string,
): Promise<ConversationMessage> {
  return requestJson<ConversationMessage>(`/api/conversations/${conversationId}/messages`, {
    method: 'POST',
    body: JSON.stringify({ role, content, provider, model }),
  });
}

// ─── Semantic leases (CONTEXT.md C4) ───

export interface ImpactedFile {
  file: string;
  distance: number;
  /** Rendered route, e.g. "auth.rs → checkout.rs → api.rs". */
  via: string;
}

export interface ImpactResponse {
  seed_files: string[];
  depth: number;
  files: ImpactedFile[];
  truncated: boolean;
  files_indexed: number;
}

export async function getImpactSet(files: string[], depth?: number): Promise<ImpactResponse> {
  const params = new URLSearchParams({ files: files.join(',') });
  if (depth !== undefined) params.set('depth', String(depth));
  return requestJson<ImpactResponse>(`/api/context/impact?${params.toString()}`);
}
