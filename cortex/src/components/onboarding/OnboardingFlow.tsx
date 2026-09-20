import { useState } from 'react';
import { ArrowLeft, ArrowRight, CheckCircle, Code2, MessageSquare, Settings, Shield, Terminal, FolderPlus } from 'lucide-react';
import { markOnboardingComplete, saveOnboardingStep, getOnboardingStep } from '../../lib/onboarding';
import ProviderStep from './ProviderStep';

interface OnboardingFlowProps {
  userId: string;
  onComplete: () => void;
}

const TOTAL_STEPS = 4;

function ProgressDots({ current }: { current: number }) {
  return (
    <div className="flex items-center justify-center gap-2">
      {Array.from({ length: TOTAL_STEPS }).map((_, i) => (
        <span
          key={i}
          className={`h-1.5 rounded-full transition-all duration-200 ${
            i === current
              ? 'w-5 bg-[var(--accent)]'
              : i < current
              ? 'w-1.5 bg-[var(--accent)]/50'
              : 'w-1.5 bg-white/15'
          }`}
        />
      ))}
    </div>
  );
}

// Step 1: Welcome
function WelcomeStep({ onNext, onSkip }: { onNext: () => void; onSkip: () => void }) {
  return (
    <div className="flex flex-col gap-6">
      <div className="text-center">
        <div className="mx-auto mb-4 flex h-14 w-14 items-center justify-center rounded-2xl bg-[var(--accent-soft)]">
          <Terminal className="h-7 w-7 text-[var(--accent)]" />
        </div>
        <h1 className="text-xl font-semibold text-white">Welcome to Cortex</h1>
        <p className="mt-2 text-sm text-[var(--muted)]">
          Your personal AI orchestration center. Transform ideas into shipped code with sovereign AI agents.
        </p>
      </div>

      <div className="rounded-xl border border-[var(--accent)]/20 bg-[var(--accent-soft)] p-4 text-center">
        <p className="text-xs text-[var(--muted)]">
          <span className="font-medium text-[var(--accent)]">⚡ Setup takes ~3 minutes</span>
          <br />
          Connect your AI subscriptions and start building immediately.
        </p>
      </div>

      <ul className="flex flex-col gap-3">
        <li className="flex items-start gap-3 rounded-xl border border-white/8 bg-white/[0.025] px-4 py-3">
          <CheckCircle className="mt-0.5 h-4 w-4 shrink-0 text-[var(--accent)]" />
          <div>
            <span className="text-sm font-medium text-white">Natural language commands</span>
            <p className="mt-0.5 text-xs text-[var(--muted)]">
              "Fix the login bug" → AI agents analyze, code, test, and submit PRs automatically.
            </p>
          </div>
        </li>
        <li className="flex items-start gap-3 rounded-xl border border-white/8 bg-white/[0.025] px-4 py-3">
          <Code2 className="mt-0.5 h-4 w-4 shrink-0 text-[var(--accent)]" />
          <div>
            <span className="text-sm font-medium text-white">Dual-provider orchestration</span>
            <p className="mt-0.5 text-xs text-[var(--muted)]">
              Route work between Claude and OpenAI based on complexity and your preferences.
            </p>
          </div>
        </li>
        <li className="flex items-start gap-3 rounded-xl border border-white/8 bg-white/[0.025] px-4 py-3">
          <Shield className="mt-0.5 h-4 w-4 shrink-0 text-[var(--accent)]" />
          <div>
            <span className="text-sm font-medium text-white">Your subscriptions, your control</span>
            <p className="mt-0.5 text-xs text-[var(--muted)]">
              Bring your own AI subscriptions. No vendor lock-in, usage tracking, or surprise bills.
            </p>
          </div>
        </li>
      </ul>

      <div className="flex flex-col gap-3">
        <button
          type="button"
          onClick={onNext}
          className="inline-flex h-10 w-full items-center justify-center gap-2 rounded-lg bg-[var(--accent)] text-sm font-medium text-black transition hover:brightness-110 active:scale-95"
        >
          Set up Cortex
          <ArrowRight className="h-4 w-4" />
        </button>
        <button
          type="button"
          onClick={onSkip}
          className="text-center text-xs text-[var(--muted)] transition hover:text-white"
        >
          Explore without setup
        </button>
      </div>
    </div>
  );
}

// Step 2: Connect Provider
function ConnectProviderStep({
  onNext,
  onBack,
  onSkip,
}: {
  onNext: () => void;
  onBack: () => void;
  onSkip: () => void;
}) {
  const [showProviderSetup, setShowProviderSetup] = useState(false);

  if (showProviderSetup) {
    return (
      <div className="flex flex-col gap-4">
        <div>
          <h2 className="text-base font-semibold text-white">Connect your AI providers</h2>
          <p className="mt-1.5 text-sm text-[var(--muted)]">
            Set up your Claude and OpenAI subscriptions directly here.
          </p>
        </div>

        <div className="rounded-xl border border-white/8 bg-white/[0.02] p-1">
          <ProviderStep onNext={onNext} />
        </div>

        <button
          type="button"
          onClick={() => setShowProviderSetup(false)}
          className="inline-flex items-center gap-1 text-sm text-[var(--muted)] transition hover:text-white"
        >
          <ArrowLeft className="h-4 w-4" />
          Back to overview
        </button>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-6">
      <div>
        <h2 className="text-base font-semibold text-white">Connect your AI providers</h2>
        <p className="mt-1.5 text-sm text-[var(--muted)]">
          Cortex uses your own Claude and OpenAI subscriptions. Connect at least
          one provider to run agents.
        </p>
      </div>

      <div className="flex flex-col gap-3">
        {[
          { initials: 'CL', label: 'Claude (Anthropic)', description: 'claude.ai Pro subscription' },
          { initials: 'OA', label: 'OpenAI (Codex)', description: 'ChatGPT Plus/Pro subscription' },
        ].map(({ initials, label, description }) => (
          <div
            key={label}
            className="flex items-center gap-3 rounded-xl border border-white/8 bg-white/[0.025] px-4 py-3"
          >
            <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-white/6 text-xs font-bold text-[var(--muted-strong)]">
              {initials}
            </div>
            <div className="min-w-0 flex-1">
              <div className="text-sm font-medium text-white">{label}</div>
              <div className="text-xs text-[var(--muted)]">{description}</div>
            </div>
          </div>
        ))}
      </div>

      <div className="rounded-xl border border-[var(--accent)]/20 bg-[var(--accent-soft)] p-4">
        <p className="text-xs text-[var(--muted)]">
          ✨ We'll guide you through connecting your subscriptions step-by-step.
          This takes about 1 minute per provider.
        </p>
        <button
          type="button"
          onClick={() => setShowProviderSetup(true)}
          className="mt-3 inline-flex items-center gap-1.5 text-xs font-medium text-[var(--accent)] transition hover:brightness-110"
        >
          <Settings className="h-3.5 w-3.5" />
          Set up providers now
        </button>
      </div>

      <div className="flex flex-col gap-3">
        <div className="flex items-center justify-between">
          <button
            type="button"
            onClick={onBack}
            className="inline-flex items-center gap-1 text-sm text-[var(--muted)] transition hover:text-white"
          >
            <ArrowLeft className="h-4 w-4" />
            Back
          </button>
          <button
            type="button"
            onClick={onNext}
            className="inline-flex h-9 items-center gap-2 rounded-lg bg-[var(--accent-soft)] px-4 text-sm font-medium text-[var(--accent)] transition hover:bg-[var(--accent)]/20 active:scale-95"
          >
            Continue
            <ArrowRight className="h-4 w-4" />
          </button>
        </div>
        <button
          type="button"
          onClick={onSkip}
          className="text-center text-xs text-[var(--muted)] transition hover:text-white"
        >
          I'll set up later
        </button>
      </div>
    </div>
  );
}

// Step 3: Create Project
function ProjectStep({
  onNext,
  onBack,
  onSkip,
}: {
  onNext: () => void;
  onBack: () => void;
  onSkip: () => void;
}) {
  return (
    <div className="flex flex-col gap-6">
      <div>
        <h2 className="text-base font-semibold text-white">Create your first project</h2>
        <p className="mt-1.5 text-sm text-[var(--muted)]">
          Projects help organize your code and tasks. You can create one now or skip and set up later.
        </p>
      </div>

      <div className="space-y-3">
        {[
          {
            icon: '🌐',
            title: 'Web Application',
            description: 'React/Next.js frontend with TypeScript',
            popular: true
          },
          {
            icon: '🔌',
            title: 'API Service',
            description: 'REST API with authentication and database',
            popular: false
          },
          {
            icon: '📱',
            title: 'Mobile App',
            description: 'React Native or Flutter application',
            popular: false
          },
          {
            icon: '📝',
            title: 'Start from scratch',
            description: 'Create a blank project with basic structure',
            popular: false
          }
        ].map((template) => (
          <div
            key={template.title}
            className="flex items-center gap-3 rounded-xl border border-white/8 bg-white/[0.025] px-4 py-3 hover:border-white/15 hover:bg-white/[0.04] transition cursor-pointer"
          >
            <span className="text-2xl">{template.icon}</span>
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-2">
                <span className="font-medium text-white">{template.title}</span>
                {template.popular && (
                  <span className="px-2 py-0.5 bg-[var(--accent)]/20 text-[var(--accent)] text-[10px] rounded">
                    Popular
                  </span>
                )}
              </div>
              <p className="text-xs text-[var(--muted)]">{template.description}</p>
            </div>
            <FolderPlus className="h-4 w-4 text-[var(--muted)]" />
          </div>
        ))}
      </div>

      <div className="rounded-xl border border-blue-400/20 bg-blue-400/10 p-4">
        <p className="text-xs text-blue-200">
          Don't worry about choosing the perfect template now. You can always create more projects
          later or import existing code from GitHub.
        </p>
      </div>

      <div className="flex flex-col gap-3">
        <div className="flex items-center justify-between">
          <button
            type="button"
            onClick={onBack}
            className="inline-flex items-center gap-1 text-sm text-[var(--muted)] transition hover:text-white"
          >
            <ArrowLeft className="h-4 w-4" />
            Back
          </button>
          <button
            type="button"
            onClick={onNext}
            className="inline-flex h-9 items-center gap-2 rounded-lg bg-[var(--accent)] px-4 text-sm font-medium text-black transition hover:brightness-110 active:scale-95"
          >
            I'll set up later
            <ArrowRight className="h-4 w-4" />
          </button>
        </div>
        <button
          type="button"
          onClick={onSkip}
          className="text-center text-xs text-[var(--muted)] transition hover:text-white"
        >
          Skip project setup
        </button>
      </div>
    </div>
  );
}

// Step 4: First Task
function FirstTaskStep({
  onComplete,
  onBack,
  onSkip,
}: {
  onComplete: () => void;
  onBack: () => void;
  onSkip: () => void;
}) {
  const [showSuccess, setShowSuccess] = useState(false);

  const handleTryNow = () => {
    setShowSuccess(true);
    setTimeout(() => {
      onComplete();
    }, 2000);
  };

  if (showSuccess) {
    return (
      <div className="flex flex-col items-center gap-6 py-8">
        <div className="mx-auto flex h-16 w-16 items-center justify-center rounded-full bg-emerald-500/20 text-emerald-300">
          <CheckCircle className="h-8 w-8" />
        </div>
        <div className="text-center">
          <h2 className="text-lg font-semibold text-white">Welcome to Cortex! 🎉</h2>
          <p className="mt-2 text-sm text-[var(--muted)]">
            You're all set up and ready to orchestrate AI agents.
          </p>
        </div>
        <div className="text-center">
          <p className="text-xs text-[var(--muted)]">Taking you to your workspace...</p>
        </div>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-6">
      <div>
        <h2 className="text-base font-semibold text-white">Try your first AI command</h2>
        <p className="mt-1.5 text-sm text-[var(--muted)]">
          Test Cortex with a simple command. Your AI agents will handle the work.
        </p>
      </div>

      <div className="rounded-xl border border-emerald-500/20 bg-emerald-500/10 p-4">
        <div className="flex flex-col gap-3">
          <h3 className="text-sm font-medium text-emerald-200">Suggested first command:</h3>
          <div className="flex items-center gap-3 rounded-lg border border-emerald-500/20 bg-black/20 px-3 py-2.5">
            <MessageSquare className="h-3.5 w-3.5 shrink-0 text-emerald-300" />
            <span className="font-mono text-sm text-white">create task: review the project README</span>
          </div>
          <p className="text-xs text-emerald-200">
            ✨ This will create a task, assign it to your agents, and show you how the task management works.
          </p>
        </div>
      </div>

      <div className="flex flex-col gap-2">
        <p className="text-xs font-medium uppercase tracking-wider text-[var(--muted)]">
          More examples
        </p>
        {[
          'fix the login bug in auth.tsx',
          'write tests for the user service',
          'deploy the app to production',
        ].map((cmd) => (
          <div
            key={cmd}
            className="flex items-center gap-3 rounded-lg border border-white/6 bg-white/[0.03] px-3 py-2.5"
          >
            <MessageSquare className="h-3.5 w-3.5 shrink-0 text-[var(--accent)]" />
            <span className="font-mono text-sm text-white">{cmd}</span>
          </div>
        ))}
      </div>

      <div className="flex flex-col gap-3">
        <div className="flex items-center justify-between">
          <button
            type="button"
            onClick={onBack}
            className="inline-flex items-center gap-1 text-sm text-[var(--muted)] transition hover:text-white"
          >
            <ArrowLeft className="h-4 w-4" />
            Back
          </button>
          <button
            type="button"
            onClick={handleTryNow}
            className="inline-flex h-9 items-center gap-2 rounded-lg bg-[var(--accent)] px-4 text-sm font-medium text-black transition hover:brightness-110 active:scale-95"
          >
            Start building
            <ArrowRight className="h-4 w-4" />
          </button>
        </div>
        <button
          type="button"
          onClick={onSkip}
          className="text-center text-xs text-[var(--muted)] transition hover:text-white"
        >
          I'll explore later
        </button>
      </div>
    </div>
  );
}

export default function OnboardingFlow({ userId, onComplete }: OnboardingFlowProps) {
  const [step, setStep] = useState(() => getOnboardingStep(userId));

  function completeOnboarding() {
    markOnboardingComplete(userId);
    onComplete();
  }

  function goToStep(nextStep: number) {
    setStep(nextStep);
    saveOnboardingStep(userId, nextStep);
  }

  return (
    <main className="flex min-h-screen items-center justify-center bg-[var(--bg)] px-4 text-[var(--fg)]">
      <section className="w-full max-w-md rounded-2xl border border-white/10 bg-[var(--panel)] p-6 shadow-2xl">
        <div className="mb-6">
          <ProgressDots current={step} />
        </div>

        {step === 0 && (
          <WelcomeStep
            onNext={() => goToStep(1)}
            onSkip={completeOnboarding}
          />
        )}
        {step === 1 && (
          <ConnectProviderStep
            onNext={() => goToStep(2)}
            onBack={() => goToStep(0)}
            onSkip={completeOnboarding}
          />
        )}
        {step === 2 && (
          <ProjectStep
            onNext={() => goToStep(3)}
            onBack={() => goToStep(1)}
            onSkip={completeOnboarding}
          />
        )}
        {step === 3 && (
          <FirstTaskStep
            onComplete={completeOnboarding}
            onBack={() => goToStep(2)}
            onSkip={completeOnboarding}
          />
        )}
      </section>
    </main>
  );
}
