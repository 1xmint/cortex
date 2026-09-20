import { useEffect, useState } from 'react';
import { CheckCircle, Container, Loader2, AlertTriangle } from 'lucide-react';

interface ContainerStatusProps {
  provider: string;
  status: 'provisioning' | 'ready' | 'error' | 'idle';
  message?: string;
}

export default function ContainerStatus({ provider, status, message }: ContainerStatusProps) {
  const [progress, setProgress] = useState(0);

  useEffect(() => {
    if (status === 'provisioning') {
      const interval = setInterval(() => {
        setProgress((prev) => Math.min(prev + Math.random() * 20, 85));
      }, 500);
      return () => clearInterval(interval);
    }
    if (status === 'ready') {
      setProgress(100);
    }
  }, [status]);

  if (status === 'idle') {
    return null;
  }

  const statusConfig = {
    provisioning: {
      icon: Loader2,
      color: 'text-blue-300',
      bg: 'bg-blue-500/10 border-blue-500/20',
      title: 'Setting up secure container...',
      description: 'Creating isolated environment for your AI agents'
    },
    ready: {
      icon: CheckCircle,
      color: 'text-emerald-300',
      bg: 'bg-emerald-500/10 border-emerald-500/20',
      title: 'Container ready',
      description: 'Your secure environment is ready for AI interactions'
    },
    error: {
      icon: AlertTriangle,
      color: 'text-red-300',
      bg: 'bg-red-500/10 border-red-500/20',
      title: 'Setup failed',
      description: message || 'Failed to create secure container'
    }
  };

  const config = statusConfig[status];
  const Icon = config.icon;

  return (
    <div className={`rounded-xl border p-4 ${config.bg}`}>
      <div className="flex items-start gap-3">
        <Icon className={`h-5 w-5 shrink-0 ${config.color} ${status === 'provisioning' ? 'animate-spin' : ''}`} />
        <div className="flex-1 min-w-0">
          <h3 className="text-sm font-medium text-white">{config.title}</h3>
          <p className="mt-1 text-xs text-[var(--muted)]">{config.description}</p>

          {status === 'provisioning' && (
            <div className="mt-3">
              <div className="flex items-center justify-between text-xs text-[var(--muted)] mb-1">
                <span>Progress</span>
                <span>{Math.round(progress)}%</span>
              </div>
              <div className="h-1.5 bg-white/10 rounded-full overflow-hidden">
                <div
                  className="h-full bg-blue-400 transition-all duration-500 ease-out"
                  style={{ width: `${progress}%` }}
                />
              </div>
            </div>
          )}

          <div className="flex items-center gap-2 mt-2 text-xs text-[var(--muted)]">
            <Container className="h-3.5 w-3.5" />
            <span>{provider.charAt(0).toUpperCase() + provider.slice(1)} container</span>
            {status === 'ready' && (
              <span className="text-emerald-300">• Secure & isolated</span>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}