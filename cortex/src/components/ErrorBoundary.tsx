import { Component, type ErrorInfo, type ReactNode } from 'react';
import { AlertTriangle, Home, RefreshCcw } from 'lucide-react';

interface ErrorBoundaryProps {
  children: ReactNode;
  /** Optional callback invoked when an error is caught — useful for error tracking integrations. */
  onError?: (error: Error, info: ErrorInfo) => void;
}

interface ErrorBoundaryState {
  hasError: boolean;
}

export default class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { hasError: false };

  static getDerivedStateFromError(): ErrorBoundaryState {
    return { hasError: true };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error('Cortex UI crashed', error, info);
    this.props.onError?.(error, info);
  }

  render() {
    if (!this.state.hasError) return this.props.children;

    return (
      <div className="flex min-h-screen items-center justify-center bg-[var(--bg)] px-4 text-[var(--fg)]">
        <div className="w-full max-w-sm rounded-2xl border border-white/8 bg-[var(--panel)] p-5 text-center shadow-2xl">
          <div className="mx-auto flex h-10 w-10 items-center justify-center rounded-xl bg-red-500/10 text-red-200">
            <AlertTriangle className="h-5 w-5" />
          </div>
          <h1 className="mt-4 text-base font-semibold text-white">Something went wrong</h1>
          <p className="mt-2 text-sm leading-6 text-[var(--muted)]">
            An unexpected error occurred in the interface. Your data is safe — conversations and tasks are stored in the backend.
          </p>
          <div className="mt-5 flex flex-col gap-2 sm:flex-row sm:justify-center">
            <button
              type="button"
              onClick={() => window.location.reload()}
              className="inline-flex items-center justify-center gap-2 rounded-xl bg-[var(--accent)] px-4 py-2 text-sm font-medium text-black transition hover:brightness-110 active:scale-95"
            >
              <RefreshCcw className="h-4 w-4" />
              Try again
            </button>
            <a
              href="/app"
              className="inline-flex items-center justify-center gap-2 rounded-xl border border-white/10 bg-white/6 px-4 py-2 text-sm font-medium text-white transition hover:bg-white/10 active:scale-95"
            >
              <Home className="h-4 w-4" />
              Go home
            </a>
          </div>
        </div>
      </div>
    );
  }
}
