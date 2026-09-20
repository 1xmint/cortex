import { useCallback, useState } from 'react';
import { useParams, useNavigate } from 'react-router';
import { X } from 'lucide-react';
import ProjectsList from './ProjectsList';
import ProjectDashboard from './ProjectDashboard';
import ProjectSetupWizard from './ProjectSetupWizard';
import ProjectImportModal from './ProjectImportModal';
import { useAuthGate } from '../../lib/useAuthGate';
import SignInScreen from '../auth/SignInScreen';
import type { Project } from '../../lib/projectApi';

export default function ProjectsView() {
  const { projectId } = useParams<{ projectId: string }>();
  const navigate = useNavigate();
  const { isLoaded, isSignedIn, clerkEnabled } = useAuthGate();
  const [showSetupWizard, setShowSetupWizard] = useState(false);
  const [showImportModal, setShowImportModal] = useState(false);

  const handleCreateProject = useCallback(() => {
    setShowSetupWizard(true);
  }, []);

  const handleImportProject = useCallback(() => {
    setShowImportModal(true);
  }, []);

  const handleSelectProject = useCallback((project: Project) => {
    navigate(`/projects/${project.id}`);
  }, [navigate]);

  const handleBackToList = useCallback(() => {
    navigate('/projects');
  }, [navigate]);

  const handleSetupComplete = useCallback((newProjectId: string) => {
    setShowSetupWizard(false);
    navigate(`/projects/${newProjectId}`);
  }, [navigate]);

  const handleSetupCancel = useCallback(() => {
    setShowSetupWizard(false);
  }, []);

  const handleImportComplete = useCallback((newProjectId: string) => {
    setShowImportModal(false);
    navigate(`/projects/${newProjectId}`);
  }, [navigate]);

  const handleImportCancel = useCallback(() => {
    setShowImportModal(false);
  }, []);

  const handleOpenProjectChat = useCallback((projectId: string) => {
    // Navigate to project chat - for now, redirect to main chat with project context
    navigate(`/?project=${projectId}`);
  }, [navigate]);

  // Loading state
  if (!isLoaded) {
    return (
      <div className="min-h-screen bg-[var(--bg)] flex items-center justify-center">
        <div className="text-[var(--muted)]">Loading...</div>
      </div>
    );
  }

  // Auth gate
  if (clerkEnabled && !isSignedIn) {
    return <SignInScreen />;
  }

  return (
    <div className="min-h-screen bg-[var(--bg)] text-[var(--fg)]">
      {/* Header */}
      <header className="border-b border-white/6 bg-[var(--panel)]">
        <div className="flex items-center justify-between px-6 py-4">
          <div className="flex items-center gap-4">
            <button
              onClick={() => navigate('/')}
              className="p-1 text-[var(--muted)] hover:text-white transition"
              title="Back to Cortex"
            >
              <X className="h-5 w-5" />
            </button>
            <div>
              <h1 className="text-lg font-semibold text-white">Projects</h1>
              <p className="text-xs text-[var(--muted)]">
                {projectId ? 'Project Dashboard' : 'Manage your development projects'}
              </p>
            </div>
          </div>
        </div>
      </header>

      {/* Main Content */}
      <main className="container mx-auto px-6 py-8 max-w-7xl">
        {projectId ? (
          <ProjectDashboard
            projectId={projectId}
            onBack={handleBackToList}
            onOpenChat={handleOpenProjectChat}
          />
        ) : (
          <ProjectsList
            onCreateProject={handleCreateProject}
            onImportProject={handleImportProject}
            onSelectProject={handleSelectProject}
          />
        )}
      </main>

      {/* Setup Wizard Modal */}
      {showSetupWizard && (
        <ProjectSetupWizard
          onComplete={handleSetupComplete}
          onCancel={handleSetupCancel}
        />
      )}

      {/* Import Modal */}
      {showImportModal && (
        <ProjectImportModal
          onComplete={handleImportComplete}
          onClose={handleImportCancel}
        />
      )}
    </div>
  );
}