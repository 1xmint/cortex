import type { CortexGroup } from '../groups';

const WINDOW_STATE_KEY = 'cortex:window-state';
const WINDOW_CHANNEL = 'cortex-window-manager';

export interface CortexWindowState {
  panel: 'task-manager' | 'work-surface' | 'settings';
  groupId: string;
  left: number;
  top: number;
  width: number;
  height: number;
  updatedAt: string;
}

function readWindowStates(): Record<string, CortexWindowState> {
  try {
    const raw = window.localStorage.getItem(WINDOW_STATE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed)
      ? parsed as Record<string, CortexWindowState>
      : {};
  } catch {
    return {};
  }
}

function writeWindowStates(states: Record<string, CortexWindowState>) {
  try {
    window.localStorage.setItem(WINDOW_STATE_KEY, JSON.stringify(states));
  } catch {
    // Detached window state is a convenience feature.
  }
}

function publishWindowEvent(payload: unknown) {
  if (typeof BroadcastChannel === 'undefined') return;
  const channel = new BroadcastChannel(WINDOW_CHANNEL);
  channel.postMessage(payload);
  channel.close();
}

export function persistDetachedWindowState(id: string, state: CortexWindowState) {
  writeWindowStates({ ...readWindowStates(), [id]: state });
  publishWindowEvent({ type: 'window-state', id, state });
}

export function readDetachedWindowState(id: string) {
  return readWindowStates()[id] ?? null;
}

export function openDetachedPanel(panel: CortexWindowState['panel'], group: CortexGroup) {
  const id = `${panel}:${group.id}`;
  const saved = readDetachedWindowState(id);
  const screenWithOffsets = window.screen as Screen & { availLeft?: number; availTop?: number };
  const availLeft = screenWithOffsets.availLeft ?? 0;
  const availTop = screenWithOffsets.availTop ?? 0;
  const width = saved?.width ?? Math.min(1280, Math.max(960, Math.floor(window.screen.availWidth * 0.46)));
  const height = saved?.height ?? Math.min(900, Math.max(720, Math.floor(window.screen.availHeight * 0.86)));
  const left = saved?.left ?? Math.max(0, availLeft + window.screen.availWidth - width - 24);
  const top = saved?.top ?? Math.max(0, availTop + 32);
  const url = new URL(window.location.href);
  url.pathname = `/app/groups/${group.id}/tasks`;
  url.searchParams.set('detached', panel);
  url.searchParams.set('windowId', id);

  const features = [
    `left=${left}`,
    `top=${top}`,
    `width=${width}`,
    `height=${height}`,
    'popup=yes',
    'resizable=yes',
    'scrollbars=yes',
  ].join(',');
  const child = window.open(url.toString(), id.replace(/[^a-z0-9]/gi, '-'), features);
  child?.focus();
  persistDetachedWindowState(id, {
    panel,
    groupId: group.id,
    left,
    top,
    width,
    height,
    updatedAt: new Date().toISOString(),
  });
}
