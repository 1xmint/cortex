# Live Orchestration Mapping View

Status: proposal

## Title

Cortex Live Orchestration Mapping View

## Problem

Cortex can orchestrate work across conversations, runs, steps, workers, providers, repos, and groups, but a chat transcript or flat task list does not show the live system shape. Users need to answer these questions in seconds:

- What is active right now?
- Which projects, repos, agents, and people are involved?
- Where is work blocked?
- What changed since I last looked?
- Which session should I inspect next?

The mapping view should make orchestration visible as a live operating surface, not a decorative graph.

## Why Now

The product vision depends on users trusting Cortex as the orchestration brain. Live visibility is a differentiator because most agent products hide coordination behind logs or vague status text. Cortex should expose enough real-time structure that a developer can understand the whole workflow at a glance and drill into the exact session, step, artifact, or decision when needed.

## Broad Idea

Build an `OrchestrationMap` surface that can render the same orchestration state through four coordinated layouts:

- `Network`: spatial graph of groups, projects, repos, sessions, workers, tasks, and dependencies.
- `Timeline`: left-to-right flow of orchestration over time.
- `Board`: status columns for active operational control.
- `Tree`: hierarchical breakdown from group to project to run to step.

All layouts share one normalized mapping store and one live event stream. Changing filters, time range, selected node, or group changes every layout consistently.

## What 10/10 Looks Like

A developer opens Cortex and immediately sees the live topology of their work:

- active groups are visible without hunting through navigation
- busy projects pulse subtly and blocked work is visually louder than successful work
- edges show why things are connected: dependency, assignment, data flow, delegation, artifact, or ownership
- clicking any node opens the current state, recent events, artifacts, decisions, and next action
- the timeline scrubber can replay the last hour/day of orchestration without leaving the screen
- the view remains smooth with hundreds of nodes by clustering inactive nodes and virtualizing side panels
- every visible state is backed by persisted orchestration records or live events, not guessed client state

The target feeling is: "I can see my entire workflow at a glance, and I know where to click."

## Fitness Check

- vision fit: strong; makes Cortex's orchestration brain visible and useful.
- real user/operator need: strong for multi-agent coding, team work, incident response, and long-running runs.
- security exposure: medium; it may reveal repo names, user names, task objectives, artifacts, provider choices, and group topology.
- evidence this is needed now: product direction explicitly requires live orchestration visibility; current dashboard has no map surface.
- keep / reshape / pause / remove: keep as proposal; implement foundation-first with read-only live state before control actions.

## Evidence Ledger

- current status: design-only; no shipped `OrchestrationMap` component exists in the current dashboard.
- upstream dependencies: canonical run/step/worker state, group membership model, authz rules, event stream contract, and artifact/detail APIs.
- missing evidence: expected graph sizes, multi-group permission boundaries, exact live transport strategy, and first user workflow.
- blocks current work: no.
- next gate: approve data contract and MVP slice.
- terminal condition: shipped as read-only live map with docs/tests/rollout, or superseded by a newer dashboard architecture decision.

## Repo Ownership

- protocol truth: Soma only if mapping exposes signed receipts, Heart identity, Pulse Tree state, or delegation semantics.
- platform/runtime truth: `claw-net`/Cortex owns orchestration run, step, worker, event, and dashboard integration.
- product/integration truth: Cortex product docs own user experience, layout behavior, keyboard shortcuts, and information architecture.
- internal-only material: early visual explorations, launch positioning, and unvalidated product claims.

## First Consumer

Cortex dashboard. Initial consumer should be a read-only operator/developer view over existing run, step, worker, decision, outcome, artifact, and provider state.

## Design Principles

1. Show orchestration, not telemetry noise.
2. Make blocked work visually obvious.
3. Preserve context while drilling down.
4. Use motion to signal state change, not to decorate.
5. Treat group boundaries as security boundaries.
6. Keep the map useful when real-time transport is degraded.
7. Prefer stable spatial positions so users build memory of where work lives.

## Information Architecture

Primary screen regions:

```text
+--------------------------------------------------------------------------------+
| Group switcher | Search | Filters: Status Time Type Provider Repo | Share Export |
+----------------+---------------------------------------------------------------+
| Layout rail     | Live canvas                                                   |
| Network         |                                                               |
| Timeline        |      [Group]----owns----[Project]----uses----[Repo]           |
| Board           |          \                  |                 /               |
| Tree            |           \              [Run]                               |
|                 |            \            /  |  \                               |
|                 |         [Worker]--assigned [Step]--depends_on--[Step]         |
|                 |                         |            |                        |
|                 |                     [Decision]    [Artifact]                  |
|                 |                                                               |
|                 | Bottom timeline scrubber: |----past----now----live|           |
+----------------+-------------------------------+-------------------------------+
| Activity feed                                  | Inspector                     |
| 12:04 test step blocked                        | selected node details         |
| 12:03 worker leased build step                 | events, metrics, artifacts    |
| 12:02 run planned 7 steps                      | open session / copy link      |
+--------------------------------------------------------------------------------+
```

Responsive behavior:

- desktop: canvas center, feed bottom-left or collapsible, inspector right
- tablet: canvas center, inspector as drawer, feed collapsible below
- mobile: default to Board or Timeline, canvas supports pinch/drag, inspector becomes full-screen sheet

## Visual Language

### Node Types

| Node | Shape | Primary signal | Opens |
|---|---|---|---|
| group | rounded square cluster | group color and active count | group workload |
| project | hex / solid tile | status, runs, owner | project detail |
| repo | document/repo tile | branch, dirty/clean, PR state | repo activity |
| member | avatar circle | availability, assignment count | member workload |
| worker/agent | ring circle | connected, grace, lost, busy | worker session |
| run | large circle | progress and risk | run detail |
| step | small circle | status and kind | step detail |
| decision | diamond | provider/model/rationale | decision ledger |
| artifact | small square | log, patch, report, json | artifact preview |

### Edge Types

| Edge | Style | Meaning |
|---|---|---|
| owns | muted solid | group/project/repo ownership |
| assigned | bright solid | worker or member assigned to run/step |
| depends_on | directional line | step dependency |
| data_flow | animated dashed line | context, artifact, or result movement |
| delegation | double-line | scoped authority or provider delegation |
| blocked_by | red/orange line | explicit blocker |
| observes | dotted line | watch/subscription relationship |

### Status Semantics

| Status | Visual treatment |
|---|---|
| idle | low-contrast outline, no pulse |
| planning | slow blue/cyan ring pulse |
| working | active ring pulse and subtle edge flow |
| blocked | amber/red halo, thicker edge to blocker |
| done | green completion ring, then fades to quiet |
| failed | red mark, event pinned until acknowledged |
| cancelled | muted slash treatment |
| lost/grace | dashed worker ring with heartbeat age |

Motion should be bounded: active pulses run at low opacity, state transitions animate for 150-250ms, and users can disable motion.

## Layout Modes

### Network

Default for "what is connected to what?" Use React Flow for the MVP because it gives selection, pan/zoom, minimap, keyboard navigation, custom nodes, and edge handles quickly. Move large static layers or historical replay to Canvas/WebGL only if measured graph size requires it.

Network layout rules:

- group clusters are top-level lanes or soft containers
- projects sit near their group and active repos
- runs orbit projects; steps orbit runs or flow left-to-right when expanded
- workers sit near assigned steps and provider capability edges
- blocked nodes pin to the foreground
- inactive completed nodes collapse into count badges after the selected time window

### Timeline

Best for "what happened when?" It shows runs as rows and steps as spans. Dependencies are curved connectors, retries stack vertically, and the scrubber controls the visible event window.

```text
time ->  11:58        12:00        12:02        12:04        live
Run A    plan | search ----- build ---- test X
Run B             plan | review ---- patch ---- done
Worker 1          lease      active      active
Events   o--------o----------o----------o----------o
```

### Board

Best for "what needs attention?" Columns are status buckets: Planning, Ready, Working, Blocked, Review, Done. Cards are runs or steps depending on zoom level. This is the mobile-friendly operational default.

### Tree

Best for "what is inside this orchestration?" Hierarchical view:

```text
Personal
  project: cortex-dashboard
    repo: heyvera
      run: add billing UI
        step: inspect routes
        step: implement page
        step: run tests
Team A
  project: soma-rotation
    run: first-consumer integration
```

## Component Architecture

```text
OrchestrationMap
  OrchestrationMapShell
    MapToolbar
      GroupSwitcher
      MapSearch
      FilterBar
      ShareExportMenu
    LayoutRail
    MapViewport
      NetworkGraphView
        GraphCanvas
        GraphNodeRenderer
        GraphEdgeRenderer
        ClusterLayer
        PresenceLayer
      TimelineGraphView
      StatusBoardView
      TreeBreakdownView
    TimelineScrubber
    ActivityFeed
    NodeInspector
      SummaryPanel
      EventPanel
      MetricsPanel
      ArtifactPanel
      DecisionPanel
    KeyboardShortcutLayer
    LiveConnectionStatus
```

Suggested file structure:

```text
dashboard/src/features/orchestration-map/
  OrchestrationMap.tsx
  components/
    ActivityFeed.tsx
    FilterBar.tsx
    LayoutRail.tsx
    MapToolbar.tsx
    NodeInspector.tsx
    TimelineScrubber.tsx
  graph/
    NetworkGraphView.tsx
    GraphNodeRenderer.tsx
    GraphEdgeRenderer.tsx
    layout.ts
  timeline/
    TimelineGraphView.tsx
  board/
    StatusBoardView.tsx
  tree/
    TreeBreakdownView.tsx
  state/
    orchestrationMapStore.ts
    selectors.ts
    liveEvents.ts
    types.ts
```

## Mapping State Data Structure

The client should normalize state by ID and derive visual nodes/edges through selectors. Visual layout state must stay separate from orchestration truth.

```ts
type OrchestrationNodeKind =
  | 'group'
  | 'project'
  | 'repo'
  | 'member'
  | 'worker'
  | 'run'
  | 'step'
  | 'decision'
  | 'artifact';

type OrchestrationStatus =
  | 'idle'
  | 'planning'
  | 'ready'
  | 'working'
  | 'blocked'
  | 'done'
  | 'failed'
  | 'cancelled'
  | 'lost'
  | 'grace';

type MapNode = {
  id: string;
  kind: OrchestrationNodeKind;
  groupId: string;
  label: string;
  status: OrchestrationStatus;
  risk?: 'low' | 'medium' | 'high';
  updatedAt: number;
  startedAt?: number;
  finishedAt?: number;
  metrics?: {
    progress?: number;
    activeSteps?: number;
    blockedSteps?: number;
    completionRate?: number;
    durationMs?: number;
    tokenEstimate?: number;
  };
  refs: {
    projectId?: string;
    repoId?: string;
    runId?: string;
    stepId?: string;
    workerId?: string;
    decisionId?: string;
    artifactId?: string;
    sessionId?: string;
  };
};

type MapEdgeKind =
  | 'owns'
  | 'assigned'
  | 'depends_on'
  | 'data_flow'
  | 'delegation'
  | 'blocked_by'
  | 'observes';

type MapEdge = {
  id: string;
  kind: MapEdgeKind;
  source: string;
  target: string;
  status: OrchestrationStatus;
  label?: string;
  updatedAt: number;
  weight?: number;
};

type MapEvent = {
  id: string;
  sequence: number;
  timestamp: number;
  groupId: string;
  actorNodeId?: string;
  targetNodeId?: string;
  edgeId?: string;
  type:
    | 'node.created'
    | 'node.updated'
    | 'node.removed'
    | 'edge.created'
    | 'edge.updated'
    | 'edge.removed'
    | 'run.planned'
    | 'step.leased'
    | 'step.started'
    | 'step.completed'
    | 'step.blocked'
    | 'artifact.created'
    | 'decision.recorded'
    | 'worker.heartbeat'
    | 'snapshot.compacted';
  summary: string;
  payload: Record<string, unknown>;
};

type MapViewState = {
  layout: 'network' | 'timeline' | 'board' | 'tree';
  selectedNodeId?: string;
  selectedEdgeId?: string;
  groupFilter: 'all' | string;
  statusFilter: OrchestrationStatus[];
  kindFilter: OrchestrationNodeKind[];
  timeWindow: { from: number; to: number | 'live' };
  searchQuery: string;
  camera: { x: number; y: number; zoom: number };
  expandedClusters: string[];
};
```

Server-backed source entities should remain the existing domain records where possible:

- `runs` map to run nodes
- `steps` map to step nodes and dependency edges
- `workers` and `worker_sessions` map to worker nodes
- `provider_capabilities` enrich worker nodes
- `decisions` map to decision nodes and provider/model detail
- `outcomes` update status, timing, and completion metrics
- `artifacts` map to artifact nodes and data-flow edges

## Real-Time Update Integration Plan

### Transport

Use a dedicated authenticated WebSocket endpoint for live orchestration updates:

```text
GET /api/orchestration-map/snapshot?group_id=...&since=...
WS  /api/orchestration-map/live?group_id=...&cursor=...
```

The initial page load fetches a snapshot, then opens a WebSocket from the returned cursor. If the socket drops, the client reconnects with the last applied sequence. If the gap is too large, the server returns `snapshot_required` and the client refetches.

### Event Contract

Events must be:

- ordered per user/group stream with a monotonic `sequence`
- idempotent by event ID
- scoped by authz before leaving the server
- small enough for frequent updates
- replayable from persisted orchestration state

Example:

```json
{
  "id": "evt_01J...",
  "sequence": 4128,
  "timestamp": 1779441840000,
  "groupId": "grp_personal",
  "type": "step.blocked",
  "actorNodeId": "worker_w1",
  "targetNodeId": "step_s9",
  "summary": "Test step blocked on missing env var",
  "payload": {
    "runId": "run_123",
    "stepId": "step_s9",
    "reason": "MISSING_ENV",
    "blockedBy": ["artifact_log_22"]
  }
}
```

### Client Update Flow

1. Fetch snapshot and hydrate normalized store.
2. Build derived visual graph through selectors.
3. Apply live events transactionally.
4. Mark changed nodes/edges with a short `recentlyChanged` visual flag.
5. Append event to activity feed.
6. Recompute affected metrics only.
7. Preserve selected node and camera unless the selected entity disappears.

### Degraded Mode

If live updates fail, the map remains usable:

- show connection status
- keep the last known snapshot
- poll snapshot every 15-30 seconds
- disable "live" indicator
- preserve all filters and selection

## Interactive Feature Specifications

### Selection And Drilldown

Clicking a node opens the inspector without navigating away. Double-clicking opens the canonical detail route if one exists. The inspector should include:

- current status and age
- owning group/project/repo
- recent events
- upstream/downstream edges
- active assignment
- artifacts and logs
- decision rationale
- links to conversation/session/run detail

### Search

Search should match labels, run goals, repo names, branch names, worker IDs, step objectives, and artifact names. Results are highlighted in the current layout and listed in the inspector drawer.

### Filters

Required filters:

- group: Personal, Team A, Team B, All permitted groups
- status: idle, planning, working, blocked, done, failed, cancelled
- type: group, project, repo, member, worker, run, step, decision, artifact
- time: live, last 15 minutes, last hour, today, custom
- provider/model when available
- repo/project when available

### Timeline Scrubber

The scrubber changes the map from live mode to replay mode. In replay mode:

- the canvas shows state as of the selected timestamp
- activity feed shows events around that timestamp
- live updates continue buffering but do not move the viewport
- "Return to live" reapplies buffered events and jumps to now

### Keyboard Shortcuts

Initial shortcuts:

| Shortcut | Action |
|---|---|
| `1` | Network layout |
| `2` | Timeline layout |
| `3` | Board layout |
| `4` | Tree layout |
| `/` | Focus search |
| `f` | Open filters |
| `g` | Group switcher |
| `b` | Toggle blocked-only |
| `r` | Return to live |
| `Esc` | Clear selection / close drawer |
| `+` / `-` | Zoom in / out |
| arrow keys | Move focus between visible nodes |
| `Enter` | Open focused node inspector |

### Export And Share

MVP export/share:

- copy link with layout, filters, selected node, and time window
- export PNG/SVG of current view
- export JSON event slice for debugging

Do not include hidden groups or unauthorized nodes in shared/exported views.

## Security / Reliability Requirements

- threat model: graph may leak sensitive project topology, repo names, objectives, provider choices, and user activity.
- rollback or recovery: map is read-only in MVP; disabling the feature flag should remove the surface without affecting orchestration.
- auditability: live events should be reproducible from persisted run/step/worker/decision/outcome/artifact state.
- failure modes: socket disconnect, out-of-order events, missing authz check, graph overload, stale node state, replay mismatch, hidden-group leakage.

Security rules:

- authorize snapshot and event streams by group membership before serialization
- never send nodes for groups the viewer cannot access
- avoid prompt/content payloads in graph events; link to authorized detail views instead
- redact secrets in labels and summaries using existing masking helpers
- treat export/share as a fresh authorization boundary

## Performance Plan

MVP target:

- smooth interaction for 250 visible nodes and 500 edges
- cluster or hide completed historical nodes by default
- memoized selectors for visible graph
- debounced layout recalculation
- virtualized activity feed and inspector lists
- progressive detail loading for artifacts/logs

Scaling path:

1. React Flow SVG/HTML nodes for MVP.
2. Canvas layer for high-volume edges or historical event replay.
3. Server-side graph compaction for large group/cross-group views.
4. Web worker layout computation if local graph layout becomes expensive.

## Delivery Shape

Smallest safe slices:

1. Static mock data route and read-only dashboard component with Network layout.
2. Normalized client store and selectors from real snapshot API.
3. Node inspector for run, step, worker, decision, and artifact nodes.
4. WebSocket live updates with reconnect and cursor handling.
5. Activity feed and changed-node animations.
6. Filters, search, and group switcher.
7. Timeline scrubber replay.
8. Board and Tree layouts.
9. Export/share.
10. Large-graph performance hardening.

MVP should deliberately avoid write/control actions such as cancel, reassign, retry, or approve. Add controls only after the read-only state model is stable.

## ADR Needed?

- yes.
- if adopted, write an ADR for the dashboard graph/event architecture because it introduces a durable live event contract, shared map state model, and likely a new frontend dependency such as React Flow.

## Open Questions

- What is the canonical group model in the current Cortex implementation?
- Is the first version operator-only, team-visible, or user-visible?
- Which exact records define a "project" before repos/runs exist?
- Should WebSocket events come directly from DB mutations or from an orchestration event log table?
- What is the maximum expected graph size per group and cross-group?
- Which node labels are safe to reveal in shared links?
- Should timeline replay be exact historical reconstruction or approximate event playback?

## Links

- related docs: `docs/ARCHITECTURE.md`
- related docs: `internal/vision/cortex.md`
- related docs: `internal/specs/heyvera-v1-spec.md`
