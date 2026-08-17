import { useState } from "react";
import { useNavigate } from "react-router-dom";

import { api, ApiError } from "../lib/api";
import { rememberCounterpartLink, storeToken } from "../lib/tokens";
import { Button, Card, Masthead, Notice, Page, Spinner } from "../components/ui";

/**
 * There is no form here on purpose.
 *
 * Names used to be typed in before anything else, which is a strange thing to
 * ask of two people already standing in front of each other. They say who they
 * are out loud instead, and the witness picks it up — so the only thing this
 * screen does is open the session and hand each party their link.
 */
export default function Start() {
  const navigate = useNavigate();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function begin() {
    setBusy(true);
    setError(null);
    try {
      // Placeholders. Enrolment replaces them with what people call themselves,
      // and which of them is buying is settled later by the conversation.
      const started = await api.startSession("Voice A", "Voice B");
      // Both sides are stored: one phone between two people is the normal case,
      // and the link below covers the moment they walk away from each other.
      storeToken(started.deal_id, "seller", started.seller_token);
      storeToken(started.deal_id, "buyer", started.buyer_token);
      // Persisted, not just held in state: this component unmounts on the very
      // next line, and an earlier version set it and navigated away — leaving
      // the second party with no way into the deal at all.
      rememberCounterpartLink(
        started.deal_id,
        `${location.origin}/deal/${started.deal_id}?t=${started.seller_token}`,
      );
      navigate(`/witness/${started.session_id}`);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Could not start a session.");
      setBusy(false);
    }
  }

  return (
    <Page>
      <Masthead sub="A witnessed record of what two people actually agreed." />

      <Card className="mb-4">
        <p className="text-sm text-paper/85">
          Put the phone between you and talk. The witness listens, writes down
          every price either of you names, and hands the result back for you both
          to confirm.
        </p>
        <p className="mt-3 text-sm text-muted">
          Nothing binds anyone until you have both looked at what it heard.
        </p>
        <div className="mt-5">
          {busy ? (
            <Spinner label="Opening the session…" />
          ) : (
            <Button onClick={() => void begin()} full>
              Begin a handshake
            </Button>
          )}
        </div>
      </Card>

      {error && (
        <div className="mb-4">
          <Notice tone="bad">{error}</Notice>
        </div>
      )}


      <div className="mt-8 space-y-3 text-sm text-muted">
        <p className="font-display text-base text-paper">How it works</p>
        <ol className="list-decimal space-y-1.5 pl-5">
          <li>
            You each say who you are. The witness works out which voice is which,
            and you confirm it.
          </li>
          <li>Talk. Every price named gets written down, in order, in your words.</li>
          <li>
            You both confirm what it heard. Only then is the agreement frozen,
            hashed, and signed.
          </li>
          <li>
            The buyer funds escrow; the seller hands the item over; the buyer
            confirms. Funds release 24 hours later.
          </li>
          <li>
            You each keep a receipt anyone can verify — including a fingerprint of
            the recording it came from.
          </li>
        </ol>
      </div>
    </Page>
  );
}
