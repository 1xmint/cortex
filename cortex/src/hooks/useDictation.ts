import { useCallback, useEffect, useRef, useState } from 'react';
import { CortexApiError } from '../lib/cortexApi';
import { requestDictationToken } from '../lib/voiceApi';

/**
 * Where the browser sends its SDP offer for a dictation (transcription-only)
 * session -- straight to OpenAI, with the ephemeral token
 * `/api/voice/dictation/token` minted, never through Cortex. See
 * `crates/api/src/voice.rs`'s module doc for why: dictation has no
 * server-observable usage, so there is nothing for Cortex to sit in the
 * middle of.
 */
const REALTIME_CALLS_URL = 'https://api.openai.com/v1/realtime/calls';

export type DictationStatus = 'idle' | 'requesting' | 'listening' | 'error';

interface UseDictationOptions {
  /** Called with each transcribed text fragment as it arrives. */
  onTranscript: (text: string) => void;
}

function microphoneErrorMessage(error: unknown): string {
  if (error instanceof Error && error.name === 'NotAllowedError') {
    return 'Microphone access was denied. Allow microphone access to use dictation.';
  }
  if (error instanceof Error && error.name === 'NotFoundError') {
    return 'No microphone was found. Connect a microphone to use dictation.';
  }
  if (error instanceof Error && error.message === 'unsupported') {
    return 'Dictation is not supported in this browser.';
  }
  return 'Could not access the microphone.';
}

/**
 * Press-to-dictate: speech becomes text in the composer's draft. The user
 * still presses send -- this never sends a message on its own.
 */
export function useDictation({ onTranscript }: UseDictationOptions) {
  const [status, setStatus] = useState<DictationStatus>('idle');
  const [error, setError] = useState<string | null>(null);

  const pcRef = useRef<RTCPeerConnection | null>(null);
  const dcRef = useRef<RTCDataChannel | null>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const expiryTimerRef = useRef<number | null>(null);
  // Synchronous re-entrancy guard: React state updates are not visible
  // within the same tick, so a second press before the first `start()` has
  // had a chance to re-render must still be caught here.
  const startingRef = useRef(false);
  // Set by an unmount or a user-initiated stop while `start` is still
  // in-flight -- see `useLiveVoiceToggle`'s `cancelledRef` for the same
  // pattern and why it matters here too (a stray token request after the
  // user has already left).
  const cancelledRef = useRef(false);
  const onTranscriptRef = useRef(onTranscript);
  useEffect(() => {
    onTranscriptRef.current = onTranscript;
  }, [onTranscript]);

  const releaseMic = useCallback(() => {
    streamRef.current?.getTracks().forEach((track) => track.stop());
    streamRef.current = null;
  }, []);

  const teardown = useCallback(() => {
    if (expiryTimerRef.current !== null) {
      window.clearTimeout(expiryTimerRef.current);
      expiryTimerRef.current = null;
    }
    dcRef.current?.close();
    dcRef.current = null;
    pcRef.current?.close();
    pcRef.current = null;
    releaseMic();
  }, [releaseMic]);

  const stop = useCallback(() => {
    cancelledRef.current = true;
    teardown();
    setStatus('idle');
  }, [teardown]);

  const start = useCallback(async () => {
    if (startingRef.current || status === 'listening' || status === 'requesting') return;
    startingRef.current = true;
    cancelledRef.current = false;
    setError(null);
    setStatus('requesting');
    try {
      // Ask for the mic before ever calling the token route: a denial or a
      // missing device must show a message and charge nothing.
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
      if (cancelledRef.current) {
        releaseMic();
        return;
      }

      const idempotencyKey = crypto.randomUUID();
      const tokenResponse = await requestDictationToken(idempotencyKey);
      if (cancelledRef.current) {
        releaseMic();
        return;
      }

      const pc = new RTCPeerConnection();
      pcRef.current = pc;
      const [track] = stream.getTracks();
      if (track) pc.addTrack(track, stream);

      const dc = pc.createDataChannel('oai-events');
      dcRef.current = dc;
      dc.addEventListener('message', (event) => {
        try {
          const payload = JSON.parse(event.data as string) as {
            type?: string;
            delta?: string;
          };
          if (
            payload.type === 'conversation.item.input_audio_transcription.delta' &&
            typeof payload.delta === 'string'
          ) {
            onTranscriptRef.current(payload.delta);
          } else if (payload.type === 'conversation.item.input_audio_transcription.completed') {
            // A completed turn has no trailing space of its own; add one so
            // the next utterance doesn't run into this one.
            onTranscriptRef.current(' ');
          }
        } catch {
          // Not JSON, or not a shape we handle -- ignore.
        }
      });

      const offer = await pc.createOffer();
      if (cancelledRef.current) {
        teardown();
        return;
      }
      await pc.setLocalDescription(offer);
      if (cancelledRef.current) {
        teardown();
        return;
      }

      const sdpResponse = await fetch(REALTIME_CALLS_URL, {
        method: 'POST',
        body: offer.sdp,
        headers: {
          Authorization: `Bearer ${tokenResponse.token}`,
          'Content-Type': 'application/sdp',
        },
      });
      if (cancelledRef.current) {
        teardown();
        return;
      }
      if (!sdpResponse.ok) {
        throw new Error('Dictation is temporarily unavailable. Please try again in a moment.');
      }
      const answerSdp = await sdpResponse.text();
      if (cancelledRef.current) {
        teardown();
        return;
      }
      await pc.setRemoteDescription({ type: 'answer', sdp: answerSdp });
      if (cancelledRef.current) {
        teardown();
        return;
      }

      pc.addEventListener('connectionstatechange', () => {
        if (
          pc.connectionState === 'failed' ||
          pc.connectionState === 'closed' ||
          pc.connectionState === 'disconnected'
        ) {
          teardown();
          setStatus('idle');
        }
      });

      // The token's lifetime is fixed and charged for up front server-side
      // (`DICTATION_SECONDS`); stop and release the mic when it runs out
      // instead of leaving a dead connection open.
      expiryTimerRef.current = window.setTimeout(() => {
        stop();
      }, tokenResponse.seconds * 1000);

      setStatus('listening');
    } catch (err) {
      teardown();
      setError(
        err instanceof CortexApiError
          ? err.message
          : err instanceof Error
            ? err.message
            : 'Dictation is temporarily unavailable. Please try again in a moment.',
      );
      setStatus('idle');
    } finally {
      startingRef.current = false;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [status, teardown, stop]);

  const toggle = useCallback(() => {
    if (status === 'listening' || status === 'requesting') {
      stop();
    } else {
      void start();
    }
  }, [status, start, stop]);

  // Release the mic and close the connection on unmount, no matter what
  // state dictation was in.
  useEffect(
    () => () => {
      cancelledRef.current = true;
      teardown();
    },
    [teardown],
  );

  return { status, error, toggle };
}
