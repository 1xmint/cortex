import { useCallback, useEffect, useState } from 'react';
import {
  ArrowLeft,
  ExternalLink,
  FileText,
  GitBranch,
  MessageSquare,
  Settings,
  Users,
  Activity,
  Folder,
  Code2,
  CheckCircle
} from 'lucide-react';
import {
  getProject,
  getProjectFiles,
  getProjectWorkspaceUrl,
  syncProject,
  type Project,
  type ProjectFileNode
} from '../../lib/projectApi';

interface ProjectDashboardProps {
  projectId: string;
  onBack: () => void;
  onOpenChat: (projectId: string) => void;
}

function ProjectStats({ project }: { project: Project }) {
  const stats = [
    {
      label: 'Files',
      value: project.stats.files,
      icon: FileText,
      color: 'text-blue-400'
    },
    {
      label: 'Tasks',
      value: project.stats.tasks,
      icon: CheckCircle,
      color: 'text-green-400'
    },
    {
      label: 'Commits',
      value: project.stats.commits,
      icon: GitBranch,
      color: 'text-purple-400'
    },
    {
      label: 'Collaborators',
      value: project.collaborators.length + 1, // +1 for owner
      icon: Users,
      color: 'text-orange-400'
    }
  ];

  return (
    <div className="grid grid-cols-2 lg:grid-cols-4 gap-4">
      {stats.map(({ label, value, icon: Icon, color }) => (
        <div key={label} className="p-4 bg-white/[0.02] border border-white/8 rounded-xl">
          <div className="flex items-center gap-3">
            <Icon className={`h-5 w-5 ${color}`} />
            <div>
              <div className="text-lg font-semibold text-white">{value.toLocaleString()}</div>
              <div className="text-xs text-[var(--muted)]">{label}</div>
            </div>
          </div>
        </div>
      ))}
    </div>
  );
}

function FileTree({ files }: { files: ProjectFileNode[] }) {
  const [expandedFolders, setExpandedFolders] = useState<Set<string>>(new Set());

  const toggleFolder = useCallback((path: string) => {
    setExpandedFolders(prev => {
      const next = new Set(prev);
      if (next.has(path)) {
        next.delete(path);
      } else {
        next.add(path);
      }
      return next;
    });
  }, []);

  const renderNode = (node: ProjectFileNode, depth = 0) => {
    const isExpanded = expandedFolders.has(node.path);

    return (
      <div key={node.path}>
        <div
          className="flex items-center gap-2 py-1 px-2 hover:bg-white/[0.03] rounded cursor-pointer"
          style={{ paddingLeft: `${depth * 16 + 8}px` }}
          onClick={() => node.type === 'directory' && toggleFolder(node.path)}
        >
          {node.type === 'directory' ? (
            <Folder className="h-4 w-4 text-blue-400" />
          ) : (
            <FileText className="h-4 w-4 text-[var(--muted)]" />
          )}
          <span className="text-sm text-white truncate">{node.name}</span>
          {node.size && (
            <span className="text-xs text-[var(--muted)] ml-auto">
              {(node.size / 1024).toFixed(1)}KB
            </span>
          )}
        </div>

        {node.type === 'directory' && isExpanded && node.children && (
          <div>
            {node.children.map(child => renderNode(child, depth + 1))}
          </div>
        )}
      </div>
    );
  };

  if (files.length === 0) {
    return (
      <div className="flex items-center justify-center h-32 text-[var(--muted)]">
        No files found
      </div>
    );
  }

  return (
    <div className="max-h-96 overflow-y-auto">
      {files.map(node => renderNode(node))}
    </div>
  );
}

function QuickActions({
  project,
  onOpenChat,
  onOpenWorkspace,
  onSync
}: {
  project: Project;
  onOpenChat: (projectId: string) => void;
  onOpenWorkspace: () => void;
  onSync: () => void;
}) {
  const actions = [
    {
      label: 'Open Chat',
      description: 'Start working with AI agents',
      icon: MessageSquare,
      onClick: () => onOpenChat(project.id),
      primary: true
    },
    {
      label: 'Open Workspace',
      description: 'Edit code in external editor',
      icon: ExternalLink,
      onClick: onOpenWorkspace
    },
    {
      label: 'View Code',
      description: 'Browse files and folders',
      icon: Code2,
      onClick: () => {} // TODO: Implement file browser
    }
  ];

  if (project.repoUrl) {
    actions.push({
      label: 'Sync Git',
      description: 'Pull latest changes',
      icon: GitBranch,
      onClick: onSync
    });
  }

  return (
    <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
      {actions.map(({ label, description, icon: Icon, onClick, primary }) => (
        <button
          key={label}
          onClick={onClick}
          className={`p-4 rounded-xl border text-left transition-all hover:scale-[1.02] ${
            primary
              ? 'border-[var(--accent)] bg-[var(--accent)] text-black'
              : 'border-white/8 bg-white/[0.02] hover:border-white/15'
          }`}
        >
          <div className="flex items-start gap-3">
            <Icon className="h-5 w-5 mt-0.5" />
            <div>
              <div className="font-medium">{label}</div>
              <div className={`text-xs mt-1 ${primary ? 'text-black/70' : 'text-[var(--muted)]'}`}>
                {description}
              </div>
            </div>
          </div>
        </button>
      ))}
    </div>
  );
}

function RecentActivity() {
  // Mock activity data - in real implementation, this would come from the API
  const activities = [
    {
      id: '1',
      type: 'task_completed',
      message: 'Task "Add user authentication" completed',
      timestamp: '2024-01-15T10:30:00Z'
    },
    {
      id: '2',
      type: 'file_modified',
      message: 'Modified src/components/Login.tsx',
      timestamp: '2024-01-15T09:15:00Z'
    },
    {
      id: '3',
      type: 'git_push',
      message: 'Pushed 3 commits to main branch',
      timestamp: '2024-01-15T08:45:00Z'
    }
  ];

  const formatTimeAgo = (timestamp: string) => {
    const date = new Date(timestamp);
    const now = new Date();
    const diffMs = now.getTime() - date.getTime();
    const diffHours = Math.floor(diffMs / (1000 * 60 * 60));

    if (diffHours < 1) return 'Just now';
    if (diffHours < 24) return `${diffHours}h ago`;
    return `${Math.floor(diffHours / 24)}d ago`;
  };

  return (
    <div className="space-y-3">
      {activities.map(activity => (
        <div key={activity.id} className="flex items-center gap-3 p-3 bg-white/[0.02] rounded-lg">
          <Activity className="h-4 w-4 text-[var(--accent)]" />
          <div className="flex-1">
            <p className="text-sm text-white">{activity.message}</p>
            <p className="text-xs text-[var(--muted)]">{formatTimeAgo(activity.timestamp)}</p>
          </div>
        </div>
      ))}
    </div>
  );
}

export default function ProjectDashboard({
  projectId,
  onBack,
  onOpenChat
}: ProjectDashboardProps) {
  const [project, setProject] = useState<Project | null>(null);
  const [files, setFiles] = useState<ProjectFileNode[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadProject = useCallback(async () => {
    try {
      setLoading(true);
      setError(null);

      const [projectData, filesData] = await Promise.all([
        getProject(projectId),
        getProjectFiles(projectId).catch(() => []) // Files might not be available
      ]);

      setProject(projectData);
      setFiles(filesData);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load project');
    } finally {
      setLoading(false);
    }
  }, [projectId]);

  useEffect(() => {
    loadProject();
  }, [loadProject]);

  const handleOpenWorkspace = useCallback(async () => {
    if (!project) return;

    try {
      const workspaceUrl = await getProjectWorkspaceUrl(project.id);
      window.open(workspaceUrl, '_blank');
    } catch (err) {
      console.error('Failed to open workspace:', err);
    }
  }, [project]);

  const handleSync = useCallback(async () => {
    if (!project) return;

    try {
      await syncProject(project.id);
      // Reload project data after sync
      await loadProject();
    } catch (err) {
      console.error('Failed to sync project:', err);
    }
  }, [project, loadProject]);

  if (loading) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="text-[var(--muted)]">Loading project...</div>
      </div>
    );
  }

  if (error || !project) {
    return (
      <div className="flex flex-col items-center justify-center h-64 gap-3">
        <p className="text-red-400">{error || 'Project not found'}</p>
        <button
          onClick={onBack}
          className="px-4 py-2 bg-[var(--accent)] text-black rounded-lg hover:brightness-110 transition"
        >
          Go Back
        </button>
      </div>
    );
  }

  const statusColor = {
    creating: 'text-blue-400 bg-blue-400/10',
    active: 'text-green-400 bg-green-400/10',
    paused: 'text-yellow-400 bg-yellow-400/10',
    archived: 'text-gray-400 bg-gray-400/10'
  }[project.status];

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-start justify-between">
        <div className="flex items-center gap-4">
          <button
            onClick={onBack}
            className="p-2 text-[var(--muted)] hover:text-white transition"
          >
            <ArrowLeft className="h-5 w-5" />
          </button>
          <div>
            <div className="flex items-center gap-3">
              <h1 className="text-2xl font-semibold text-white">{project.name}</h1>
              <span className={`px-2 py-1 rounded text-xs font-medium ${statusColor}`}>
                {project.status}
              </span>
            </div>
            {project.description && (
              <p className="text-[var(--muted)] mt-1">{project.description}</p>
            )}
            <div className="flex items-center gap-4 mt-2 text-xs text-[var(--muted)]">
              <span>Created {new Date(project.createdAt).toLocaleDateString()}</span>
              {project.repoUrl && (
                <div className="flex items-center gap-1">
                  <GitBranch className="h-3 w-3" />
                  <span>{project.branch || 'main'}</span>
                </div>
              )}
              <span>Owner: {project.owner}</span>
            </div>
          </div>
        </div>

        <button className="p-2 text-[var(--muted)] hover:text-white transition">
          <Settings className="h-5 w-5" />
        </button>
      </div>

      {/* Stats */}
      <ProjectStats project={project} />

      {/* Main Content */}
      <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
        {/* Quick Actions */}
        <div className="lg:col-span-2">
          <h2 className="text-lg font-semibold text-white mb-4">Quick Actions</h2>
          <QuickActions
            project={project}
            onOpenChat={onOpenChat}
            onOpenWorkspace={handleOpenWorkspace}
            onSync={handleSync}
          />
        </div>

        {/* Recent Activity */}
        <div>
          <h2 className="text-lg font-semibold text-white mb-4">Recent Activity</h2>
          <RecentActivity />
        </div>
      </div>

      {/* Project Files */}
      <div>
        <h2 className="text-lg font-semibold text-white mb-4">Project Files</h2>
        <div className="bg-white/[0.02] border border-white/8 rounded-xl p-4">
          <FileTree files={files} />
        </div>
      </div>
    </div>
  );
}