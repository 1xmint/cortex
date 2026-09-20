import { Plus, UsersRound, FolderOpen } from 'lucide-react';
import { NavLink, useLocation } from 'react-router';
import type { BillingStatus } from '../../lib/cortexApi';
import type { CortexGroup } from '../../lib/groups';
import Sidebar from '../Sidebar';

type SettingsTab = 'providers' | 'integrations' | 'spend' | 'billing';

interface GroupSidebarProps {
  groups: CortexGroup[];
  activeGroupId: string;
  userId: string;
  isSignedIn: boolean;
  activeConversationId: string | null;
  refreshKey: number;
  isAdmin: boolean;
  billing: BillingStatus | null;
  onCreateGroup: () => void;
  onNewChat: () => void;
  onSelectConversation: (id: string) => void;
  onConversationsChanged: () => void;
  onOpenSettings: (tab?: SettingsTab) => void;
  onOpenAdmin: () => void;
}

function memberLabel(count: number) {
  return count === 1 ? '1 member' : `${count} members`;
}

export default function GroupSidebar({
  groups,
  activeGroupId,
  userId,
  isSignedIn,
  activeConversationId,
  refreshKey,
  isAdmin,
  billing,
  onCreateGroup,
  onNewChat,
  onSelectConversation,
  onConversationsChanged,
  onOpenSettings,
  onOpenAdmin,
}: GroupSidebarProps) {
  const location = useLocation();
  const isProjectsActive = location.pathname.startsWith('/projects');

  return (
    <div className="flex h-full w-80 flex-col border-r border-white/6 bg-[var(--bg)]">
      <div className="border-b border-white/6 p-3">
        <div className="mb-2 flex items-center justify-between gap-2">
          <div className="min-w-0">
            <div className="text-xs font-medium uppercase tracking-[0.08em] text-[var(--muted)]">
              Groups
            </div>
            <div className="truncate text-sm font-medium text-white">Task managers</div>
          </div>
          <button
            type="button"
            onClick={onCreateGroup}
            className="inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-lg border border-white/8 bg-white/4 text-[var(--muted)] transition hover:bg-white/8 hover:text-white active:scale-95"
            aria-label="Create team group"
            title="Create team group"
          >
            <Plus className="h-4 w-4" />
          </button>
        </div>

        <nav className="space-y-1" aria-label="Task manager groups">
          {groups.map((group) => (
            <NavLink
              key={group.id}
              to={`/app/groups/${group.id}/tasks`}
              className={({ isActive }) => {
                const active = isActive || activeGroupId === group.id;
                return `group flex min-w-0 items-center gap-2 rounded-xl px-2.5 py-2 text-left transition ${
                  active
                    ? 'bg-white/8 text-white shadow-[inset_0_0_0_1px_rgba(255,255,255,0.04)]'
                    : 'text-[var(--muted-strong)] hover:bg-white/5 hover:text-white'
                }`;
              }}
            >
              <span
                className="h-8 w-8 shrink-0 rounded-lg border border-white/10"
                style={{ backgroundColor: `${group.accent}22` }}
                aria-hidden="true"
              >
                <span
                  className="m-2 block h-4 w-4 rounded-md"
                  style={{ backgroundColor: group.accent }}
                />
              </span>
              <span className="min-w-0 flex-1">
                <span className="block truncate text-sm font-medium">{group.name}</span>
                <span className="mt-0.5 flex items-center gap-1 truncate text-[11px] text-[var(--muted)]">
                  <UsersRound className="h-3 w-3 shrink-0" />
                  {group.kind === 'personal' ? group.description : `${group.description} · ${memberLabel(group.members)}`}
                </span>
              </span>
            </NavLink>
          ))}
        </nav>
      </div>

      {/* Projects Section */}
      <div className="border-b border-white/6 p-3">
        <div className="mb-2">
          <div className="text-xs font-medium uppercase tracking-[0.08em] text-[var(--muted)]">
            Development
          </div>
        </div>

        <nav aria-label="Development tools">
          <NavLink
            to="/projects"
            className={`group flex min-w-0 items-center gap-2 rounded-xl px-2.5 py-2 text-left transition ${
              isProjectsActive
                ? 'bg-white/8 text-white shadow-[inset_0_0_0_1px_rgba(255,255,255,0.04)]'
                : 'text-[var(--muted-strong)] hover:bg-white/5 hover:text-white'
            }`}
          >
            <div className="h-8 w-8 shrink-0 rounded-lg border border-white/10 bg-blue-500/10 flex items-center justify-center">
              <FolderOpen className="h-4 w-4 text-blue-400" />
            </div>
            <span className="min-w-0 flex-1">
              <span className="block truncate text-sm font-medium">Projects</span>
              <span className="mt-0.5 block truncate text-[11px] text-[var(--muted)]">
                Code repositories and workspaces
              </span>
            </span>
          </NavLink>
        </nav>
      </div>

      <div className="min-h-0 flex-1">
        <Sidebar
          className="w-full"
          showBorder={false}
          userId={userId}
          isSignedIn={isSignedIn}
          activeConversationId={activeConversationId}
          refreshKey={refreshKey}
          onNewChat={onNewChat}
          onSelectConversation={onSelectConversation}
          onConversationsChanged={onConversationsChanged}
          onOpenSettings={onOpenSettings}
          onOpenAdmin={onOpenAdmin}
          isAdmin={isAdmin}
          billing={billing}
        />
      </div>
    </div>
  );
}
