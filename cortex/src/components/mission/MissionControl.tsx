import { useEffect, useState } from 'react';
import { NavLink, Outlet, useNavigate } from 'react-router';
import { Activity, BadgeCheck, Coins, GitBranch, MessageSquare, Moon, Shield, Sun } from 'lucide-react';
import { useAuthGate } from '../../lib/useAuthGate';
import SignInScreen from '../auth/SignInScreen';

/**
 * Mission control — the six panes defined in cortex/plan/SURFACE.md.
 *
 * The web app is mission control, not the product: delivery happens at the
 * forge (SURFACE.md ranks the GitHub App first). Chat is pane six and a
 * doorway, never the identity of the app.
 *
 * Panes mount the components that already exist and state plainly where one
 * does not — an empty pane that says why is honest; a pane filled with
 * numbers the ledger cannot back is not (SURFACE.md, "What not to do").
 */

const PANES = [
  { to: '/runs', label: 'Runs', icon: Activity, hint: 'live steps and state' },
  { to: '/receipts', label: 'Receipts', icon: BadgeCheck, hint: 'proof the checks ran' },
  { to: '/ledger', label: 'Ledger', icon: Coins, hint: 'credits, spend, refunds' },
  { to: '/leases', label: 'Leases', icon: GitBranch, hint: 'who holds what surface' },
  { to: '/admin', label: 'Admin', icon: Shield, hint: 'members, policy, keys' },
  { to: '/', label: 'Chat', icon: MessageSquare, hint: 'a doorway, not the product' },
] as const;

const THEME_STORAGE_KEY = 'cortex:theme';

function readTheme(): 'dark' | 'light' {
  try {
    return window.localStorage.getItem(THEME_STORAGE_KEY) === 'light' ? 'light' : 'dark';
  } catch {
    return 'dark';
  }
}

export default function MissionControl() {
  const { isLoaded, isSignedIn, clerkEnabled } = useAuthGate();
  const navigate = useNavigate();
  const [theme, setTheme] = useState<'dark' | 'light'>(readTheme);

  // Theme lives on <html> so tokens cascade everywhere, including portals.
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    try {
      window.localStorage.setItem(THEME_STORAGE_KEY, theme);
    } catch {
      // preference persistence is best-effort
    }
  }, [theme]);

  // Alt+1..6 jumps between panes from anywhere, including text inputs —
  // Alt+digit types nothing, so there is no conflict with the composer.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!event.altKey || event.ctrlKey || event.metaKey) return;
      const index = Number.parseInt(event.key, 10) - 1;
      if (Number.isInteger(index) && index >= 0 && index < PANES.length) {
        event.preventDefault();
        navigate(PANES[index].to);
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [navigate]);

  if (clerkEnabled && !isLoaded) return null;
  if (clerkEnabled && !isSignedIn) return <SignInScreen />;

  return (
    <div className="flex h-screen w-full bg-[var(--bg)] text-[var(--fg)]">
      <nav
        aria-label="Mission control panes"
        className="flex w-44 shrink-0 flex-col border-r border-[var(--line)] bg-[var(--inset)]"
      >
        <button
          type="button"
          onClick={() => navigate('/')}
          className="flex h-11 shrink-0 items-center border-b border-[var(--line)] px-4 text-left t-title tracking-wide"
        >
          Cortex
        </button>
        <div className="flex flex-col gap-0.5 p-2">
          {PANES.map(({ to, label, icon: Icon, hint }, index) => (
            <NavLink
              key={to}
              to={to}
              end={to === '/'}
              title={`${hint} (Alt+${index + 1})`}
              className={({ isActive }) =>
                `group flex items-center gap-2.5 rounded-md px-2.5 py-1.5 t-body transition-colors ${
                  isActive
                    ? 'bg-[var(--surface-active)] text-[var(--fg)]'
                    : 'text-[var(--muted)] hover:bg-[var(--surface-hover)] hover:text-[var(--fg)]'
                }`
              }
            >
              <Icon className="h-4 w-4 shrink-0" aria-hidden />
              <span className="flex-1">{label}</span>
              <span className="t-micro text-[var(--muted)] opacity-0 transition-opacity group-hover:opacity-60">
                {index + 1}
              </span>
            </NavLink>
          ))}
        </div>
        <div className="mt-auto border-t border-[var(--line)] p-2">
          <button
            type="button"
            onClick={() => setTheme((current) => (current === 'dark' ? 'light' : 'dark'))}
            className="flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 t-micro text-[var(--muted)] transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--fg)]"
            title="The chat doorway stays dark until its pane-six rewrite"
          >
            {theme === 'dark' ? <Sun className="h-3.5 w-3.5" aria-hidden /> : <Moon className="h-3.5 w-3.5" aria-hidden />}
            {theme === 'dark' ? 'Light theme' : 'Dark theme'}
          </button>
        </div>
      </nav>

      <main className="flex min-w-0 flex-1 flex-col overflow-hidden">
        <Outlet />
      </main>
    </div>
  );
}

/**
 * Honest empty state for a pane that is not built yet. Names the build task
 * that fills it so the screen is never a mystery to whoever opens it —
 * including Josh, dogfooding.
 */
export function PaneStub({
  title,
  blockedOn,
  children,
}: {
  title: string;
  blockedOn: string;
  children?: React.ReactNode;
}) {
  return (
    <div className="overflow-y-auto">
      <div className="mx-auto max-w-2xl p-8">
        <h1 className="t-title">{title}</h1>
        <p className="t-body mt-3 text-[var(--muted)]">{children}</p>
        <p className="t-micro mt-4 rounded-lg border border-[var(--line)] bg-[var(--surface)] px-3 py-2 text-[var(--muted)]">
          Not built yet — waiting on <span className="text-[var(--fg)]">{blockedOn}</span>.
        </p>
      </div>
    </div>
  );
}
