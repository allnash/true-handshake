# True Handshake

**An AI witness for two-party deals, and a receipt anyone can verify.**

Two people negotiate out loud. True Handshake listens, reconstructs what they
agreed — every price named, in order, in their own words — and puts that reading
in front of both of them. When they both confirm it, the agreement is frozen,
hashed, and signed. Then it holds the buyer's money until the item actually
changes hands.

```
Nash:   Hey Stella — I like your fitbit, how much is it?
Stella: Well I got it for $80
Nash:   Well how much is it today?
Stella: Nash, if you want it, maybe 50
Nash:   I will offer you 30
Stella: That's too low. How about 40
Nash:   We have a deal
```

From that, the witness produces:

| # | Who | What | Words |
|---|---|---|---|
| 0 | Stella | mentioned $80 | *"well I got it for $80"* |
| 1 | Stella | asked $50 | *"if you want it maybe 50"* |
| 2 | Nash | offered $30 | *"I will offer you 30"* |
| 3 | Stella | countered **$40** | *"That's too low, how about 40"* |
| 4 | Nash | agreed | *"We have a deal"* |

That ladder is the point. Any escrow can record "both parties agreed to $40".
True Handshake records how they got there, attributed and quoted, so the receipt
is worth more than a checkbox.

---

## The rule everything hangs off

> **The witness proposes. The humans attest. The chain records.**

An AI reading of a conversation is **never binding on its own**. It is evidence
put in front of two people, who confirm or correct it. Only then is it frozen and
hashed. A mis-transcribed "forty" can never become a $40 obligation without both
parties looking at it first — and if either corrects anything, *both*
confirmations are withdrawn and they confirm again.

This matters because "the AI is the trust" and "you don't have to trust anyone"
pull in opposite directions. True Handshake resolves it by making the AI a
**witness that produces evidence**, never an authority that decides.

---

## The lifecycle

```
     ┌─ witness listens ─┐
Draft ──────────────────► PendingAgreement ──both confirm──► Agreed
                              │  ▲                             │
                     correct ─┘  │ (resets both)         buyer funds
                                 │                             ▼
                                 └─────────────────────────► Funded
                                                               │
                                                    seller proves handoff
                                                               ▼
                                                        HandoffProved
                                                               │
                                                     buyer confirms receipt
                                                               ▼
                                    ┌───────────────────► Holding ──24h──► Completed
                                    │                        │
                              (withdrawn)                  dispute
                                    │                        ▼
                                    └──────────────────── Disputed ──► Resolved
```

**The 24-hour hold is the safety valve.** Between "I have it" and the money
moving, either party can freeze the transfer by disputing. Nobody can pressure
anyone with a countdown running, because opening a dispute stops the clock.

Deals cannot hang. A silent seller means the buyer is refunded automatically. A
silent buyer means the release clock starts anyway — but the receipt is labelled
*receipt never confirmed*, permanently and on its face.

---

## What a receipt proves, and what it does not

Every deal produces a public receipt at `/v/{id}`, readable with no account.
Verification runs **in your browser**, against a key published at
`/.well-known/true-handshake-keys.json`:

```
payload_hash[n]    = hex(SHA-256(canonical_json(payload[n])))
prev_chain_hash[0] = hex(SHA-256("true-handshake/v1/genesis:" || deal_id))
prev_chain_hash[n] = chain_hash[n-1]
chain_hash[n]      = hex(SHA-256(prev_chain_hash[n] || payload_hash[n]))
signature[n]       = Ed25519(key, "true-handshake/v1/attestation:" || chain_hash[n])
```

**It proves** both parties agreed to exactly these terms, precisely when each of
them consented, that nobody — including us — has altered the record since, and
(when a recording was captured) that the transcript came from a specific audio
track whose fingerprint is fixed in the chain.

**It does not prove** the item matched its description, or that money moved. v1
settles on the `attested` tier: signed claims, not observed transfers. Swapping
the mock ledger for a real PSP promotes the tier to `observed` **with no change
to the domain model**; that seam is `SettlementProvider` in `th-app/src/ports.rs`.

Two design decisions worth knowing about:

- **Genesis is domain-separated by deal id.** Without it, two deals whose first
  payloads happened to be identical would produce identical chain hashes and
  therefore identical signatures — letting an attestation be transplanted between
  deals.
- **Hashes concatenate as lowercase ASCII hex, not raw bytes.** Slightly
  wasteful, deliberately so: an independent verifier reimplements string
  concatenation without a single byte-order question. `web/src/lib/verify.ts` is
  a second implementation written against the spec, not the Rust source. If the
  two ever disagree, the *format* is what's wrong.

---

## Getting started

```bash
# prerequisites: Rust 1.83+, Node 20+, Docker

cp .env.example .env
docker compose up -d                      # Postgres on :5433
cargo run -p th-api -- --generate-seed    # paste into TH_SIGNING_SEED

cargo test --workspace                    # 55 tests, no database needed
cargo run -p th-api                        # :8080, migrations run on boot

cd web && npm install && npm run dev       # :5173
```

Set `ANTHROPIC_API_KEY` to use the real witness (Claude Opus 5). Without it the
server falls back to an **offline witness** that scans transcripts for numbers
and cannot understand a conversation — enough to walk the UI, useless as a
witness, and it says so on every reading it produces.

Without `TH_SIGNING_SEED` the server mints an ephemeral key and warns that every
receipt it signs will stop verifying after a restart.

---

## How it fits together

Hexagonal, five crates. The dependency arrow never points outward from the domain.

```
crates/
├── th-domain/   pure: state machine, canonical JSON, hash chain, money
│                no async runtime, no database, no clock — time is a parameter
├── th-app/      use cases + ports (Clock, Witness, Vision, Settlement, Signer…)
├── th-infra/    Postgres, Claude, mock escrow ledger, Ed25519, offline witness
├── th-api/      Axum HTTP, RFC 9457 errors, public receipt endpoint
└── th-jobs/     durable timer worker (SKIP LOCKED, one process for now)

web/             React 19 + TypeScript + Vite + Tailwind 4
├── lib/verify.ts    independent receipt verification (WebCrypto)
├── lib/speech.ts    browser STT; voices separated by tap, named by the witness
├── lib/recorder.ts  MediaRecorder track, hashed into the chain as evidence
└── lib/clock.ts     countdowns anchored to server time, not the device's
```

Notable properties, each with a test that fails if it stops being true:

- **Time is injected.** A full lifecycle — propose, confirm, fund, hand off, hold
  24 hours, release — runs deterministically in microseconds.
- **State and history commit together.** No transition exists without its
  attestation, in the same transaction. `(deal_id, seq)` is unique, so two racing
  writers cannot fork a chain.
- **Authorization is a domain concern.** "Only the buyer may release funds" is a
  fact about deals, not about routes; the route layer is a second line, never the
  only one.
- **Firing a timer is not a privileged path.** A timer invokes the same use case a
  human would, with a `System` actor, and the domain decides if it is still legal.
  A worker resuming six hours late does the right thing or nothing.
- **Money order is direction-dependent.** Escrow *takes* custody before the commit
  (never record funds we failed to take) and *releases* after it (never move money
  we failed to record). The ledger is double-entry, so "it balances" is a query.
- **No floats anywhere.** Money is integer minor units; canonical JSON rejects
  non-integers outright. A float in the terms would make the encoding
  implementation-dependent, and the whole receipt rests on two implementations
  agreeing byte for byte.

---

## Why a web app and not a desktop app

The second party must not have to install anything. Nash opens True Handshake;
Stella receives a link and taps it. Any install step between "we have a deal" and
"confirm what we agreed" loses half the deals — and it is the *counterparty*,
the person with the least investment, who would bear it.

Everything the product needs is already in the browser: microphone and speech
recognition, `capture="environment"` for the handoff photo, and WebCrypto for
verifying receipts on the reader's own machine. Tauri earns its place later, for
long unattended recordings, offline/local transcription, or desktop tooling for
mediators — none of which is on the critical path for two people and a Fitbit.

---

## Known limits

Things that are honestly not solved yet, in rough priority order:

1. **Voices are separated by tapping, not by listening.** The Web Speech API
   returns text with no speaker information, so the capture screen asks which
   side is talking. Names are no longer typed, though: both parties say who they
   are, the witness binds each voice to a name from that self-identification, and
   a human confirms the mapping before any price is discussed. Dropping in a
   diarizer replaces one thing — "which side was tapped" becomes "which cluster
   the audio came from" — and the binding, confirmation, and everything
   downstream are unchanged.
2. **Browser STT sends audio to the browser vendor** and stops on silence. The
   recording itself is now kept and hashed into the chain, so the remaining step
   is transcribing that track server-side rather than trusting the browser's
   transcript.
3. **Identity is a bearer token per party**, not authentication. Anyone with the
   link is that party. `PartyBinding` is the single place that changes.
4. **The signing key lives in process.** Production wants a KMS or HSM with only
   the digest crossing the boundary.
5. **No Merkle anchoring yet.** The chain proves *we* did not quietly rewrite a
   receipt you already hold. It does not yet stop an operator who controls the
   signing key from rewriting history nobody kept a copy of. Cross-publishing
   daily roots to an external witness closes that, and is cheap.
6. **Mediation has one endpoint and no queue.** `TH_MEDIATOR_TOKEN` guards it.

## Related documents

[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) and
[`docs/TRUST-MODEL.md`](docs/TRUST-MODEL.md) describe the reputation layer —
double-blind reviews, weighting, collusion detection — which sits *downstream* of
what is built here and is not implemented yet. A handshake receipt is the input
that layer consumes.

## License

Apache 2.0 — see [`LICENSE`](LICENSE).
