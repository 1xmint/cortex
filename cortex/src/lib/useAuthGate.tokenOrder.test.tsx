// @vitest-environment jsdom
import { useEffect } from 'react';
import { cleanup, render, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// Clerk is mocked as already loaded and signed in: the production failure
// happens on exactly that commit -- the first one where MissionControl stops
// gating and CortexShell mounts together with its children.
const clerkGetToken = vi.fn(async () => 'clerk-jwt');
vi.mock('@clerk/clerk-react', () => ({
  useAuth: () => ({ isLoaded: true, isSignedIn: true, userId: 'user_1', getToken: clerkGetToken }),
}));
vi.mock('../components/auth/SignInScreen', () => ({ default: () => null }));

type Api = typeof import('./cortexApi');
type Gate = typeof import('./useAuthGate');

let api: Api;
let gate: Gate;
let unauthorizedEvents: number;
const onUnauthorized = () => {
  unauthorizedEvents += 1;
};

// Server stand-in: 401 "bearer token required" when no bearer is sent, as
// crates/api/src/clerk.rs does, 200 otherwise.
const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
  const auth = new Headers(init?.headers).get('Authorization');
  return auth === 'Bearer clerk-jwt'
    ? new Response('[]', { status: 200 })
    : new Response(JSON.stringify({ error: 'bearer token required' }), { status: 401 });
});

beforeEach(async () => {
  vi.stubEnv('VITE_CLERK_PUBLISHABLE_KEY', 'pk_test_x');
  vi.stubGlobal('fetch', fetchMock);
  vi.resetModules(); // fresh module state: no token getter registered yet
  api = await import('./cortexApi');
  gate = await import('./useAuthGate');
  fetchMock.mockClear();
  unauthorizedEvents = 0;
  window.addEventListener('cortex:unauthorized', onUnauthorized);
});

afterEach(() => {
  cleanup();
  window.removeEventListener('cortex:unauthorized', onUnauthorized);
  vi.unstubAllEnvs();
  vi.unstubAllGlobals();
});

/** Stands in for Sidebar / ModelPicker / useTaskManager: fetch on mount. */
function FetchOnMount() {
  useEffect(() => {
    void api.listConversations().catch(() => undefined);
  }, []);
  return null;
}

describe('auth token getter registration order', () => {
  it('reproduces the bug: a parent that registers in useEffect is too late for its children', async () => {
    function ParentRegisteringInEffect() {
      const { getToken } = gate.useAuthGate();
      useEffect(() => {
        if (getToken) api.setAuthTokenGetter(getToken);
      }, [getToken]);
      return <FetchOnMount />;
    }

    render(<ParentRegisteringInEffect />);

    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
    const init = fetchMock.mock.calls[0][1];
    expect(new Headers(init?.headers).has('Authorization')).toBe(false);
  });

  it('AuthTokenRegistrar registers during render, so first-mount fetches carry the bearer', async () => {
    render(
      <>
        <gate.AuthTokenRegistrar />
        <FetchOnMount />
      </>,
    );

    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
    const init = fetchMock.mock.calls[0][1];
    expect(new Headers(init?.headers).get('Authorization')).toBe('Bearer clerk-jwt');
    await waitFor(() => expect(clerkGetToken).toHaveBeenCalled());
    expect(unauthorizedEvents).toBe(0);
  });

  it('does not report an expired session for a 401 sent before any token getter exists', async () => {
    await expect(api.listConversations()).rejects.toMatchObject({ status: 401 });
    expect(unauthorizedEvents).toBe(0);
  });

  it('still reports a 401 once a token getter is registered (real expiry is not hidden)', async () => {
    api.setAuthTokenGetter(async () => 'stale-jwt');
    await expect(api.listConversations()).rejects.toMatchObject({ status: 401 });
    expect(unauthorizedEvents).toBe(1);
  });
});
