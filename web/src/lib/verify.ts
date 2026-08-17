/**
 * Receipt verification, in the browser.
 *
 * This is a second, independent implementation of the published spec — written
 * against the documented procedure, not against the Rust source. That is the
 * whole point: verification only the server can perform is not verification. If
 * this file and the backend ever disagree, the receipt format is ambiguous and
 * the format is what's wrong.
 *
 * Everything here runs on WebCrypto against the key from
 * `/.well-known/true-handshake-keys.json`. No trust in this page's origin is
 * required beyond serving you the code.
 */

import type { Attestation, KeyDocument, Receipt } from "./api";

/** RFC 8785 (JCS), restricted to integers — same restriction as the backend. */
export function canonicalize(value: unknown): string {
  if (value === null) return "null";
  if (value === true) return "true";
  if (value === false) return "false";

  if (typeof value === "number") {
    if (!Number.isInteger(value)) {
      throw new Error(`canonical JSON cannot encode the non-integer ${value}`);
    }
    return String(value);
  }

  // JSON.stringify escapes exactly what JCS requires for strings: quote,
  // backslash, the short forms for \b \f \n \r \t, and \u00xx for the remaining
  // control characters. Non-ASCII stays literal, as it must.
  if (typeof value === "string") return JSON.stringify(value);

  if (Array.isArray(value)) {
    return `[${value.map(canonicalize).join(",")}]`;
  }

  if (typeof value === "object") {
    // JCS orders members by UTF-16 code units, which is precisely what the
    // default JS string sort does.
    const keys = Object.keys(value as Record<string, unknown>).sort();
    const parts = keys.map(
      (k) => `${JSON.stringify(k)}:${canonicalize((value as Record<string, unknown>)[k])}`,
    );
    return `{${parts.join(",")}}`;
  }

  throw new Error(`cannot canonicalize ${typeof value}`);
}

const enc = new TextEncoder();

export async function sha256Hex(input: string | Uint8Array): Promise<string> {
  const bytes = typeof input === "string" ? enc.encode(input) : input;
  const digest = await crypto.subtle.digest("SHA-256", bytes as BufferSource);
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

export function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export const GENESIS_DOMAIN = "true-handshake/v1/genesis:";
export const SIGNATURE_DOMAIN = "true-handshake/v1/attestation:";

export interface LinkResult {
  seq: number;
  action: string;
  at: string;
  chainHash: string;
  payloadOk: boolean;
  linkOk: boolean;
  signatureOk: boolean | null;
}

export interface VerificationResult {
  ok: boolean;
  links: LinkResult[];
  /** Set when the environment cannot check Ed25519 at all. */
  signatureSupport: "checked" | "unsupported";
  error?: string;
}

async function importEd25519(publicKeyB64: string): Promise<CryptoKey | null> {
  try {
    return await crypto.subtle.importKey(
      "raw",
      base64ToBytes(publicKeyB64) as BufferSource,
      { name: "Ed25519" },
      false,
      ["verify"],
    );
  } catch {
    // Older browsers without Ed25519 in WebCrypto. The hash chain still
    // verifies; we say plainly that the signature did not.
    return null;
  }
}

export async function verifyReceipt(
  receipt: Receipt,
  keys: KeyDocument,
): Promise<VerificationResult> {
  const links: LinkResult[] = [];

  try {
    const published = keys.keys.find((k) => k.key_id === receipt.key_id) ?? keys.keys[0];
    if (!published) {
      return { ok: false, links, signatureSupport: "unsupported", error: "no published key" };
    }
    const cryptoKey = await importEd25519(published.public_key_b64);
    const signatureSupport = cryptoKey ? "checked" : "unsupported";

    // The chain must start from this deal's domain-separated genesis, so an
    // attestation cannot be transplanted from another deal.
    let prev = await sha256Hex(GENESIS_DOMAIN + receipt.deal_id);
    let ok = true;

    for (const [i, a] of receipt.attestations.entries()) {
      const payloadHash = await sha256Hex(canonicalize((a as Attestation).payload));
      const payloadOk = payloadHash === a.payload_hash;

      const chainHash = await sha256Hex(prev + a.payload_hash);
      const linkOk = a.prev_chain_hash === prev && chainHash === a.chain_hash && a.seq === i;

      let signatureOk: boolean | null = null;
      if (cryptoKey && a.signature) {
        signatureOk = await crypto.subtle.verify(
          "Ed25519",
          cryptoKey,
          base64ToBytes(a.signature) as BufferSource,
          enc.encode(SIGNATURE_DOMAIN + a.chain_hash) as BufferSource,
        );
      }

      links.push({
        seq: a.seq,
        action: a.action,
        at: a.at,
        chainHash: a.chain_hash,
        payloadOk,
        linkOk,
        signatureOk,
      });

      if (!payloadOk || !linkOk || signatureOk === false) ok = false;
      prev = a.chain_hash;
    }

    return { ok, links, signatureSupport };
  } catch (e) {
    return {
      ok: false,
      links,
      signatureSupport: "unsupported",
      error: e instanceof Error ? e.message : String(e),
    };
  }
}

/**
 * Independently recompute the frozen terms hash. This is the check that matters
 * most to a human: it proves the terms displayed on this page are byte-for-byte
 * the ones both parties signed, not a later edit.
 */
export async function verifyTermsHash(receipt: Receipt): Promise<boolean | null> {
  if (!receipt.terms || !receipt.terms_hash) return null;
  return (await sha256Hex(canonicalize(receipt.terms))) === receipt.terms_hash;
}
