/**
 * Records the actual audio of a session, alongside the speech recognizer.
 *
 * The recognizer gives us text and throws the sound away. That's what made the
 * receipt's guarantee incomplete: it could prove the transcript was not altered,
 * but nothing tied the transcript to what was really said. Keeping the track and
 * hashing it into the attestation closes that.
 *
 * Two mic consumers run at once — `getUserMedia` here and the recognizer's own
 * internal capture. Browsers handle that fine in practice, but the permission
 * prompt appears once and covers both, so this hook is started first.
 *
 * The recording never leaves the device until the parties ask the witness to
 * read the conversation. Abandon the session and nothing was uploaded.
 */

import { useCallback, useEffect, useRef, useState } from "react";

export interface Recording {
  blob: Blob;
  mediaType: string;
  durationMs: number;
}

export interface RecorderState {
  supported: boolean;
  recording: boolean;
  /** Milliseconds captured so far, updated about once a second. */
  elapsedMs: number;
  error: string | null;
  start: () => Promise<void>;
  /** Stops and resolves with the finished track, or null if nothing was captured. */
  stop: () => Promise<Recording | null>;
}

/** The first container the browser will actually give us. */
function pickMimeType(): string {
  const candidates = [
    "audio/webm;codecs=opus",
    "audio/webm",
    "audio/mp4",
    "audio/ogg;codecs=opus",
  ];
  for (const type of candidates) {
    if (typeof MediaRecorder !== "undefined" && MediaRecorder.isTypeSupported(type)) {
      return type;
    }
  }
  return "";
}

export function useRecorder(): RecorderState {
  const [supported] = useState(
    () => typeof MediaRecorder !== "undefined" && !!navigator.mediaDevices?.getUserMedia,
  );
  const [recording, setRecording] = useState(false);
  const [elapsedMs, setElapsedMs] = useState(0);
  const [error, setError] = useState<string | null>(null);

  const recorder = useRef<MediaRecorder | null>(null);
  const stream = useRef<MediaStream | null>(null);
  const chunks = useRef<Blob[]>([]);
  const startedAt = useRef<number>(0);
  const ticker = useRef<number | undefined>(undefined);

  const teardown = useCallback(() => {
    if (ticker.current) window.clearInterval(ticker.current);
    ticker.current = undefined;
    stream.current?.getTracks().forEach((t) => t.stop());
    stream.current = null;
  }, []);

  const start = useCallback(async () => {
    setError(null);
    if (!supported) {
      setError("This browser cannot record audio.");
      return;
    }
    if (recorder.current?.state === "recording") return;

    try {
      const media = await navigator.mediaDevices.getUserMedia({
        audio: {
          echoCancellation: true,
          noiseSuppression: true,
          autoGainControl: true,
        },
      });
      stream.current = media;
      chunks.current = [];

      const mimeType = pickMimeType();
      const rec = new MediaRecorder(media, mimeType ? { mimeType } : undefined);
      rec.ondataavailable = (e) => {
        if (e.data.size > 0) chunks.current.push(e.data);
      };
      // A timeslice means we hold chunks rather than one growing buffer, so a
      // crash mid-negotiation loses seconds instead of everything.
      rec.start(1000);
      recorder.current = rec;
      startedAt.current = Date.now();
      setElapsedMs(0);
      setRecording(true);

      ticker.current = window.setInterval(
        () => setElapsedMs(Date.now() - startedAt.current),
        1000,
      );
    } catch (e) {
      setError(
        e instanceof DOMException && e.name === "NotAllowedError"
          ? "Microphone permission was denied, so nothing can be recorded."
          : "Could not start recording.",
      );
    }
  }, [supported]);

  const stop = useCallback(async (): Promise<Recording | null> => {
    const rec = recorder.current;
    if (!rec || rec.state === "inactive") {
      teardown();
      setRecording(false);
      return null;
    }

    const durationMs = Date.now() - startedAt.current;
    const finished = new Promise<Recording | null>((resolve) => {
      rec.onstop = () => {
        const blob = new Blob(chunks.current, { type: rec.mimeType || "audio/webm" });
        resolve(blob.size > 0 ? { blob, mediaType: rec.mimeType || "audio/webm", durationMs } : null);
      };
    });

    rec.stop();
    recorder.current = null;
    teardown();
    setRecording(false);
    return finished;
  }, [teardown]);

  useEffect(
    () => () => {
      recorder.current?.state === "recording" && recorder.current.stop();
      teardown();
    },
    [teardown],
  );

  return { supported, recording, elapsedMs, error, start, stop };
}

/** Base64 for upload. The server rehashes the bytes; this is transport only. */
export async function blobToBase64(blob: Blob): Promise<string> {
  const buf = new Uint8Array(await blob.arrayBuffer());
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < buf.length; i += chunk) {
    binary += String.fromCharCode(...buf.subarray(i, i + chunk));
  }
  return btoa(binary);
}

export function formatElapsed(ms: number): string {
  const s = Math.floor(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}
