export type CortexGroupKind = 'personal' | 'team';

export interface CortexGroup {
  id: string;
  name: string;
  kind: CortexGroupKind;
  description: string;
  members: number;
  accent: string;
}

const GROUPS_STORAGE_KEY = 'cortex:groups';
const GROUP_CONVERSATIONS_STORAGE_KEY = 'cortex:group-conversations';

export const DEFAULT_GROUPS: CortexGroup[] = [
  {
    id: 'personal',
    name: 'Personal',
    kind: 'personal',
    description: 'Private task manager',
    members: 1,
    accent: '#9cc7b8',
  },
  {
    id: 'team-alpha',
    name: 'Team A',
    kind: 'team',
    description: 'Shared coordination',
    members: 3,
    accent: '#f3c969',
  },
  {
    id: 'team-beta',
    name: 'Team B',
    kind: 'team',
    description: 'Shared coordination',
    members: 4,
    accent: '#8fb4ff',
  },
];

function isCortexGroup(value: unknown): value is CortexGroup {
  if (!value || typeof value !== 'object') return false;
  const candidate = value as Partial<CortexGroup>;
  return (
    typeof candidate.id === 'string'
    && candidate.id.length > 0
    && typeof candidate.name === 'string'
    && candidate.name.length > 0
    && (candidate.kind === 'personal' || candidate.kind === 'team')
    && typeof candidate.description === 'string'
    && typeof candidate.members === 'number'
    && Number.isFinite(candidate.members)
    && typeof candidate.accent === 'string'
  );
}

function mergeWithDefaultGroups(groups: CortexGroup[]) {
  const byId = new Map(groups.map((group) => [group.id, group]));
  for (const group of DEFAULT_GROUPS) {
    if (!byId.has(group.id)) byId.set(group.id, group);
  }
  return Array.from(byId.values());
}

export function readGroups(): CortexGroup[] {
  try {
    const raw = window.localStorage.getItem(GROUPS_STORAGE_KEY);
    if (!raw) return DEFAULT_GROUPS;
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return DEFAULT_GROUPS;
    const groups = parsed.filter(isCortexGroup);
    return groups.length > 0 ? mergeWithDefaultGroups(groups) : DEFAULT_GROUPS;
  } catch {
    return DEFAULT_GROUPS;
  }
}

export function writeGroups(groups: CortexGroup[]) {
  try {
    window.localStorage.setItem(GROUPS_STORAGE_KEY, JSON.stringify(groups));
  } catch {
    // Local group state is a convenience until the backend group API lands.
  }
}

function isConversationMap(value: unknown): value is Record<string, string | null> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
  return Object.values(value).every((entry) => entry === null || typeof entry === 'string');
}

export function readGroupConversationMap(): Record<string, string | null> {
  try {
    const raw = window.localStorage.getItem(GROUP_CONVERSATIONS_STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    return isConversationMap(parsed) ? parsed : {};
  } catch {
    return {};
  }
}

export function writeGroupConversationMap(value: Record<string, string | null>) {
  try {
    window.localStorage.setItem(GROUP_CONVERSATIONS_STORAGE_KEY, JSON.stringify(value));
  } catch {
    // Ignore local preference persistence failures.
  }
}

export function createTeamGroup(existingGroups: CortexGroup[]): CortexGroup {
  const existingNames = new Set(existingGroups.map((group) => group.name));
  const labels = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ';
  let suffix = 'C';
  for (const label of labels) {
    const candidate = `Team ${label}`;
    if (!existingNames.has(candidate)) {
      suffix = label;
      break;
    }
  }

  const idBase = `team-${suffix.toLowerCase()}`;
  let id = idBase;
  let counter = 2;
  const existingIds = new Set(existingGroups.map((group) => group.id));
  while (existingIds.has(id)) {
    id = `${idBase}-${counter}`;
    counter += 1;
  }

  return {
    id,
    name: `Team ${suffix}`,
    kind: 'team',
    description: 'Shared coordination',
    members: 2,
    accent: '#d9a5ff',
  };
}
