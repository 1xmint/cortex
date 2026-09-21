import { useCallback, useEffect, useRef, useState } from 'react';
import { apiUrl, getAuthToken, CortexApiError } from '../lib/cortexApi';
import { closeLiveVoiceSession, startLiveVoiceSession } from '../lib/voiceApi';
import { routeLiveVoiceEvent } from './liveVoiceEvents';

export type LiveVoiceStatus = 'idle' | 'connecting' | 'active';

/**
 * `session.closed`'s `reason` (per the GPT-Live WebRTC guide) is
 * `close_requested` when the app sent `session.close` or hung up --
 * `expired` is the OpenAI-side duration limit. The Cortex SERVER also sends
 * `session.close` on its own sideband connection when a session's credits
 * run out (`crates/api/src/voice_session.rs`'s billing loop), which reaches
 * the browser as `session.closed` with reason `close_requested` too -- the
 * same reason a user-initiated stop produces. `closeRequestedRef` is what
 * tells those two apart: it is only set when *this* browser asked for the
 * close, so a `close_requested` the browser didn't ask for is the credits
 * close, and a `close_requested` it did ask for stays silent.
 */
function sessionClosedMessage(reason: unknown, closeRequestedByUs: boolean): string | null {
  if (reason === 'close_requested') {
    return closeRequestedByUs ? null : 'Live voice ended: your credits ran out.';
  }
  if (reason === 'expired') return 'Live voice ended: the session reached its time limit.';
  const label = typeof reason === 'string' && reason ? reason.replace(/_/g, ' ') : 'the connection ended';
  return `Live voice ended. ${label}.`;
}

/**
 * The offer sent to Cortex should carry every ICE candidate this browser
 * can gather, not just the ones gathered by the time `createOffer`
 * resolves -- otherwise a network with slow candidate gathering loses
 * connectivity options it should have had. Caps the wait rather than
 * blocking forever on a browser/network that never reports `complete`.
 */
function waitForIceGatheringComplete(pc: RTCPeerConnection, timeoutMs = 2000): Promise<void> {
  if (pc.iceGatheringState === 'complete') return Promise.resolve();
  return new Promise((resolve) => {
    let settled = false;
    const finish = () => {
      if (settled) return;
      settled = true;
      pc.removeEventListener('icegatheringstatechange', onChange);
      window.clearTimeout(timer);
      resolve();
    };
    const onChange = () => {
      if (pc.iceGatheringState === 'complete') finish();
    };
    pc.addEventListener('icegatheringstatechange', onChange);
    const timer = window.setTimeout(finish, timeoutMs);
  });
}

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
 * can close the call itself when credits run out, over its own sideband
 * connection -- when that happens the browser gets a `session.closed`
 * message with reason `close_requested`, the same reason a user-initiated
 * stop produces, so `closeRequestedRef` is what this hook checks to tell a
 * credits close from its own toggle-off.
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
  // The model's audio, played out through an autoplay <audio> element that
  // is never attached to the DOM -- WebRTC only hands us the remote track,
  // it doesn't play it for us.
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const sessionIdRef = useRef<string | null>(null);
  const tokenRef = useRef<string | null>(null);
  // Synchronous re-entrancy guard against a fast double click -- see
  // useDictation's `startingRef` for why this cannot be state.
  const startingRef = useRef(false);
  // Set by an unmount or a user-initiated stop while `start` is still
  // in-flight. Checked after every await in `start` so a session that
  // finishes connecting after the user has already left never gets
  // stranded open and billed.
  const cancelledRef = useRef(false);
  const disconnectTimerRef = useRef<number | null>(null);
  // Set whenever this browser itself asked for the close -- the toggle's
  // `stop()` or the pagehide handler -- before anything is sent. Checked by
  // the `session.closed` handler to tell the server's own credits-close
  // (which arrives as reason `close_requested`, same as a user-initiated
  // one) apart from a stop the user actually asked for.
  const closeRequestedRef = useRef(false);

  const releaseLocal = useCallback(() => {
    dcRef.current?.close();
    dcRef.current = null;
    pcRef.current?.close();
    pcRef.current = null;
    streamRef.current?.getTracks().forEach((track) => track.stop());
    streamRef.current = null;
    if (audioRef.current) {
      audioRef.current.srcObject = null;
      audioRef.current = null;
    }
    if (disconnectTimerRef.current !== null) {
      window.clearTimeout(disconnectTimerRef.current);
      disconnectTimerRef.current = null;
    }
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
   * `keepalive` fetch is allowed to outlive the page. It reads whatever
   * token is currently cached in `tokenRef` rather than asking Clerk again,
   * since there is no guarantee anything async gets to finish once
   * `pagehide` has fired -- `tokenRef` is refreshed periodically while a
   * session is active (see the refresh effect below) precisely so that
   * cached token is never more than ~30s stale, since Clerk tokens only
   * last ~60s and a session can run far longer than that.
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
    cancelledRef.current = true;
    closeRequestedRef.current = true;
    sendClose(false);
    releaseLocal();
    setStatus('idle');
  }, [sendClose, releaseLocal]);

  const start = useCallback(async () => {
    if (startingRef.current || status !== 'idle') return;
    startingRef.current = true;
    cancelledRef.current = false;
    closeRequestedRef.current = false;
    setError(null);
    setStatus('connecting');
    // A cancellation (unmount or user stop) can land between any two awaits
    // below. Bailing out here -- instead of pressing on to open a session
    // nobody wants -- is what keeps a cancelled `start` from ever leaving a
    // billed session running with no UI pointed at it.
    const bailIfCancelled = () => {
      if (!cancelledRef.current) return false;
      sendClose(false);
      releaseLocal();
      return true;
    };
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
      if (bailIfCancelled()) return;
      tokenRef.current = await getAuthToken();
      if (bailIfCancelled()) return;

      const pc = new RTCPeerConnection();
      pcRef.current = pc;
      const [track] = stream.getTracks();
      if (track) {
        pc.addTrack(track, stream);
        // The OS/browser can end the mic track on its own (device
        // unplugged, another app took it) without the peer connection ever
        // noticing -- end the session the same way the toggle would.
        track.addEventListener('ended', () => stop());
      }

      pc.addEventListener('track', (event: RTCTrackEvent) => {
        const audio = audioRef.current ?? new Audio();
        audio.autoplay = true;
        audio.srcObject = new MediaStream([event.track]);
        audioRef.current = audio;
      });

      const dc = pc.createDataChannel('oai-events');
      dcRef.current = dc;
      dc.addEventListener('message', (event) => {
        let payload: { type?: string; reason?: unknown };
        try {
          payload = JSON.parse(event.data as string);
        } catch {
          return; // Not JSON -- nothing to route.
        }
        if (payload.type === 'session.closed') {
          // The server can end this call on its own (credits ran out,
          // OpenAI hung up, ...); a silent toggle-off would hide that from
          // the user, so only a close *we* asked for stays quiet.
          const message = sessionClosedMessage(payload.reason, closeRequestedRef.current);
          if (message) setError(message);
          sendClose(false);
          releaseLocal();
          setStatus('idle');
          return;
        }
        routeLiveVoiceEvent(payload);
      });

      const offer = await pc.createOffer();
      if (bailIfCancelled()) return;
      await pc.setLocalDescription(offer);
      if (bailIfCancelled()) return;
      await waitForIceGatheringComplete(pc);
      if (bailIfCancelled()) return;

      const response = await startLiveVoiceSession(pc.localDescription?.sdp ?? offer.sdp ?? '');
      sessionIdRef.current = response.session_id;
      if (bailIfCancelled()) return;

      await pc.setRemoteDescription({ type: 'answer', sdp: response.sdp });
      if (bailIfCancelled()) return;

      const endForConnectionFailure = () => {
        setError('Live voice ended.');
        sendClose(false);
        releaseLocal();
        setStatus('idle');
      };
      pc.addEventListener('connectionstatechange', () => {
        // `connectionState` never reaches `closed` on its own here -- that
        // only happens after this handler's own `pc.close()` (via
        // `releaseLocal`) has already run -- so there is nothing for a
        // `'closed'` branch to catch that isn't already handled below.
        if (pc.connectionState === 'failed') {
          if (disconnectTimerRef.current !== null) {
            window.clearTimeout(disconnectTimerRef.current);
            disconnectTimerRef.current = null;
          }
          endForConnectionFailure();
          return;
        }
        if (pc.connectionState === 'disconnected') {
          // A brief blip (network handoff, momentary ICE hiccup) recovers
          // on its own; only treat it as final once it has held for a few
          // seconds.
          if (disconnectTimerRef.current === null) {
            disconnectTimerRef.current = window.setTimeout(() => {
              disconnectTimerRef.current = null;
              if (pc.connectionState === 'disconnected') endForConnectionFailure();
            }, 5000);
          }
          return;
        }
        if (disconnectTimerRef.current !== null) {
          window.clearTimeout(disconnectTimerRef.current);
          disconnectTimerRef.current = null;
        }
      });

      setStatus('active');
    } catch (err) {
      if (cancelledRef.current) {
        // A user cancel (stop, or unmount) landed while this was still
        // connecting -- `bailIfCancelled` above already handles that on
        // every await it guards, but a rejection thrown between two of
        // those (or by one of the calls it guards, e.g. `setRemoteDescription`)
        // lands here instead. Either way it is not a failure worth
        // reporting to the user.
        sendClose(false);
        releaseLocal();
        return;
      }
      // A failure here can land after the POST already opened (and is
      // billing) a session -- e.g. setRemoteDescription rejecting. Closing
      // it is the first thing this does, before anything else about the
      // failure is handled.
      sendClose(false);
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

  // Refresh the cached auth token every 30s while a session is active, so
  // the keepalive DELETE sent from `pagehide` (which cannot await Clerk)
  // never carries a token old enough for the server to reject with a 401.
  useEffect(() => {
    if (status !== 'active') return;
    const interval = window.setInterval(() => {
      void getAuthToken()
        .then((token) => {
          tokenRef.current = token;
        })
        .catch(() => {
          // Best-effort refresh; a failure here leaves the previous token
          // cached, which is still good for a while.
        });
    }, 30000);
    return () => window.clearInterval(interval);
  }, [status]);

  useEffect(() => {
    const handlePageHide = () => {
      closeRequestedRef.current = true;
      // `session.close` is synchronous and needs no auth, so it goes out
      // first, over the data channel, even if the keepalive DELETE below
      // ends up racing the page's actual teardown.
      const dc = dcRef.current;
      if (dc && dc.readyState === 'open') {
        try {
          dc.send(JSON.stringify({ type: 'session.close' }));
        } catch {
          // Best-effort; the keepalive DELETE below still runs.
        }
      }
      sendClose(true);
    };
    // `pagehide` only -- not `beforeunload`, which also fires when the
    // browser is merely asking whether to leave (e.g. a "Leave site?"
    // prompt the user then cancels), which would end the call for a tab
    // that never actually closed.
    window.addEventListener('pagehide', handlePageHide);
    return () => {
      window.removeEventListener('pagehide', handlePageHide);
      // A plain unmount (in-app navigation) keeps the tab alive, so the
      // regular DELETE path is reliable here -- no need for the beacon.
      cancelledRef.current = true;
      sendClose(false);
      releaseLocal();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return { status, error, toggle };
}
