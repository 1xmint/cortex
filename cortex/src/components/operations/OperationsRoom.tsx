import { useCallback, useEffect, useMemo, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router';
import {
  AlertTriangle,
  ArrowLeft,
  CheckCircle2,
  Clock,
  GitBranch,
  Loader2,
  RefreshCw,
  Shield,
  User,
  XCircle,
} from 'lucide-react';
import AuthorityScopesPanel from '../authority/AuthorityScopesPanel';
import OperationsGraphPanel from '../tasks/OperationsGraphPanel';
import {
  getAuthorityScopes,
  getGroupOperationsSummary,
  getPersonalOperationsSummary,
  listGroupApprovals,
  type CortexApprovalRequest,
  type CortexAuthorityScope,
  type GroupOperationsSummary,
  type PersonalOperationsSummary,
} from '../../lib/cortexApi';

const GROUP_LABELS: Record<string, string> = {
  personal: 'Personal',
  heyvera: 'HeyVera',
  cortex: 'Cortex',
};

function groupLabel(groupId: string) {
  return GROUP_LABELS[groupId] ?? groupId;
}

function SummaryCard({ label, value, detail, tone }: { label: string; value: number; detail?: string; tone?: 'normal' | 'warning' | 'danger' }) {
  const border = tone === 'danger' ? 'border-red-300/20' : tone === 'warning' ? 'border-amber-300/20' : 'border-white/8';
  const bg = tone === 'danger' ? 'bg-red-400/[0.06]' : tone === 'warning' ? 'bg-amber-300/[0.06]' : 'bg-white/[0.03]';
  return (
    <div className={`rounded-lg border ${border} ${bg} px-4 py-3`}>
      <p className="text-[10px] uppercase tracking-wider text-[var(--muted)]">{label}</p>
      <p className="mt-1 text-2xl font-bold text-white">{value}</p>
      {detail && <p className="mt-0.5 text-[11px] text-[var(--muted)]">{detail}</p>}
    </div>
  );
}

const AUTHORITY_EVENT_KEYWORDS = ['authority', 'handoff', 'scope', 'approval'];

function isAuthorityEvent(eventType: string) {
  const lower = eventType.toLowerCase();
  return AUTHORITY_EVENT_KEYWORDS.some((keyword) => lower.includes(keyword));
}

function EventTimeline({
  events,
  authorityOnly,
  onToggleAuthority,
}: {
  events: GroupOperationsSummary['recent_events'];
  authorityOnly: boolean;
  onToggleAuthority: () => void;
}) {
  const filtered = authorityOnly
    ? events.filter((event) => isAuthorityEvent(event.event_type))
    : events;

  return (
    <div>
      <div className="mb-2 flex gap-1">
        <button
          type="button"
          onClick={() => { if (authorityOnly) onToggleAuthority(); }}
          className={`rounded-md px-2.5 py-1 text-[11px] font-medium transition ${
            !authorityOnly ? 'bg-white/10 text-white' : 'text-[var(--muted)] hover:text-white'
          }`}
        >
          All Events
        </button>
        <button
          type="button"
          onClick={() => { if (!authorityOnly) onToggleAuthority(); }}
          className={`inline-flex items-center gap-1 rounded-md px-2.5 py-1 text-[11px] font-medium transition ${
            authorityOnly ? 'bg-white/10 text-white' : 'text-[var(--muted)] hover:text-white'
          }`}
        >
          <Shield className="h-3 w-3" />
          Authority Events
        </button>
      </div>
      {filtered.length === 0 ? (
        <div className="flex h-32 items-center justify-center rounded-lg border border-white/8 bg-black/15 text-sm text-[var(--muted)]">
          {authorityOnly ? 'No authority events' : 'No recent events'}
        </div>
      ) : (
        <div className="max-h-[400px] space-y-1.5 overflow-y-auto rounded-lg border border-white/8 bg-black/15 p-3">
          {filtered.slice(0, 30).map((event) => {
            const isAuthority = isAuthorityEvent(event.event_type);
            const authorityMeta = isAuthority && event.payload
              ? Object.entries(event.payload)
                  .filter(([key]) => ['actor', 'delegated_by', 'scope_name', 'authority_level', 'granted_to'].includes(key))
                  .map(([key, val]) => `${key}: ${String(val)}`)
                  .join(' · ')
              : null;
            return (
              <div
                key={event.id}
                className={`flex items-start gap-3 rounded-md border px-3 py-2 ${
                  isAuthority
                    ? 'border-sky-300/15 bg-sky-400/[0.04]'
                    : 'border-white/6 bg-white/[0.02]'
                }`}
              >
                <div className="mt-0.5 shrink-0">
                  {event.event_type.includes('failed') ? (
                    <XCircle className="h-3.5 w-3.5 text-red-300" />
                  ) : event.event_type.includes('completed') || event.event_type.includes('verified') ? (
                    <CheckCircle2 className="h-3.5 w-3.5 text-emerald-300" />
                  ) : isAuthority ? (
                    <Shield className="h-3.5 w-3.5 text-sky-300" />
                  ) : (
                    <GitBranch className="h-3.5 w-3.5 text-[var(--muted)]" />
                  )}
                </div>
                <div className="min-w-0 flex-1">
                  <p className="truncate text-xs font-medium text-white">{event.event_type.replaceAll('.', ' ').replaceAll('_', ' ')}</p>
                  <p className="mt-0.5 truncate text-[10px] text-[var(--muted)]">
                    {event.entity_type} {event.entity_id ? `· ${event.entity_id.slice(0, 8)}` : ''}
                  </p>
                  {authorityMeta && (
                    <p className="mt-0.5 truncate text-[10px] text-sky-200/70">{authorityMeta}</p>
                  )}
                </div>
                <span className="shrink-0 text-[10px] text-[var(--muted)]">
                  {new Date(event.created_at).toLocaleTimeString()}
                </span>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

function AttentionList({ items }: { items: GroupOperationsSummary['attention'] }) {
  if (items.length === 0) return null;
  return (
    <div className="space-y-1.5">
      {items.map((item, index) => (
        <div key={index} className="flex items-center gap-2 rounded-md border border-amber-300/15 bg-amber-300/[0.06] px-3 py-2">
          <AlertTriangle className="h-3.5 w-3.5 shrink-0 text-amber-200" />
          <span className="min-w-0 truncate text-xs text-amber-100">
            {item.kind.replaceAll('_', ' ')} {item.status ? `(${item.status})` : ''}
          </span>
        </div>
      ))}
    </div>
  );
}

export default function OperationsRoom() {
  const { groupId = 'personal' } = useParams<{ groupId: string }>();
  const navigate = useNavigate();
  const [viewScope, setViewScope] = useState<'group' | 'personal'>('group');
  const [summary, setSummary] = useState<GroupOperationsSummary | null>(null);
  const [personalSummary, setPersonalSummary] = useState<PersonalOperationsSummary | null>(null);
  const [scopes, setScopes] = useState<CortexAuthorityScope[]>([]);
  const [approvals, setApprovals] = useState<CortexApprovalRequest[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [focusedTaskId, setFocusedTaskId] = useState<string | null>(null);
  const [authorityFilter, setAuthorityFilter] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const [next, scopeResponse, approvalList, personal] = await Promise.all([
        getGroupOperationsSummary(groupId),
        getAuthorityScopes().catch(() => ({ scopes: [] })),
        listGroupApprovals(groupId).catch(() => []),
        getPersonalOperationsSummary().catch(() => null),
      ]);
      setSummary(next);
      setPersonalSummary(personal);
      setScopes(scopeResponse.scopes);
      setApprovals(approvalList);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Operations data unavailable');
    } finally {
      setLoading(false);
    }
  }, [groupId]);

  useEffect(() => {
    setLoading(true);
    refresh();
    const interval = window.setInterval(refresh, 15_000);
    return () => window.clearInterval(interval);
  }, [refresh]);

  const activeSummary = viewScope === 'personal' && personalSummary
    ? {
        tasks: personalSummary.tasks,
        runs: { ...personalSummary.runs, latest_run_id: undefined as string | null | undefined },
        steps: personalSummary.steps,
        approvals: personalSummary.approvals,
        resource_leases: personalSummary.resource_leases,
        attention: personalSummary.attention,
        recent_events: personalSummary.recent_events,
      }
    : summary;
  const attentionItems = useMemo(() => activeSummary?.attention ?? [], [activeSummary]);
  const failedCount = (activeSummary?.runs.failed ?? 0) + (activeSummary?.steps.failed ?? 0);
  const pendingApprovals = approvals.filter((a) => a.status === 'pending');
  const recentApprovals = approvals.filter((a) => a.status !== 'pending').slice(0, 10);

  return (
    <div className="flex h-screen flex-col bg-[var(--bg)] text-white">
      <div className="shrink-0 border-b border-white/8 px-6 pt-3 pb-2">
        <Link
          to={`/app/groups/${groupId}/tasks`}
          className="inline-flex items-center gap-1 text-sm text-zinc-400 transition hover:text-zinc-200"
        >
          &larr; Back to Tasks
        </Link>
      </div>
      <header className="flex h-14 shrink-0 items-center justify-between border-b border-white/8 px-6">
        <div className="flex items-center gap-4">
          <button
            type="button"
            onClick={() => navigate(`/app/groups/${groupId}/tasks`)}
            className="inline-flex h-8 w-8 items-center justify-center rounded-lg border border-white/8 bg-white/[0.03] text-[var(--muted)] transition hover:text-white"
          >
            <ArrowLeft className="h-4 w-4" />
          </button>
          <div>
            <div className="flex items-center gap-3">
              <h1 className="text-sm font-semibold text-white">{viewScope === 'personal' ? 'Personal' : groupLabel(groupId)} Operations Room</h1>
              <div className="flex gap-0.5 rounded-md border border-white/8 bg-white/[0.03] p-0.5">
                <button
                  type="button"
                  onClick={() => setViewScope('group')}
                  className={`inline-flex items-center gap-1 rounded px-2 py-1 text-[10px] font-medium transition ${
                    viewScope === 'group' ? 'bg-white/10 text-white' : 'text-[var(--muted)] hover:text-white'
                  }`}
                >
                  <GitBranch className="h-3 w-3" />
                  Group
                </button>
                <button
                  type="button"
                  onClick={() => setViewScope('personal')}
                  className={`inline-flex items-center gap-1 rounded px-2 py-1 text-[10px] font-medium transition ${
                    viewScope === 'personal' ? 'bg-white/10 text-white' : 'text-[var(--muted)] hover:text-white'
                  }`}
                >
                  <User className="h-3 w-3" />
                  Personal
                </button>
              </div>
            </div>
            <p className="text-[11px] text-[var(--muted)]">
              {loading ? 'Loading...' : viewScope === 'personal' && personalSummary
                ? `${personalSummary.tasks.open + personalSummary.tasks.active} open tasks · ${personalSummary.runs.active} active runs across ${personalSummary.groups_total} groups`
                : summary
                  ? `${summary.tasks.open + summary.tasks.active} open tasks · ${summary.runs.active} active runs · refreshed ${new Date().toLocaleTimeString()}`
                  : 'Waiting for data'}
            </p>
          </div>
        </div>
        <button
          type="button"
          onClick={() => void refresh()}
          className="inline-flex h-8 w-8 items-center justify-center rounded-lg border border-white/8 bg-white/[0.03] text-[var(--muted)] transition hover:text-white"
          aria-label="Refresh"
        >
          <RefreshCw className={`h-3.5 w-3.5 ${loading ? 'animate-spin' : ''}`} />
        </button>
      </header>

      {error && (
        <div className="mx-6 mt-4 flex items-center gap-2 rounded-lg border border-amber-300/20 bg-amber-300/10 px-4 py-2.5 text-xs text-amber-100">
          <AlertTriangle className="h-4 w-4 shrink-0" />
          <span>{error}</span>
        </div>
      )}

      <div className="flex-1 overflow-y-auto px-6 py-5">
        {loading && !summary ? (
          <div className="flex h-64 items-center justify-center text-[var(--muted)]">
            <Loader2 className="mr-2 h-5 w-5 animate-spin" />
            Loading operations data
          </div>
        ) : (
          <div className="space-y-6">
            {activeSummary && (
              <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-5">
                <SummaryCard label="Open Tasks" value={activeSummary.tasks.open} detail={`${activeSummary.tasks.active} active`} />
                <SummaryCard label="Active Runs" value={activeSummary.runs.active} detail={`${activeSummary.runs.total} total`} />
                <SummaryCard
                  label="Completion"
                  value={activeSummary.tasks.completion.gated_done ?? activeSummary.tasks.done_raw}
                  detail={activeSummary.tasks.completion.gated_done_available
                    ? `${activeSummary.tasks.completion.done_without_evidence ?? 0} need evidence`
                    : `${activeSummary.tasks.done_raw} raw done`}
                />
                <SummaryCard
                  label="Attention"
                  value={attentionItems.length}
                  tone={attentionItems.length > 0 ? 'warning' : 'normal'}
                  detail={`${activeSummary.approvals?.pending ?? 0} pending approvals`}
                />
                <SummaryCard
                  label="Failures"
                  value={failedCount}
                  tone={failedCount > 0 ? 'danger' : 'normal'}
                  detail={`${activeSummary.runs.failed} runs · ${activeSummary.steps.failed} steps`}
                />
              </div>
            )}

            {viewScope === 'personal' && personalSummary && (
              <div>
                <h2 className="mb-3 text-sm font-semibold text-white">Aggregated Group Stats</h2>
                <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
                  {personalSummary.groups.map((g) => (
                    <div key={g.group_id} className="rounded-lg border border-white/8 bg-white/[0.03] px-4 py-3">
                      <div className="flex items-center gap-2">
                        <div className="h-2.5 w-2.5 rounded-full" style={{ backgroundColor: g.accent }} />
                        <p className="text-sm font-medium text-white">{g.name}</p>
                      </div>
                      <div className="mt-2 grid grid-cols-3 gap-2 text-center text-[10px]">
                        <div>
                          <p className="text-sm font-semibold text-white">{g.tasks.open}</p>
                          <p className="uppercase text-[var(--muted)]">Open</p>
                        </div>
                        <div>
                          <p className="text-sm font-semibold text-emerald-300">{g.runs.active}</p>
                          <p className="uppercase text-[var(--muted)]">Active</p>
                        </div>
                        <div>
                          <p className="text-sm font-semibold text-blue-300">{g.tasks.completion.gated_done ?? g.tasks.done_raw}</p>
                          <p className="uppercase text-[var(--muted)]">Done</p>
                        </div>
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            )}

            <div>
              <h2 className="mb-3 text-sm font-semibold text-white">Live Map</h2>
              <div className="overflow-x-auto">
                <OperationsGraphPanel
                  groupId={groupId}
                  groupName={groupLabel(groupId)}
                  focusedTaskId={focusedTaskId}
                  onFocusTask={setFocusedTaskId}
                />
              </div>
            </div>

            <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_340px]">
              <div>
                <h2 className="mb-3 text-sm font-semibold text-white">Event Timeline</h2>
                <EventTimeline
                  events={activeSummary?.recent_events ?? []}
                  authorityOnly={authorityFilter}
                  onToggleAuthority={() => setAuthorityFilter((v) => !v)}
                />
              </div>
              <div>
                <h2 className="mb-3 text-sm font-semibold text-white">
                  Attention ({attentionItems.length})
                </h2>
                {attentionItems.length > 0 ? (
                  <AttentionList items={attentionItems} />
                ) : (
                  <div className="flex h-32 items-center justify-center rounded-lg border border-white/8 bg-black/15 text-sm text-[var(--muted)]">
                    No items need attention
                  </div>
                )}

                {activeSummary && (activeSummary.resource_leases?.active ?? 0) > 0 && (
                  <div className="mt-4">
                    <h3 className="mb-2 text-xs font-semibold uppercase text-[var(--muted)]">Resource Leases</h3>
                    <div className="grid grid-cols-2 gap-2">
                      <div className="rounded-md border border-white/8 bg-black/10 px-3 py-2 text-center">
                        <p className="text-lg font-semibold text-white">{activeSummary.resource_leases?.by_type.path ?? 0}</p>
                        <p className="text-[9px] uppercase text-[var(--muted)]">Path Locks</p>
                      </div>
                      <div className="rounded-md border border-white/8 bg-black/10 px-3 py-2 text-center">
                        <p className="text-lg font-semibold text-white">{activeSummary.resource_leases?.by_type.task ?? 0}</p>
                        <p className="text-[9px] uppercase text-[var(--muted)]">Task Locks</p>
                      </div>
                    </div>
                  </div>
                )}
              </div>
            </div>

            {/* Authority Scopes Management Panel */}
            <AuthorityScopesPanel />

            {/* Authority & Approvals */}
            <div className="grid gap-6 lg:grid-cols-2">
              {scopes.length > 0 && (
                <div>
                  <h2 className="mb-3 text-sm font-semibold text-white">Authority Scopes</h2>
                  <div className="space-y-2">
                    {scopes.map((scope) => (
                      <div key={scope.id} className="rounded-lg border border-white/8 bg-white/[0.03] px-4 py-3">
                        <div className="flex items-center justify-between gap-3">
                          <div className="min-w-0">
                            <p className="truncate text-sm font-medium text-white">{scope.name}</p>
                            <p className="truncate text-[11px] text-[var(--muted)]">
                              {scope.kind} · {scope.role} · {scope.resources.length} resource{scope.resources.length === 1 ? '' : 's'}
                            </p>
                          </div>
                          <span className={`shrink-0 rounded-full border px-2 py-0.5 text-[10px] ${
                            scope.status === 'active' ? 'border-emerald-300/20 bg-emerald-400/10 text-emerald-100' : 'border-white/8 bg-white/[0.04] text-[var(--muted)]'
                          }`}>
                            {scope.status}
                          </span>
                        </div>
                        {scope.resources.length > 0 && (
                          <div className="mt-2 flex flex-wrap gap-1">
                            {scope.resources.slice(0, 5).map((resource) => (
                              <span key={resource.id} className="rounded-md border border-white/6 bg-black/10 px-2 py-0.5 text-[10px] text-[var(--muted)]">
                                {resource.resource_type}: {resource.resource_key.length > 30 ? `${resource.resource_key.slice(0, 27)}...` : resource.resource_key}
                                <span className="ml-1 text-[var(--muted-strong)]">{resource.access}</span>
                              </span>
                            ))}
                            {scope.resources.length > 5 && (
                              <span className="rounded-md border border-white/6 bg-black/10 px-2 py-0.5 text-[10px] text-[var(--muted)]">
                                +{scope.resources.length - 5} more
                              </span>
                            )}
                          </div>
                        )}
                      </div>
                    ))}
                  </div>
                </div>
              )}

              <div>
                <h2 className="mb-3 text-sm font-semibold text-white">
                  Approval Audit Trail ({approvals.length})
                </h2>
                {pendingApprovals.length > 0 && (
                  <div className="mb-3 space-y-1.5">
                    <p className="text-[10px] uppercase tracking-wider text-amber-200">Pending</p>
                    {pendingApprovals.map((approval) => (
                      <div key={approval.id} className="rounded-lg border border-amber-300/20 bg-amber-300/[0.06] px-3 py-2">
                        <p className="truncate text-xs font-medium text-white">{approval.title}</p>
                        <p className="mt-0.5 text-[10px] text-[var(--muted)]">
                          {approval.requested_by ?? 'system'} · {new Date(approval.created_at).toLocaleString()}
                        </p>
                      </div>
                    ))}
                  </div>
                )}
                {recentApprovals.length > 0 ? (
                  <div className="space-y-1.5">
                    <p className="text-[10px] uppercase tracking-wider text-[var(--muted)]">Recent decisions</p>
                    {recentApprovals.map((approval) => (
                      <div key={approval.id} className="flex items-center gap-3 rounded-lg border border-white/6 bg-white/[0.02] px-3 py-2">
                        <div className="shrink-0">
                          {approval.status === 'approved' ? (
                            <CheckCircle2 className="h-3.5 w-3.5 text-emerald-300" />
                          ) : approval.status === 'rejected' ? (
                            <XCircle className="h-3.5 w-3.5 text-red-300" />
                          ) : (
                            <Clock className="h-3.5 w-3.5 text-[var(--muted)]" />
                          )}
                        </div>
                        <div className="min-w-0 flex-1">
                          <p className="truncate text-xs font-medium text-white">{approval.title}</p>
                          <p className="text-[10px] text-[var(--muted)]">
                            {approval.status} · {new Date(approval.updated_at ?? approval.created_at).toLocaleString()}
                          </p>
                        </div>
                      </div>
                    ))}
                  </div>
                ) : pendingApprovals.length === 0 ? (
                  <div className="flex h-32 items-center justify-center rounded-lg border border-white/8 bg-black/15 text-sm text-[var(--muted)]">
                    No approval activity
                  </div>
                ) : null}
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
