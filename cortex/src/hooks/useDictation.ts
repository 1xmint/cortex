import { useCallback, useEffect, useRef, useState } from 'react';
import { CortexApiError } from '../lib/cortexApi';
import { transcribeDictation } from '../lib/voiceApi';

export type DictationStatus = 'idle' | 'requesting' | 'listening' | 'transcribing' | 'error';

/** The longest recording the server accepts (`MAX_AUDIO_SECONDS`). */
export const MAX_DICTATION_MS = 120_000;

/** Shown on a 402 from `POST /api/voice/dictation`. */
export const OUT_OF_CREDITS_MESSAGE = 'Out of credits — top up';
/** Shown on a 503 that survived the one retry. */
export const UNAVAILABLE_MESSAGE = 'Dictation unavailable, try again later';

interface UseDictationOptions {
  /** Called once with the transcribed text after a recording is sent. */
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

function pickMimeType(): string | undefined {
  if (typeof MediaRecorder === 'undefined' || typeof MediaRecorder.isTypeSupported !== 'function') {
    return undefined;
  }
  return ['audio/webm;codecs=opus', 'audio/webm', 'audio/mp4', 'audio/ogg;codecs=opus'].find((type) =>
    MediaRecorder.isTypeSupported(type),
  );
}

function failureMessage(err: unknown): string {
  if (err instanceof CortexApiError) {
    if (err.status === 402) return OUT_OF_CREDITS_MESSAGE;
    if (err.status === 503) return UNAVAILABLE_MESSAGE;
    if (err.status === 413) return 'That recording was too long. Keep dictation under two minutes.';
    return err.message;
  }
  return UNAVAILABLE_MESSAGE;
}

/**
 * Press-to-dictate: press the mic, speak, press again. The recording is sent
 * to Cortex (`POST /api/voice/dictation`), which transcribes it and charges
 * what the transcription cost; the text lands in the composer's draft. The
 * user still presses send -- this never sends a message on its own.
 */
export function useDictation({ onTranscript }: UseDictationOptions) {
  const [status, setStatus] = useState<DictationStatus>('idle');
  const [error, setError] = useState<string | null>(null);

  const recorderRef = useRef<MediaRecorder | null>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const chunksRef = useRef<Blob[]>([]);
  const startedAtRef = useRef(0);
  const limitTimerRef = useRef<number | null>(null);
  // Synchronous re-entrancy guard: React state updates are not visible
  // within the same tick, so a second press before the first `start()` has
  // had a chance to re-render must still be caught here.
  const busyRef = useRef(false);
  // Set by an unmount while a recording or upload is in flight: the result
  // is dropped instead of written into a composer that is gone.
  const cancelledRef = useRef(false);
  const onTranscriptRef = useRef(onTranscript);
  useEffect(() => {
    onTranscriptRef.current = onTranscript;
  }, [onTranscript]);

  const releaseMic = useCallback(() => {
    streamRef.current?.getTracks().forEach((track) => track.stop());
    streamRef.current = null;
  }, []);

  const clearLimitTimer = useCallback(() => {
    if (limitTimerRef.current !== null) {
      window.clearTimeout(limitTimerRef.current);
      limitTimerRef.current = null;
    }
  }, []);

  const upload = useCallback(async (blob: Blob, durationMs: number) => {
    // One idempotency key per recording, reused by the retry: the server
    // charges once per key, and a 503 charged nothing.
    const idempotencyKey = crypto.randomUUID();
    setStatus('transcribing');
    try {
      let result: { text: string };
      try {
        result = await transcribeDictation(blob, idempotencyKey, durationMs);
      } catch (first) {
        if (!(first instanceof CortexApiError && first.status === 503) || cancelledRef.current) {
          throw first;
        }
        result = await transcribeDictation(blob, idempotencyKey, durationMs);
      }
      if (cancelledRef.current) return;
      const text = result.text.trim();
      if (text) onTranscriptRef.current(text + ' ');
      setStatus('idle');
    } catch (err) {
      if (cancelledRef.current) return;
      setError(failureMessage(err));
      setStatus('idle');
    } finally {
      busyRef.current = false;
    }
  }, []);

  const finish = useCallback(() => {
    clearLimitTimer();
    const recorder = recorderRef.current;
    if (recorder && recorder.state !== 'inactive') recorder.stop();
  }, [clearLimitTimer]);

  const start = useCallback(async () => {
    if (busyRef.current) return;
    busyRef.current = true;
    cancelledRef.current = false;
    setError(null);
    setStatus('requesting');
    try {
      // Ask for the mic before anything else: a denial or a missing device
      // must show a message and send nothing.
      let stream: MediaStream;
      try {
        if (!navigator.mediaDevices?.getUserMedia || typeof MediaRecorder === 'undefined') {
          throw new Error('unsupported');
        }
        stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      } catch (mediaError) {
        setError(microphoneErrorMessage(mediaError));
        setStatus('idle');
        busyRef.current = false;
        return;
      }
      streamRef.current = stream;
      if (cancelledRef.current) {
        releaseMic();
        busyRef.current = false;
        return;
      }

      const mimeType = pickMimeType();
      const recorder = mimeType ? new MediaRecorder(stream, { mimeType }) : new MediaRecorder(stream);
      recorderRef.current = recorder;
      chunksRef.current = [];
      recorder.addEventListener('dataavailable', (event: BlobEvent) => {
        if (event.data && event.data.size > 0) chunksRef.current.push(event.data);
      });
      recorder.addEventListener('stop', () => {
        releaseMic();
        recorderRef.current = null;
        const chunks = chunksRef.current;
        chunksRef.current = [];
        if (cancelledRef.current) {
          busyRef.current = false;
          return;
        }
        const type = recorder.mimeType || mimeType || 'audio/webm';
        const blob = new Blob(chunks, { type });
        const durationMs = Math.min(Math.max(Date.now() - startedAtRef.current, 1), MAX_DICTATION_MS);
        if (blob.size === 0) {
          setStatus('idle');
          busyRef.current = false;
          return;
        }
        void upload(blob, durationMs);
      });
      recorder.start();
      startedAtRef.current = Date.now();
      // The server refuses anything over two minutes; stop before it can.
      limitTimerRef.current = window.setTimeout(finish, MAX_DICTATION_MS);
      setStatus('listening');
    } catch {
      releaseMic();
      recorderRef.current = null;
      setError('Could not access the microphone.');
      setStatus('idle');
      busyRef.current = false;
    }
  }, [finish, releaseMic, upload]);

  const toggle = useCallback(() => {
    if (status === 'listening') {
      finish();
    } else if (status === 'idle' || status === 'error') {
      void start();
    }
  }, [status, start, finish]);

  // Release the mic on unmount and drop any recording or upload in flight.
  useEffect(
    () => () => {
      cancelledRef.current = true;
      clearLimitTimer();
      const recorder = recorderRef.current;
      if (recorder && recorder.state !== 'inactive') recorder.stop();
      releaseMic();
    },
    [clearLimitTimer, releaseMic],
  );

  return { status, error, toggle };
}
