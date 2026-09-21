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



