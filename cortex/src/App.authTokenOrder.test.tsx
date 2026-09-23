// @vitest-environment jsdom
import { cleanup, render, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// Same Clerk mock shape as useAuthGate.tokenOrder.test.tsx: loaded and signed
// in, so App mounts CortexShell/MissionControl instead of SignInScreen.
const clerkGetToken = vi.fn(async () => 'clerk-jwt');
vi.mock('@clerk/clerk-react', () => ({
  useAuth: () => ({ isLoaded: true, isSignedIn: true, userId: 'user_1', getToken: clerkGetToken }),
  useClerk: () => ({ signOut: vi.fn() }),
  useUser: () => ({ user: { firstName: 'Test', primaryEmailAddress: { emailAddress: 'test@example.com' } } }),
}));
vi.mock('./components/auth/SignInScreen', () => ({ default: () => null }));

// Server stand-in, same as useAuthGate.tokenOrder.test.tsx: 401 "bearer token
// required" when no bearer is sent, 200 otherwise.
const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
  const auth = new Headers(init?.headers).get('Authorization');
  return auth === 'Bearer clerk-jwt'
    ? new Response('[]', { status: 200 })
    : new Response(JSON.stringify({ error: 'bearer token required' }), { status: 401 });
});

beforeEach(() => {
  vi.stubEnv('VITE_CLERK_PUBLISHABLE_KEY', 'pk_test_x');
  vi.stubGlobal('fetch', fetchMock);
  fetchMock.mockClear();
});

afterEach(() => {
  cleanup();
  vi.unstubAllEnvs();
  vi.unstubAllGlobals();
});

describe('App wires AuthTokenRegistrar above every route', () => {
  it('carries the bearer on the first fetch a pane makes on mount', async () => {
    window.history.pushState({}, '', '/runs');
    const { default: App } = await import('./App');

    render(<App />);

    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    const [, init] = fetchMock.mock.calls[0];
    expect(new Headers((init as RequestInit | undefined)?.headers).get('Authorization')).toBe('Bearer clerk-jwt');
  });
});
