import { useCallback, useEffect, useRef, useState } from 'react';
import { apiUrl, getAuthToken, CortexApiError } from '../lib/cortexApi';
import {
  closeLiveVoiceSession,
  openLiveVoiceEventsStream,
  postPromptEnded,
  startLiveVoiceSession,
  type LiveVoiceSessionEvent,
} from '../lib/voiceApi';
import { routeLiveVoiceEvent } from './liveVoiceEvents';
import { SpokenPromptEndDetector } from '../lib/spokenPromptEndDetector';

/** How often the spoken-prompt watcher polls the remote receiver's audio level. */
const PROMPT_WATCH_POLL_MS = 100;

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
    return closeRequestedByUs ? null : 'Live voice ended: you reached your credit or session limit.';
  }
  if (reason === 'expired') return 'Live voice ended: the session reached its time limit.';
  if (reason === 'content') return 'Live voice ended by a safety filter.';
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
export interface UseLiveVoiceToggleOptions {
  /**
   * Resolves the conversation id a voice turn's user text and assistant
   * answer get saved to -- creating one first (the same call the chat uses
   * for a new chat) if none is open yet. `undefined`/`null` starts the
   * session with no conversation attached.
   */
  getConversationId?: () => Promise<string | null | undefined>;
  /** Every event off `GET /api/voice/live/sessions/{id}/events`. */
  onVoiceEvent?: (event: LiveVoiceSessionEvent) => void;
}

export function useLiveVoiceToggle(options: UseLiveVoiceToggleOptions = {}) {
  const [status, setStatus] = useState<LiveVoiceStatus>('idle');
  const [error, setError] = useState<string | null>(null);

  const pcRef = useRef<RTCPeerConnection | null>(null);
  const dcRef = useRef<RTCDataChannel | null>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const eventsControllerRef = useRef<AbortController | null>(null);
  // Kept current across renders without re-running `start`/`releaseLocal`'s
  // `useCallback`s on every parent render.
  const getConversationIdRef = useRef(options.getConversationId);
  const onVoiceEventRef = useRef(options.onVoiceEvent);
  useEffect(() => {
    getConversationIdRef.current = options.getConversationId;
    onVoiceEventRef.current = options.onVoiceEvent;
  });
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
  // The remote (model) audio receiver, set from the `track` event -- the
  // spoken-prompt watcher below polls its `audioLevel` to tell when the
  // prompt has finished playing.
  const remoteReceiverRef = useRef<RTCRtpReceiver | null>(null);
  const promptWatcherRef = useRef<number | null>(null);
  // The detector driving the current watcher, if any -- transcript deltas
  // off the `oai-events` data channel are fed into it as they arrive (see
  // the `dc` message handler below). `null` whenever no watcher is running.
  const promptWatcherDetectorRef = useRef<SpokenPromptEndDetector | null>(null);
  // The action id the current watcher is for, so a `confirm_resolved` event
  // for that same action can stop it early.
  const promptWatcherActionIdRef = useRef<string | null>(null);

  const stopPromptWatcher = useCallback(() => {
    if (promptWatcherRef.current !== null) {
      window.clearInterval(promptWatcherRef.current);
      promptWatcherRef.current = null;
    }
    promptWatcherDetectorRef.current = null;
    promptWatcherActionIdRef.current = null;
  }, []);

  /**
   * Watches the model's own transcript and the remote receiver's audio level
   * after a `confirm_required` event to detect when the spoken confirm
   * prompt has finished playing, and reports it once via
   * `POST .../prompt-ended`. The prompt is anchored on its fixed tail text
   * ("tap Confirm on screen") arriving over the transcript -- audio level is
   * ignored until that anchor is seen, so speech from *before* the prompt
   * (e.g. the model musing about the action) can never be mistaken for the
   * prompt ending. See `SpokenPromptEndDetector` for the full rule. A new
   * call (a fresh `confirm_required`) replaces whatever watcher is already
   * running for a previous action, and a `confirm_resolved` for the same
   * action stops it early (see the events-stream handler below).
   */
  const watchForPromptEnd = useCallback(
    (sessionId: string, actionId: string) => {
      stopPromptWatcher();
      const receiver = remoteReceiverRef.current;
      if (!receiver || typeof receiver.getSynchronizationSources !== 'function') {
        // No remote receiver reachable in this code path -- nothing to poll,
        // so there is no prompt-ended signal to send. Left silent: the 45s
        // window still opens from a tap, just without the spoken shortcut.
        return;
      }
      const detector = new SpokenPromptEndDetector();
      promptWatcherDetectorRef.current = detector;
      promptWatcherActionIdRef.current = actionId;
      promptWatcherRef.current = window.setInterval(() => {
        const sources = receiver.getSynchronizationSources();
        const level = sources[0]?.audioLevel ?? 0;
        const result = detector.sample(level, Date.now());
        if (result === 'listening') return;
        stopPromptWatcher();
        if (result === 'ended') {
          void postPromptEnded(sessionId, actionId).catch(() => {
            // Best-effort -- a failed report just means the card falls back
            // to the tap-to-confirm path already on screen.
          });
        }
      }, PROMPT_WATCH_POLL_MS);
    },
    [stopPromptWatcher],
  );

  const releaseLocal = useCallback(() => {
    dcRef.current?.close();
    dcRef.current = null;
    pcRef.current?.close();
    pcRef.current = null;
    streamRef.current?.getTracks().forEach((track) => track.stop());
    streamRef.current = null;
    eventsControllerRef.current?.abort();
    eventsControllerRef.current = null;
    stopPromptWatcher();
    remoteReceiverRef.current = null;
    if (audioRef.current) {
      audioRef.current.srcObject = null;
      audioRef.current = null;
    }
    if (disconnectTimerRef.current !== null) {
      window.clearTimeout(disconnectTimerRef.current);
      disconnectTimerRef.current = null;
    }
  }, [stopPromptWatcher]);

  /**
   * Sends the DELETE for the currently open session, if any, exactly once.
   * Clearing `sessionIdRef` first makes every other caller (toggle, unmount,
   * pagehide, connection-state-change) a no-op once one of them has already
   * fired -- there is no separate "already closing" flag to fall out of
   * sync with it.
   *
   * `useBeacon` is for the tab-closing path (`pagehide`), where a normal
   * `fetch` can be aborted mid-flight by the navigation; a
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
        remoteReceiverRef.current = event.receiver;
      });

      const dc = pc.createDataChannel('oai-events');
      dcRef.current = dc;
      dc.addEventListener('message', (event) => {
        let payload: { type?: string; reason?: unknown; delta?: unknown };
        try {
          payload = JSON.parse(event.data as string);
        } catch {
          return; // Not JSON -- nothing to route.
        }
        if (payload.type === 'session.output_transcript.delta') {
          // The model's own speech transcript -- fed to whichever
          // spoken-prompt watcher is currently running, if any, so it can
          // recognize the fixed confirm-prompt tail text.
          if (typeof payload.delta === 'string') {
            promptWatcherDetectorRef.current?.onTranscript(payload.delta);
          }
          return;
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

      // Resolved (and, if none is open yet, created) before the session
      // starts so the server can save this turn's text to it from the
      // first message onward -- there is no way to attach a conversation
      // to a session after the fact.
      const conversationId = (await getConversationIdRef.current?.()) ?? null;
      if (bailIfCancelled()) return;

      const response = await startLiveVoiceSession(
        pc.localDescription?.sdp ?? offer.sdp ?? '',
        conversationId,
      );
      sessionIdRef.current = response.session_id;
      if (bailIfCancelled()) return;

      await pc.setRemoteDescription({ type: 'answer', sdp: response.sdp });
      if (bailIfCancelled()) return;

      // The events stream mirrors voice turns and risky-action confirms
      // into the open conversation's UI; it is best-effort text alongside
      // the live audio, not a dependency the call itself needs, so opening
      // it never blocks reaching `active`.
      eventsControllerRef.current = openLiveVoiceEventsStream(response.session_id, (event) => {
        if (event.type === 'confirm_required') {
          watchForPromptEnd(response.session_id, event.action_id);
        } else if (
          event.type === 'confirm_resolved' &&
          promptWatcherActionIdRef.current === event.action_id
        ) {
          // The action was resolved (confirmed/denied/expired) some other
          // way before the spoken-prompt watcher decided anything -- stop
          // watching rather than posting a stale prompt-ended afterward.
          stopPromptWatcher();
        }
        onVoiceEventRef.current?.(event);
      });

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
    const handlePageHide = (event: PageTransitionEvent) => {
      cancelledRef.current = true;
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
      // `event.persisted` means the page is going into the back/forward
      // cache instead of unloading -- the tab (and this hook's state) can
      // come back on a `pageshow`. The RTCPeerConnection and mic track do
      // not survive bfcache regardless, so tear them down here too and
      // reset to idle, rather than leaving the UI showing a session that
      // no longer exists once the page is restored.
      if (event.persisted) {
        releaseLocal();
        setStatus('idle');
      }
    };
    // `pagehide` only -- not `beforeunload`, which also fires when the
    // browser is merely asking whether to leave (e.g. a "Leave site?"
    // prompt the user then cancels), which would end the call for a tab
    // that never actually closed.
    window.addEventListener('pagehide', handlePageHide);
    // A page restored from bfcache resumes with whatever React state it
    // was frozen with; `handlePageHide` above already tore down the
    // connection and set `idle` when `persisted` was true, but this is a
    // second, defensive pass in case restoration raced that reset.
    const handlePageShow = (event: PageTransitionEvent) => {
      if (event.persisted) setStatus('idle');
    };
    window.addEventListener('pageshow', handlePageShow);
    return () => {
      window.removeEventListener('pagehide', handlePageHide);
      window.removeEventListener('pageshow', handlePageShow);
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
