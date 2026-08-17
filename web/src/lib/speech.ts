/**
 * Browser speech-to-text.
 *
 * Two honest limitations, both surfaced in the UI rather than hidden:
 *
 * 1. **No diarization.** The Web Speech API returns text, not "who said it".
 *    So the app asks: whoever is tapped as the active speaker owns the next
 *    finalized phrase. Attribution is a human input, and both parties can fix
 *    it before anything is confirmed.
 * 2. **Recognizers stop.** They end on silence, on tab blur, and sometimes for
 *    no reason. We restart automatically while the session is meant to be live,
 *    and show a clear indicator when we are not actually listening.
 *
 * The recognizer also sends audio to the browser vendor. That is worth knowing
 * before this handles anyone's real conversations, and is the main reason
 * server-side transcription of a recorded track is the natural next step.
 */

import { useCallback, useEffect, useRef, useState } from "react";

type Ctor = new () => SpeechRecognition;

function recognizerCtor(): Ctor | null {
  const w = window as unknown as {
    SpeechRecognition?: Ctor;
    webkitSpeechRecognition?: Ctor;
  };
  return w.SpeechRecognition ?? w.webkitSpeechRecognition ?? null;
}

export interface FinalPhrase {
  text: string;
  confidencePct: number | null;
}

export interface SpeechState {
  supported: boolean;
  listening: boolean;
  interim: string;
  error: string | null;
  start: () => void;
  stop: () => void;
}

export function useSpeech(onFinal: (phrase: FinalPhrase) => void): SpeechState {
  const [supported] = useState(() => recognizerCtor() !== null);
  const [listening, setListening] = useState(false);
  const [interim, setInterim] = useState("");
  const [error, setError] = useState<string | null>(null);

  const recognition = useRef<SpeechRecognition | null>(null);
  const wantListening = useRef(false);
  const handler = useRef(onFinal);
  handler.current = onFinal;

  const build = useCallback(() => {
    const Ctor = recognizerCtor();
    if (!Ctor) return null;

    const r = new Ctor();
    r.continuous = true;
    r.interimResults = true;
    r.lang = navigator.language || "en-US";

    r.onresult = (event: SpeechRecognitionEvent) => {
      let pending = "";
      for (let i = event.resultIndex; i < event.results.length; i++) {
        const result = event.results[i];
        if (!result) continue;
        const alt = result[0];
        if (!alt) continue;

        if (result.isFinal) {
          const text = alt.transcript.trim();
          if (text) {
            handler.current({
              text,
              // Recognizers report 0 when they have no opinion; treat that as
              // "no confidence given" rather than "zero confidence".
              confidencePct:
                typeof alt.confidence === "number" && alt.confidence > 0
                  ? Math.round(alt.confidence * 100)
                  : null,
            });
          }
        } else {
          pending += alt.transcript;
        }
      }
      setInterim(pending);
    };

    r.onerror = (event: SpeechRecognitionErrorEvent) => {
      // `no-speech` and `aborted` are routine; only real problems surface.
      if (event.error === "no-speech" || event.error === "aborted") return;
      setError(
        event.error === "not-allowed"
          ? "Microphone permission was denied. Allow it and start again."
          : `Speech recognition error: ${event.error}`,
      );
      wantListening.current = false;
      setListening(false);
    };

    r.onend = () => {
      setInterim("");
      if (wantListening.current) {
        // Silence ended the recognizer, not the user. Pick it back up.
        try {
          r.start();
        } catch {
          setListening(false);
        }
      } else {
        setListening(false);
      }
    };

    return r;
  }, []);

  const start = useCallback(() => {
    setError(null);
    if (!supported) return;
    if (!recognition.current) recognition.current = build();
    wantListening.current = true;
    try {
      recognition.current?.start();
      setListening(true);
    } catch {
      // Already started; harmless.
      setListening(true);
    }
  }, [build, supported]);

  const stop = useCallback(() => {
    wantListening.current = false;
    recognition.current?.stop();
    setListening(false);
    setInterim("");
  }, []);

  useEffect(
    () => () => {
      wantListening.current = false;
      recognition.current?.abort();
    },
    [],
  );

  return { supported, listening, interim, error, start, stop };
}
