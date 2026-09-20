import {
  ArrowRight,
  Command,
  ExternalLink,
  Keyboard,
  LayoutDashboard,
  MessageSquare,
  Search,
  Users,
  X,
} from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ConversationSummary } from '../../lib/cortexApi';
import type { CortexGroup } from '../../lib/groups';
import { DEFAULT_SHORTCUTS, findShortcutConflicts, formatShortcut } from '../../lib/shell/shortcuts';
import type { TaskManagerState } from '../../types';

type PaletteItemKind = 'action' | 'group' | 'task' | 'conversation' | 'shortcut';

export interface PaletteItem {
  id: string;
  kind: PaletteItemKind;
  title: string;
  subtitle: string;
  keywords: string[];
  shortcut?: string;
  recent?: boolean;
  perform: () => void;
}

interface CommandPaletteProps {
  open: boolean;
  groups: CortexGroup[];
  activeGroupId: string;
  conversations: ConversationSummary[];
  taskState: TaskManagerState | null;
  onClose: () => void;
  onCreateTask: () => void;
  onCreateGroup: () => void;
  onNewChat: () => void;
  onSelectGroup: (groupId: string) => void;
  onSelectConversation: (conversationId: string) => void;
  onOpenSettings: () => void;
  onPopOutTaskManager: () => void;
}

const RECENTS_KEY = 'cortex:command-palette:recents';

function readRecents() {
  try {
    const parsed = JSON.parse(window.localStorage.getItem(RECENTS_KEY) ?? '[]');
    return Array.isArray(parsed) ? parsed.filter((item): item is string => typeof item === 'string') : [];
  } catch {
    return [];
  }
}

function writeRecents(ids: string[]) {
  try {
    window.localStorage.setItem(RECENTS_KEY, JSON.stringify(ids.slice(0, 8)));
  } catch {
    // Recent command storage is optional.
  }
}

function fuzzyScore(query: string, item: PaletteItem) {
  const normalizedQuery = query.trim().toLowerCase();
  if (!normalizedQuery) return item.recent ? 4 : 1;
  const haystack = [item.title, item.subtitle, item.kind, ...item.keywords].join(' ').toLowerCase();
  if (haystack.includes(normalizedQuery)) return 100 - haystack.indexOf(normalizedQuery);

  let score = 0;
  let position = 0;
  for (const letter of normalizedQuery) {
    const found = haystack.indexOf(letter, position);
    if (found === -1) return 0;
    score += found === position ? 8 : 3;
    position = found + 1;
  }
  return score;
}

function iconForKind(kind: PaletteItemKind) {
  if (kind === 'group') return Users;
  if (kind === 'task') return LayoutDashboard;
  if (kind === 'conversation') return MessageSquare;
  if (kind === 'shortcut') return Keyboard;
  return Command;
}

export default function CommandPalette({
  open,
  groups,
  activeGroupId,
  conversations,
  taskState,
  onClose,
  onCreateTask,
  onCreateGroup,
  onNewChat,
  onSelectGroup,
  onSelectConversation,
  onOpenSettings,
  onPopOutTaskManager,
}: CommandPaletteProps) {
  const [query, setQuery] = useState('');
  const [activeIndex, setActiveIndex] = useState(0);
  const [recents, setRecents] = useState<string[]>(readRecents);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const conflicts = useMemo(() => findShortcutConflicts(DEFAULT_SHORTCUTS), []);

  useEffect(() => {
    if (!open) return;
    setQuery('');
    setActiveIndex(0);
    window.setTimeout(() => inputRef.current?.focus(), 0);
  }, [open]);

  const runItem = useCallback((item: PaletteItem) => {
    const nextRecents = [item.id, ...recents.filter((id) => id !== item.id)];
    setRecents(nextRecents);
    writeRecents(nextRecents);
    item.perform();
    onClose();
  }, [onClose, recents]);

  const items = useMemo<PaletteItem[]>(() => {
    const baseItems: PaletteItem[] = [
      {
        id: 'action:create-task',
        kind: 'action',
        title: 'Create task',
        subtitle: 'Draft a structured task command',
        keywords: ['add', 'todo', 'assignment'],
        shortcut: formatShortcut(['mod', 'k']),
        perform: onCreateTask,
      },
      {
        id: 'action:new-chat',
        kind: 'action',
        title: 'New chat',
        subtitle: 'Start a fresh conversation',
        keywords: ['conversation', 'compose'],
        shortcut: formatShortcut(['mod', 'n']),
        perform: onNewChat,
      },
      {
        id: 'action:popout-task-manager',
        kind: 'action',
        title: 'Pop out Task Manager',
        subtitle: 'Open this group in a detached monitor window',
        keywords: ['window', 'monitor', 'detach'],
        shortcut: formatShortcut(['mod', 'shift', 'o']),
        perform: onPopOutTaskManager,
      },
      {
        id: 'action:create-group',
        kind: 'action',
        title: 'Create team group',
        subtitle: 'Add another coordinated team space',
        keywords: ['team', 'group'],
        perform: onCreateGroup,
      },
      {
        id: 'action:settings',
        kind: 'action',
        title: 'Open settings',
        subtitle: 'Providers, integrations, spend, and billing',
        keywords: ['providers', 'billing', 'integrations'],
        shortcut: formatShortcut(['mod', ',']),
        perform: onOpenSettings,
      },
    ];

    const groupItems = groups.map((group): PaletteItem => ({
      id: `group:${group.id}`,
      kind: 'group',
      title: `Switch to ${group.name}`,
      subtitle: group.id === activeGroupId ? 'Current group' : group.description,
      keywords: [group.name, group.kind, group.description],
      perform: () => onSelectGroup(group.id),
    }));

    const taskItems = (taskState?.tasks ?? []).map((task): PaletteItem => ({
      id: `task:${task.id}`,
      kind: 'task',
      title: task.title,
      subtitle: `${task.status.replace('-', ' ')}${task.repo ? ` in ${task.repo}` : ''}`,
      keywords: [task.priority, task.status, task.repo ?? '', task.createdBy],
      perform: () => onCreateTask(),
    }));

    const conversationItems = conversations.map((conversation): PaletteItem => ({
      id: `conversation:${conversation.id}`,
      kind: 'conversation',
      title: conversation.title || 'New conversation',
      subtitle: conversation.last_message_preview || 'Open conversation',
      keywords: [conversation.title ?? '', conversation.last_message_preview ?? ''],
      perform: () => onSelectConversation(conversation.id),
    }));

    const shortcutItems = DEFAULT_SHORTCUTS.map((shortcut): PaletteItem => ({
      id: `shortcut:${shortcut.id}`,
      kind: 'shortcut',
      title: shortcut.label,
      subtitle: shortcut.description,
      keywords: [shortcut.scope, shortcut.description],
      shortcut: formatShortcut(shortcut.keys),
      perform: () => undefined,
    }));

    const allItems = [...baseItems, ...groupItems, ...taskItems, ...conversationItems, ...shortcutItems];
    return allItems.map((item) => ({ ...item, recent: recents.includes(item.id) }));
  }, [
    activeGroupId,
    conversations,
    groups,
    onCreateGroup,
    onCreateTask,
    onNewChat,
    onOpenSettings,
    onPopOutTaskManager,
    onSelectConversation,
    onSelectGroup,
    recents,
    taskState?.tasks,
  ]);

  const filtered = useMemo(() => {
    return items
      .map((item) => ({ item, score: fuzzyScore(query, item) }))
      .filter(({ score }) => score > 0)
      .sort((a, b) => b.score - a.score)
      .slice(0, 18)
      .map(({ item }) => item);
  }, [items, query]);

  useEffect(() => {
    setActiveIndex((index) => Math.min(index, Math.max(filtered.length - 1, 0)));
  }, [filtered.length]);

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-[80] bg-black/55 p-3 backdrop-blur-md sm:p-6" role="dialog" aria-modal="true" aria-label="Command palette">
      <button
        type="button"
        aria-label="Close command palette"
        className="absolute inset-0 cursor-default"
        onClick={onClose}
      />
      <div className="relative mx-auto mt-[8vh] flex max-h-[78vh] w-full max-w-3xl flex-col overflow-hidden rounded-xl border border-white/10 bg-[#111414] shadow-2xl">
        <div className="flex h-14 items-center gap-3 border-b border-white/8 px-4">
          <Search className="h-4 w-4 text-[var(--muted)]" />
          <input
            ref={inputRef}
            value={query}
            aria-label="Search commands"
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Escape') onClose();
              if (event.key === 'ArrowDown') {
                event.preventDefault();
                setActiveIndex((index) => Math.min(index + 1, filtered.length - 1));
              }
              if (event.key === 'ArrowUp') {
                event.preventDefault();
                setActiveIndex((index) => Math.max(index - 1, 0));
              }
              if (event.key === 'Enter' && filtered[activeIndex]) {
                event.preventDefault();
                runItem(filtered[activeIndex]);
              }
            }}
            placeholder="Search groups, tasks, conversations, actions, shortcuts..."
            className="min-w-0 flex-1 bg-transparent text-sm text-white outline-none placeholder:text-[var(--muted)]"
          />
          <button
            type="button"
            onClick={onClose}
            className="inline-flex h-8 w-8 items-center justify-center rounded-lg text-[var(--muted)] transition hover:bg-white/8 hover:text-white"
            aria-label="Close command palette"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        {conflicts.length > 0 && (
          <div className="border-b border-amber-300/15 bg-amber-300/8 px-4 py-2 text-xs text-amber-100">
            {conflicts.length} shortcut conflict detected. Search shortcuts to review bindings.
          </div>
        )}

        <div className="min-h-0 flex-1 overflow-y-auto p-2">
          {filtered.length === 0 ? (
            <div className="flex min-h-44 flex-col items-center justify-center px-4 text-center">
              <Command className="h-7 w-7 text-[var(--muted)]" />
              <p className="mt-3 text-sm font-medium text-white">No command found</p>
              <p className="mt-1 text-xs text-[var(--muted)]">Try a group name, task title, provider, or shortcut.</p>
            </div>
          ) : (
            <div role="listbox" aria-label="Commands">
            {filtered.map((item, index) => {
              const Icon = iconForKind(item.kind);
              const active = index === activeIndex;
              return (
                <button
                  key={item.id}
                  type="button"
                  role="option"
                  aria-selected={active}
                  onMouseEnter={() => setActiveIndex(index)}
                  onClick={() => runItem(item)}
                  className={`flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left transition ${
                    active ? 'bg-white/10 text-white' : 'text-[var(--muted-strong)] hover:bg-white/6 hover:text-white'
                  }`}
                >
                  <span className="inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-lg border border-white/8 bg-white/[0.03]">
                    <Icon className="h-4 w-4" />
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="flex min-w-0 items-center gap-2">
                      <span className="truncate text-sm font-medium">{item.title}</span>
                      {item.recent && <span className="shrink-0 rounded-full bg-white/8 px-1.5 py-0.5 text-[10px] text-[var(--muted)]">recent</span>}
                    </span>
                    <span className="mt-0.5 block truncate text-xs text-[var(--muted)]">{item.subtitle}</span>
                  </span>
                  {item.shortcut && <kbd className="hidden shrink-0 rounded-md border border-white/8 bg-black/20 px-1.5 py-0.5 text-[10px] text-[var(--muted)] sm:inline">{item.shortcut}</kbd>}
                  {item.kind === 'action' ? <ArrowRight className="h-4 w-4 shrink-0 text-[var(--muted)]" /> : <ExternalLink className="h-3.5 w-3.5 shrink-0 text-[var(--muted)]" />}
                </button>
              );
            })}
            </div>
          )}
        </div>

        <div className="flex items-center justify-between border-t border-white/8 px-4 py-2 text-[11px] text-[var(--muted)]">
          <span>Arrow keys navigate. Enter runs. Esc closes.</span>
          <span className="hidden sm:inline">{filtered.length} results</span>
        </div>
      </div>
    </div>
  );
}
