import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  AlertTriangle,
  CheckCircle2,
  CircleDot,
  GitBranch,
  Loader2,
  MessageSquareText,
  RefreshCw,
  Search,
  ShieldCheck,
  ZoomIn,
  ZoomOut,
} from 'lucide-react';
import {
  getGroupOperationsGraph,
  type GroupOperationsGraph,
  type OperationsGraphNode,
} from '../../lib/cortexApi';

interface OperationsGraphPanelProps {
  groupId: string;
  groupName: string;
  focusedTaskId?: string | null;
  onFocusTask?: (taskId: string) => void;
}

type Point = { x: number; y: number };

const GRAPH_WIDTH = 900;
const NODE_WIDTH = 148;
const NODE_HEIGHT = 48;
const COLUMN_X: Record<string, number> = {
  task: 28,
  chat: 204,
  run: 380,
  step: 556,
  attempt: 644,
  evidence: 732,
  approval: 732,
  resource_lease: 732,
};

const NODE_ORDER = ['task', 'chat', 'run', 'step', 'attempt', 'evidence', 'approval', 'resource_lease'];

function useGroupOperationsGraph(groupId: string) {
  const [graph, setGraph] = useState<GroupOperationsGraph | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const next = await getGroupOperationsGraph(groupId);
      setGraph(next);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Operations graph unavailable');
    } finally {
      setLoading(false);
    }
  }, [groupId]);

  useEffect(() => {
    setLoading(true);
    refresh();
    const interval = window.setInterval(refresh, 12_000);
    return () => window.clearInterval(interval);
  }, [refresh]);

  return { graph, loading, error, refresh };
}

function nodeRank(node: OperationsGraphNode) {
  const index = NODE_ORDER.indexOf(node.type);
  return index >= 0 ? index : NODE_ORDER.length;
}

function nodeTone(node: OperationsGraphNode) {
  const status = typeof node.status === 'string' ? node.status : '';
  if (node.lease_stale || ['failed', 'orphaned', 'cancelled', 'rejected'].includes(status)) {
    return 'border-rose-300/30 bg-rose-400/10 text-rose-50';
  }
  if (['running', 'leased', 'pending', 'in-progress'].includes(status)) {
    return 'border-emerald-300/30 bg-emerald-400/10 text-emerald-50';
  }
  if (node.type === 'approval' && status === 'pending') {
    return 'border-sky-300/30 bg-sky-400/10 text-sky-50';
  }
  if (node.type === 'attempt') {
    return 'border-violet-300/30 bg-violet-400/10 text-violet-50';
  }
  if (node.type === 'evidence' || status === 'verified' || status === 'manual_override') {
    return 'border-blue-300/30 bg-blue-400/10 text-blue-50';
  }
  return 'border-white/10 bg-white/[0.04] text-white';
}

function nodeIcon(node: OperationsGraphNode) {
  if (node.type === 'chat') return MessageSquareText;
  if (node.type === 'run') return GitBranch;
  if (node.type === 'attempt') return RefreshCw;
  if (node.type === 'evidence') return ShieldCheck;
  if (node.type === 'approval') return CheckCircle2;
  if (node.type === 'resource_lease') return CircleDot;
  return CircleDot;
}

function shortId(value?: string | null) {
  if (!value) return null;
  if (value.length <= 12) return value;
  return `${value.slice(0, 6)}…${value.slice(-4)}`;
}

function nodeLabel(node: OperationsGraphNode) {
  const label = typeof node.label === 'string' && node.label.trim() ? node.label : node.entity_id;
  return label.length > 42 ? `${label.slice(0, 39)}...` : label;
}

function buildLayout(graph: GroupOperationsGraph | null) {
  const nodes = [...(graph?.nodes ?? [])].sort((left, right) => {
    const rank = nodeRank(left) - nodeRank(right);
    if (rank !== 0) return rank;
    return left.id.localeCompare(right.id);
  });
  const laneCounts = new Map<string, number>();
  const positions = new Map<string, Point>();

  for (const node of nodes) {
    const column = COLUMN_X[node.type] ?? 732;
    const index = laneCounts.get(node.type) ?? 0;
    laneCounts.set(node.type, index + 1);
    positions.set(node.id, {
      x: column,
      y: 34 + index * 72,
    });
  }

  const maxLane = Math.max(3, ...laneCounts.values());
  return {
    nodes,
    positions,
    height: maxLane * 72 + 64,
  };
}

function isBlockedOrStale(node: OperationsGraphNode) {
  return node.status === 'blocked' || node.lease_stale === true;
}

function GraphNodeButton({
  node,
  position,
  selected,
  onSelect,
}: {
  node: OperationsGraphNode;
  position: Point;
  selected: boolean;
  onSelect: (node: OperationsGraphNode) => void;
}) {
  const Icon = nodeIcon(node);
  const blocked = isBlockedOrStale(node);
  return (
    <button
      type="button"
      onClick={() => onSelect(node)}
      className={[
        'absolute flex h-12 w-[148px] items-center gap-2 rounded-lg border px-2 text-left shadow-[0_8px_24px_rgba(0,0,0,0.18)] transition hover:brightness-110',
        nodeTone(node),
        selected ? 'ring-2 ring-[var(--accent)]/60' : '',
        blocked ? 'animate-pulse ring-2 ring-amber-400/50' : '',
      ].join(' ')}
      style={{
        left: position.x,
        top: position.y,
      }}
    >
      {/* eslint-disable-next-line react-hooks/static-components -- `nodeIcon`
          returns one of the lucide components imported at module scope. It
          creates nothing, so the identity is stable across renders and the
          state reset this rule guards against cannot happen. */}
      <Icon className="h-4 w-4 shrink-0" />
      <span className="min-w-0">
        <span className="block truncate text-[11px] font-semibold capitalize">{node.type.replaceAll('_', ' ')}</span>
        <span className="block truncate text-[10px] opacity-75">{nodeLabel(node)}</span>
      </span>
    </button>
  );
}

function DetailRow({ label, value }: { label: string; value: unknown }) {
  if (value === null || value === undefined || value === '') return null;
  return (
    <div className="grid grid-cols-[84px_minmax(0,1fr)] gap-2 text-xs">
      <span className="text-[var(--muted)]">{label}</span>
      <span className="min-w-0 truncate text-[var(--muted-strong)]">{String(value)}</span>
    </div>
  );
}

export default function OperationsGraphPanel({
  groupId,
  groupName,
  focusedTaskId,
  onFocusTask,
}: OperationsGraphPanelProps) {
  const { graph, loading, error, refresh } = useGroupOperationsGraph(groupId);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [zoom, setZoom] = useState(0.82);
  const layout = useMemo(() => buildLayout(graph), [graph]);
  const selectedNode =
    layout.nodes.find((node) => node.id === selectedNodeId) ?? layout.nodes[0] ?? null;
  const selectedTaskId = typeof selectedNode?.task_id === 'string'
    ? selectedNode.task_id
    : selectedNode?.type === 'task'
      ? selectedNode.entity_id
      : null;

  useEffect(() => {
    if (selectedNodeId && layout.nodes.some((node) => node.id === selectedNodeId)) return;
    setSelectedNodeId(layout.nodes[0]?.id ?? null);
  }, [layout.nodes, selectedNodeId]);

  const conflictEdgeIds = useMemo(() => {
    if (!graph) return new Set<string>();
    const ids = new Set<string>();
    const nodeMap = new Map(layout.nodes.map((n) => [n.id, n]));
    for (const edge of graph.edges) {
      const fromNode = nodeMap.get(edge.from);
      const toNode = nodeMap.get(edge.to);
      if (!fromNode || !toNode) continue;
      const shareStep = fromNode.step_id && toNode.step_id && fromNode.step_id === toNode.step_id && fromNode.id !== toNode.id;
      const shareTask = fromNode.task_id && toNode.task_id && fromNode.task_id === toNode.task_id && fromNode.type === toNode.type && fromNode.id !== toNode.id;
      if (shareStep || shareTask) {
        ids.add(edge.id);
      }
    }
    return ids;
  }, [graph, layout.nodes]);

  const counts = useMemo(() => {
    const byType = new Map<string, number>();
    for (const node of layout.nodes) {
      byType.set(node.type, (byType.get(node.type) ?? 0) + 1);
    }
    return byType;
  }, [layout.nodes]);

  const selectedEvents = useMemo(() => {
    if (!selectedNode || !graph) return [];
    return graph.recent_events
      .filter((event) => (
        event.entity_id === selectedNode.entity_id
        || event.task_id === selectedNode.task_id
        || event.run_id === selectedNode.run_id
        || event.step_id === selectedNode.step_id
      ))
      .slice(0, 4);
  }, [graph, selectedNode]);

  const handleSelectNode = useCallback((node: OperationsGraphNode) => {
    setSelectedNodeId(node.id);
    const taskId = typeof node.task_id === 'string'
      ? node.task_id
      : node.type === 'task'
        ? node.entity_id
        : null;
    if (taskId) onFocusTask?.(taskId);
  }, [onFocusTask]);

  return (
    <section className="rounded-xl border border-white/8 bg-white/[0.025] p-3">
      <div className="mb-3 flex items-center justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <GitBranch className="h-4 w-4 text-[var(--accent)]" />
            <h3 className="truncate text-sm font-semibold text-white">{groupName} live map</h3>
          </div>
          <p className="mt-0.5 text-xs text-[var(--muted)]">
            {graph
              ? `${layout.nodes.length} nodes · ${graph.edges.length} links · refreshed ${new Date(graph.generated_at).toLocaleTimeString()}`
              : 'Loading graph projection'}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          <button
            type="button"
            onClick={() => setZoom((value) => Math.max(0.56, Number((value - 0.08).toFixed(2))))}
            className="inline-flex h-8 w-8 items-center justify-center rounded-lg border border-white/8 bg-white/[0.03] text-[var(--muted)] transition hover:text-white"
            aria-label="Zoom out"
          >
            <ZoomOut className="h-3.5 w-3.5" />
          </button>
          <button
            type="button"
            onClick={() => setZoom((value) => Math.min(1.12, Number((value + 0.08).toFixed(2))))}
            className="inline-flex h-8 w-8 items-center justify-center rounded-lg border border-white/8 bg-white/[0.03] text-[var(--muted)] transition hover:text-white"
            aria-label="Zoom in"
          >
            <ZoomIn className="h-3.5 w-3.5" />
          </button>
          <button
            type="button"
            onClick={() => void refresh()}
            className="inline-flex h-8 w-8 items-center justify-center rounded-lg border border-white/8 bg-white/[0.03] text-[var(--muted)] transition hover:text-white"
            aria-label="Refresh live map"
          >
            <RefreshCw className={`h-3.5 w-3.5 ${loading ? 'animate-spin' : ''}`} />
          </button>
        </div>
      </div>

      {error && (
        <div className="mb-3 flex items-center gap-2 rounded-lg border border-amber-300/20 bg-amber-300/10 px-3 py-2 text-xs text-amber-100">
          <AlertTriangle className="h-4 w-4 shrink-0" />
          <span className="min-w-0 truncate">{error}</span>
        </div>
      )}

      {loading && !graph ? (
        <div className="flex h-48 items-center justify-center rounded-lg border border-white/8 bg-black/15 text-sm text-[var(--muted)]">
          <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          Loading live map
        </div>
      ) : layout.nodes.length === 0 ? (
        <div className="flex h-48 items-center justify-center rounded-lg border border-white/8 bg-black/15 text-sm text-[var(--muted)]">
          No operations graph nodes yet
        </div>
      ) : (
        <div className="grid gap-3 xl:grid-cols-[minmax(0,1fr)_220px]">
          <div className="min-w-0 overflow-auto rounded-lg border border-white/8 bg-black/15">
            <div
              className="relative origin-top-left"
              style={{
                width: GRAPH_WIDTH * zoom,
                height: layout.height * zoom,
              }}
            >
              <div
                className="relative"
                style={{
                  width: GRAPH_WIDTH,
                  height: layout.height,
                  transform: `scale(${zoom})`,
                  transformOrigin: 'top left',
                }}
              >
                <svg
                  className="absolute inset-0"
                  width={GRAPH_WIDTH}
                  height={layout.height}
                  viewBox={`0 0 ${GRAPH_WIDTH} ${layout.height}`}
                  aria-hidden="true"
                >
                  {graph?.edges.map((edge) => {
                    const from = layout.positions.get(edge.from);
                    const to = layout.positions.get(edge.to);
                    if (!from || !to) return null;
                    const startX = from.x + NODE_WIDTH;
                    const startY = from.y + NODE_HEIGHT / 2;
                    const endX = to.x;
                    const endY = to.y + NODE_HEIGHT / 2;
                    const mid = Math.max(34, (endX - startX) / 2);
                    const isConflict = conflictEdgeIds.has(edge.id);
                    return (
                      <path
                        key={edge.id}
                        d={`M ${startX} ${startY} C ${startX + mid} ${startY}, ${endX - mid} ${endY}, ${endX} ${endY}`}
                        fill="none"
                        stroke={isConflict ? 'rgba(251,191,36,0.7)' : 'rgba(148,163,184,0.34)'}
                        strokeWidth={isConflict ? 2.5 : 1.5}
                        strokeDasharray={isConflict ? '6 3' : undefined}
                      />
                    );
                  })}
                </svg>
                {layout.nodes.map((node) => {
                  const position = layout.positions.get(node.id);
                  if (!position) return null;
                  return (
                    <GraphNodeButton
                      key={node.id}
                      node={node}
                      position={position}
                      selected={node.id === selectedNode?.id}
                      onSelect={handleSelectNode}
                    />
                  );
                })}
              </div>
            </div>
          </div>

          <aside className="rounded-lg border border-white/8 bg-white/[0.03] p-3">
            <div className="mb-3 grid grid-cols-3 gap-2">
              <div className="rounded-md border border-white/8 bg-black/15 px-2 py-1.5 text-center">
                <div className="text-sm font-semibold text-white">{counts.get('run') ?? 0}</div>
                <div className="text-[9px] uppercase text-[var(--muted)]">Runs</div>
              </div>
              <div className="rounded-md border border-white/8 bg-black/15 px-2 py-1.5 text-center">
                <div className="text-sm font-semibold text-white">{counts.get('step') ?? 0}</div>
                <div className="text-[9px] uppercase text-[var(--muted)]">Steps</div>
              </div>
              <div className="rounded-md border border-white/8 bg-black/15 px-2 py-1.5 text-center">
                <div className="text-sm font-semibold text-white">{counts.get('evidence') ?? 0}</div>
                <div className="text-[9px] uppercase text-[var(--muted)]">Proof</div>
              </div>
            </div>

            {selectedNode && (
              <div className="space-y-2">
                <div>
                  <div className="text-[11px] font-semibold uppercase text-[var(--muted)]">
                    {selectedNode.type.replaceAll('_', ' ')}
                  </div>
                  <div className="mt-1 line-clamp-2 text-sm font-semibold text-white">
                    {nodeLabel(selectedNode)}
                  </div>
                </div>
                <DetailRow label="Status" value={selectedNode.status} />
                <DetailRow label="Task" value={shortId(selectedNode.task_id)} />
                <DetailRow label="Run" value={shortId(selectedNode.run_id)} />
                <DetailRow label="Step" value={shortId(selectedNode.step_id)} />
                <DetailRow label="Risk" value={selectedNode.risk} />
                <DetailRow label="Worker" value={selectedNode.assigned_worker} />
                <DetailRow label="Verify" value={selectedNode.verification_status ?? selectedNode.verdict} />

                {selectedNode.lease_stale && (
                  <div className="flex items-center gap-2 rounded-md border border-amber-300/20 bg-amber-300/[0.08] px-2 py-1.5">
                    <AlertTriangle className="h-3.5 w-3.5 shrink-0 text-amber-200" />
                    <span className="text-[11px] font-medium text-amber-100">Stale lease</span>
                  </div>
                )}

                {selectedTaskId && (
                  <button
                    type="button"
                    onClick={() => onFocusTask?.(selectedTaskId)}
                    className={[
                      'inline-flex h-8 w-full items-center justify-center gap-2 rounded-md border px-2 text-xs font-medium transition active:scale-[0.99]',
                      selectedTaskId === focusedTaskId
                        ? 'border-[var(--accent)]/30 bg-[var(--accent)]/15 text-[var(--accent)]'
                        : 'border-white/8 bg-white/[0.04] text-[var(--muted-strong)] hover:bg-white/[0.08] hover:text-white',
                    ].join(' ')}
                  >
                    <Search className="h-3.5 w-3.5" />
                    {selectedTaskId === focusedTaskId ? 'Focused in board' : 'Focus task in board'}
                  </button>
                )}

                <div className="pt-2">
                  <div className="mb-2 text-[11px] font-semibold uppercase text-[var(--muted)]">Recent events</div>
                  <div className="space-y-1.5">
                    {selectedEvents.length === 0 ? (
                      <div className="rounded-md border border-white/8 bg-black/10 px-2 py-2 text-xs text-[var(--muted)]">
                        No matching events in the current window
                      </div>
                    ) : (
                      selectedEvents.map((event) => (
                        <div key={event.id} className="rounded-md border border-white/8 bg-black/10 px-2 py-1.5">
                          <div className="truncate text-[11px] font-medium text-white">{event.event_type}</div>
                          <div className="text-[10px] text-[var(--muted)]">
                            {new Date(event.created_at).toLocaleTimeString()}
                          </div>
                        </div>
                      ))
                    )}
                  </div>
                </div>
              </div>
            )}
          </aside>
        </div>
      )}

      {layout.nodes.length > 0 && (
        <div className="mt-3 flex flex-wrap items-center gap-4 border-t border-white/6 pt-3 text-[10px]">
          <div className="flex items-center gap-1.5">
            <span className="inline-block h-2.5 w-2.5 rounded-full bg-emerald-400" />
            <span className="text-[var(--muted)]">Active</span>
          </div>
          <div className="flex items-center gap-1.5">
            <span className="inline-block h-2.5 w-2.5 rounded-full bg-blue-400" />
            <span className="text-[var(--muted)]">Verified</span>
          </div>
          <div className="flex items-center gap-1.5">
            <span className="inline-block h-2.5 w-2.5 rounded-full bg-rose-400" />
            <span className="text-[var(--muted)]">Failed</span>
          </div>
          <div className="flex items-center gap-1.5">
            <span className="inline-block h-2.5 w-2.5 rounded-full bg-amber-400 animate-pulse" />
            <span className="text-[var(--muted)]">Blocked / Stale</span>
          </div>
        </div>
      )}
    </section>
  );
}
