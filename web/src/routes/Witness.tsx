import { useCallback, useEffect, useRef, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";

import { api, ApiError, type Utterance } from "../lib/api";
import { useSpeech } from "../lib/speech";
import { blobToBase64, formatElapsed, useRecorder } from "../lib/recorder";
import { Button, Card, Masthead, Notice, Page, Spinner } from "../components/ui";

interface Line {
  id: string;
  text: string;
  at: string;
  confidencePct: number | null;
  corrected: boolean;
}

/**
 * Capture: one button.
 *
 * An earlier version asked each party to tap their name before speaking, so the
 * transcript arrived pre-attributed. Nobody haggling over a used watch is going
 * to do that, and a capture step people skip is worse than no capture step —
 * it produces confidently mislabelled evidence.
 *
 * So the transcript is unattributed, and the witness works out who said what
 * from the conversation itself: people say their names near the start, and roles
 * follow from who owns the thing and who offers money. The parties then confirm
 * that reading, which is the same safety net that already covers the price.
 *
 * When a diarizer replaces browser recognition, it fills in `speaker` on each
 * line and the witness's job gets easier. Nothing else changes.
 */
export default function Witness() {
  const { sessionId = "" } = useParams();
  const navigate = useNavigate();

  const draftKey = `th:draft:${sessionId}`;
  const [lines, setLines] = useState<Line[]>(() => {
    try {
      return JSON.parse(localStorage.getItem(draftKey) ?? "[]") as Line[];
    } catch {
      return [];
    }
  });
  const [started, setStarted] = useState(false);
  const [reading, setReading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const bottom = useRef<HTMLDivElement>(null);

  const recorder = useRecorder();

  useEffect(() => {
    localStorage.setItem(draftKey, JSON.stringify(lines));
    bottom.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [lines, draftKey]);

  const addLine = useCallback((text: string, confidencePct: number | null) => {
    setLines((prev) => [
      ...prev,
      {
        id: crypto.randomUUID(),
        text,
        at: new Date().toISOString(),
        confidencePct,
        corrected: false,
      },
    ]);
  }, []);

  const speech = useSpeech(
    useCallback((phrase) => addLine(phrase.text, phrase.confidencePct), [addLine]),
  );


  async function startListening() {
    setError(null);
    // Recording first: one permission prompt covers both consumers.
    await recorder.start();
    speech.start();
    setStarted(true);
  }

  function stopListening() {
    speech.stop();
  }

  async function readConversation() {
    setReading(true);
    setError(null);
    try {
      speech.stop();
      const recording = await recorder.stop();

      if (recording) {
        await api.attachAudio(
          sessionId,
          recording.mediaType,
          await blobToBase64(recording.blob),
          recording.durationMs,
        );
      }

      // Sent without a `speaker`: the witness attributes each line, and both
      // parties confirm that reading before anything binds.
      const utterances: Utterance[] = lines.map((l, i) => ({
        seq: i,
        text: l.text,
        at: l.at,
        confidence_pct: l.confidencePct,
        corrected: l.corrected,
      }));
      await api.appendUtterances(sessionId, utterances);

      const result = await api.propose(sessionId);
      localStorage.removeItem(draftKey);
      navigate(`/deal/${result.deal_id}`);
    } catch (e) {
      setError(
        e instanceof ApiError ? e.message : "The witness could not read this conversation.",
      );
      setReading(false);
    }
  }

  return (
    <Page>
      <Masthead
        sub={
          started
            ? "Talk normally. Say who you are if you haven't yet."
            : "Put the phone between you and press start."
        }
      />


      {!started ? (
        <>
          <Card title="Before you start" className="mb-4">
            <p className="text-sm text-paper/85">
              Say your names somewhere near the beginning —{" "}
              <em className="text-cyan">“Hey, I'm Stella”</em>,{" "}
              <em className="text-cyan">“this is Nash”</em>. That's how the
              witness works out who is buying and who is selling.
            </p>
            <p className="mt-3 text-sm text-muted">
              Then just talk. Every price either of you names gets written down,
              in order, in your own words.
            </p>
            <p className="mt-3 text-xs text-muted">
              The conversation is recorded and its fingerprint goes into the
              receipt. Nothing leaves this device until you ask the witness to
              read it.
            </p>
          </Card>
          <Button full onClick={() => void startListening()}>
            Start listening
          </Button>
        </>
      ) : (
        <>
          <Card className="mb-4">
            <div className="flex items-center justify-between">
              <span className="flex items-center gap-2.5 text-sm">
                {speech.listening ? (
                  <>
                    <span
                      className="pulse-dot h-2.5 w-2.5 rounded-full bg-bad"
                      aria-hidden="true"
                    />
                    <span className="text-bad">listening</span>
                  </>
                ) : (
                  <>
                    <span className="h-2.5 w-2.5 rounded-full bg-line" aria-hidden="true" />
                    <span className="text-muted">paused</span>
                  </>
                )}
              </span>
              <span className="tabular font-mono text-sm text-muted">
                {formatElapsed(recorder.elapsedMs)}
              </span>
            </div>

            <div className="mt-4">
              {speech.listening ? (
                <Button variant="ghost" full onClick={stopListening}>
                  Stop listening
                </Button>
              ) : (
                <Button variant="ghost" full onClick={speech.start}>
                  Start listening again
                </Button>
              )}
            </div>
          </Card>

          {!speech.supported && (
            <div className="mb-4">
              <Notice tone="warn" title="This browser has no speech recognition">
                Chrome and Safari support it. You can still type each line below —
                the witness reads text, and the recording is kept either way.
              </Notice>
            </div>
          )}
          {speech.error && (
            <div className="mb-4">
              <Notice tone="bad">{speech.error}</Notice>
            </div>
          )}
          {recorder.error && (
            <div className="mb-4">
              <Notice tone="warn" title="Recording unavailable">
                {recorder.error} The conversation can still be transcribed, but the
                receipt will not be able to point back at the audio.
              </Notice>
            </div>
          )}

          <Card title="What was said" className="mb-4">
            {lines.length === 0 && !speech.interim && (
              <p className="text-sm text-muted">
                Nothing yet. Everything stays on this device until you ask the
                witness to read it.
              </p>
            )}

            <ul className="space-y-2.5">
              {lines.map((line, i) => (
                <li key={line.id} className="group flex items-baseline gap-2">
                  <span
                    aria-hidden="true"
                    className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-line"
                  />
                  <input
                    value={line.text}
                    onChange={(e) =>
                      setLines((prev) =>
                        prev.map((l, j) =>
                          j === i ? { ...l, text: e.target.value, corrected: true } : l,
                        ),
                      )
                    }
                    className="min-w-0 flex-1 border-b border-transparent bg-transparent text-sm text-paper outline-none focus:border-line"
                  />
                  <button
                    onClick={() => setLines((prev) => prev.filter((_, j) => j !== i))}
                    aria-label={`Delete line ${i + 1}`}
                    className="shrink-0 px-1 text-muted opacity-0 transition-opacity group-focus-within:opacity-100 group-hover:opacity-100"
                  >
                    ×
                  </button>
                </li>
              ))}
            </ul>

            {speech.interim && (
              <p className="mt-3 pl-3.5 text-sm text-muted italic">{speech.interim}…</p>
            )}

            <div ref={bottom} />

            <form
              className="mt-4 flex gap-2 border-t border-line pt-4"
              onSubmit={(e) => {
                e.preventDefault();
                const input = e.currentTarget.elements.namedItem("manual") as HTMLInputElement;
                if (input.value.trim()) {
                  addLine(input.value.trim(), null);
                  input.value = "";
                }
              }}
            >
              <input
                name="manual"
                placeholder="Type a line"
                className="min-w-0 flex-1 rounded-lg border border-line bg-raised px-3 py-2 text-sm text-paper outline-none placeholder:text-muted/60 focus:border-cyan"
              />
              <Button type="submit" variant="ghost">
                Add
              </Button>
            </form>
          </Card>

          {error && (
            <div className="mb-4">
              <Notice tone="bad">{error}</Notice>
            </div>
          )}

          {reading ? (
            <Spinner label="The witness is reading the conversation…" />
          ) : (
            <Button full disabled={lines.length < 2} onClick={() => void readConversation()}>
              Have the witness read it
            </Button>
          )}

          <p className="mt-3 text-center text-xs text-muted">
            Nothing is binding yet. You will both see what it heard — including who
            it thinks said what — and can correct it before agreeing.
          </p>
        </>
      )}
    </Page>
  );
}

export function ShareLink({ url }: { url: string }) {
  const [copied, setCopied] = useState(false);
  // `navigator.share` is absent on desktop browsers; TS types it as always
  // defined, so probe the object rather than the function.
  const canShare = typeof navigator !== "undefined" && "share" in navigator;

  async function share() {
    // The native sheet is the fastest path on a phone: it offers Messages,
    // AirDrop, WhatsApp — whatever the two of them already use.
    if (canShare) {
      try {
        await navigator.share({ title: "True Handshake", url });
        return;
      } catch {
        // Cancelled; fall through to copying.
      }
    }
    await navigator.clipboard.writeText(url);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  }

  return (
    <div className="flex items-center gap-2">
      <code className="min-w-0 flex-1 truncate rounded-lg bg-ink/60 px-3 py-2 font-mono text-xs text-muted">
        {url}
      </code>
      <Button variant="cyan" onClick={() => void share()}>
        {copied ? "Copied" : canShare ? "Send" : "Copy"}
      </Button>
    </div>
  );
}
