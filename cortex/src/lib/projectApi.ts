// Import internal bearerFetch function or use direct fetch for now
// This would typically be done through the main cortexApi module

const API_BASE = import.meta.env.VITE_CORTEX_API as string | undefined ??
  (import.meta.env.DEV ? 'http://localhost:3001' : '');

async function fetchWithAuth(path: string, init?: RequestInit): Promise<Response> {
  // For now, use direct fetch - in production this should use auth tokens
  const url = `${API_BASE}${path}`;

  // Add basic headers
  const headers = new Headers(init?.headers);
  if (!headers.has('Content-Type') && init?.method && init.method !== 'GET') {
    headers.set('Content-Type', 'application/json');
  }

  const response = await fetch(url, { ...init, headers });

  if (!response.ok && response.status === 401) {
    // Dispatch unauthorized event for auth handling
    window.dispatchEvent(new CustomEvent('cortex:unauthorized'));
  }

  return response;
}

export interface ProjectTemplate {
  id: string;
  name: string;
  description: string;
  icon: string;
  features: string[];
  estimatedTime: string;
}

export interface Project {
  id: string;
  name: string;
  description: string;
  template?: ProjectTemplate;
  repoUrl?: string;
  branch?: string;
  status: 'creating' | 'active' | 'paused' | 'archived';
  createdAt: string;
  updatedAt: string;
  owner: string;
  collaborators: string[];
  stats: {
    files: number;
    commits: number;
    tasks: number;
    lastActivity: string;
  };
}

export interface CreateProjectRequest {
  name: string;
  description: string;
  template: ProjectTemplate | null;
  files?: FileList | null;
}

export interface ImportFromGitHubRequest {
  name: string;
  description: string;
  repoUrl: string;
  branch?: string;
}

export interface ProjectValidation {
  valid: boolean;
  error?: string;
}

/**
 * Validates a project name for uniqueness and format
 */
export async function validateProjectName(name: string): Promise<ProjectValidation> {
  try {
    const response = await fetchWithAuth('/api/projects/validate-name', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name }),
    });

    if (!response.ok) {
      const errorData = await response.json().catch(() => ({}));
      return {
        valid: false,
        error: errorData.error || 'Name validation failed'
      };
    }

    return await response.json();
  } catch {
    return {
      valid: false,
      error: 'Unable to validate project name'
    };
  }
}

/**
 * Creates a new project from a template
 */
export async function createProject(request: CreateProjectRequest): Promise<string> {
  const formData = new FormData();
  formData.append('name', request.name);
  formData.append('description', request.description);

  if (request.template) {
    formData.append('template', JSON.stringify(request.template));
  }

  if (request.files) {
    Array.from(request.files).forEach((file, index) => {
      formData.append(`file_${index}`, file);
    });
  }

  const response = await fetchWithAuth('/api/projects', {
    method: 'POST',
    body: formData,
  });

  if (!response.ok) {
    const errorData = await response.json().catch(() => ({}));
    throw new Error(errorData.error || 'Failed to create project');
  }

  const result = await response.json();
  return result.projectId;
}

/**
 * Imports a project from GitHub
 */
export async function importFromGitHub(request: ImportFromGitHubRequest): Promise<string> {
  const response = await fetchWithAuth('/api/projects/import/github', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(request),
  });

  if (!response.ok) {
    const errorData = await response.json().catch(() => ({}));
    throw new Error(errorData.error || 'Failed to import from GitHub');
  }

  const result = await response.json();
  return result.projectId;
}

/**
 * Lists all projects for the current user
 */
export async function listProjects(): Promise<Project[]> {
  const response = await fetchWithAuth('/api/projects');

  if (!response.ok) {
    throw new Error('Failed to fetch projects');
  }

  return await response.json();
}

/**
 * Gets details for a specific project
 */
export async function getProject(projectId: string): Promise<Project> {
  const response = await fetchWithAuth(`/api/projects/${projectId}`);

  if (!response.ok) {
    if (response.status === 404) {
      throw new Error('Project not found');
    }
    throw new Error('Failed to fetch project');
  }

  return await response.json();
}

/**
 * Updates project details
 */
export async function updateProject(
  projectId: string,
  updates: Partial<Pick<Project, 'name' | 'description' | 'status'>>
): Promise<Project> {
  const response = await fetchWithAuth(`/api/projects/${projectId}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(updates),
  });

  if (!response.ok) {
    const errorData = await response.json().catch(() => ({}));
    throw new Error(errorData.error || 'Failed to update project');
  }

  return await response.json();
}

/**
 * Deletes a project
 */
export async function deleteProject(projectId: string): Promise<void> {
  const response = await fetchWithAuth(`/api/projects/${projectId}`, {
    method: 'DELETE',
  });

  if (!response.ok) {
    const errorData = await response.json().catch(() => ({}));
    throw new Error(errorData.error || 'Failed to delete project');
  }
}

/**
 * Gets project file tree
 */
export async function getProjectFiles(projectId: string): Promise<ProjectFileNode[]> {
  const response = await fetchWithAuth(`/api/projects/${projectId}/files`);

  if (!response.ok) {
    throw new Error('Failed to fetch project files');
  }

  return await response.json();
}

export interface ProjectFileNode {
  name: string;
  path: string;
  type: 'file' | 'directory';
  size?: number;
  modifiedAt?: string;
  children?: ProjectFileNode[];
}

/**
 * Gets project workspace URL for opening in external tools
 */
export async function getProjectWorkspaceUrl(projectId: string): Promise<string> {
  const response = await fetchWithAuth(`/api/projects/${projectId}/workspace`);

  if (!response.ok) {
    throw new Error('Failed to get workspace URL');
  }

  const result = await response.json();
  return result.workspaceUrl;
}

/**
 * Syncs project with remote repository
 */
export async function syncProject(projectId: string): Promise<void> {
  const response = await fetchWithAuth(`/api/projects/${projectId}/sync`, {
    method: 'POST',
  });

  if (!response.ok) {
    const errorData = await response.json().catch(() => ({}));
    throw new Error(errorData.error || 'Failed to sync project');
  }
}

/**
 * Gets available project templates
 */
export async function getProjectTemplates(): Promise<ProjectTemplate[]> {
  try {
    const response = await fetchWithAuth('/api/projects/templates');

    if (!response.ok) {
      // Fall back to client-side templates if backend not available
      return getDefaultTemplates();
    }

    return await response.json();
  } catch {
    // Return default templates as fallback
    return getDefaultTemplates();
  }
}

function getDefaultTemplates(): ProjectTemplate[] {
  return [
    {
      id: 'web-app',
      name: 'Web Application',
      description: 'React/Next.js frontend with TypeScript',
      icon: '🌐',
      features: ['React/Next.js', 'TypeScript', 'Tailwind CSS', 'Vite'],
      estimatedTime: '5 minutes'
    },
    {
      id: 'api-service',
      name: 'API Service',
      description: 'REST API with Node.js/Express or Rust/Axum',
      icon: '🔌',
      features: ['REST endpoints', 'Database models', 'Authentication', 'OpenAPI docs'],
      estimatedTime: '10 minutes'
    },
    {
      id: 'full-stack',
      name: 'Full-Stack App',
      description: 'Complete web application with frontend and backend',
      icon: '🏗️',
      features: ['Frontend + Backend', 'Database', 'Auth system', 'Deployment ready'],
      estimatedTime: '15 minutes'
    },
    {
      id: 'mobile-app',
      name: 'Mobile App',
      description: 'React Native or Flutter mobile application',
      icon: '📱',
      features: ['Cross-platform', 'Navigation', 'State management', 'Native features'],
      estimatedTime: '20 minutes'
    },
    {
      id: 'blank',
      name: 'Blank Project',
      description: 'Start from scratch with basic structure',
      icon: '📝',
      features: ['Basic folder structure', 'Git repository', 'README template'],
      estimatedTime: '2 minutes'
    }
  ];
}