import { useCallback, useEffect, useState } from 'react';
import {
  Archive,
  ExternalLink,
  FolderOpen,
  GitBranch,
  Loader2,
  MoreVertical,
  Pause,
  Play,
  Plus,
  RefreshCw,
  Search,
  Trash2,
  Upload
} from 'lucide-react';
import {
  listProjects,
  deleteProject,
  updateProject,
  syncProject,
  getProjectWorkspaceUrl,
  type Project
} from '../../lib/projectApi';

interface ProjectsListProps {
  onCreateProject: () => void;
  onImportProject: () => void;
  onSelectProject: (project: Project) => void;
}

function formatTimeAgo(dateStr: string): string {
  const date = new Date(dateStr);
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffMinutes = Math.floor(diffMs / (1000 * 60));

  if (diffMinutes < 1) return 'just now';
  if (diffMinutes < 60) return `${diffMinutes}m ago`;

  const diffHours = Math.floor(diffMinutes / 60);
  if (diffHours < 24) return `${diffHours}h ago`;

  const diffDays = Math.floor(diffHours / 24);
  if (diffDays < 30) return `${diffDays}d ago`;

  const diffMonths = Math.floor(diffDays / 30);
  return `${diffMonths}mo ago`;
}

function ProjectCard({
  project,
  onSelect,
  onUpdate,
  onDelete,
  onSync,
  onOpenWorkspace
}: {
  project: Project;
  onSelect: (project: Project) => void;
  onUpdate: (id: string, updates: Partial<Project>) => void;
  onDelete: (id: string) => void;
  onSync: (id: string) => void;
  onOpenWorkspace: (id: string) => void;
}) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [openingWorkspace, setOpeningWorkspace] = useState(false);

  const handleSync = useCallback(async () => {
    setSyncing(true);
    try {
      await onSync(project.id);
    } finally {
      setSyncing(false);
    }
  }, [project.id, onSync]);

  const handleOpenWorkspace = useCallback(async () => {
    setOpeningWorkspace(true);
    try {
      await onOpenWorkspace(project.id);
    } finally {
      setOpeningWorkspace(false);
    }
  }, [project.id, onOpenWorkspace]);

  const statusColor = {
    creating: 'text-blue-400',
    active: 'text-green-400',
    paused: 'text-yellow-400',
    archived: 'text-gray-400'
  }[project.status];

  const statusIcon = {
    creating: Loader2,
    active: Play,
    paused: Pause,
    archived: Archive
  }[project.status];

  const StatusIcon = statusIcon;

  return (
    <div className="group p-4 bg-white/[0.02] border border-white/8 rounded-xl hover:border-white/15 hover:bg-white/[0.04] transition-all">
      <div className="flex items-start justify-between mb-3">
        <div className="flex-1 min-w-0">
          <button
            onClick={() => onSelect(project)}
            className="text-left group-hover:text-[var(--accent)] transition-colors"
          >
            <h3 className="font-medium text-white truncate">{project.name}</h3>
            {project.description && (
              <p className="text-xs text-[var(--muted)] mt-1 line-clamp-2">{project.description}</p>
            )}
          </button>
        </div>

        <div className="flex items-center gap-2 ml-3">
          <div className={`flex items-center gap-1 text-xs ${statusColor}`}>
            <StatusIcon className={`h-3 w-3 ${project.status === 'creating' ? 'animate-spin' : ''}`} />
            <span className="capitalize">{project.status}</span>
          </div>

          <div className="relative">
            <button
              onClick={() => setMenuOpen(!menuOpen)}
              className="p-1 text-[var(--muted)] hover:text-white transition opacity-0 group-hover:opacity-100"
            >
              <MoreVertical className="h-4 w-4" />
            </button>

            {menuOpen && (
              <>
                <div
                  className="fixed inset-0 z-10"
                  onClick={() => setMenuOpen(false)}
                />
                <div className="absolute right-0 top-6 z-20 min-w-48 py-1 bg-[var(--panel)] border border-white/10 rounded-lg shadow-xl">
                  <button
                    onClick={handleOpenWorkspace}
                    disabled={openingWorkspace}
                    className="w-full px-3 py-2 text-left text-xs text-white hover:bg-white/8 flex items-center gap-2"
                  >
                    {openingWorkspace ? (
                      <Loader2 className="h-3 w-3 animate-spin" />
                    ) : (
                      <ExternalLink className="h-3 w-3" />
                    )}
                    Open Workspace
                  </button>

                  {project.repoUrl && (
                    <button
                      onClick={handleSync}
                      disabled={syncing}
                      className="w-full px-3 py-2 text-left text-xs text-white hover:bg-white/8 flex items-center gap-2"
                    >
                      {syncing ? (
                        <Loader2 className="h-3 w-3 animate-spin" />
                      ) : (
                        <RefreshCw className="h-3 w-3" />
                      )}
                      Sync with Git
                    </button>
                  )}

                  <button
                    onClick={() => {
                      const newStatus = project.status === 'active' ? 'paused' : 'active';
                      onUpdate(project.id, { status: newStatus });
                      setMenuOpen(false);
                    }}
                    className="w-full px-3 py-2 text-left text-xs text-white hover:bg-white/8 flex items-center gap-2"
                  >
                    {project.status === 'active' ? (
                      <>
                        <Pause className="h-3 w-3" />
                        Pause Project
                      </>
                    ) : (
                      <>
                        <Play className="h-3 w-3" />
                        Resume Project
                      </>
                    )}
                  </button>

                  <button
                    onClick={() => {
                      onUpdate(project.id, { status: 'archived' });
                      setMenuOpen(false);
                    }}
                    className="w-full px-3 py-2 text-left text-xs text-white hover:bg-white/8 flex items-center gap-2"
                  >
                    <Archive className="h-3 w-3" />
                    Archive
                  </button>

                  <div className="h-px bg-white/8 my-1" />

                  <button
                    onClick={() => {
                      if (confirm(`Delete "${project.name}"? This action cannot be undone.`)) {
                        onDelete(project.id);
                      }
                      setMenuOpen(false);
                    }}
                    className="w-full px-3 py-2 text-left text-xs text-red-400 hover:bg-red-500/10 flex items-center gap-2"
                  >
                    <Trash2 className="h-3 w-3" />
                    Delete Project
                  </button>
                </div>
              </>
            )}
          </div>
        </div>
      </div>

      <div className="flex items-center justify-between text-xs text-[var(--muted)]">
        <div className="flex items-center gap-4">
          <span>{project.stats.files} files</span>
          <span>{project.stats.tasks} tasks</span>
          {project.repoUrl && (
            <div className="flex items-center gap-1">
              <GitBranch className="h-3 w-3" />
              {project.branch || 'main'}
            </div>
          )}
        </div>
        <span>Updated {formatTimeAgo(project.updatedAt)}</span>
      </div>
    </div>
  );
}

export default function ProjectsList({ onCreateProject, onImportProject, onSelectProject }: ProjectsListProps) {
  const [projects, setProjects] = useState<Project[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState('');
  const [statusFilter, setStatusFilter] = useState<string>('all');

  const loadProjects = useCallback(async () => {
    try {
      setLoading(true);
      setError(null);
      const data = await listProjects();
      setProjects(data);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load projects');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadProjects();
  }, [loadProjects]);

  const handleUpdateProject = useCallback(async (
    projectId: string,
    updates: Partial<Project>
  ) => {
    try {
      await updateProject(projectId, updates);
      setProjects(prev =>
        prev.map(p => p.id === projectId ? { ...p, ...updates } : p)
      );
    } catch (err) {
      console.error('Failed to update project:', err);
    }
  }, []);

  const handleDeleteProject = useCallback(async (projectId: string) => {
    try {
      await deleteProject(projectId);
      setProjects(prev => prev.filter(p => p.id !== projectId));
    } catch (err) {
      console.error('Failed to delete project:', err);
    }
  }, []);

  const handleSyncProject = useCallback(async (projectId: string) => {
    try {
      await syncProject(projectId);
      // Refresh projects to get updated stats
      await loadProjects();
    } catch (err) {
      console.error('Failed to sync project:', err);
    }
  }, [loadProjects]);

  const handleOpenWorkspace = useCallback(async (projectId: string) => {
    try {
      const workspaceUrl = await getProjectWorkspaceUrl(projectId);
      window.open(workspaceUrl, '_blank');
    } catch (err) {
      console.error('Failed to open workspace:', err);
    }
  }, []);

  const filteredProjects = projects.filter(project => {
    const matchesSearch = project.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
      project.description.toLowerCase().includes(searchQuery.toLowerCase());

    const matchesStatus = statusFilter === 'all' || project.status === statusFilter;

    return matchesSearch && matchesStatus;
  });

  const statusCounts = projects.reduce(
    (acc, project) => {
      acc[project.status] = (acc[project.status] || 0) + 1;
      return acc;
    },
    {} as Record<string, number>
  );

  if (loading) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="flex items-center gap-2 text-[var(--muted)]">
          <Loader2 className="h-5 w-5 animate-spin" />
          Loading projects...
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="flex flex-col items-center justify-center h-64 gap-3">
        <p className="text-red-400">{error}</p>
        <button
          onClick={loadProjects}
          className="px-4 py-2 bg-[var(--accent)] text-black rounded-lg hover:brightness-110 transition"
        >
          Retry
        </button>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-xl font-semibold text-white">Projects</h1>
          <p className="text-sm text-[var(--muted)]">Manage your development projects</p>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={onImportProject}
            className="inline-flex items-center gap-2 px-4 py-2 bg-white/10 text-white border border-white/20 rounded-lg hover:bg-white/15 transition"
          >
            <Upload className="h-4 w-4" />
            Import
          </button>
          <button
            onClick={onCreateProject}
            className="inline-flex items-center gap-2 px-4 py-2 bg-[var(--accent)] text-black font-medium rounded-lg hover:brightness-110 transition"
          >
            <Plus className="h-4 w-4" />
            New Project
          </button>
        </div>
      </div>

      {/* Search and Filters */}
      <div className="flex items-center gap-4">
        <div className="relative flex-1 max-w-md">
          <Search className="absolute left-3 top-2.5 h-4 w-4 text-[var(--muted)]" />
          <input
            type="text"
            placeholder="Search projects..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            className="w-full pl-10 pr-4 py-2 bg-white/[0.03] border border-white/10 rounded-lg text-white placeholder-[var(--muted)] focus:border-[var(--accent)] focus:outline-none"
          />
        </div>

        <select
          value={statusFilter}
          onChange={(e) => setStatusFilter(e.target.value)}
          className="px-3 py-2 bg-white/[0.03] border border-white/10 rounded-lg text-white focus:border-[var(--accent)] focus:outline-none"
        >
          <option value="all">All Status ({projects.length})</option>
          <option value="active">Active ({statusCounts.active || 0})</option>
          <option value="paused">Paused ({statusCounts.paused || 0})</option>
          <option value="archived">Archived ({statusCounts.archived || 0})</option>
        </select>

        <button
          onClick={loadProjects}
          className="p-2 text-[var(--muted)] hover:text-white transition"
          title="Refresh projects"
        >
          <RefreshCw className="h-4 w-4" />
        </button>
      </div>

      {/* Projects Grid */}
      {filteredProjects.length === 0 ? (
        <div className="flex flex-col items-center justify-center h-64 text-center">
          <FolderOpen className="h-12 w-12 text-[var(--muted)] mb-4" />
          <h3 className="text-lg font-medium text-white mb-2">
            {searchQuery || statusFilter !== 'all' ? 'No projects found' : 'No projects yet'}
          </h3>
          <p className="text-sm text-[var(--muted)] mb-4">
            {searchQuery || statusFilter !== 'all'
              ? 'Try adjusting your search or filter criteria'
              : 'Create your first project to get started with Cortex'
            }
          </p>
          {(!searchQuery && statusFilter === 'all') && (
            <button
              onClick={onCreateProject}
              className="inline-flex items-center gap-2 px-4 py-2 bg-[var(--accent)] text-black font-medium rounded-lg hover:brightness-110 transition"
            >
              <Plus className="h-4 w-4" />
              Create First Project
            </button>
          )}
        </div>
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
          {filteredProjects.map(project => (
            <ProjectCard
              key={project.id}
              project={project}
              onSelect={onSelectProject}
              onUpdate={handleUpdateProject}
              onDelete={handleDeleteProject}
              onSync={handleSyncProject}
              onOpenWorkspace={handleOpenWorkspace}
            />
          ))}
        </div>
      )}
    </div>
  );
}