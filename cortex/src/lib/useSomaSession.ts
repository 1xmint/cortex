import { useEffect, useRef, useState } from 'react';
import { createSomaSession, setSomaDelegation, SOMA_API_ENABLED, type SomaSession } from './cortexApi';

const STORAGE_PREFIX = 'cortex:soma-session:';
const REFRESH_MARGIN_MS = 60 * 60 * 1000; // refresh 1 hour before expiry

interface SomaSessionState {
  session: SomaSession | null;
  loading: boolean;
  error: string | null;
  userDid: string | null;
}

function getSessionExpiry(session: SomaSession): number {
  const expiresCaveat = session.delegation.caveats?.find(
    (c: { type?: string }) => c.type === 'expires_at',
  ) as { timestamp?: number } | undefined;
  if (expiresCaveat?.timestamp) return expiresCaveat.timestamp;
  // Fallback: 24h from issued_at
  return session.delegation.issued_at + 24 * 3600 * 1000;
}

function getStoredSession(userId: string): SomaSession | null {
  try {
    const raw = localStorage.getItem(`${STORAGE_PREFIX}${userId}`);
    if (!raw) return null;
    const session = JSON.parse(raw) as SomaSession;
    const expiresAt = getSessionExpiry(session);
    if (Date.now() > expiresAt) {
      localStorage.removeItem(`${STORAGE_PREFIX}${userId}`);
      return null;
    }
    return session;
  } catch {
    return null;
  }
}

function storeSession(userId: string, session: SomaSession) {
  localStorage.setItem(`${STORAGE_PREFIX}${userId}`, JSON.stringify(session));
}

function applySession(session: SomaSession): SomaSessionState {
  setSomaDelegation(session.delegation);
  return {
    session,
    loading: false,
    error: null,
    userDid: session.user_identity.did,
  };
}

export function useSomaSession(userId: string, isSignedIn: boolean): SomaSessionState {
  const [state, setState] = useState<SomaSessionState>({
    session: null,
    loading: false,
    error: null,
    userDid: null,
  });
  const requested = useRef(false);
  const refreshTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    // The fence. `/api/soma/session` 404s in every default build, so without
    // this every sign-in made a request that could only fail, and left a
    // console error that reads like a real outage.
    if (!SOMA_API_ENABLED) return;
    if (!isSignedIn || !userId || userId === 'local' || userId === 'anonymous') return;
    if (requested.current) return;

    const scheduleRefresh = (session: SomaSession, uid: string) => {
      if (refreshTimer.current) clearTimeout(refreshTimer.current);
      const expiresAt = getSessionExpiry(session);
      const refreshAt = expiresAt - REFRESH_MARGIN_MS;
      const delay = Math.max(refreshAt - Date.now(), 0);
      refreshTimer.current = setTimeout(() => {
        createSomaSession()
          .then((newSession) => {
            storeSession(uid, newSession);
            setState(applySession(newSession));
            scheduleRefresh(newSession, uid);
          })
          .catch((err) => {
            console.warn('Soma session refresh failed:', err);
          });
      }, delay);
    };

    const cached = getStoredSession(userId);
    if (cached) {
      setState(applySession(cached));
      scheduleRefresh(cached, userId);
      return;
    }

    requested.current = true;
    setState((s) => ({ ...s, loading: true }));

    createSomaSession()
      .then((session) => {
        storeSession(userId, session);
        setState(applySession(session));
        scheduleRefresh(session, userId);
      })
      .catch((err) => {
        setSomaDelegation(null);
        setState({
          session: null,
          loading: false,
          error: err instanceof Error ? err.message : 'Failed to create Soma session',
          userDid: null,
        });
        requested.current = false;
      });

    return () => {
      if (refreshTimer.current) clearTimeout(refreshTimer.current);
    };
  }, [userId, isSignedIn]);

  return state;
}
