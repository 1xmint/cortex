import { useAuth } from '@clerk/clerk-react';
import type { ComponentType } from 'react';
import SignInScreen from '../components/auth/SignInScreen';
import { setAuthTokenGetter } from './cortexApi';

const CLERK_ENABLED = !!import.meta.env.VITE_CLERK_PUBLISHABLE_KEY;

interface AuthGateResult {
  isLoaded: boolean;
  isSignedIn: boolean;
  userId: string;
  AuthScreen: ComponentType | null;
  getToken: ((opts?: { skipCache?: boolean }) => Promise<string | null>) | null;
  clerkEnabled: boolean;
}

function useClerkGate(): AuthGateResult {
  const { isLoaded, isSignedIn, userId, getToken } = useAuth();
  return {
    isLoaded,
    isSignedIn: isSignedIn ?? false,
    userId: userId ?? 'anonymous',
    AuthScreen: SignInScreen,
    getToken: (opts) => getToken(opts),
    clerkEnabled: true,
  };
}

function getLocalGate(): AuthGateResult {
  return {
    isLoaded: true,
    isSignedIn: true,
    userId: 'local',
    AuthScreen: null,
    getToken: null,
    clerkEnabled: false,
  };
}

export function useAuthGate(): AuthGateResult {
  if (CLERK_ENABLED) {
    // eslint-disable-next-line react-hooks/rules-of-hooks
    return useClerkGate();
  }
  return getLocalGate();
}

/**
 * Hands the Clerk token getter to the API layer during render, not in an
 * effect. Mount it once above the routes. Effects run child-first, so a getter
 * registered in a parent's `useEffect` is not yet set when the children's own
 * mount effects fire their first requests. Those requests went out with no
 * bearer, got a 401, and raised a false "session expired" banner. Clerk's
 * getToken waits for Clerk to load and reads the live session, so registering
 * it early is safe; re-registering on every render is an idempotent assignment.
 */
export function AuthTokenRegistrar(): null {
  const { getToken } = useAuthGate();
  if (getToken) setAuthTokenGetter(getToken);
  return null;
}
