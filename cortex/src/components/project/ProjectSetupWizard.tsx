import { useState, useCallback } from 'react';
import { ArrowLeft, ArrowRight, CheckCircle, FolderPlus, GitBranch, Upload, FileText, Loader2, X } from 'lucide-react';
import { createProject, importFromGitHub, validateProjectName, type ProjectTemplate } from '../../lib/projectApi';

interface ProjectSetupWizardProps {
  onComplete: (projectId: string) => void;
  onCancel: () => void;
  initialStep?: number;
}

type SetupStep = 'template' | 'details' | 'import' | 'configure' | 'review';

interface ProjectConfig {
  name: string;
  description: string;
  template: ProjectTemplate | null;
  importSource: 'new' | 'github' | 'upload';
  githubRepo?: string;
  githubBranch?: string;
  localFiles?: FileList | null;
}

const PROJECT_TEMPLATES: ProjectTemplate[] = [
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

function ProgressIndicator({ currentStep }: { currentStep: SetupStep }) {
  const steps: SetupStep[] = ['template', 'details', 'import', 'configure', 'review'];
  const currentIndex = steps.indexOf(currentStep);

  return (
    <div className="flex items-center justify-center gap-2 mb-8">
      {steps.map((step, index) => (
        <div key={step} className="flex items-center">
          <div
            className={`h-8 w-8 rounded-full flex items-center justify-center text-sm font-medium transition-colors ${
              index < currentIndex
                ? 'bg-[var(--accent)] text-black'
                : index === currentIndex
                ? 'bg-[var(--accent)]/20 border-2 border-[var(--accent)] text-[var(--accent)]'
                : 'bg-white/10 text-[var(--muted)]'
            }`}
          >
            {index < currentIndex ? (
              <CheckCircle className="h-4 w-4" />
            ) : (
              index + 1
            )}
          </div>
          {index < steps.length - 1 && (
            <div
              className={`h-0.5 w-8 mx-2 transition-colors ${
                index < currentIndex ? 'bg-[var(--accent)]' : 'bg-white/10'
              }`}
            />
          )}
        </div>
      ))}
    </div>
  );
}

function TemplateStep({
  config,
  onUpdate,
  onNext
}: {
  config: ProjectConfig;
  onUpdate: (updates: Partial<ProjectConfig>) => void;
  onNext: () => void;
}) {
  return (
    <div className="space-y-6">
      <div className="text-center">
        <h2 className="text-xl font-semibold text-white mb-2">Choose a Template</h2>
        <p className="text-sm text-[var(--muted)]">
          Select a starting point for your project, or start from scratch
        </p>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        {PROJECT_TEMPLATES.map((template) => (
          <button
            key={template.id}
            onClick={() => onUpdate({ template, importSource: 'new' })}
            className={`p-4 rounded-xl border text-left transition-all ${
              config.template?.id === template.id
                ? 'border-[var(--accent)] bg-[var(--accent)]/10'
                : 'border-white/8 bg-white/[0.02] hover:border-white/15 hover:bg-white/[0.04]'
            }`}
          >
            <div className="flex items-start gap-3">
              <span className="text-2xl">{template.icon}</span>
              <div className="flex-1">
                <h3 className="font-medium text-white mb-1">{template.name}</h3>
                <p className="text-xs text-[var(--muted)] mb-3">{template.description}</p>
                <div className="flex flex-wrap gap-1 mb-2">
                  {template.features.map((feature) => (
                    <span
                      key={feature}
                      className="px-2 py-0.5 bg-white/10 rounded text-[10px] text-[var(--muted)]"
                    >
                      {feature}
                    </span>
                  ))}
                </div>
                <div className="text-[10px] text-[var(--accent)]">
                  Est. {template.estimatedTime}
                </div>
              </div>
            </div>
          </button>
        ))}
      </div>

      <div className="flex justify-center">
        <button
          onClick={onNext}
          disabled={!config.template}
          className="inline-flex items-center gap-2 px-6 py-3 bg-[var(--accent)] text-black font-medium rounded-lg transition hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          Continue
          <ArrowRight className="h-4 w-4" />
        </button>
      </div>
    </div>
  );
}

function DetailsStep({
  config,
  onUpdate,
  onNext,
  onBack
}: {
  config: ProjectConfig;
  onUpdate: (updates: Partial<ProjectConfig>) => void;
  onNext: () => void;
  onBack: () => void;
}) {
  const [nameError, setNameError] = useState<string | null>(null);
  const [validating, setValidating] = useState(false);

  const handleNameChange = useCallback(async (name: string) => {
    onUpdate({ name });

    if (name.length < 3) {
      setNameError('Project name must be at least 3 characters');
      return;
    }

    setValidating(true);
    setNameError(null);

    try {
      const validation = await validateProjectName(name);
      if (!validation.valid) {
        setNameError(validation.error || 'Project name is not available');
      }
    } catch {
      setNameError('Unable to validate project name');
    } finally {
      setValidating(false);
    }
  }, [onUpdate]);

  const canProceed = config.name.length >= 3 && !nameError && !validating;

  return (
    <div className="space-y-6">
      <div className="text-center">
        <h2 className="text-xl font-semibold text-white mb-2">Project Details</h2>
        <p className="text-sm text-[var(--muted)]">
          Give your project a name and description
        </p>
      </div>

      <div className="max-w-md mx-auto space-y-4">
        <div>
          <label className="block text-sm font-medium text-white mb-2">
            Project Name *
          </label>
          <div className="relative">
            <input
              type="text"
              value={config.name}
              onChange={(e) => handleNameChange(e.target.value)}
              placeholder="my-awesome-project"
              className="w-full px-3 py-2 bg-white/[0.03] border border-white/10 rounded-lg text-white placeholder-[var(--muted)] focus:border-[var(--accent)] focus:outline-none"
            />
            {validating && (
              <Loader2 className="absolute right-3 top-2.5 h-4 w-4 animate-spin text-[var(--muted)]" />
            )}
          </div>
          {nameError && (
            <p className="mt-1 text-xs text-red-400">{nameError}</p>
          )}
          <p className="mt-1 text-xs text-[var(--muted)]">
            This will be your project identifier and folder name
          </p>
        </div>

        <div>
          <label className="block text-sm font-medium text-white mb-2">
            Description
          </label>
          <textarea
            value={config.description}
            onChange={(e) => onUpdate({ description: e.target.value })}
            placeholder="Describe what your project does..."
            rows={3}
            className="w-full px-3 py-2 bg-white/[0.03] border border-white/10 rounded-lg text-white placeholder-[var(--muted)] focus:border-[var(--accent)] focus:outline-none resize-none"
          />
        </div>

        <div className="p-3 bg-[var(--accent)]/10 border border-[var(--accent)]/20 rounded-lg">
          <div className="flex items-center gap-2 mb-2">
            <span className="text-lg">{config.template?.icon}</span>
            <span className="font-medium text-white">{config.template?.name}</span>
          </div>
          <p className="text-xs text-[var(--muted)]">{config.template?.description}</p>
        </div>
      </div>

      <div className="flex justify-center gap-3">
        <button
          onClick={onBack}
          className="inline-flex items-center gap-2 px-4 py-2 text-[var(--muted)] hover:text-white transition"
        >
          <ArrowLeft className="h-4 w-4" />
          Back
        </button>
        <button
          onClick={onNext}
          disabled={!canProceed}
          className="inline-flex items-center gap-2 px-6 py-3 bg-[var(--accent)] text-black font-medium rounded-lg transition hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          Continue
          <ArrowRight className="h-4 w-4" />
        </button>
      </div>
    </div>
  );
}

function ImportStep({
  config,
  onUpdate,
  onNext,
  onBack
}: {
  config: ProjectConfig;
  onUpdate: (updates: Partial<ProjectConfig>) => void;
  onNext: () => void;
  onBack: () => void;
}) {
  const importOptions = [
    {
      id: 'new' as const,
      icon: FolderPlus,
      title: 'Create New',
      description: 'Start fresh with the selected template',
      recommended: true
    },
    {
      id: 'github' as const,
      icon: GitBranch,
      title: 'Import from GitHub',
      description: 'Clone an existing repository'
    },
    {
      id: 'upload' as const,
      icon: Upload,
      title: 'Upload Files',
      description: 'Upload a zip file or drag & drop'
    }
  ];

  return (
    <div className="space-y-6">
      <div className="text-center">
        <h2 className="text-xl font-semibold text-white mb-2">Import Source</h2>
        <p className="text-sm text-[var(--muted)]">
          How would you like to start your project?
        </p>
      </div>

      <div className="max-w-md mx-auto space-y-3">
        {importOptions.map((option) => {
          const Icon = option.icon;
          const selected = config.importSource === option.id;

          return (
            <button
              key={option.id}
              onClick={() => onUpdate({ importSource: option.id })}
              className={`w-full p-4 rounded-xl border text-left transition-all ${
                selected
                  ? 'border-[var(--accent)] bg-[var(--accent)]/10'
                  : 'border-white/8 bg-white/[0.02] hover:border-white/15'
              }`}
            >
              <div className="flex items-center gap-3">
                <div className={`p-2 rounded-lg ${selected ? 'bg-[var(--accent)]/20' : 'bg-white/10'}`}>
                  <Icon className={`h-5 w-5 ${selected ? 'text-[var(--accent)]' : 'text-[var(--muted)]'}`} />
                </div>
                <div className="flex-1">
                  <div className="flex items-center gap-2">
                    <h3 className="font-medium text-white">{option.title}</h3>
                    {option.recommended && (
                      <span className="px-2 py-0.5 bg-[var(--accent)]/20 text-[var(--accent)] text-[10px] rounded">
                        Recommended
                      </span>
                    )}
                  </div>
                  <p className="text-xs text-[var(--muted)]">{option.description}</p>
                </div>
              </div>
            </button>
          );
        })}
      </div>

      {config.importSource === 'github' && (
        <div className="max-w-md mx-auto space-y-3 p-4 bg-white/[0.02] rounded-xl border border-white/8">
          <div>
            <label className="block text-sm font-medium text-white mb-2">
              GitHub Repository URL
            </label>
            <input
              type="url"
              value={config.githubRepo || ''}
              onChange={(e) => onUpdate({ githubRepo: e.target.value })}
              placeholder="https://github.com/username/repo"
              className="w-full px-3 py-2 bg-white/[0.03] border border-white/10 rounded-lg text-white placeholder-[var(--muted)] focus:border-[var(--accent)] focus:outline-none"
            />
          </div>
          <div>
            <label className="block text-sm font-medium text-white mb-2">
              Branch (optional)
            </label>
            <input
              type="text"
              value={config.githubBranch || ''}
              onChange={(e) => onUpdate({ githubBranch: e.target.value })}
              placeholder="main"
              className="w-full px-3 py-2 bg-white/[0.03] border border-white/10 rounded-lg text-white placeholder-[var(--muted)] focus:border-[var(--accent)] focus:outline-none"
            />
          </div>
        </div>
      )}

      {config.importSource === 'upload' && (
        <div className="max-w-md mx-auto">
          <div className="p-6 border-2 border-dashed border-white/20 rounded-xl text-center">
            <Upload className="h-8 w-8 text-[var(--muted)] mx-auto mb-2" />
            <p className="text-sm text-white mb-1">Drag & drop your files here</p>
            <p className="text-xs text-[var(--muted)] mb-3">or click to browse</p>
            <input
              type="file"
              multiple
              accept=".zip,.tar,.tar.gz"
              onChange={(e) => onUpdate({ localFiles: e.target.files })}
              className="hidden"
              id="file-upload"
            />
            <label
              htmlFor="file-upload"
              className="inline-block px-4 py-2 bg-white/10 text-white rounded-lg cursor-pointer hover:bg-white/15 transition"
            >
              Choose Files
            </label>
          </div>
        </div>
      )}

      <div className="flex justify-center gap-3">
        <button
          onClick={onBack}
          className="inline-flex items-center gap-2 px-4 py-2 text-[var(--muted)] hover:text-white transition"
        >
          <ArrowLeft className="h-4 w-4" />
          Back
        </button>
        <button
          onClick={onNext}
          className="inline-flex items-center gap-2 px-6 py-3 bg-[var(--accent)] text-black font-medium rounded-lg transition hover:brightness-110"
        >
          Continue
          <ArrowRight className="h-4 w-4" />
        </button>
      </div>
    </div>
  );
}

function ReviewStep({
  config,
  onComplete,
  onBack
}: {
  config: ProjectConfig;
  onComplete: (projectId: string) => void;
  onBack: () => void;
}) {
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleCreate = useCallback(async () => {
    setCreating(true);
    setError(null);

    try {
      let projectId: string;

      if (config.importSource === 'github' && config.githubRepo) {
        projectId = await importFromGitHub({
          name: config.name,
          description: config.description,
          repoUrl: config.githubRepo,
          branch: config.githubBranch
        });
      } else {
        projectId = await createProject({
          name: config.name,
          description: config.description,
          template: config.template,
          files: config.localFiles
        });
      }

      onComplete(projectId);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to create project');
    } finally {
      setCreating(false);
    }
  }, [config, onComplete]);

  return (
    <div className="space-y-6">
      <div className="text-center">
        <h2 className="text-xl font-semibold text-white mb-2">Review & Create</h2>
        <p className="text-sm text-[var(--muted)]">
          Ready to create your project? Review the settings below.
        </p>
      </div>

      <div className="max-w-md mx-auto space-y-4">
        <div className="p-4 bg-white/[0.02] rounded-xl border border-white/8">
          <h3 className="font-medium text-white mb-3">Project Summary</h3>

          <div className="space-y-3 text-sm">
            <div className="flex justify-between">
              <span className="text-[var(--muted)]">Name:</span>
              <span className="text-white font-medium">{config.name}</span>
            </div>

            <div className="flex justify-between">
              <span className="text-[var(--muted)]">Template:</span>
              <span className="text-white">{config.template?.name}</span>
            </div>

            <div className="flex justify-between">
              <span className="text-[var(--muted)]">Source:</span>
              <span className="text-white capitalize">{config.importSource}</span>
            </div>

            {config.description && (
              <div>
                <span className="text-[var(--muted)]">Description:</span>
                <p className="text-white mt-1">{config.description}</p>
              </div>
            )}

            {config.importSource === 'github' && config.githubRepo && (
              <div>
                <span className="text-[var(--muted)]">Repository:</span>
                <p className="text-white mt-1 font-mono text-xs">{config.githubRepo}</p>
              </div>
            )}
          </div>
        </div>

        <div className="p-3 bg-blue-500/10 border border-blue-500/20 rounded-lg">
          <div className="flex items-start gap-2">
            <FileText className="h-4 w-4 text-blue-400 mt-0.5" />
            <div>
              <p className="text-xs text-blue-200 font-medium">What happens next?</p>
              <p className="text-xs text-blue-300/80 mt-1">
                Your project will be set up with the selected template, and you'll be redirected
                to the project workspace where you can start coding immediately.
              </p>
            </div>
          </div>
        </div>

        {error && (
          <div className="p-3 bg-red-500/10 border border-red-500/20 rounded-lg">
            <p className="text-xs text-red-200">{error}</p>
          </div>
        )}
      </div>

      <div className="flex justify-center gap-3">
        <button
          onClick={onBack}
          disabled={creating}
          className="inline-flex items-center gap-2 px-4 py-2 text-[var(--muted)] hover:text-white transition disabled:opacity-50"
        >
          <ArrowLeft className="h-4 w-4" />
          Back
        </button>
        <button
          onClick={handleCreate}
          disabled={creating}
          className="inline-flex items-center gap-2 px-6 py-3 bg-[var(--accent)] text-black font-medium rounded-lg transition hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {creating ? (
            <>
              <Loader2 className="h-4 w-4 animate-spin" />
              Creating...
            </>
          ) : (
            <>
              Create Project
              <CheckCircle className="h-4 w-4" />
            </>
          )}
        </button>
      </div>
    </div>
  );
}

export default function ProjectSetupWizard({
  onComplete,
  onCancel,
  initialStep = 0
}: ProjectSetupWizardProps) {
  const steps: SetupStep[] = ['template', 'details', 'import', 'configure', 'review'];
  const [currentStepIndex, setCurrentStepIndex] = useState(initialStep);
  const [config, setConfig] = useState<ProjectConfig>({
    name: '',
    description: '',
    template: null,
    importSource: 'new'
  });

  const currentStep = steps[currentStepIndex];

  const updateConfig = useCallback((updates: Partial<ProjectConfig>) => {
    setConfig(prev => ({ ...prev, ...updates }));
  }, []);

  const nextStep = useCallback(() => {
    setCurrentStepIndex(prev => Math.min(prev + 1, steps.length - 1));
  }, []);

  const prevStep = useCallback(() => {
    setCurrentStepIndex(prev => Math.max(prev - 1, 0));
  }, []);


  return (
    <div className="fixed inset-0 bg-black/80 backdrop-blur-sm flex items-center justify-center p-4 z-50">
      <div className="bg-[var(--panel)] border border-white/10 rounded-2xl p-6 w-full max-w-3xl max-h-[90vh] overflow-y-auto">
        <div className="flex items-center justify-between mb-6">
          <h1 className="text-lg font-semibold text-white">Create New Project</h1>
          <button
            onClick={onCancel}
            className="p-1 text-[var(--muted)] hover:text-white transition"
          >
            <X className="h-5 w-5" />
          </button>
        </div>

        <ProgressIndicator currentStep={currentStep} />

        {currentStep === 'template' && (
          <TemplateStep
            config={config}
            onUpdate={updateConfig}
            onNext={nextStep}
          />
        )}

        {currentStep === 'details' && (
          <DetailsStep
            config={config}
            onUpdate={updateConfig}
            onNext={nextStep}
            onBack={prevStep}
          />
        )}

        {currentStep === 'import' && (
          <ImportStep
            config={config}
            onUpdate={updateConfig}
            onNext={nextStep}
            onBack={prevStep}
          />
        )}

        {currentStep === 'review' && (
          <ReviewStep
            config={config}
            onComplete={onComplete}
            onBack={prevStep}
          />
        )}
      </div>
    </div>
  );
}