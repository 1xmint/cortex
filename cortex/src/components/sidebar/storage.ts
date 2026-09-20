export interface SidebarConversationMeta {
  pinned?: boolean;
  archived?: boolean;
}

export type SidebarConversationMetaMap = Record<string, SidebarConversationMeta>;

function storageKey(userId: string): string {
  return `cortex:sidebar-meta:${userId}`;
}

export function readSidebarConversationMeta(userId: string): SidebarConversationMetaMap {
  if (typeof window === 'undefined') return {};

  try {
    const raw = window.localStorage.getItem(storageKey(userId));
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    return typeof parsed === 'object' && parsed ? parsed as SidebarConversationMetaMap : {};
  } catch {
    return {};
  }
}

export function writeSidebarConversationMeta(
  userId: string,
  meta: SidebarConversationMetaMap,
): void {
  if (typeof window === 'undefined') return;

  try {
    window.localStorage.setItem(storageKey(userId), JSON.stringify(meta));
  } catch {
    // ignore storage failures
  }
}
