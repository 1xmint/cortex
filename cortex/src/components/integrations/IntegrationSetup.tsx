import { useCallback, useEffect, useMemo, useState } from 'react';
import { AlertTriangle, CheckCircle2, Hash, Loader2, RefreshCw, TerminalSquare, Unplug, Zap } from 'lucide-react';
import {
  getIntegrationStatus,
  getReplitWorkspaces,
  getSlackChannels,
  importReplitWorkspace,
  importSlackChannels,
  startSlackOAuth,
  type IntegrationStatus,
  type ReplitWorkspace,
  type SlackChannel,
} from '../../lib/cortexApi';

function StatusPill({ connected, label }: { connected: boolean; label: string }) {
  return (
    <span className={`inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] font-medium ${
      connected
        ? 'border-emerald-300/20 bg-emerald-300/10 text-emerald-200'
        : 'border-amber-300/20 bg-amber-300/10 text-amber-200'
    }`}>
      {connected ? <CheckCircle2 className="h-3 w-3" /> : <AlertTriangle className="h-3 w-3" />}
      {label}
    </span>
  );
}

export default function IntegrationSetup() {
  const [status, setStatus] = useState<IntegrationStatus | null>(null);
  const [channels, setChannels] = useState<SlackChannel[]>([]);
  const [selectedChannels, setSelectedChannels] = useState<Set<string>>(new Set());
  const [workspaces, setWorkspaces] = useState<ReplitWorkspace[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const slackConnected = useMemo(
    () => status?.connections.some((connection) => connection.provider === 'slack' && connection.status === 'connected') ?? false,
    [status],
  );
  const replitConnected = useMemo(
    () => status?.connections.some((connection) => connection.provider === 'replit' && connection.status === 'connected') ?? false,
    [status],
  );

  const refresh = useCallback(async () => {
    setError(null);
    setBusy('refresh');
    try {
      const [nextStatus, nextChannels, nextWorkspaces] = await Promise.all([
        getIntegrationStatus(),
        getSlackChannels().catch(() => []),
        getReplitWorkspaces().catch(() => []),
      ]);
      setStatus(nextStatus);
      setChannels(nextChannels);
      setWorkspaces(nextWorkspaces);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Integration status unavailable');
    } finally {
      setBusy(null);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const connectSlack = async () => {
    setBusy('slack-oauth');
    setError(null);
    try {
      const result = await startSlackOAuth();
      window.open(result.auth_url, '_blank', 'noopener');
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Slack OAuth could not start');
    } finally {
      setBusy(null);
    }
  };

  const syncSlackChannels = async () => {
    const chosen = channels.filter((channel) => selectedChannels.has(channel.id));
    if (chosen.length === 0) return;
    setBusy('slack-import');
    setError(null);
    try {
      await importSlackChannels(chosen);
      setSelectedChannels(new Set());
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Slack channels could not be imported');
    } finally {
      setBusy(null);
    }
  };

  const syncReplitWorkspace = async (workspace: ReplitWorkspace) => {
    setBusy(`replit-${workspace.id}`);
    setError(null);
    try {
      await importReplitWorkspace(workspace);
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Replit workspace could not be imported');
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center justify-between gap-3">
        <div>
          <h3 className="text-sm font-semibold text-white">Workflow integrations</h3>
          <p className="mt-1 text-xs text-[var(--muted)]">Connect channels and workspaces to Cortex task managers.</p>
        </div>
        <button
          type="button"
          onClick={() => void refresh()}
          disabled={busy === 'refresh'}
          className="inline-flex h-8 w-8 items-center justify-center rounded-lg border border-white/8 bg-white/4 text-[var(--muted)] transition hover:bg-white/8 hover:text-white disabled:opacity-50"
          aria-label="Refresh integration status"
          title="Refresh"
        >
          {busy === 'refresh' ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}
        </button>
      </div>

      {error && (
        <div className="rounded-lg border border-red-300/20 bg-red-400/10 px-3 py-2 text-xs text-red-200">
          {error}
        </div>
      )}

      <section className="rounded-xl border border-white/8 bg-white/[0.02]">
        <div className="flex items-start justify-between gap-3 border-b border-white/6 px-4 py-3">
          <div className="flex min-w-0 items-start gap-3">
            <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-white/6 text-[var(--muted-strong)]">
              <Hash className="h-4 w-4" />
            </div>
            <div className="min-w-0">
              <div className="flex items-center gap-2">
                <h4 className="text-sm font-medium text-white">Slack</h4>
                <StatusPill connected={slackConnected} label={slackConnected ? 'Connected' : status?.slack_configured ? 'Ready' : 'Needs env'} />
              </div>
              <p className="mt-1 text-xs text-[var(--muted)]">Map channels to groups, receive task updates, and capture slash commands.</p>
            </div>
          </div>
          <button
            type="button"
            onClick={() => void connectSlack()}
            disabled={busy === 'slack-oauth'}
            className="shrink-0 rounded-lg border border-[var(--accent)]/30 bg-[var(--accent-soft)] px-3 py-2 text-xs font-semibold text-[var(--accent)] transition hover:bg-[var(--accent)]/20 disabled:opacity-50"
          >
            {busy === 'slack-oauth' ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : slackConnected ? 'Reconnect' : 'Connect'}
          </button>
        </div>

        <div className="space-y-2 px-4 py-3">
          {channels.map((channel) => (
            <label key={channel.id} className="flex items-center justify-between gap-3 rounded-lg px-2 py-2 text-sm transition hover:bg-white/5">
              <span className="min-w-0">
                <span className="block truncate text-white">#{channel.name}</span>
                <span className="text-xs text-[var(--muted)]">{channel.member_count} members</span>
              </span>
              <input
                type="checkbox"
                checked={selectedChannels.has(channel.id)}
                onChange={(event) => {
                  setSelectedChannels((current) => {
                    const next = new Set(current);
                    if (event.target.checked) next.add(channel.id);
                    else next.delete(channel.id);
                    return next;
                  });
                }}
                className="h-4 w-4 accent-[var(--accent)]"
              />
            </label>
          ))}
          <button
            type="button"
            onClick={() => void syncSlackChannels()}
            disabled={selectedChannels.size === 0 || busy === 'slack-import'}
            className="mt-2 inline-flex items-center gap-2 rounded-lg bg-white/8 px-3 py-2 text-xs font-semibold text-white transition hover:bg-white/12 disabled:opacity-40"
          >
            {busy === 'slack-import' ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Zap className="h-3.5 w-3.5" />}
            Import selected
          </button>
        </div>
      </section>

      <section className="rounded-xl border border-white/8 bg-white/[0.02]">
        <div className="flex items-start gap-3 border-b border-white/6 px-4 py-3">
          <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-white/6 text-[var(--muted-strong)]">
            <TerminalSquare className="h-4 w-4" />
          </div>
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <h4 className="text-sm font-medium text-white">Replit</h4>
              <StatusPill connected={replitConnected} label={replitConnected ? 'Connected' : status?.replit_configured ? 'Ready' : 'Needs env'} />
            </div>
            <p className="mt-1 text-xs text-[var(--muted)]">Import workspace context and link Replit projects to orchestration groups.</p>
          </div>
        </div>

        <div className="space-y-2 px-4 py-3">
          {workspaces.map((workspace) => (
            <div key={workspace.id} className="flex items-center justify-between gap-3 rounded-lg px-2 py-2 hover:bg-white/5">
              <div className="min-w-0">
                <div className="truncate text-sm text-white">{workspace.title}</div>
                <div className="text-xs text-[var(--muted)]">{workspace.language}</div>
              </div>
              <button
                type="button"
                onClick={() => void syncReplitWorkspace(workspace)}
                disabled={busy === `replit-${workspace.id}`}
                className="inline-flex items-center gap-1.5 rounded-lg border border-white/8 bg-white/5 px-3 py-1.5 text-xs font-medium text-white transition hover:bg-white/10 disabled:opacity-50"
              >
                {busy === `replit-${workspace.id}` ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Unplug className="h-3.5 w-3.5" />}
                Link
              </button>
            </div>
          ))}
        </div>
      </section>
    </div>
  );
}
