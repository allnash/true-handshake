/**
 * Typed client for the True Handshake API.
 *
 * Two conventions worth knowing:
 *
 * - Every response carries `server_time`. Countdowns anchor to it (see
 *   `clock.ts`) so a device with a wrong clock still shows the right time
 *   remaining, and the client never decides on its own that a window closed.
 * - A 409 is a designed state, not a failure. `ApiError` keeps the problem
 *   details so the UI can refetch and show what changed instead of blaming the
 *   user.
 */

export type Party = "buyer" | "seller";

export type DealState =
  | "draft"
  | "pending_agreement"
  | "agreed"
  | "funded"
  | "handoff_proved"
  | "holding"
  | "completed"
  | "refunded"
  | "cancelled"
  | "expired"
  | "disputed"
  | "resolved";

export interface Money {
  currency: string;
  minor_units: number;
}

export type OfferKind = "context" | "ask" | "offer" | "counter" | "accept";

export interface Offer {
  seq: number;
  by: Party;
  kind: OfferKind;
  amount?: Money | null;
  quote: string;
}

export interface Terms {
  item: string;
  item_detail?: string | null;
  condition?: string | null;
  price: Money;
  buyer_name: string;
  seller_name: string;
  settlement: { kind: string; app?: string; description?: string };
  handoff: "in_person" | "shipped" | "digital";
  ladder: Offer[];
  notes?: string | null;
}

export interface Utterance {
  seq: number;
  /** Absent when unattributed — the witness works out who said what. */
  speaker?: string | null;
  text: string;
  at: string;
  confidence_pct?: number | null;
  corrected?: boolean;
}

export interface TimelineEntry {
  at: string;
  kind: string;
  payload: unknown;
}

export interface ChainLink {
  seq: number;
  action: string;
  actor: { type: string; party?: Party };
  at: string;
  chain_hash: string;
}

export interface DealView {
  deal_id: string;
  state: DealState;
  version: number;
  terms_revision: number;
  your_role: Party;
  buyer_name: string;
  seller_name: string;
  terms: Terms | null;
  terms_hash: string | null;
  summary: string | null;
  you_confirmed: boolean;
  they_confirmed: boolean;
  ambiguities: string[];
  witness_confidence: "low" | "medium" | "high" | null;
  release_due_at: string | null;
  receipt_auto_confirmed: boolean;
  evidence_tier: string;
  timeline: TimelineEntry[];
  chain: ChainLink[];
  server_time: string;
}

export interface StartedView {
  session_id: string;
  deal_id: string;
  buyer_token: string;
  seller_token: string;
  buyer_link: string;
  seller_link: string;
}

export interface TranscriptView {
  session_id: string;
  deal_id: string;
  closed: boolean;
  utterances: Utterance[];
  server_time: string;
}

export interface SpeakerBinding {
  label: string;
  name: string;
  quote: string;
}

export interface SpeakerIdentification {
  bindings: SpeakerBinding[];
  unbound: string[];
  confidence: "low" | "medium" | "high";
  note?: string | null;
}

export interface AudioEvidence {
  sha256: string;
  size_bytes: number;
  media_type: string;
  duration_ms: number | null;
}

export interface CommandView {
  deal_id: string;
  state: DealState;
  version: number;
  attestation_id: string;
  chain_hash: string;
  server_time: string;
}

export interface Attestation {
  seq: number;
  action: string;
  actor: unknown;
  at: string;
  payload: unknown;
  payload_hash: string;
  prev_chain_hash: string;
  chain_hash: string;
  key_id: string;
  signature?: string | null;
}

export interface Receipt {
  receipt_id: string;
  deal_id: string;
  state: string;
  terms: Terms | null;
  terms_hash: string | null;
  amount_band: string | null;
  evidence_tier: string;
  receipt_auto_confirmed: boolean;
  attestations: Attestation[];
  key_id: string;
  public_key: string;
  verification: string;
}

export interface KeyDocument {
  keys: { key_id: string; public_key_b64: string }[];
  algorithm: string;
  canonicalization: string;
  signing_domain: string;
  genesis_domain: string;
  procedure: string[];
}

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
    readonly currentVersion?: number,
  ) {
    super(message);
    this.name = "ApiError";
  }

  /** The deal moved underneath us. The UI refetches rather than erroring. */
  get isConflict() {
    return this.status === 409;
  }
}

async function request<T>(path: string, init: RequestInit = {}, token?: string): Promise<T> {
  const headers = new Headers(init.headers);
  if (init.body) headers.set("content-type", "application/json");
  if (token) headers.set("authorization", `Bearer ${token}`);

  const res = await fetch(path, { ...init, headers });
  const text = await res.text();
  const body = text ? JSON.parse(text) : null;

  if (!res.ok) {
    throw new ApiError(
      res.status,
      body?.title ?? "error",
      body?.detail ?? `request failed (${res.status})`,
      body?.current_version,
    );
  }
  return body as T;
}

export const api = {
  startSession: (buyer_name: string, seller_name: string) =>
    request<StartedView>("/v1/sessions", {
      method: "POST",
      body: JSON.stringify({ buyer_name, seller_name }),
    }),

  getTranscript: (sessionId: string) => request<TranscriptView>(`/v1/sessions/${sessionId}`),

  appendUtterances: (sessionId: string, utterances: Utterance[]) =>
    request<TranscriptView>(`/v1/sessions/${sessionId}/utterances`, {
      method: "POST",
      body: JSON.stringify({ utterances }),
    }),

  attachAudio: (
    sessionId: string,
    media_type: string,
    data_b64: string,
    duration_ms: number,
  ) =>
    request<AudioEvidence>(`/v1/sessions/${sessionId}/audio`, {
      method: "POST",
      body: JSON.stringify({ media_type, data_b64, duration_ms }),
    }),

  identifySpeakers: (sessionId: string) =>
    request<SpeakerIdentification>(`/v1/sessions/${sessionId}/identify`, { method: "POST" }),

  confirmSpeakers: (sessionId: string, bindings: SpeakerBinding[]) =>
    request<SpeakerIdentification>(`/v1/sessions/${sessionId}/speakers`, {
      method: "POST",
      body: JSON.stringify({ bindings }),
    }),

  propose: (sessionId: string) =>
    request<CommandView>(`/v1/sessions/${sessionId}/propose`, { method: "POST" }),

  getDeal: (dealId: string, token: string) =>
    request<DealView>(`/v1/deals/${dealId}`, {}, token),

  correctTerms: (dealId: string, token: string, terms: Terms) =>
    request<CommandView>(
      `/v1/deals/${dealId}/terms`,
      { method: "POST", body: JSON.stringify({ terms }) },
      token,
    ),

  confirmTerms: (dealId: string, token: string, revision: number, sameDevice: boolean) =>
    request<CommandView>(
      `/v1/deals/${dealId}/confirm`,
      { method: "POST", body: JSON.stringify({ revision, same_device: sameDevice }) },
      token,
    ),

  fund: (dealId: string, token: string) =>
    request<CommandView>(`/v1/deals/${dealId}/fund`, { method: "POST" }, token),

  submitHandoff: (
    dealId: string,
    token: string,
    images: { media_type: string; data_b64: string }[],
    note?: string,
  ) =>
    request<CommandView>(
      `/v1/deals/${dealId}/handoff`,
      { method: "POST", body: JSON.stringify({ images, note }) },
      token,
    ),

  confirmReceipt: (dealId: string, token: string) =>
    request<CommandView>(`/v1/deals/${dealId}/receipt`, { method: "POST" }, token),

  releaseNow: (dealId: string, token: string) =>
    request<CommandView>(`/v1/deals/${dealId}/release`, { method: "POST" }, token),

  openDispute: (dealId: string, token: string, reason: string) =>
    request<CommandView>(
      `/v1/deals/${dealId}/dispute`,
      { method: "POST", body: JSON.stringify({ reason }) },
      token,
    ),

  cancel: (dealId: string, token: string, reason: string) =>
    request<CommandView>(
      `/v1/deals/${dealId}/cancel`,
      { method: "POST", body: JSON.stringify({ reason }) },
      token,
    ),

  receipt: (receiptId: string) => request<Receipt>(`/v/${receiptId}.json`),

  keys: () => request<KeyDocument>("/.well-known/true-handshake-keys.json"),
};

export function formatMoney(m: Money | null | undefined): string {
  if (!m) return "—";
  const sign = { USD: "$", EUR: "€", GBP: "£" }[m.currency] ?? `${m.currency} `;
  return `${sign}${(m.minor_units / 100).toFixed(2)}`;
}

export const STATE_LABEL: Record<DealState, string> = {
  draft: "Listening",
  pending_agreement: "Awaiting confirmation",
  agreed: "Agreed — awaiting funds",
  funded: "In escrow — awaiting handoff",
  handoff_proved: "Handed over — awaiting receipt",
  holding: "Receipt confirmed — funds releasing",
  completed: "Complete",
  refunded: "Refunded",
  cancelled: "Cancelled",
  expired: "Expired",
  disputed: "Disputed — release frozen",
  resolved: "Resolved",
};
