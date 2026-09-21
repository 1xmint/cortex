import { useCallback, useEffect, useRef, useState } from 'react';
import { apiUrl, getAuthToken, CortexApiError } from '../lib/cortexApi';
import { closeLiveVoiceSession, startLiveVoiceSession } from '../lib/voiceApi';
import { routeLiveVoiceEvent } from './liveVoiceEvents';

export type LiveVoiceStatus = 'idle' | 'connecting' | 'active';

function microphoneErrorMessage(error: unknown): string {
  if (error instanceof Error && error.name === 'NotAllowedError') {
    return 'Microphone access was denied. Allow microphone access to use live voice.';
  }
  if (error instanceof Error && error.name === 'NotFoundError') {
    return 'No microphone was found. Connect a microphone to use live voice.';
  }
  if (error instanceof Error && error.message === 'unsupported') {
    return 'Live voice is not supported in this browser.';
  }
  return 'Could not access the microphone.';
}

/**
 * A continuous spoken conversation with gpt-live-1, brokered through
 * `POST /api/voice/live/sessions` / `DELETE /api/voice/live/sessions/{id}`
 * (`crates/api/src/voice_session.rs`). The server bills this in segments and
 * can close the call itself when credits run out -- when that happens the
 * browser sees its WebRTC connection to OpenAI end, same as any other
 * disconnect, and this hook reacts the same way it would to a failure.
 *
 * Every path that can end this session -- the toggle, an unmount, the tab
 * closing, and the connection failing on its own -- funnels through
 * `sendClose`, which sends the DELETE at most once per session id.
 */
export function useLiveVoiceToggle() {
  const [status, setStatus] = useState<LiveVoiceStatus>('idle');
  const [error, setError] = useState<string | null>(null);

  const pcRef = useRef<RTCPeerConnection | null>(null);
  const dcRef = useRef<RTCDataChannel | null>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const sessionIdRef = useRef<string | null>(null);
  const tokenRef = useRef<string | null>(null);
  // Synchronous re-entrancy guard against a fast double click -- see
  // useDictation's `startingRef` for why this cannot be state.
  const startingRef = useRef(false);

  const releaseLocal = useCallback(() => {
    dcRef.current?.close();
    dcRef.current = null;
    pcRef.current?.close();
    pcRef.current = null;
    streamRef.current?.getTracks().forEach((track) => track.stop());
    streamRef.current = null;
  }, []);

  /**
   * Sends the DELETE for the currently open session, if any, exactly once.
   * Clearing `sessionIdRef` first makes every other caller (toggle, unmount,
   * pagehide, connection-state-change) a no-op once one of them has already
   * fired -- there is no separate "already closing" flag to fall out of
   * sync with it.
   *
   * `useBeacon` is for the tab-closing paths (`pagehide`/`beforeunload`),
   * where a normal `fetch` can be aborted mid-flight by the navigation; a
   * `keepalive` fetch is allowed to outlive the page. It reads the token
   * cached at session start rather than asking Clerk again, since there is
   * no guarantee anything async gets to finish once `pagehide` has fired.
   */
  const sendClose = useCallback((useBeacon: boolean) => {
    const id = sessionIdRef.current;
    if (!id) return;
    sessionIdRef.current = null;
    if (useBeacon) {
      const headers: Record<string, string> = {};
      if (tokenRef.current) headers.Authorization = `Bearer ${tokenRef.current}`;
      try {
        void fetch(apiUrl(`/api/voice/live/sessions/${encodeURIComponent(id)}`), {
          method: 'DELETE',
          headers,
          keepalive: true,
        });
      } catch {
        // Best-effort on unload; nothing to recover into.
      }
    } else {
      void closeLiveVoiceSession(id).catch(() => {
        // The session is being torn down regardless; a failed DELETE here
        // does not reopen it, and the server settles/expires it on its own.
      });
    }
  }, []);

  const stop = useCallback(() => {
    sendClose(false);
    releaseLocal();
    setStatus('idle');
  }, [sendClose, releaseLocal]);

  const start = useCallback(async () => {
    if (startingRef.current || status !== 'idle') return;
    startingRef.current = true;
    setError(null);
    setStatus('connecting');
    try {
      let stream: MediaStream;
      try {
        if (!navigator.mediaDevices?.getUserMedia) {
          throw new Error('unsupported');
        }
        stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      } catch (mediaError) {
        setError(microphoneErrorMessage(mediaError));
        setStatus('idle');
        return;
      }
      streamRef.current = stream;
      tokenRef.current = await getAuthToken();

      const pc = new RTCPeerConnection();
      pcRef.current = pc;
      const [track] = stream.getTracks();
      if (track) pc.addTrack(track, stream);

      const dc = pc.createDataChannel('oai-events');
      dcRef.current = dc;
      dc.addEventListener('message', (event) => {
        try {
          routeLiveVoiceEvent(JSON.parse(event.data as string));
        } catch {
          // Not JSON -- nothing to route.
        }
      });

      const offer = await pc.createOffer();
      await pc.setLocalDescription(offer);

      const response = await startLiveVoiceSession(offer.sdp ?? '');
      sessionIdRef.current = response.session_id;

      await pc.setRemoteDescription({ type: 'answer', sdp: response.sdp });

      pc.addEventListener('connectionstatechange', () => {
        if (
          pc.connectionState === 'failed' ||
          pc.connectionState === 'closed' ||
          pc.connectionState === 'disconnected'
        ) {
          // Covers both a real failure and the server ending the call for
          // us (credits ran out): either way the browser's connection to
          // OpenAI has ended, so tear down and (best-effort) close our side.
          sendClose(false);
          releaseLocal();
          setStatus('idle');
        }
      });

      setStatus('active');
    } catch (err) {
      releaseLocal();
      if (err instanceof CortexApiError) {
        setError(
          err.status === 409
            ? 'You already have a live voice session open. End it first.'
            : err.message,
        );
      } else {
        setError('Live voice is temporarily unavailable. Please try again in a moment.');
      }
      setStatus('idle');
    } finally {
      startingRef.current = false;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [status, sendClose, releaseLocal]);

  const toggle = useCallback(() => {
    if (status === 'active' || status === 'connecting') {
      stop();
    } else {
      void start();
    }
  }, [status, start, stop]);

  useEffect(() => {
    const handlePageHide = () => sendClose(true);
    window.addEventListener('pagehide', handlePageHide);
    window.addEventListener('beforeunload', handlePageHide);
    return () => {
      window.removeEventListener('pagehide', handlePageHide);
      window.removeEventListener('beforeunload', handlePageHide);
      // A plain unmount (in-app navigation) keeps the tab alive, so the
      // regular DELETE path is reliable here -- no need for the beacon.
      sendClose(false);
      releaseLocal();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return { status, error, toggle };
}
