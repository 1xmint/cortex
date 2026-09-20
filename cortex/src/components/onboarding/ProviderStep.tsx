import { useCallback, useEffect, useRef, useState } from 'react';
import { CheckCircle, Copy, ExternalLink, Loader2, Star, Trash2, XCircle } from 'lucide-react';
import ContainerStatus from './ContainerStatus';
import {
  getAuthStatus,
  getProviders,
  startAuth,
  submitAuthCode,
  deleteCredential,
  setDefaultCredential,
  type ProviderAuthInfo,
} from '../../lib/cortexApi';

interface ProviderStepProps {
  onNext: () => void;
}

interface ProviderAuthState {
  phase: 'idle' | 'starting' | 'awaiting_code' | 'submitting' | 'polling';
  authUrl: string | null;
  deviceCode: string | null;
  codeInput: string;
  error: string | null;
  containerStatus: 'idle' | 'provisioning' | 'ready' | 'error';
}

const INITIAL_STATE: ProviderAuthState = {
  phase: 'idle',
  authUrl: null,
  deviceCode: null,
  codeInput: '',
  error: null,
  containerStatus: 'idle',
};

interface AvailableProvider {
  provider: string;
  available: boolean;
  health: string;
}

function readProviderRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function parseAvailableProviders(payload: unknown): AvailableProvider[] {
  const root = readProviderRecord(payload);
  const providers = root?.providers ?? payload;
  const entries = Array.isArray(providers)
    ? providers.map((provider) => [undefined, provider] as const)
    : Object.entries(readProviderRecord(providers) ?? {});

  return entries
    .map(([key, value]) => {
      const record = readProviderRecord(value);
      if (!record) return null;
      const provider = String(record.provider ?? record.id ?? record.name ?? key ?? '').toLowerCase();
      if (!provider) return null;
      return {
        provider,
        available: Boolean(record.available ?? record.authenticated ?? record.connected ?? record.enabled ?? true),
        health: String(record.health ?? record.status ?? record.pressure_state ?? 'unknown'),
      };
    })
    .filter((provider): provider is AvailableProvider => Boolean(provider));
}

function providerLabel(provider: string) {
  if (provider === 'claude' || provider === 'anthropic') return 'Claude';
  if (provider === 'openai') return 'OpenAI';
  if (provider === 'gemini' || provider === 'google') return 'Gemini';
  return provider.replace(/^\w/, (letter) => letter.toUpperCase());
}

export default function ProviderStep({ onNext }: ProviderStepProps) {
  const [providers, setProviders] = useState<ProviderAuthInfo[]>([]);
  const [availableProviders, setAvailableProviders] = useState<AvailableProvider[]>([]);
  const [loading, setLoading] = useState(true);
  const [fetchError, setFetchError] = useState<string | null>(null);
  const [authStates, setAuthStates] = useState<Record<string, ProviderAuthState>>({});
  const inputRefs = useRef<Record<string, HTMLInputElement | null>>({});

  const fetchStatus = useCallback(async () => {
    try {
      const status = await getAuthStatus();
      const availability = await getProviders().then(parseAvailableProviders).catch(() => []);
      setProviders(status);
      setAvailableProviders(availability);
      setFetchError(null);
    } catch {
      setFetchError('Cannot reach Cortex backend');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchStatus();
  }, [fetchStatus]);

  const getState = (provider: string): ProviderAuthState =>
    authStates[provider] ?? INITIAL_STATE;

  const updateState = (provider: string, update: Partial<ProviderAuthState>) => {
    setAuthStates((prev) => ({
      ...prev,
      [provider]: { ...(prev[provider] ?? INITIAL_STATE), ...update },
    }));
  };

  const isDeviceCodeFlow = (provider: string) => provider === 'openai';

  const handleConnect = async (provider: string) => {
    updateState(provider, { phase: 'starting', error: null, containerStatus: 'provisioning' });

    try {
      const result = await startAuth(provider, 'subscription');
      if (result.auth_url) {
        // Simulate container ready after a short delay
        setTimeout(() => {
          updateState(provider, { containerStatus: 'ready' });
        }, 1500);

        if (isDeviceCodeFlow(provider) && result.device_code) {
          updateState(provider, {
            phase: 'polling',
            authUrl: result.auth_url,
            deviceCode: result.device_code,
          });
          window.open(result.auth_url, '_blank', 'noopener');
          pollUntilAuth(provider);
        } else {
          updateState(provider, {
            phase: 'awaiting_code',
            authUrl: result.auth_url,
            deviceCode: result.device_code,
          });
          window.open(result.auth_url, '_blank', 'noopener');
          setTimeout(() => inputRefs.current[provider]?.focus(), 100);
        }
      } else {
        updateState(provider, {
          phase: 'idle',
          containerStatus: 'error',
          error: result.message || 'Could not start authentication',
        });
      }
    } catch (err) {
      updateState(provider, {
        phase: 'idle',
        containerStatus: 'error',
        error: err instanceof Error ? err.message : 'Failed to start authentication',
      });
    }
  };

  const handleSubmitCode = async (provider: string) => {
    const state = getState(provider);
    const code = state.codeInput.trim();
    if (!code) return;

    updateState(provider, { phase: 'submitting', error: null });

    try {
      const result = await submitAuthCode(provider, code, `${providerLabel(provider)} subscription`, 'subscription');
      if (result.success) {
        // Show success state briefly before resetting
        updateState(provider, { ...state, phase: 'idle', error: null });
        await fetchStatus();

        // Reset to clean state after showing success
        setTimeout(() => {
          updateState(provider, { ...INITIAL_STATE });
        }, 2000);
      } else {
        updateState(provider, { phase: 'awaiting_code', error: result.message || 'Authentication failed' });
      }
    } catch {
      updateState(provider, { phase: 'awaiting_code', error: 'Failed to submit code' });
    }
  };

  const pollUntilAuth = async (provider: string) => {
    for (let i = 0; i < 20; i++) {
      await new Promise((r) => setTimeout(r, 3000));
      try {
        const status = await getAuthStatus();
        setProviders(status);
        if (status.find((s) => s.provider === provider)?.authenticated) {
          updateState(provider, { ...INITIAL_STATE });
          return;
        }
      } catch {
        // keep polling
      }
    }
    updateState(provider, {
      phase: 'idle',
      error: 'Timed out — try again',
    });
  };

  const handleKeyDown = (provider: string, e: React.KeyboardEvent) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      handleSubmitCode(provider);
    }
  };

  const handleDelete = async (credentialId: string) => {
    try {
      await deleteCredential(credentialId);
      await fetchStatus();
    } catch {
      // silently fail, fetchStatus will show current state
    }
  };

  const handleSetDefault = async (credentialId: string) => {
    try {
      await setDefaultCredential(credentialId);
      await fetchStatus();
    } catch {
      // silently fail
    }
  };

  const displayProviders = providers.length > 0
    ? providers
    : availableProviders.map((provider) => ({
        provider: provider.provider,
        credential_type: 'subscription',
        label: null,
        authenticated: false,
        email: null,
        is_default: false,
        credential_id: '',
        status: 'active',
      }));
  const anyAuthed = providers.some((p) => p.authenticated);

  if (loading) {
    return (
      <div className="flex items-center justify-center gap-2 p-8 text-sm text-[var(--muted)]">
        <Loader2 className="h-4 w-4 animate-spin" />
        Checking providers...
      </div>
    );
  }

  if (fetchError) {
    return (
      <div className="flex flex-col gap-4 p-5">
        <div className="rounded-xl border border-red-500/20 bg-red-500/10 p-3 text-xs text-red-200">
          {fetchError}
        </div>
        <button
          onClick={() => { setLoading(true); fetchStatus(); }}
          className="rounded-lg bg-[var(--accent-soft)] px-4 py-2 text-sm font-medium text-[var(--accent)]"
        >
          Retry
        </button>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-4 p-5">
      <div>
        <h2 className="text-base font-medium text-white">Connect your AI providers</h2>
        <p className="mt-1 text-sm text-[var(--muted)]">
          Link your existing subscriptions. Connect at least one to get started.
        </p>
      </div>

      <div className="flex flex-col gap-3">
        {displayProviders.map((p) => {
          const state = getState(p.provider);
          const isActive = state.phase !== 'idle';
          const availability = availableProviders.find((provider) => provider.provider === p.provider);
          return (
            <div
              key={p.provider}
              className="rounded-xl border border-white/8 bg-white/4"
            >
              {/* Provider header */}
              <div className="flex items-center justify-between px-4 py-3">
                <div className="flex items-center gap-3">
                  <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-white/6 text-xs font-bold uppercase text-[var(--muted-strong)]">
                      {providerLabel(p.provider).slice(0, 2).toUpperCase()}
                  </div>
                  <div>
                    <div className="text-sm font-medium text-white">
                      {providerLabel(p.provider)}
                    </div>
                    {p.authenticated ? (
                      <div className="flex items-center gap-1.5 text-xs text-emerald-300">
                        <CheckCircle className="h-3 w-3" />
                        {p.label ?? p.email ?? 'Connected'}
                        <span className="rounded-full border border-emerald-500/30 bg-emerald-500/10 px-1.5 py-0.5 text-[10px] capitalize text-emerald-400">
                          {p.credential_type}
                        </span>
                        {p.is_default && (
                          <span className="rounded-full border border-amber-500/30 bg-amber-500/10 px-1.5 py-0.5 text-[10px] text-amber-400">
                            default
                          </span>
                        )}
                      </div>
                    ) : (
                      <div className="flex items-center gap-1 text-xs text-[var(--muted)]">
                        <XCircle className="h-3 w-3" />
                        Not connected
                      </div>
                    )}
                  </div>
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  {availability && (
                    <span className="rounded-full border border-white/8 bg-white/4 px-2 py-0.5 text-[10px] capitalize text-[var(--muted)]">
                      {availability.available ? availability.health : 'unavailable'}
                    </span>
                  )}

                  {p.authenticated && p.credential_id && (
                    <>
                      {!p.is_default && (
                        <button
                          onClick={() => handleSetDefault(p.credential_id)}
                          title="Set as default"
                          className="rounded-lg border border-white/8 bg-white/4 p-1.5 text-[var(--muted)] transition hover:text-amber-400"
                        >
                          <Star className="h-3.5 w-3.5" />
                        </button>
                      )}
                      <button
                        onClick={() => handleDelete(p.credential_id)}
                        title="Remove credential"
                        className="rounded-lg border border-white/8 bg-white/4 p-1.5 text-[var(--muted)] transition hover:text-red-400"
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </button>
                    </>
                  )}

                  {!p.authenticated && !isActive && (
                    <button
                      onClick={() => handleConnect(p.provider)}
                      className="rounded-lg border border-[var(--accent)]/30 bg-[var(--accent-soft)] px-3 py-1.5 text-xs font-medium text-[var(--accent)] transition hover:bg-[var(--accent)]/20"
                    >
                      Connect
                    </button>
                  )}

                  {state.phase === 'starting' && (
                    <span className="flex items-center gap-1.5 text-xs text-[var(--muted)]">
                      <Loader2 className="h-3 w-3 animate-spin" />
                      Setting up secure container...
                    </span>
                  )}
                </div>
              </div>

              {/* Auth flow panel — device code flow (OpenAI) */}
              {isDeviceCodeFlow(p.provider) && state.phase === 'polling' && state.deviceCode && (
                <div className="border-t border-white/6 px-4 py-3">
                  <div className="flex flex-col gap-3">
                    <div className="flex items-start gap-2">
                      <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-[var(--accent-soft)] text-[10px] font-bold text-[var(--accent)]">1</span>
                      <div>
                        <p className="text-xs text-[var(--muted)]">
                          Copy this code:
                        </p>
                        <button
                          onClick={() => {
                            navigator.clipboard.writeText(state.deviceCode!);
                          }}
                          className="mt-1 inline-flex items-center gap-2 rounded-lg border border-white/10 bg-[var(--composer)] px-3 py-1.5 font-mono text-base font-bold tracking-widest text-white transition hover:border-[var(--accent)]/40"
                        >
                          {state.deviceCode}
                          <Copy className="h-3.5 w-3.5 text-[var(--muted)]" />
                        </button>
                      </div>
                    </div>

                    <div className="flex items-start gap-2">
                      <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-[var(--accent-soft)] text-[10px] font-bold text-[var(--accent)]">2</span>
                      <div>
                        <p className="text-xs text-[var(--muted)]">
                          Open OpenAI and enter the code:
                        </p>
                        <a
                          href={state.authUrl!}
                          target="_blank"
                          rel="noopener noreferrer"
                          className="mt-1 inline-flex items-center gap-1.5 text-xs font-medium text-[var(--accent)] underline"
                        >
                          Open OpenAI <ExternalLink className="h-3 w-3" />
                        </a>
                      </div>
                    </div>

                    <div className="flex items-center gap-2 rounded-lg bg-white/4 px-3 py-2">
                      <Loader2 className="h-3.5 w-3.5 animate-spin text-[var(--accent)]" />
                      <span className="text-xs text-[var(--muted)]">Waiting for authorization...</span>
                    </div>
                  </div>
                </div>
              )}

              {/* Auth flow panel — paste code flow (Claude) */}
              {!isDeviceCodeFlow(p.provider) && (state.phase === 'awaiting_code' || state.phase === 'submitting' || state.phase === 'polling') && (
                <div className="border-t border-white/6 px-4 py-3">
                  <div className="flex flex-col gap-3">
                    <div className="flex items-start gap-2">
                      <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-[var(--accent-soft)] text-[10px] font-bold text-[var(--accent)]">1</span>
                      <div>
                        <p className="text-xs text-[var(--muted)]">
                          Sign in and authorize access:
                        </p>
                        <a
                          href={state.authUrl!}
                          target="_blank"
                          rel="noopener noreferrer"
                          className="mt-1 inline-flex items-center gap-1.5 text-xs font-medium text-[var(--accent)] underline"
                        >
                          Open Anthropic <ExternalLink className="h-3 w-3" />
                        </a>
                      </div>
                    </div>

                    <div className="flex items-start gap-2">
                      <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-[var(--accent-soft)] text-[10px] font-bold text-[var(--accent)]">2</span>
                      <div className="flex-1">
                        <p className="mb-1.5 text-xs text-[var(--muted)]">
                          Copy the authorization code and paste it here:
                        </p>
                        <div className="flex gap-2">
                          <input
                            ref={(el) => { inputRefs.current[p.provider] = el; }}
                            type="text"
                            placeholder="Paste your code here"
                            value={state.codeInput}
                            onChange={(e) => updateState(p.provider, { codeInput: e.target.value })}
                            onKeyDown={(e) => handleKeyDown(p.provider, e)}
                            disabled={state.phase === 'submitting' || state.phase === 'polling'}
                            className="flex-1 rounded-lg border border-white/10 bg-[var(--composer)] px-3 py-2 font-mono text-sm text-white placeholder:text-white/20 focus:border-[var(--accent)]/50 focus:outline-none disabled:opacity-50"
                            autoComplete="off"
                            spellCheck={false}
                          />
                          <button
                            onClick={() => handleSubmitCode(p.provider)}
                            disabled={!state.codeInput.trim() || state.phase === 'submitting' || state.phase === 'polling'}
                            className="shrink-0 rounded-lg bg-[var(--accent-soft)] px-3 py-2 text-xs font-medium text-[var(--accent)] transition hover:bg-[var(--accent)]/20 disabled:opacity-30"
                          >
                            {state.phase === 'submitting' ? (
                              <Loader2 className="h-3.5 w-3.5 animate-spin" />
                            ) : state.phase === 'polling' ? (
                              <span className="flex items-center gap-1">
                                <Loader2 className="h-3 w-3 animate-spin" />
                                Verifying
                              </span>
                            ) : (
                              'Submit'
                            )}
                          </button>
                        </div>
                      </div>
                    </div>
                  </div>
                </div>
              )}

              {/* Container Status */}
              {(state.containerStatus === 'provisioning' || state.containerStatus === 'ready' || state.containerStatus === 'error') && (
                <div className="border-t border-white/6 px-4 py-3">
                  <ContainerStatus
                    provider={p.provider}
                    status={state.containerStatus}
                    message={state.containerStatus === 'error' ? (state.error || undefined) : undefined}
                  />
                </div>
              )}

              {/* Error */}
              {state.error && (
                <div className="border-t border-white/6 px-4 py-2.5">
                  <div className="flex flex-col gap-2 rounded-lg border border-red-500/20 bg-red-500/10 p-3">
                    <p className="text-xs text-red-300">{state.error}</p>
                    <button
                      onClick={() => handleConnect(p.provider)}
                      className="self-start rounded px-2 py-1 text-xs bg-red-500/20 text-red-200 hover:bg-red-500/30 transition"
                    >
                      Try again
                    </button>
                  </div>
                </div>
              )}
            </div>
          );
        })}
      </div>

      <div className="flex items-center justify-between pt-2">
        <button
          onClick={onNext}
          className="text-xs text-[var(--muted)] transition hover:text-white"
        >
          Skip for now
        </button>
        <button
          onClick={onNext}
          disabled={!anyAuthed}
          className="rounded-lg bg-[var(--accent-soft)] px-4 py-2 text-sm font-medium text-[var(--accent)] transition hover:bg-[var(--accent)]/20 disabled:opacity-30"
        >
          Continue
        </button>
      </div>
    </div>
  );
}
