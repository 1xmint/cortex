import { useState, useCallback } from 'react';
import { X, GitBranch, Upload, ExternalLink, Loader2, CheckCircle, AlertCircle } from 'lucide-react';
import { importFromGitHub, validateProjectName, type ImportFromGitHubRequest } from '../../lib/projectApi';

interface ProjectImportModalProps {
  onClose: () => void;
  onComplete: (projectId: string) => void;
}

type ImportSource = 'github' | 'upload';

interface ImportState {
  source: ImportSource;
  name: string;
  description: string;
  githubRepo: string;
  githubBranch: string;
  files: FileList | null;
}

export default function ProjectImportModal({ onClose, onComplete }: ProjectImportModalProps) {
  const [state, setState] = useState<ImportState>({
    source: 'github',
    name: '',
    description: '',
    githubRepo: '',
    githubBranch: '',
    files: null
  });

  const [importing, setImporting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [nameValidation, setNameValidation] = useState<{ valid: boolean; error?: string } | null>(null);

  const updateState = useCallback((updates: Partial<ImportState>) => {
    setState(prev => ({ ...prev, ...updates }));
  }, []);

  const handleNameChange = useCallback(async (name: string) => {
    updateState({ name });

    if (name.length < 3) {
      setNameValidation({ valid: false, error: 'Project name must be at least 3 characters' });
      return;
    }

    try {
      const validation = await validateProjectName(name);
      setNameValidation(validation);
    } catch {
      setNameValidation({ valid: false, error: 'Unable to validate project name' });
    }
  }, [updateState]);

  const handleImport = useCallback(async () => {
    if (!state.name || !nameValidation?.valid) return;

    setImporting(true);
    setError(null);

    try {
      let projectId: string;

      if (state.source === 'github') {
        if (!state.githubRepo) {
          throw new Error('GitHub repository URL is required');
        }

        const request: ImportFromGitHubRequest = {
          name: state.name,
          description: state.description,
          repoUrl: state.githubRepo,
          branch: state.githubBranch || undefined
        };

        projectId = await importFromGitHub(request);
      } else {
        // TODO: Implement file upload import
        throw new Error('File upload import not yet implemented');
      }

      onComplete(projectId);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Import failed');
    } finally {
      setImporting(false);
    }
  }, [state, nameValidation, onComplete]);

  const canImport = state.name.length >= 3 && nameValidation?.valid &&
    (state.source === 'github' ? state.githubRepo : state.files);

  return (
    <div className="fixed inset-0 bg-black/80 backdrop-blur-sm flex items-center justify-center p-4 z-50">
      <div className="bg-[var(--panel)] border border-white/10 rounded-2xl p-6 w-full max-w-md max-h-[90vh] overflow-y-auto">
        <div className="flex items-center justify-between mb-6">
          <h1 className="text-lg font-semibold text-white">Import Project</h1>
          <button
            onClick={onClose}
            className="p-1 text-[var(--muted)] hover:text-white transition"
          >
            <X className="h-5 w-5" />
          </button>
        </div>

        <div className="space-y-6">
          {/* Import Source */}
          <div>
            <label className="block text-sm font-medium text-white mb-3">Import Source</label>
            <div className="grid grid-cols-2 gap-2">
              <button
                onClick={() => updateState({ source: 'github' })}
                className={`p-3 rounded-xl border text-left transition ${
                  state.source === 'github'
                    ? 'border-[var(--accent)] bg-[var(--accent)]/10'
                    : 'border-white/8 bg-white/[0.02] hover:border-white/15'
                }`}
              >
                <GitBranch className={`h-5 w-5 mb-2 ${
                  state.source === 'github' ? 'text-[var(--accent)]' : 'text-[var(--muted)]'
                }`} />
                <div className="font-medium text-white text-sm">GitHub</div>
                <div className="text-xs text-[var(--muted)]">Clone repository</div>
              </button>

              <button
                onClick={() => updateState({ source: 'upload' })}
                className={`p-3 rounded-xl border text-left transition ${
                  state.source === 'upload'
                    ? 'border-[var(--accent)] bg-[var(--accent)]/10'
                    : 'border-white/8 bg-white/[0.02] hover:border-white/15'
                }`}
              >
                <Upload className={`h-5 w-5 mb-2 ${
                  state.source === 'upload' ? 'text-[var(--accent)]' : 'text-[var(--muted)]'
                }`} />
                <div className="font-medium text-white text-sm">Upload</div>
                <div className="text-xs text-[var(--muted)]">ZIP archive</div>
              </button>
            </div>
          </div>

          {/* Project Details */}
          <div className="space-y-4">
            <div>
              <label className="block text-sm font-medium text-white mb-2">
                Project Name *
              </label>
              <div className="relative">
                <input
                  type="text"
                  value={state.name}
                  onChange={(e) => handleNameChange(e.target.value)}
                  placeholder="my-project"
                  className="w-full px-3 py-2 bg-white/[0.03] border border-white/10 rounded-lg text-white placeholder-[var(--muted)] focus:border-[var(--accent)] focus:outline-none"
                />
                {nameValidation && (
                  <div className="absolute right-3 top-2.5">
                    {nameValidation.valid ? (
                      <CheckCircle className="h-4 w-4 text-green-400" />
                    ) : (
                      <AlertCircle className="h-4 w-4 text-red-400" />
                    )}
                  </div>
                )}
              </div>
              {nameValidation && !nameValidation.valid && (
                <p className="mt-1 text-xs text-red-400">{nameValidation.error}</p>
              )}
            </div>

            <div>
              <label className="block text-sm font-medium text-white mb-2">
                Description
              </label>
              <textarea
                value={state.description}
                onChange={(e) => updateState({ description: e.target.value })}
                placeholder="Describe your project..."
                rows={2}
                className="w-full px-3 py-2 bg-white/[0.03] border border-white/10 rounded-lg text-white placeholder-[var(--muted)] focus:border-[var(--accent)] focus:outline-none resize-none"
              />
            </div>
          </div>

          {/* GitHub Fields */}
          {state.source === 'github' && (
            <div className="space-y-4 p-4 bg-white/[0.02] rounded-xl border border-white/8">
              <div>
                <label className="block text-sm font-medium text-white mb-2">
                  Repository URL *
                </label>
                <input
                  type="url"
                  value={state.githubRepo}
                  onChange={(e) => updateState({ githubRepo: e.target.value })}
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
                  value={state.githubBranch}
                  onChange={(e) => updateState({ githubBranch: e.target.value })}
                  placeholder="main"
                  className="w-full px-3 py-2 bg-white/[0.03] border border-white/10 rounded-lg text-white placeholder-[var(--muted)] focus:border-[var(--accent)] focus:outline-none"
                />
              </div>
            </div>
          )}

          {/* Upload Fields */}
          {state.source === 'upload' && (
            <div className="p-4 bg-white/[0.02] rounded-xl border border-white/8">
              <div className="p-6 border-2 border-dashed border-white/20 rounded-xl text-center">
                <Upload className="h-8 w-8 text-[var(--muted)] mx-auto mb-2" />
                <p className="text-sm text-white mb-1">Drag & drop your files here</p>
                <p className="text-xs text-[var(--muted)] mb-3">or click to browse</p>
                <input
                  type="file"
                  multiple
                  accept=".zip,.tar,.tar.gz"
                  onChange={(e) => updateState({ files: e.target.files })}
                  className="hidden"
                  id="file-upload"
                />
                <label
                  htmlFor="file-upload"
                  className="inline-block px-4 py-2 bg-white/10 text-white rounded-lg cursor-pointer hover:bg-white/15 transition"
                >
                  Choose Files
                </label>
                {state.files && (
                  <div className="mt-3 text-xs text-[var(--muted)]">
                    {state.files.length} file(s) selected
                  </div>
                )}
              </div>
            </div>
          )}

          {error && (
            <div className="p-3 bg-red-500/10 border border-red-500/20 rounded-lg">
              <p className="text-xs text-red-200">{error}</p>
            </div>
          )}

          {/* Actions */}
          <div className="flex gap-3">
            <button
              onClick={onClose}
              disabled={importing}
              className="flex-1 px-4 py-2 text-[var(--muted)] hover:text-white transition disabled:opacity-50"
            >
              Cancel
            </button>
            <button
              onClick={handleImport}
              disabled={!canImport || importing}
              className="flex-1 inline-flex items-center justify-center gap-2 px-4 py-2 bg-[var(--accent)] text-black font-medium rounded-lg transition hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {importing ? (
                <>
                  <Loader2 className="h-4 w-4 animate-spin" />
                  Importing...
                </>
              ) : (
                <>
                  Import Project
                  <ExternalLink className="h-4 w-4" />
                </>
              )}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}