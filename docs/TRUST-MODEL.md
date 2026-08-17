# TruReview — Trust Model

> **Status note.** This document describes the *reputation layer* — the part that
> turns finished handshakes into a portable score. It is design, not code: the
> built system today is True Handshake itself (AI-witnessed capture, frozen
> agreements, escrow, verifiable receipts). See [`../README.md`](../README.md)
> for what actually runs. Where this document says "TruReview", read "the
> reputation layer of True Handshake".


How a pile of receipts becomes a number, and why that number is hard to fake.

Companion to [`../README.md`](../README.md) and
[`ARCHITECTURE.md`](ARCHITECTURE.md).

---

## Contents

- [Threat model](#threat-model)
- [Verification tiers](#verification-tiers)
- [Review weighting](#review-weighting)
- [The reputation score](#the-reputation-score)
- [Publishing the score](#publishing-the-score)
- [Collusion detection](#collusion-detection)
- [Enforcement and appeals](#enforcement-and-appeals)
- [Cold start](#cold-start)
- [Governance commitments](#governance-commitments)

---

## Threat model

Ordered by expected frequency times damage.

| # | Attack | Mechanism | Primary defense |
| --- | --- | --- | --- |
| 1 | **Fabricated reviews** | Post reviews for deals that never happened | Reviews are structurally impossible without a two-party deal with frozen terms |
| 2 | **Reciprocal ring** | Two or more accounts trade fake deals and 5★ reviews | Counterparty-diversity decay + collusion graph analysis + verification tiers |
| 3 | **Sybil farm** | Many cheap accounts inflate one target | Bayesian shrinkage, reviewer-credibility weighting, tier gating, superlinear cost |
| 4 | **Retaliation** | Punish a reviewer with a revenge review | Double-blind simultaneous reveal |
| 5 | **Extortion** | "Refund me or I trash you" | Double-blind + dispute freezes reveal |
| 6 | **Bad-faith dispute** | Weaponize disputes to smear a counterparty | Asymmetric outcomes; a rejected dispute counts against the raiser's credibility |
| 7 | **Reputation laundering** | Buy or transfer an aged account | Account-bound scores, ID binding at high tiers, ownership-change detection resets |
| 8 | **Value gaming** | Stack fake high-value deals to dominate a profile | Hard-capped value weighting, plus value anomaly signals |
| 9 | **Competitor smear** | A rival poses as a customer to leave a bad review | Requires a real accepted deal with a real counterparty; dispute path; identity tier visible |
| 10 | **Operator tampering** | TruReview alters or suppresses history | Hash chain + signed daily Merkle anchors + independent verification + logged break-glass |

Attack 10 matters more than it looks. A reputation platform that *could*
secretly sell removals will eventually be accused of it. The architecture makes
the accusation checkable rather than a matter of trust in a company.

---

## Verification tiers

| Tier | Name | Requirement | Weight multiplier `τ` |
| --- | --- | --- | --- |
| 0 | Unverified | Email only | 0.15 |
| 1 | Contactable | Email + phone (SMS-verified, VoIP-screened) | 0.50 |
| 2 | Identified | Government ID + liveness via an IdV provider | 1.00 |
| 3 | Anchored | Tier 2 + a verified payment handle or business registration | 1.15 |

- The tier of **both** parties is displayed on every review, permanently.
- A deal between two Tier-0 accounts is worth `0.15 × 0.15 ≈ 0.02` — visible, but
  nearly weightless. Fake-review farms can run all day at that price and buy
  nothing.
- Tier 2+ binds to a real identity document. One ID maps to one high-tier
  account; re-verification with an ID already in use flags both.
- Tiers can be revoked. Revocation retroactively reweights every review that
  account authored.

---

## Review weighting

Every review contributes a weight `w ∈ [0, ~1.5]`, the product of six factors.
Weights are computed in batch and stored in `reputation_inputs` with a full
breakdown, so any user can be shown exactly why their review counted for what it
did.

```
w = τ_author · τ_subject · δ_diversity · κ_credibility · ν_value · ε_evidence · φ_flag
```

### `τ` — verification tiers

The multipliers above, for author and subject.

### `δ` — counterparty diversity

The *n*-th completed deal between the same pair carries:

```
δ(n) = 1 / (1 + ln n)
```

| n | δ |
| --- | --- |
| 1 | 1.00 |
| 2 | 0.59 |
| 5 | 0.38 |
| 10 | 0.30 |
| 50 | 0.20 |

Genuine repeat business still counts — it *should*, it's a good signal — but
sums to a bounded contribution. A pair cannot manufacture unbounded reputation
between themselves at any volume.

### `κ` — reviewer credibility

A scalar in `[0.1, 1.2]` from the author's own history: account age, number of
distinct verified counterparties, their own reputation confidence, dispute
record as a raiser (repeatedly-rejected disputes lower it), and any active abuse
flags.

v1 is this scalar. v2 upgrades to iterative trust propagation over the deal
graph — an EigenTrust/PageRank-style fixed point where credibility flows from
well-connected honest accounts. That is deferred deliberately: the graph
algorithm is only as good as the data it runs on, and the data doesn't exist
until the platform does.

### `ν` — deal value

```
ν = 1 + 0.25 · min( ln(1 + amount / median_deal_value), 2.0 )
```

Range `[1.0, 1.5]`. A $5,000 deal outweighs a $20 deal by 50% at most, never by
250×. Value tells you something; it must not tell you everything, or the whole
system becomes gameable by anyone willing to declare large fake numbers.
Deals with no declared amount take `ν = 1.0`.

### `ε` — evidence tier

| Tier | `ε` | When |
| --- | --- | --- |
| `Attested` | 1.00 | v1 default — both parties signed claims |
| `Observed` | 1.30 | v1.0 escrow — a processor observed the transfer |

Also downweighted here: `Abandoned` deals take `ε = 0.4` and are labeled
*unconfirmed* on the review itself.

### `φ` — abuse flag

`1.0` normally, `0.0` for edges inside a confirmed collusion cluster. Applied
silently (see [Collusion detection](#collusion-detection)).

---

## The reputation score

### Time decay

```
λ(t) = 0.5 ^ (age_days / 365)
```

A 12-month half-life. Two-year-old behavior contributes a quarter as much as
last month's. Reputation describes the present.

### Weighted Bayesian mean

```
        Σᵢ wᵢ λᵢ rᵢ  +  m · C
score = ─────────────────────
          Σᵢ wᵢ λᵢ  +  m
```

- `rᵢ` — the review's overall rating, 1–5
- `wᵢ` — the composite weight above
- `λᵢ` — time decay
- `C` — the global prior, the platform-wide weighted mean (recomputed daily)
- `m` — prior strength, currently **5.0 effective reviews**

Consequences that matter:

- One 5★ review from a Tier-0 stranger moves a new account's score by almost
  nothing — `w ≈ 0.02` against a prior of `m = 5.0`.
- Forty full-weight reviews at 4.6★ produce a score near 4.6 with high
  confidence.
- Buying reputation requires buying *weighted, diverse, ID-verified, aged*
  reviews. That's not a market that can be cheaply supplied.

### Subscores

`communication`, `timeliness`, and `terms_accuracy` use the same formula over
their own dimensions with their own priors. `terms_accuracy` is the one most
worth watching: it measures the gap between what someone agreed to and what they
delivered, which is exactly what a stranger wants to know.

### Dispute adjustment

Applied after the mean, to the party the outcome went against:

| Outcome | Effect |
| --- | --- |
| `upheld` | −0.35 on the subject's score, decaying over 18 months; permanent badge on the deal |
| `rejected` | No effect on the subject; −0.05 to the raiser's `κ` credibility |
| `mutual_fault` | −0.15 to both, decaying over 12 months |
| `withdrawn` | No score effect; recorded on the receipt |
| `inconclusive` | No score effect; recorded, and the deal's reviews are labeled |

Raising a dispute in good faith is never itself penalized. Only a pattern of
disputes that mediators reject touches the raiser, and only through credibility,
never through their public score.

### Confidence

Every score ships with `confidence = Σ wᵢλᵢ / (Σ wᵢλᵢ + m)`, in `[0, 1)`.
Below 0.35, the profile shows **Building reputation** rather than a number.
Publishing "5.0★" from one deal is a lie of presentation, and TruReview doesn't
tell it.

---

## Publishing the score

```
┌─────────────────────────────────────────────┐
│  @alice-builds            ✔ Identified      │
│                                             │
│  Trusted · 4.6            38 verified deals │
│  ████████████████████░░   confidence 0.87   │
│                                             │
│  Communication   4.8                        │
│  Timeliness      4.4                        │
│  Terms accuracy  4.7                        │
│                                             │
│  1 dispute · resolved in their favor        │
│  Active since Mar 2024 · last deal 6d ago   │
└─────────────────────────────────────────────┘
```

| Band | Score | Minimum confidence |
| --- | --- | --- |
| Exceptional | ≥ 4.8 | 0.75 |
| Trusted | ≥ 4.3 | 0.60 |
| Reliable | ≥ 3.8 | 0.45 |
| Mixed | ≥ 3.0 | 0.35 |
| Poor | < 3.0 | 0.35 |
| Building reputation | any | < 0.35 |

Rules of presentation:

- **One decimal, never two.** The model does not support more precision than
  that, so displaying more would be false.
- **Count is always adjacent to the score.** "4.9" alone is not a fact.
- **Every score is clickable through to its evidence** — the deals, their dates,
  amount bands, and the weight each contributed.
- **Recency is shown.** A 4.9 from someone last active in 2023 is not the same
  claim as a 4.9 from someone who closed a deal last week.
- **Disputes are surfaced on the profile**, with their outcomes, not buried.

---

## Collusion detection

Runs as a batch job over the deal graph — accounts as nodes, completed deals as
weighted edges.

### Signals

| Signal | Description |
| --- | --- |
| **Reciprocity isolation** | A pair whose deals are almost entirely with each other |
| **Clique density** | A small group with far higher internal edge density than the network baseline |
| **Value implausibility** | Declared amounts inconsistent with category norms, or suspiciously uniform |
| **Velocity anomaly** | Deal creation → acceptance → completion far faster than the population, repeatedly |
| **Timing correlation** | Deals and reviews clustering in tight bursts across supposedly independent accounts |
| **Rating homogeneity** | Near-zero variance across a group's mutual reviews |
| **Device / network correlation** | Shared device fingerprints, IP subnets, or registration fingerprints |
| **Text similarity** | Embedding-space clustering of review bodies across a group |
| **Registration cohort** | Accounts created within a narrow window that transact only with each other |
| **Handle reuse** | Same payment handle or contact across nominally distinct accounts |

No single signal acts alone. A weighted composite crosses a threshold and opens
a cluster case.

### Response ladder

1. **Silent weight-zeroing.** Flagged edges get `φ = 0`. Scores drift down over
   the next recompute. Nothing in the UI announces it.
2. **Tier gating.** New deals from cluster members require higher verification.
3. **Human review** for high-severity or high-visibility clusters.
4. **Public labeling** only after human confirmation, with an appeal already
   available.
5. **Suspension** for confirmed, deliberate, repeated fraud.

Why silent: an attacker with immediate feedback binary-searches the detector.
An attacker who only sees a slow, unexplained score drift cannot tell which
signal caught them, or whether anything caught them at all. The cost is real —
false positives are invisible to the victim — which is why any *visible*
consequence requires human confirmation first, and why appeals are handled by a
person with the signals disclosed.

### False-positive management

Genuine tight-knit communities look like collusion rings. A regular
subcontractor pair, a small trade group, a village. Mitigations: diversity decay
already caps their contribution without any flag; thresholds are calibrated
against labeled honest clusters; human review precedes anything visible;
appeals restore weight retroactively and feed back as training data.

---

## Enforcement and appeals

- **Every enforcement action is a record** — actor, reason code, signals,
  timestamp — appended to the same audit substrate as everything else.
- **Every action is appealable** to a human who did not make the original
  decision.
- **Appeals disclose the reason category and the general signal class**, but not
  detector thresholds. Users learn enough to contest; attackers don't learn
  enough to tune.
- **Reversal is retroactive.** A restored account's weights and scores recompute
  as though the flag never existed.
- **Enforcement statistics are published quarterly**: actions by category,
  appeal volume, reversal rate. A platform that grades its own homework should
  at least publish the grades.

---

## Cold start

The chicken-and-egg problem — reputation is worthless with no users, and users
won't come without reputation. The design responses:

1. **The receipt is valuable at n = 2.** Two strangers on a marketplace get an
   attested, verifiable record of their agreement whether or not anyone else uses
   TruReview. That's a complete product for one deal.
2. **Verification substitutes for history.** A brand-new Tier-3 account displays
   *Identified · verified payment handle · new*, which is a materially better
   signal than an anonymous stranger, before a single review exists.
3. **Honest "Building reputation."** No fake-precision score on thin data. The
   band is what a new user shows, and it says exactly what it means.
4. **Import as context, never as score.** Off-platform history (eBay, Upwork,
   Etsy) may be displayed as a linked, clearly-labeled external claim. It never
   enters the TruReview score. The whole premise is that the number is made only
   of verified deals; the moment imported data enters it, the premise is gone.

---

## Governance commitments

These are product constraints, deliberately recorded here so that changing one
requires changing this document:

1. **No pay-to-remove, pay-to-suppress, pay-to-delay, or pay-to-reorder.** No
   surface exists that offers it, and none will be built.
2. **Removals are visible.** A removed review leaves a tombstone naming the
   reason category. Silent removal is not implemented.
3. **Scoring is documented publicly**, including weights and constants. Security
   through obscurity of the *scoring* model doesn't work; obscurity of the
   *detector thresholds* does, and that is the only thing kept private.
4. **Scoring model changes are versioned, announced, and shown as a diff** on
   affected profiles. Reputation must not silently change under someone's feet.
5. **Users can export everything**, including every weight and its breakdown.
6. **TruReview is never a party to a deal** and never mediates a dispute in which
   it has an interest.
7. **Break-glass access to sealed reviews is logged, two-person-authorized, and
   disclosed to the affected parties.**

---

## Parameter reference

Constants are configuration, not literals, and are versioned with the scoring
model.

| Parameter | Symbol | Value |
| --- | --- | --- |
| Prior strength | `m` | 5.0 |
| Global prior | `C` | platform weighted mean, daily |
| Decay half-life | | 365 days |
| Diversity decay | `δ(n)` | `1 / (1 + ln n)` |
| Value weight cap | `ν_max` | 1.5 |
| Credibility range | `κ` | 0.1 – 1.2 |
| Tier multipliers | `τ` | 0.15 / 0.50 / 1.00 / 1.15 |
| Observed-evidence bonus | `ε` | 1.30 |
| Abandoned-deal weight | `ε` | 0.40 |
| Upheld-dispute penalty | | −0.35 over 18 months |
| Display threshold | confidence | 0.35 |
