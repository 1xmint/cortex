import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Archive,
  ChevronDown,
  ChevronRight,
  MessageSquarePlus,
  MoreHorizontal,
  Pencil,
  Pin,
  RefreshCcw,
  Search,
  Settings,
  ShieldCheck,
  Trash2,
} from 'lucide-react';
import {
  deleteConversation,
  listConversations,
  updateConversationTitle,
  type BillingStatus,
  type ConversationSummary,
} from '../lib/cortexApi';
import {
  readSidebarConversationMeta,
  writeSidebarConversationMeta,
  type SidebarConversationMetaMap,
} from './sidebar/storage';

interface SidebarProps {
  userId: string;
  isSignedIn: boolean;
  activeConversationId: string | null;
  refreshKey: number;
  onNewChat: () => void;
  onSelectConversation: (id: string) => void;
  onConversationsChanged: () => void;
  onOpenSettings: (tab?: 'providers' | 'integrations' | 'spend' | 'billing') => void;
  onOpenAdmin?: () => void;
  isAdmin?: boolean;
  billing: BillingStatus | null;
  className?: string;
  showBorder?: boolean;
}

interface MenuState {
  conversationId: string;
  x: number;
  y: number;
}

interface DecoratedConversation extends ConversationSummary {
  pinned: boolean;
  archived: boolean;
}

const CLICK_DELAY_MS = 220;

export default function Sidebar({
  userId,
  activeConversationId,
  refreshKey,
  onNewChat,
  onSelectConversation,
  onConversationsChanged,
  onOpenSettings,
  onOpenAdmin,
  isAdmin,
  className = 'w-72',
  showBorder = true,
}: SidebarProps) {
  const [conversations, setConversations] = useState<ConversationSummary[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [search, setSearch] = useState('');
  const [meta, setMeta] = useState<SidebarConversationMetaMap>({});
  const [archivedExpanded, setArchivedExpanded] = useState(false);
  const [menu, setMenu] = useState<MenuState | null>(null);
  const [confirmingDeleteId, setConfirmingDeleteId] = useState<string | null>(null);
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState('');
  const clickTimeoutRef = useRef<number | null>(null);
  const menuRef = useRef<HTMLDivElement | null>(null);
  const renameInputRef = useRef<HTMLInputElement | null>(null);
  const skipRenameBlurSaveRef = useRef(false);

  const fetchConversations = useCallback(async () => {
    try {
      const list = await listConversations(userId);
      setConversations(list);
      setLoadError(null);
    } catch (err) {
      setLoadError(err instanceof Error ? err.message : 'Could not load conversations');
    } finally {
      setIsLoading(false);
    }
  }, [userId]);

  useEffect(() => {
    fetchConversations();
    const interval = window.setInterval(fetchConversations, 5000);
    return () => window.clearInterval(interval);
  }, [fetchConversations, refreshKey]);

  useEffect(() => {
    setMeta(readSidebarConversationMeta(userId));
  }, [userId]);

  useEffect(() => {
    if (!menu) {
      setConfirmingDeleteId(null);
      return;
    }

    const handlePointerDown = (event: MouseEvent) => {
      if (menuRef.current?.contains(event.target as Node)) return;
      setMenu(null);
    };

    const handleEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setMenu(null);
    };

    window.addEventListener('mousedown', handlePointerDown);
    window.addEventListener('contextmenu', handlePointerDown);
    window.addEventListener('keydown', handleEscape);

    return () => {
      window.removeEventListener('mousedown', handlePointerDown);
      window.removeEventListener('contextmenu', handlePointerDown);
      window.removeEventListener('keydown', handleEscape);
    };
  }, [menu]);

  useEffect(() => {
    if (!renamingId) return;
    renameInputRef.current?.focus();
    renameInputRef.current?.select();
  }, [renamingId]);

  useEffect(() => {
    return () => {
      if (clickTimeoutRef.current) window.clearTimeout(clickTimeoutRef.current);
    };
  }, []);

  const persistMeta = (nextMeta: SidebarConversationMetaMap) => {
    setMeta(nextMeta);
    writeSidebarConversationMeta(userId, nextMeta);
  };

  const updateConversationMeta = (
    conversationId: string,
    updater: (current: SidebarConversationMetaMap[string]) => SidebarConversationMetaMap[string],
  ) => {
    setMenu(null);
    setMeta((current) => {
      const nextMeta = { ...current, [conversationId]: updater(current[conversationId]) };
      writeSidebarConversationMeta(userId, nextMeta);
      return nextMeta;
    });
  };

  const startRename = (conversation: ConversationSummary) => {
    setMenu(null);
    setConfirmingDeleteId(null);
    setRenamingId(conversation.id);
    setRenameValue(conversation.title || 'New conversation');
  };

  const cancelRename = () => {
    skipRenameBlurSaveRef.current = true;
    setRenamingId(null);
    setRenameValue('');
  };

  const saveRename = async (conversationId: string) => {
    const nextTitle = renameValue.trim();
    skipRenameBlurSaveRef.current = false;
    if (!nextTitle) {
      cancelRename();
      return;
    }

    await updateConversationTitle(conversationId, nextTitle, userId);
    setConversations((current) =>
      current.map((conversation) =>
        conversation.id === conversationId
          ? { ...conversation, title: nextTitle }
          : conversation,
      ),
    );
    cancelRename();
    onConversationsChanged();
  };

  const handleDelete = async (id: string) => {
    setMenu(null);
    setConfirmingDeleteId(null);
    await deleteConversation(id, userId);
    setConversations((prev) => prev.filter((conversation) => conversation.id !== id));

    const nextMeta = { ...meta };
    delete nextMeta[id];
    persistMeta(nextMeta);

    onConversationsChanged();
    if (activeConversationId === id) onNewChat();
  };

  const normalizedSearch = search.trim().toLowerCase();

  const decoratedConversations = useMemo<DecoratedConversation[]>(() => {
    return conversations.map((conversation) => ({
      ...conversation,
      pinned: Boolean(meta[conversation.id]?.pinned),
      archived: Boolean(meta[conversation.id]?.archived),
    }));
  }, [conversations, meta]);

  const filteredConversations = useMemo(() => {
    if (!normalizedSearch) return decoratedConversations;
    return decoratedConversations.filter((conversation) => {
      const haystack = [
        conversation.title || '',
        conversation.last_message_preview || '',
      ].join(' ').toLowerCase();
      return haystack.includes(normalizedSearch);
    });
  }, [decoratedConversations, normalizedSearch]);

  const pinnedConversations = filteredConversations
    .filter((conversation) => conversation.pinned && !conversation.archived)
    .sort((a, b) => b.updated_at.localeCompare(a.updated_at));
  const recentConversations = filteredConversations
    .filter((conversation) => !conversation.pinned && !conversation.archived)
    .sort((a, b) => b.updated_at.localeCompare(a.updated_at));
  const archivedConversations = filteredConversations
    .filter((conversation) => conversation.archived)
    .sort((a, b) => b.updated_at.localeCompare(a.updated_at));

  const showArchived = archivedExpanded || normalizedSearch.length > 0;
  const activeMenuConversation = menu
    ? decoratedConversations.find((conversation) => conversation.id === menu.conversationId) ?? null
    : null;

  const queueSelectConversation = (conversationId: string) => {
    if (clickTimeoutRef.current) window.clearTimeout(clickTimeoutRef.current);
    clickTimeoutRef.current = window.setTimeout(() => {
      onSelectConversation(conversationId);
      clickTimeoutRef.current = null;
    }, CLICK_DELAY_MS);
  };

  const handleDoubleClick = (conversation: ConversationSummary) => {
    if (clickTimeoutRef.current) {
      window.clearTimeout(clickTimeoutRef.current);
      clickTimeoutRef.current = null;
    }
    startRename(conversation);
  };

  const handleContextMenu = (
    event: React.MouseEvent<HTMLButtonElement>,
    conversationId: string,
  ) => {
    event.preventDefault();
    event.stopPropagation();
    setMenu({
      conversationId,
      x: event.clientX,
      y: event.clientY,
    });
    setConfirmingDeleteId(null);
  };

  const formatTime = (iso: string) => {
    const date = new Date(iso);
    const now = new Date();
    const diff = now.getTime() - date.getTime();
    if (diff < 60_000) return 'Just now';
    if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
    if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`;
    if (diff < 604_800_000) return `${Math.floor(diff / 86_400_000)}d ago`;
    return date.toLocaleDateString();
  };

  const renderConversationRow = (conversation: DecoratedConversation) => {
    const isActive = activeConversationId === conversation.id;
    const isRenaming = renamingId === conversation.id;

    return (
      <div
        key={conversation.id}
        className={`group relative mb-0.5 rounded-xl transition-all duration-150 ${
          isActive ? 'bg-white/8 shadow-[inset_0_0_0_1px_rgba(255,255,255,0.04)]' : 'hover:bg-white/5'
        }`}
      >
        <button
          type="button"
          aria-label={`Open conversation: ${conversation.title || 'New conversation'}`}
          aria-current={isActive ? 'true' : undefined}
          onContextMenu={(event) => handleContextMenu(event, conversation.id)}
          onClick={() => queueSelectConversation(conversation.id)}
          onDoubleClick={() => handleDoubleClick(conversation)}
          className={`flex w-full min-w-0 items-center gap-2 rounded-xl px-3 py-2 text-left outline-none transition-all duration-150 ${
            isActive ? 'text-white' : 'text-[var(--muted-strong)]'
          }`}
        >
          <button
            type="button"
            aria-label={conversation.pinned ? 'Unpin conversation' : 'Pin conversation'}
            onClick={(event) => {
              event.stopPropagation();
              updateConversationMeta(conversation.id, (current) => ({
                ...current,
                pinned: !current?.pinned,
                archived: current?.archived ?? false,
              }));
            }}
            className={`shrink-0 rounded-md p-1 transition-all duration-150 ${
              conversation.pinned
                ? 'text-amber-300 opacity-100'
                : 'text-[var(--muted)] opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 hover:bg-white/6 hover:text-white'
            }`}
          >
            <Pin className={`h-3.5 w-3.5 ${conversation.pinned ? 'fill-current' : ''}`} />
          </button>

          <div className="min-w-0 flex-1">
            {isRenaming ? (
              <input
                ref={renameInputRef}
                value={renameValue}
                onChange={(event) => setRenameValue(event.target.value)}
                onClick={(event) => event.stopPropagation()}
                onDoubleClick={(event) => event.stopPropagation()}
                onBlur={() => {
                  if (skipRenameBlurSaveRef.current) {
                    skipRenameBlurSaveRef.current = false;
                    return;
                  }
                  void saveRename(conversation.id);
                }}
                onKeyDown={(event) => {
                  if (event.key === 'Enter') {
                    event.preventDefault();
                    void saveRename(conversation.id);
                  } else if (event.key === 'Escape') {
                    event.preventDefault();
                    cancelRename();
                  }
                }}
                className="w-full rounded-md border border-white/10 bg-white/8 px-2 py-1 text-sm text-white outline-none ring-0 transition focus:border-white/16"
              />
            ) : (
              <>
                <div className="truncate text-sm font-medium">
                  {conversation.title || 'New conversation'}
                </div>
                <div className="mt-0.5 truncate text-[11px] text-[var(--muted)]">
                  {conversation.last_message_preview || formatTime(conversation.updated_at)}
                </div>
              </>
            )}
          </div>

          <button
            type="button"
            aria-label={`Actions for ${conversation.title || 'New conversation'}`}
            onClick={(event) => {
              event.stopPropagation();
              const rect = event.currentTarget.getBoundingClientRect();
              setMenu({
                conversationId: conversation.id,
                x: rect.right,
                y: rect.bottom + 6,
              });
              setConfirmingDeleteId(null);
            }}
            className="shrink-0 rounded-md p-1 text-[var(--muted)] opacity-0 transition-all duration-150 group-hover:opacity-100 group-focus-within:opacity-100 hover:bg-white/6 hover:text-white"
          >
            <MoreHorizontal className="h-3.5 w-3.5" />
          </button>
        </button>
      </div>
    );
  };

  const renderSection = (label: string, items: DecoratedConversation[]) => {
    if (items.length === 0) return null;

    return (
      <section className="mb-3">
        <div className="px-3 pb-1 text-[11px] font-medium uppercase tracking-[0.08em] text-[var(--muted)]">
          {label}
        </div>
        <div>{items.map(renderConversationRow)}</div>
      </section>
    );
  };

  const noResults = pinnedConversations.length === 0
    && recentConversations.length === 0
    && archivedConversations.length === 0;
  const showInitialLoading = isLoading && conversations.length === 0;
  const showLoadError = Boolean(loadError) && conversations.length === 0;

  return (
    <div className={`relative flex h-full flex-col bg-[var(--bg)] ${showBorder ? 'border-r border-white/6' : ''} ${className}`}>
      <div className="p-3">
        <button
          type="button"
          onClick={onNewChat}
          aria-label="New chat"
          className="flex w-full items-center gap-2 rounded-xl border border-white/8 bg-white/4 px-3 py-2 text-sm text-white transition-all duration-150 hover:bg-white/8 active:scale-[0.98]"
        >
          <MessageSquarePlus className="h-4 w-4 text-[var(--accent)]" />
          New chat
        </button>
      </div>

      <div className="px-3 pb-2">
        <div className="flex items-center gap-2 rounded-xl border border-white/6 bg-white/4 px-2.5 py-2 shadow-[inset_0_0_0_1px_rgba(255,255,255,0.02)] transition focus-within:border-white/10 focus-within:bg-white/5">
          <Search className="h-3.5 w-3.5 text-[var(--muted)]" />
          <input
            type="text"
            placeholder="Search chats..."
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            className="w-full bg-transparent text-xs text-white outline-none placeholder:text-[var(--muted)]"
          />
        </div>
      </div>

      <div className="flex-1 overflow-y-auto px-2 pb-3">
        {showInitialLoading ? (
          <div className="space-y-2 px-2 py-3">
            {[0, 1, 2, 3].map((item) => (
              <div key={item} className="rounded-xl px-3 py-2">
                <div className="h-4 w-36 animate-pulse rounded-full bg-white/[0.06]" />
                <div className="mt-2 h-3 w-24 animate-pulse rounded-full bg-white/[0.04]" />
              </div>
            ))}
          </div>
        ) : showLoadError ? (
          <div className="mx-2 mt-4 rounded-xl border border-red-400/15 bg-red-400/8 px-3 py-3">
            <p className="text-sm font-medium text-red-100">Conversation list unavailable</p>
            <p className="mt-1 text-xs leading-5 text-red-100/70">
              Cortex could not load saved chats. You can still start a new chat.
            </p>
            <button
              type="button"
              onClick={() => {
                setIsLoading(true);
                void fetchConversations();
              }}
              className="mt-3 inline-flex items-center gap-2 rounded-lg border border-red-200/15 bg-red-200/10 px-2.5 py-1.5 text-xs text-red-50 transition hover:bg-red-200/15 active:scale-95"
            >
              <RefreshCcw className="h-3.5 w-3.5" />
              Retry
            </button>
          </div>
        ) : noResults ? (
          <p className="px-2 py-4 text-center text-xs text-[var(--muted)]">
            {normalizedSearch ? 'No matches' : 'No conversations yet'}
          </p>
        ) : (
          <>
            {renderSection('Pinned', pinnedConversations)}
            {renderSection('Recent', recentConversations)}

            {(archivedConversations.length > 0 || normalizedSearch) && (
              <section className="mt-2">
                <button
                  type="button"
                  onClick={() => setArchivedExpanded((value) => !value)}
                  className="flex w-full items-center gap-1.5 px-3 pb-1 text-[11px] font-medium uppercase tracking-[0.08em] text-[var(--muted)] transition hover:text-white"
                >
                  {showArchived ? (
                    <ChevronDown className="h-3.5 w-3.5" />
                  ) : (
                    <ChevronRight className="h-3.5 w-3.5" />
                  )}
                  Archived
                  <span className="ml-auto text-[10px] normal-case tracking-normal text-[var(--muted)]">
                    {archivedConversations.length}
                  </span>
                </button>
                {showArchived && <div>{archivedConversations.map(renderConversationRow)}</div>}
              </section>
            )}
          </>
        )}
      </div>

      <div className="border-t border-white/6 p-3">
        {isAdmin && onOpenAdmin && (
          <button
            onClick={onOpenAdmin}
            className="flex w-full items-center gap-2 rounded-lg px-2 py-2 text-xs text-[var(--muted)] transition hover:bg-white/4 hover:text-white active:scale-[0.98]"
          >
            <ShieldCheck className="h-4 w-4 text-[var(--accent)]" />
            Admin
          </button>
        )}
        <button
          onClick={() => onOpenSettings()}
          className="flex w-full items-center gap-2 rounded-lg px-2 py-2 text-xs text-[var(--muted)] transition hover:bg-white/4 hover:text-white active:scale-[0.98]"
        >
          <Settings className="h-4 w-4" />
          Settings
        </button>
      </div>

      {menu && activeMenuConversation && (
        <div
          ref={menuRef}
          style={{
            left: Math.min(menu.x, window.innerWidth - 188),
            top: Math.min(menu.y, window.innerHeight - 220),
          }}
          className="fixed z-50 w-44 rounded-xl border border-white/10 bg-[#17181c] p-1.5 shadow-2xl shadow-black/40"
        >
          <button
            type="button"
            onClick={() => startRename(activeMenuConversation)}
            className="flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-sm text-[var(--muted-strong)] transition hover:bg-white/6 hover:text-white"
          >
            <Pencil className="h-3.5 w-3.5" />
            Rename
          </button>
          <button
            type="button"
            onClick={() =>
              updateConversationMeta(activeMenuConversation.id, (current) => ({
                ...current,
                pinned: !current?.pinned,
                archived: current?.archived ?? false,
              }))
            }
            className="flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-sm text-[var(--muted-strong)] transition hover:bg-white/6 hover:text-white"
          >
            <Pin className={`h-3.5 w-3.5 ${activeMenuConversation.pinned ? 'fill-current' : ''}`} />
            {activeMenuConversation.pinned ? 'Unpin' : 'Pin'}
          </button>
          <button
            type="button"
            onClick={() =>
              updateConversationMeta(activeMenuConversation.id, (current) => ({
                ...current,
                archived: !current?.archived,
                pinned: current?.pinned ?? false,
              }))
            }
            className="flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-sm text-[var(--muted-strong)] transition hover:bg-white/6 hover:text-white"
          >
            <Archive className="h-3.5 w-3.5" />
            {activeMenuConversation.archived ? 'Unarchive' : 'Archive'}
          </button>
          <div className="my-1 border-t border-white/8" />
          {confirmingDeleteId === activeMenuConversation.id ? (
            <div className="rounded-lg bg-red-500/10 p-2">
              <p className="text-xs leading-4 text-red-100/80">Delete this conversation?</p>
              <div className="mt-2 flex gap-1.5">
                <button
                  type="button"
                  onClick={() => {
                    void handleDelete(activeMenuConversation.id);
                  }}
                  className="flex-1 rounded-md bg-red-400 px-2 py-1.5 text-xs font-medium text-black transition hover:brightness-110 active:scale-95"
                >
                  Delete
                </button>
                <button
                  type="button"
                  onClick={() => setConfirmingDeleteId(null)}
                  className="flex-1 rounded-md border border-white/8 px-2 py-1.5 text-xs text-[var(--muted-strong)] transition hover:bg-white/6 active:scale-95"
                >
                  Cancel
                </button>
              </div>
            </div>
          ) : (
            <button
              type="button"
              onClick={() => setConfirmingDeleteId(activeMenuConversation.id)}
              className="flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-sm text-red-300 transition hover:bg-red-500/12 hover:text-red-200"
            >
              <Trash2 className="h-3.5 w-3.5" />
              Delete
            </button>
          )}
        </div>
      )}
    </div>
  );
}
