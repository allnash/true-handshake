import { useEffect, useState } from "react";
import { useParams } from "react-router-dom";

import { api, type KeyDocument, type Receipt as ReceiptDoc } from "../lib/api";
import { verifyReceipt, verifyTermsHash, type VerificationResult } from "../lib/verify";
import {
  Card,
  Field,
  Hash,
  Ladder,
  Masthead,
  Notice,
  Page,
  Seal,
  Spinner,
  TermsSheet,
} from "../components/ui";

/**
 * The public receipt. No account, no token, nothing privileged.
 *
 * Verification runs in this browser against the published key — see
 * `lib/verify.ts`. If it only ran on our servers it would not be verification,
 * it would be us saying "trust me" with extra steps.
 */
export default function Receipt() {
  const { receiptId = "" } = useParams();
  const [doc, setDoc] = useState<ReceiptDoc | null>(null);
  const [keys, setKeys] = useState<KeyDocument | null>(null);
  const [result, setResult] = useState<VerificationResult | null>(null);
  const [termsOk, setTermsOk] = useState<boolean | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    (async () => {
      try {
        const [r, k] = await Promise.all([api.receipt(receiptId), api.keys()]);
        setDoc(r);
        setKeys(k);
        setResult(await verifyReceipt(r, k));
        setTermsOk(await verifyTermsHash(r));
      } catch {
        setError("No receipt with that id.");
      }
    })();
  }, [receiptId]);

  if (error) {
    return (
      <Page>
        <Masthead />
        <Notice tone="bad">{error}</Notice>
      </Page>
    );
  }

  if (!doc || !keys || !result) {
    return (
      <Page>
        <Masthead />
        <Spinner label="Fetching and verifying…" />
      </Page>
    );
  }

  const chainOk = result.links.every((l) => l.payloadOk && l.linkOk);
  const sigOk = result.links.every((l) => l.signatureOk !== false);

  // The proposal attestation commits to the recording the transcript came from.
  const audio = (
    doc.attestations.find((a) => a.action === "witness_proposed")?.payload as
      | { audio?: { sha256: string; duration_ms?: number | null; media_type: string } }
      | undefined
  )?.audio;

  // If the agreement was frozen from one device, the receipt says so. Both
  // parties did read it in front of each other, which is most of the value —
  // but it is not two independent confirmations, and should not read as though
  // it were.
  const sameDevice =
    (
      doc.attestations.find((a) => a.action === "terms_frozen")?.payload as
        | { same_device?: boolean }
        | undefined
    )?.same_device === true;

  return (
    <Page>
      <Masthead sub="Public receipt" />

      <div className="mb-6">
        <Seal
          ok={result.ok}
          label={
            result.ok
              ? "checked in your browser, against the published key"
              : "this document does not verify"
          }
        />
      </div>

      <div className="mb-5">
        {result.ok ? (
          <Notice tone="good" title="Verified in your browser">
            Every link in this record hashes to the next, and the platform's
            signature checks out against its published key. Nothing here was
            taken on our word.
          </Notice>
        ) : (
          <Notice tone="bad" title="This receipt does not verify">
            {result.error ??
              "At least one link does not follow from its predecessor. Treat this document as unreliable."}
          </Notice>
        )}
      </div>

      <Card title="What was agreed" className="mb-4">
        {doc.terms ? (
          <>
            <TermsSheet terms={doc.terms} hash={doc.terms_hash} />
            {termsOk !== null && (
              <p className={`mt-3 text-xs ${termsOk ? "text-good" : "text-bad"}`}>
                {termsOk
                  ? "✓ These terms hash to the value both parties signed — they have not been edited since."
                  : "! These terms do not match their recorded hash."}
              </p>
            )}
          </>
        ) : (
          <p className="text-sm text-muted">
            This deal never reached a frozen agreement, so there are no public
            terms to show.
          </p>
        )}
      </Card>

      {doc.terms && doc.terms.ladder.length > 0 && (
        <Card title="How they got there" className="mb-4">
          <Ladder terms={doc.terms} />
        </Card>
      )}

      <Card title="The record" className="mb-4">
        <ol className="space-y-3">
          {result.links.map((link) => (
            <li key={link.seq} className="border-b border-line pb-3 last:border-0 last:pb-0">
              <div className="flex items-baseline justify-between gap-3">
                <span className="text-sm text-paper">
                  {link.seq}. {link.action.replace(/_/g, " ")}
                </span>
                <span
                  className={`text-xs ${
                    link.payloadOk && link.linkOk && link.signatureOk !== false
                      ? "text-good"
                      : "text-bad"
                  }`}
                >
                  {link.payloadOk && link.linkOk ? "✓ intact" : "! broken"}
                  {link.signatureOk === true && " · signed"}
                  {link.signatureOk === false && " · bad signature"}
                </span>
              </div>
              <div className="mt-1 flex items-baseline justify-between gap-3">
                <time className="text-xs text-muted">
                  {new Date(link.at).toLocaleString()}
                </time>
                <Hash value={link.chainHash} />
              </div>
            </li>
          ))}
        </ol>
      </Card>

      <Card title="Verification" className="mb-4">
        <dl>
          <Field label="Hash chain">
            <span className={chainOk ? "text-good" : "text-bad"}>
              {chainOk ? "✓ intact" : "! broken"}
            </span>
          </Field>
          <Field label="Signature">
            {result.signatureSupport === "unsupported" ? (
              <span className="text-warn">
                not checked — this browser lacks Ed25519
              </span>
            ) : (
              <span className={sigOk ? "text-good" : "text-bad"}>
                {sigOk ? "✓ valid" : "! invalid"}
              </span>
            )}
          </Field>
          <Field label="Signing key">
            <Hash value={doc.key_id} />
          </Field>
          <Field label="Canonicalization">{keys.canonicalization}</Field>
          <Field label="Confirmations">
            {sameDevice ? (
              <span className="text-warn">both made on one device</span>
            ) : (
              <span className="text-good">made independently</span>
            )}
          </Field>
          <Field label="Recording">
            {sameDevice && (
          <p className="mb-2">
            Both parties confirmed from the same device, so this record shows two
            people who read the terms together — not two independent
            confirmations from two devices.
          </p>
        )}
        {audio ? (
              <Hash value={audio.sha256} />
            ) : (
              <span className="text-muted">none captured</span>
            )}
          </Field>
          <Field label="Amount">{doc.amount_band ?? "—"}</Field>
          <Field label="Evidence">
            {doc.evidence_tier === "attested" ? "attested" : "observed"}
          </Field>
        </dl>

        <details className="mt-4">
          <summary className="cursor-pointer text-sm text-muted">
            Verify this yourself
          </summary>
          <p className="mt-2 text-xs text-muted">
            The procedure below is the whole specification. Reimplement it in any
            language, fetch this receipt as JSON, and you should reproduce every
            hash on this page.
          </p>
          <pre className="mt-2 overflow-x-auto rounded-lg bg-ink p-3 font-mono text-[11px] leading-relaxed text-muted">
            {keys.procedure.join("\n")}
          </pre>
        </details>
      </Card>

      <Notice tone="info" title="What this proves, and what it does not">
        <p className="mb-2">
          It proves both parties agreed to exactly these terms, when each of them
          consented, and that nobody — including us — has altered the record
          since.
        </p>
        <p className="mb-2">
          It does not prove the item was as described, or that money moved: this
          deal settled on the {doc.evidence_tier} tier, meaning the platform
          recorded signed claims rather than observing a transfer.
        </p>
        {sameDevice && (
          <p className="mb-2">
            Both parties confirmed from the same device, so this record shows two
            people who read the terms together — not two independent
            confirmations from two devices.
          </p>
        )}
        {audio ? (
          <p>
            A recording was captured and its fingerprint is fixed in the record
            above, so the transcript can be checked against the audio it came
            from. The audio itself is held separately and can be destroyed on
            request — at which point this receipt stays valid and simply stops
            being checkable against it.
          </p>
        ) : (
          <p>
            No recording was captured for this deal, so the transcript rests on
            what the parties confirmed rather than on audio anyone can re-check.
          </p>
        )}
      </Notice>
    </Page>
  );
}
