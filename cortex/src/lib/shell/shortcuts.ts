export type ShortcutScope = 'global' | 'task-manager' | 'navigation' | 'window';

export interface ShortcutDefinition {
  id: string;
  label: string;
  description: string;
  keys: string[];
  scope: ShortcutScope;
}

export const DEFAULT_SHORTCUTS: ShortcutDefinition[] = [
  {
    id: 'command-palette',
    label: 'Open command palette',
    description: 'Search groups, tasks, conversations, and commands.',
    keys: ['mod', 'k'],
    scope: 'global',
  },
  {
    id: 'new-chat',
    label: 'New chat',
    description: 'Start a new Cortex conversation in the active group.',
    keys: ['mod', 'n'],
    scope: 'global',
  },
  {
    id: 'toggle-sidebar',
    label: 'Toggle sidebar',
    description: 'Open or close the Task Manager sidebar.',
    keys: ['mod', 'b'],
    scope: 'global',
  },
  {
    id: 'open-work-surface',
    label: 'Open work surface',
    description: 'Open mapping and orchestration context.',
    keys: ['mod', 'j'],
    scope: 'global',
  },
  {
    id: 'open-settings',
    label: 'Open settings',
    description: 'Open Cortex settings.',
    keys: ['mod', ','],
    scope: 'global',
  },
  {
    id: 'pop-out-task-manager',
    label: 'Pop out Task Manager',
    description: 'Open the active task manager in a detached window.',
    keys: ['mod', 'shift', 'o'],
    scope: 'window',
  },
  {
    id: 'mobile-chat',
    label: 'Focus chat',
    description: 'Switch the mobile Task Manager view to chat.',
    keys: ['mod', '1'],
    scope: 'task-manager',
  },
  {
    id: 'mobile-board',
    label: 'Focus board',
    description: 'Switch the mobile Task Manager view to the board.',
    keys: ['mod', '2'],
    scope: 'task-manager',
  },
  {
    id: 'mobile-team',
    label: 'Focus team',
    description: 'Switch the mobile Task Manager view to team status.',
    keys: ['mod', '3'],
    scope: 'task-manager',
  },
  {
    id: 'vim-board',
    label: 'Vim board focus',
    description: 'Power-user shortcut for the task board when focus is not in a text field.',
    keys: ['g', 'b'],
    scope: 'task-manager',
  },
  {
    id: 'vim-team',
    label: 'Vim team focus',
    description: 'Power-user shortcut for team status when focus is not in a text field.',
    keys: ['g', 't'],
    scope: 'task-manager',
  },
];

export function formatShortcut(keys: string[]) {
  const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform);
  return keys
    .map((key) => {
      if (key === 'mod') return isMac ? 'Cmd' : 'Ctrl';
      if (key === 'shift') return 'Shift';
      return key.length === 1 ? key.toUpperCase() : key;
    })
    .join(' ');
}

export function shortcutSignature(keys: string[]) {
  return keys.map((key) => key.toLowerCase()).join('+');
}

export function findShortcutConflicts(shortcuts: ShortcutDefinition[]) {
  const bySignature = new Map<string, ShortcutDefinition[]>();
  for (const shortcut of shortcuts) {
    const signature = shortcutSignature(shortcut.keys);
    bySignature.set(signature, [...(bySignature.get(signature) ?? []), shortcut]);
  }
  return Array.from(bySignature.entries())
    .filter(([, entries]) => entries.length > 1)
    .map(([signature, entries]) => ({ signature, entries }));
}
