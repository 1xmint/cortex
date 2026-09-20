import { AlertTriangle, LayoutGrid } from 'lucide-react';

export default function NotFoundPage() {
  return (
    <div className="flex min-h-screen items-center justify-center bg-[var(--bg)] px-4 text-[var(--fg)]">
      <div className="w-full max-w-sm rounded-2xl border border-white/8 bg-[var(--panel)] p-6 text-center shadow-2xl">
        <div className="mx-auto flex h-12 w-12 items-center justify-center rounded-xl bg-amber-500/10 text-amber-200">
          <AlertTriangle className="h-6 w-6" />
        </div>
        <p className="mt-4 text-xs font-medium uppercase tracking-widest text-[var(--muted)]">404</p>
        <h1 className="mt-1 text-lg font-semibold text-white">Page not found</h1>
        <p className="mt-2 text-sm leading-6 text-[var(--muted)]">
          The page you are looking for does not exist or has been moved.
        </p>
        <a
          href="/app"
          className="mt-5 inline-flex items-center gap-2 rounded-xl bg-[var(--accent)] px-4 py-2 text-sm font-medium text-black transition hover:brightness-110 active:scale-95"
        >
          <LayoutGrid className="h-4 w-4" />
          Go to dashboard
        </a>
      </div>
    </div>
  );
}
