// Workspace routing utilities for BYOS integration

export interface WorkspaceContext {
  workspaceId?: string;
  projectId?: string;
}

/**
 * Extract workspace context from URL parameters
 */
export function getWorkspaceContext(): WorkspaceContext {
  const params = new URLSearchParams(window.location.search);
  return {
    workspaceId: params.get('workspace') || undefined,
    projectId: params.get('project') || undefined,
  };
}

/**
 * Check if current session should route to a workspace
 */
export function shouldRouteToWorkspace(): boolean {
  const { workspaceId } = getWorkspaceContext();
  return !!workspaceId;
}

/**
 * Get the modified user ID for workspace routing
 */
export function getWorkspaceUserId(originalUserId: string): string {
  const { workspaceId } = getWorkspaceContext();
  if (workspaceId) {
    return `workspace:${workspaceId}`;
  }
  return originalUserId;
}

/**
 * Get workspace-aware routing preferences for chat
 */
export function getWorkspaceRoutingPreferences() {
  const context = getWorkspaceContext();
  if (!context.workspaceId) {
    return undefined;
  }

  return {
    speed: 'balanced',
    intelligence: 'balanced',
    autonomy: 'guided',
    profile: 'workspace',
    model_tier: 'balanced',
  };
}

/**
 * Generate workspace status message for chat header
 */
export function getWorkspaceStatusMessage(): string | null {
  const { workspaceId, projectId } = getWorkspaceContext();
  if (!workspaceId) {
    return null;
  }

  return `🔧 Workspace Mode: ${workspaceId}${projectId ? ` (Project: ${projectId})` : ''}`;
}

/**
 * Update URL with workspace context (for programmatic navigation)
 */
export function setWorkspaceContext(context: WorkspaceContext) {
  const params = new URLSearchParams(window.location.search);

  if (context.workspaceId) {
    params.set('workspace', context.workspaceId);
  } else {
    params.delete('workspace');
  }

  if (context.projectId) {
    params.set('project', context.projectId);
  } else {
    params.delete('project');
  }

  const newUrl = `${window.location.pathname}?${params.toString()}`;
  window.history.replaceState({}, '', newUrl);
}

/**
 * Clear workspace context from URL
 */
export function clearWorkspaceContext() {
  setWorkspaceContext({});
}