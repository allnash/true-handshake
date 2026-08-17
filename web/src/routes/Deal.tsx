import { useCallback, useEffect, useRef, useState } from "react";
import { Link, useLocation, useParams } from "react-router-dom";

import { api, ApiError, formatMoney, type DealView } from "../lib/api";
import { anchor, useCountdown, usePoll } from "../lib/clock";
import {
  getActiveRole,
  getToken,
  heldRoles,
  incomingToken,
  recallCounterpartLink,
  setActiveRole,
  storeToken,
  type Role,
} from "../lib/tokens";
import { ShareLink } from "./Witness";
import {
  Button,
  Card,
  Countdown,
  Field,
  Hash,
  Ladder,
  Masthead,
  Notice,
  Page,
  Spinner,
  StateBadge,
  TermsSheet,
} from "../components/ui";

async function fileToBase64(file: File): Promise<{ media_type: string; data_b64: string }> {
  const buf = await file.arrayBuffer();
  let binary = "";
  const bytes = new Uint8Array(buf);
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return { media_type: file.type || "image/jpeg", data_b64: btoa(binary) };
}

export default function Deal() {
  const { dealId = "" } = useParams();
  const { search } = useLocation();

  // A link in the URL wins: following one is an explicit statement of which
  // party you are. Its role is unknown until the server says, so it gets filed
  // under that role once the deal loads.
  const [urlToken] = useState(() => incomingToken(search));
  const [role, setRole] = useState<Role | null>(() => getActiveRole(dealId));
  const [held, setHeld] = useState<Role[]>(() => heldRoles(dealId));
  /** Who to pass the phone to next, once you have confirmed. */
  const [handover, setHandover] = useState<Role | null>(null);

  const bothOnThisDevice = held.length === 2;
  const token = (role && getToken(dealId, role)) || urlToken;

  const [deal, setDeal] = useState<DealView | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [moved, setMoved] = useState<string | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const prevState = useRef<string | null>(null);

  const load = useCallback(async () => {
    if (!token) return;
    try {
      const d = await api.getDeal(dealId, token);
      // Anchor countdowns to the server's clock, not this device's.
      anchor(d.server_time);
      // A change made by the other party is news, not an error.
      if (prevState.current && prevState.current !== d.state) {
        const other = d.your_role === "buyer" ? d.seller_name : d.buyer_name;
        setMoved(`${other} moved this on while you were looking at it.`);
      }
      prevState.current = d.state;
      // File this token under the role the server says it is, so the device can
      // hold both sides and switch between them.
      if (token) {
        storeToken(dealId, d.your_role, token);
        setRole(d.your_role);
        setHeld(heldRoles(dealId));
      }
      setDeal(d);
      setError(null);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Could not load this deal.");
    }
  }, [dealId, token]);

  function switchTo(next: Role) {
    setActiveRole(dealId, next);
    setRole(next);
    setHandover(null);
    setDeal(null);
    prevState.current = null;
  }

  useEffect(() => {
    void load();
  }, [load]);

  // The other party acts on their own phone, so keep this view fresh.
  usePoll(() => void load(), 4000, Boolean(token));

  const act = useCallback(
    async (name: string, fn: () => Promise<unknown>) => {
      setBusy(name);
      setError(null);
      setMoved(null);
      try {
        await fn();
        await load();
      } catch (e) {
        if (e instanceof ApiError && e.isConflict) {
          setMoved("This deal changed while you were looking at it. Here is where it stands now.");
          await load();
        } else {
          setError(e instanceof ApiError ? e.message : "That did not go through.");
        }
      } finally {
        setBusy(null);
      }
    },
    [load],
  );

  const countdown = useCountdown(deal?.release_due_at ?? null);
  const counterpartLink = recallCounterpartLink(dealId);

  if (!token) {
    return (
      <Page>
        <Masthead />
        <Notice tone="bad" title="No access token">
          This link is missing the part that says which side of the deal you are.
          Ask the other party to resend it.
        </Notice>
      </Page>
    );
  }

  if (!deal) {
    return (
      <Page>
        <Masthead />
        {error ? <Notice tone="bad">{error}</Notice> : <Spinner label="Loading the deal…" />}
      </Page>
    );
  }

  const you = deal.your_role === "buyer" ? deal.buyer_name : deal.seller_name;
  const them = deal.your_role === "buyer" ? deal.seller_name : deal.buyer_name;
  const isBuyer = deal.your_role === "buyer";

  // Passing the phone is a moment, not a second button next to the first one.
  // A full-screen gate makes it hard to confirm both sides by accident, which
  // is the failure this whole mode has to design against.
  if (handover) {
    const nextName = handover === "buyer" ? deal.buyer_name : deal.seller_name;
    return (
      <Page>
        <Masthead sub="Your confirmation is recorded." />
        <Card className="text-center">
          <p className="font-display text-3xl text-paper">Hand the phone to</p>
          <p className="mt-2 font-display text-5xl text-seal">{nextName}</p>
          <p className="mx-auto mt-6 max-w-sm text-sm text-muted">
            {nextName} should read what the witness heard and confirm it
            themselves. Both confirmations from one device are recorded as such on
            the receipt — it does not pretend they were independent.
          </p>
          <div className="mt-8">
            <Button full onClick={() => switchTo(handover)}>
              I'm {nextName}
            </Button>
          </div>
          <button
            onClick={() => setHandover(null)}
            className="mt-4 w-full text-center text-sm text-muted underline underline-offset-2"
          >
            Not yet — keep looking at it
          </button>
        </Card>
      </Page>
    );
  }

  return (
    <Page>
      <Masthead sub={`You are ${you} · ${isBuyer ? "buying" : "selling"}`} />

      <div className="mb-4 flex items-center justify-between gap-3">
        <StateBadge state={deal.state} />
        <Link
          to={`/v/${deal.deal_id}`}
          className="text-xs text-muted underline underline-offset-2 hover:text-paper"
        >
          public receipt
        </Link>
      </div>

      {bothOnThisDevice && (
        <div className="mb-4 flex items-center justify-between rounded-xl border border-line bg-raised/50 px-4 py-2.5">
          <span className="text-sm text-muted">
            Viewing as <span className="font-semibold text-paper">{you}</span>
          </span>
          <button
            onClick={() => switchTo(deal.your_role === "buyer" ? "seller" : "buyer")}
            className="text-sm text-cyan underline underline-offset-2"
          >
            Switch to {them}
          </button>
        </div>
      )}

      {counterpartLink && deal.state !== "pending_agreement" && !deal.state.startsWith("c") && (
        <div className="mb-4">
          <Notice tone="info" title={`${them} will need this once you separate`}>
            <p className="mb-2 text-xs">
              Everything from here — the handoff, confirming receipt, the 24-hour
              hold — happens when you are no longer standing together. This link
              is how {them} gets in on their own phone. No account.
            </p>
            <ShareLink url={counterpartLink} />
          </Notice>
        </div>
      )}

      {moved && (
        <div className="mb-4">
          <Notice tone="warn">{moved}</Notice>
        </div>
      )}
      {error && (
        <div className="mb-4">
          <Notice tone="bad">{error}</Notice>
        </div>
      )}

      {/* ---- the reading, awaiting both confirmations ---- */}
      {deal.state === "pending_agreement" && deal.terms && (
        <>
          <Card title="What the witness heard" className="mb-4">
            <p className="mb-4 text-sm text-muted">
              Read this carefully. Nothing binds either of you until you both
              confirm it — and if it is wrong, correcting it takes a second now
              and an argument later.
            </p>
            <Ladder terms={deal.terms} />
          </Card>

          {deal.ambiguities.length > 0 && (
            <div className="mb-4">
              <Notice tone="warn" title="The witness was not sure about this">
                <ul className="list-disc space-y-1 pl-4">
                  {deal.ambiguities.map((a, i) => (
                    <li key={i}>{a}</li>
                  ))}
                </ul>
              </Notice>
            </div>
          )}

          <Card title="The agreement" className="mb-4">
            <TermsSheet terms={deal.terms} />
            <div className="mt-4 space-y-2 text-sm">
              <p className={deal.you_confirmed ? "text-good" : "text-muted"}>
                {deal.you_confirmed ? "✓" : "•"} {you}{" "}
                {deal.you_confirmed ? "confirmed" : "has not confirmed"}
              </p>
              <p className={deal.they_confirmed ? "text-good" : "text-muted"}>
                {deal.they_confirmed ? "✓" : "•"} {them}{" "}
                {deal.they_confirmed ? "confirmed" : "has not confirmed"}
              </p>
            </div>
          </Card>

          {!deal.you_confirmed && (
            <div className="space-y-3">
              <Button
                full
                disabled={busy !== null}
                onClick={() =>
                  void act("confirm", () =>
                    api.confirmTerms(
                      dealId,
                      token,
                      deal.terms_revision,
                      bothOnThisDevice,
                    ),
                  ).then(() => {
                    // Both sides live on this phone and the other party has not
                    // confirmed: the next step is a person, not a button.
                    if (bothOnThisDevice && !deal.they_confirmed) {
                      setHandover(deal.your_role === "buyer" ? "seller" : "buyer");
                    }
                  })
                }
              >
                {busy === "confirm"
                  ? "Recording your confirmation…"
                  : `Confirm — ${formatMoney(deal.terms.price)} for ${deal.terms.item}`}
              </Button>
              <PriceCorrection deal={deal} token={token} onDone={load} busy={busy} act={act} />
              <button
                disabled={busy !== null}
                onClick={() =>
                  void act("swap", () =>
                    api.correctTerms(dealId, token, {
                      ...deal.terms!,
                      buyer_name: deal.terms!.seller_name,
                      seller_name: deal.terms!.buyer_name,
                      // Every rung moves with the roles, or the ladder would
                      // still say the buyer named the asking price.
                      ladder: deal.terms!.ladder.map((o) => ({
                        ...o,
                        by: o.by === "buyer" ? ("seller" as const) : ("buyer" as const),
                      })),
                    }),
                  )
                }
                className="w-full text-center text-sm text-muted underline underline-offset-2 hover:text-paper"
              >
                {busy === "swap"
                  ? "Swapping…"
                  : `No — ${deal.seller_name} is buying, ${deal.buyer_name} is selling`}
              </button>
            </div>
          )}

          {deal.you_confirmed && !deal.they_confirmed && (
            <Notice tone="info">
              Waiting for {them}. The moment they confirm, these terms are frozen
              and hashed.
            </Notice>
          )}
        </>
      )}

      {/* ---- frozen ---- */}
      {deal.state !== "pending_agreement" && deal.state !== "draft" && deal.terms && (
        <Card title="The frozen agreement" className="mb-4">
          <TermsSheet terms={deal.terms} hash={deal.terms_hash} />
        </Card>
      )}

      {/* ---- funding ---- */}
      {deal.state === "agreed" &&
        (isBuyer ? (
          <div className="space-y-3">
            <Notice tone="info" title="No real money moves">
              Escrow here is a mock ledger. It runs the full hold-and-release
              cycle so the flow is real, but nothing leaves your account.
            </Notice>
            <Button
              full
              disabled={busy !== null}
              onClick={() => act("fund", () => api.fund(dealId, token))}
            >
              {busy === "fund"
                ? "Placing funds in escrow…"
                : `Put ${formatMoney(deal.terms?.price)} in escrow`}
            </Button>
          </div>
        ) : (
          <Notice tone="info">
            Waiting for {them} to fund escrow. Do not hand anything over until
            this page says the money is held.
          </Notice>
        ))}

      {/* ---- handoff ---- */}
      {deal.state === "funded" &&
        (isBuyer ? (
          <Notice tone="good" title="Your funds are held">
            {them} can see that the money is in escrow. Waiting for them to hand
            the item over.
          </Notice>
        ) : (
          <div className="space-y-3">
            <Notice tone="good" title="The money is in escrow">
              {them} has funded {formatMoney(deal.terms?.price)}. Hand the item
              over, and photograph it if you can — a photo is optional, and it is
              the thing a later dispute will turn on.
            </Notice>
            <input
              ref={fileInput}
              type="file"
              accept="image/*"
              capture="environment"
              className="hidden"
              onChange={async (e) => {
                const file = e.target.files?.[0];
                e.target.value = "";
                if (!file) return;
                const image = await fileToBase64(file);
                await act("handoff", () => api.submitHandoff(dealId, token, [image]));
              }}
            />
            <Button full disabled={busy !== null} onClick={() => fileInput.current?.click()}>
              {busy === "handoff" ? "Recording the handoff…" : "Photograph the handoff"}
            </Button>
            <Button
              variant="ghost"
              full
              disabled={busy !== null}
              onClick={() =>
                act("handoff", () => api.submitHandoff(dealId, token, [], "handed over in person"))
              }
            >
              Handed it over without a photo
            </Button>
          </div>
        ))}

      {/* ---- receipt ---- */}
      {deal.state === "handoff_proved" &&
        (isBuyer ? (
          <div className="space-y-3">
            <Notice tone="info" title={`${them} says they handed it over`}>
              Check you actually have it, and that it is what you agreed to. If it
              is not, dispute instead of confirming — once you confirm, a 24-hour
              clock starts.
            </Notice>
            <Button
              full
              disabled={busy !== null}
              onClick={() => act("receipt", () => api.confirmReceipt(dealId, token))}
            >
              {busy === "receipt" ? "Recording…" : "I have it — start the release clock"}
            </Button>
            <DisputeButton dealId={dealId} token={token} busy={busy} act={act} />
          </div>
        ) : (
          <Notice tone="info">
            Waiting for {them} to confirm they received it.
          </Notice>
        ))}

      {/* ---- holding ---- */}
      {deal.state === "holding" && (
        <div className="space-y-4">
          <Card>
            <Countdown
              label={`Funds release to ${deal.seller_name} in`}
              value={countdown?.label ?? "—"}
              note="Counted against the server's clock, not this device's. Either of you can dispute until the moment it releases."
            />
            {deal.receipt_auto_confirmed && (
              <p className="mt-4 border-t border-line pt-3 text-center text-xs text-warn">
                Receipt was never confirmed by the buyer — the window elapsed. This
                is recorded on the receipt permanently.
              </p>
            )}
          </Card>

          {isBuyer && (
            <Button
              variant="ghost"
              full
              disabled={busy !== null}
              onClick={() => act("release", () => api.releaseNow(dealId, token))}
            >
              {busy === "release" ? "Releasing…" : "Release now — do not wait"}
            </Button>
          )}
          <DisputeButton dealId={dealId} token={token} busy={busy} act={act} />
        </div>
      )}

      {deal.state === "disputed" && (
        <Notice tone="bad" title="Release is frozen">
          A dispute is open, so no money moves until it is resolved. That freeze is
          the reason nobody can pressure anyone with a countdown running.
        </Notice>
      )}

      {(deal.state === "completed" || deal.state === "resolved") && (
        <Notice tone="good" title="Done">
          <Link to={`/v/${deal.deal_id}`} className="underline underline-offset-2">
            Open the receipt
          </Link>{" "}
          — anyone can verify it without an account.
        </Notice>
      )}

      {/* ---- history ---- */}
      <Card title="What has happened" className="mt-6">
        <ol className="space-y-2">
          {deal.chain.map((link) => (
            <li key={link.seq} className="flex items-baseline justify-between gap-3 text-sm">
              <span className="text-paper">{link.action.replace(/_/g, " ")}</span>
              <Hash value={link.chain_hash} />
            </li>
          ))}
        </ol>
        <div className="mt-4 border-t border-line pt-3">
          <Field label="Evidence">
            {deal.evidence_tier === "attested"
              ? "attested — signed claims, not observed transfers"
              : "observed"}
          </Field>
        </div>
      </Card>
    </Page>
  );
}

function PriceCorrection({
  deal,
  token,
  busy,
  act,
}: {
  deal: DealView;
  token: string;
  onDone: () => Promise<void>;
  busy: string | null;
  act: (name: string, fn: () => Promise<unknown>) => Promise<void>;
}) {
  const [open, setOpen] = useState(false);
  const [value, setValue] = useState(
    deal.terms ? (deal.terms.price.minor_units / 100).toFixed(2) : "",
  );

  if (!deal.terms) return null;

  if (!open) {
    return (
      <button
        onClick={() => setOpen(true)}
        className="w-full text-center text-sm text-muted underline underline-offset-2 hover:text-paper"
      >
        That is not what we agreed
      </button>
    );
  }

  return (
    <Card>
      <p className="mb-3 text-sm text-muted">
        Correcting the price withdraws both confirmations — you will each confirm
        the new figure.
      </p>
      <div className="flex gap-2">
        <input
          value={value}
          inputMode="decimal"
          onChange={(e) => setValue(e.target.value)}
          className="min-w-0 flex-1 rounded-lg border border-line bg-raised px-3 py-2 font-mono text-paper outline-none focus:border-seal"
        />
        <Button
          disabled={busy !== null}
          onClick={() => {
            const minor = Math.round(Number(value) * 100);
            if (!Number.isFinite(minor) || minor <= 0) return;
            void act("correct", () =>
              api.correctTerms(deal.deal_id, token, {
                ...deal.terms!,
                price: { ...deal.terms!.price, minor_units: minor },
              }),
            ).then(() => setOpen(false));
          }}
        >
          Correct
        </Button>
      </div>
    </Card>
  );
}

function DisputeButton({
  dealId,
  token,
  busy,
  act,
}: {
  dealId: string;
  token: string;
  busy: string | null;
  act: (name: string, fn: () => Promise<unknown>) => Promise<void>;
}) {
  const [reason, setReason] = useState("");
  const [open, setOpen] = useState(false);

  if (!open) {
    return (
      <button
        onClick={() => setOpen(true)}
        className="w-full text-center text-sm text-bad underline underline-offset-2"
      >
        Something is wrong — raise a dispute
      </button>
    );
  }

  return (
    <Card>
      <p className="mb-3 text-sm text-muted">
        Opening a dispute freezes the release. Say what is wrong in your own
        words; both of you will see exactly this text.
      </p>
      <textarea
        value={reason}
        onChange={(e) => setReason(e.target.value)}
        rows={3}
        className="w-full rounded-lg border border-line bg-raised px-3 py-2 text-sm text-paper outline-none focus:border-seal"
        placeholder="It is not the model we agreed on…"
      />
      <div className="mt-3">
        <Button
          variant="danger"
          full
          disabled={busy !== null || reason.trim().length < 8}
          onClick={() => void act("dispute", () => api.openDispute(dealId, token, reason.trim()))}
        >
          Freeze the release and open a dispute
        </Button>
      </div>
    </Card>
  );
}
