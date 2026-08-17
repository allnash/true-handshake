//! The deal lifecycle, as one total function over values.
//!
//! Nothing outside this module is permitted to construct a `DealState`. Every
//! transition — including the ones a timer causes — goes through `transition`,
//! which returns the next deal alongside the events, attestation, timers, and
//! settlement instruction it implies. The caller persists all of that in one
//! transaction or none of it.
//!
//! Authorization lives here rather than at the HTTP layer, because "only the
//! buyer may release funds" is a fact about deals, not about routes.

use serde::{Deserialize, Serialize};
use serde_json::json;
use time::{Duration, OffsetDateTime};

use crate::chain::{Actor, AttestationAction, AttestationDraft};
use crate::error::DomainError;
use crate::ids::DealId;
use crate::money::Money;
use crate::terms::{EvidenceTier, Party, Terms};
use crate::witness::{AudioEvidence, HandoffAssessment, WitnessExtraction};

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

/// How long an extracted agreement waits for both confirmations.
pub const AGREEMENT_WINDOW: Duration = Duration::hours(24);
/// How long a frozen agreement waits for the buyer to fund escrow.
pub const FUNDING_WINDOW: Duration = Duration::hours(48);
/// How long the seller has to evidence handoff once funds are held.
pub const HANDOFF_WINDOW: Duration = Duration::days(7);
/// How long the buyer has to confirm receipt after the seller proves handoff.
pub const RECEIPT_WINDOW: Duration = Duration::hours(72);
/// The cooling period between "I received it" and the money actually moving.
/// This is the window in which a dispute can still freeze the transfer.
pub const RELEASE_HOLD: Duration = Duration::hours(24);

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisputeOutcome {
    ReleaseToSeller,
    RefundToBuyer,
    Withdrawn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DealState {
    /// A witness session is capturing; no reading exists yet.
    Draft,
    /// The witness proposed a reading. Awaiting both parties' confirmation.
    PendingAgreement,
    /// Both confirmed. Terms are frozen and hashed. Awaiting funding.
    Agreed,
    /// Escrow holds the buyer's funds. Awaiting the seller's handoff proof.
    Funded,
    /// The seller evidenced handoff. Awaiting the buyer's receipt confirmation.
    HandoffProved,
    /// Receipt confirmed; the release clock is running.
    Holding,
    /// Funds released to the seller. Terminal.
    Completed,
    /// Funds returned to the buyer. Terminal.
    Refunded,
    /// Unwound before funding. Terminal.
    Cancelled,
    /// A window elapsed before the deal got off the ground. Terminal.
    Expired,
    /// Release is frozen pending mediation.
    Disputed,
    /// A dispute was closed. Terminal.
    Resolved { outcome: DisputeOutcome },
}

impl DealState {
    pub fn as_str(&self) -> &'static str {
        use DealState::*;
        match self {
            Draft => "draft",
            PendingAgreement => "pending_agreement",
            Agreed => "agreed",
            Funded => "funded",
            HandoffProved => "handoff_proved",
            Holding => "holding",
            Completed => "completed",
            Refunded => "refunded",
            Cancelled => "cancelled",
            Expired => "expired",
            Disputed => "disputed",
            Resolved { .. } => "resolved",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            DealState::Completed
                | DealState::Refunded
                | DealState::Cancelled
                | DealState::Expired
                | DealState::Resolved { .. }
        )
    }

    /// Whether money is currently held by the settlement provider.
    pub fn funds_held(&self) -> bool {
        matches!(
            self,
            DealState::Funded
                | DealState::HandoffProved
                | DealState::Holding
                | DealState::Disputed
        )
    }
}

// ---------------------------------------------------------------------------
// Aggregate
// ---------------------------------------------------------------------------

/// Everything the transition function needs to decide. A plain value: no
/// database handle, no clock, no I/O.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Deal {
    pub id: DealId,
    pub state: DealState,
    /// Optimistic concurrency. Bumped on every accepted transition.
    pub version: u32,
    /// Bumped whenever a party corrects the reading, which resets confirmations.
    pub terms_revision: u32,
    /// Which revision each party has confirmed, if any.
    pub buyer_confirmed: Option<u32>,
    pub seller_confirmed: Option<u32>,
    /// Candidate terms while `PendingAgreement`; frozen terms from `Agreed` on.
    pub terms: Option<Terms>,
    /// Set at freeze; the thing both parties are bound to.
    pub terms_hash: Option<String>,
    pub evidence_tier: EvidenceTier,
    /// Set when the buyer's receipt confirmation was supplied by a timer rather
    /// than by the buyer. Surfaced on the receipt, never hidden.
    pub receipt_auto_confirmed: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub frozen_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub release_due_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub terminal_at: Option<OffsetDateTime>,
}

impl Deal {
    pub fn new(id: DealId, now: OffsetDateTime) -> Self {
        Self {
            id,
            state: DealState::Draft,
            version: 0,
            terms_revision: 0,
            buyer_confirmed: None,
            seller_confirmed: None,
            terms: None,
            terms_hash: None,
            evidence_tier: EvidenceTier::Attested,
            receipt_auto_confirmed: false,
            created_at: now,
            frozen_at: None,
            release_due_at: None,
            terminal_at: None,
        }
    }

    pub fn confirmed_by(&self, party: Party) -> bool {
        let at = match party {
            Party::Buyer => self.buyer_confirmed,
            Party::Seller => self.seller_confirmed,
        };
        at == Some(self.terms_revision)
    }

    pub fn both_confirmed(&self) -> bool {
        self.confirmed_by(Party::Buyer) && self.confirmed_by(Party::Seller)
    }

    pub fn price(&self) -> Option<&Money> {
        self.terms.as_ref().map(|t| &t.price)
    }
}

// ---------------------------------------------------------------------------
// Commands and effects
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum DealCommand {
    /// The witness read the conversation. Advisory until confirmed.
    ProposeExtraction {
        extraction: Box<WitnessExtraction>,
        transcript_hash: String,
        /// The recording the transcript came from, if one was captured. Present
        /// only as a digest — this is what lets a receipt commit to the sound in
        /// the room, not merely to our reading of it.
        audio: Option<Box<AudioEvidence>>,
    },
    /// A party edited the reading. Resets both confirmations.
    CorrectTerms { terms: Box<Terms> },
    /// A party confirmed the reading as it currently stands.
    ConfirmTerms {
        revision: u32,
        /// True when this confirmation was made on a device that also holds the
        /// other party's credentials — two people around one phone.
        ///
        /// It is recorded rather than prevented. Both parties genuinely did read
        /// the terms in front of each other, which is most of the value; but
        /// whoever holds the phone *could* have tapped twice, so a receipt must
        /// not present it as two independent confirmations.
        same_device: bool,
    },
    /// The buyer funded escrow.
    Fund,
    /// The seller evidenced handing the item over.
    SubmitHandoffProof {
        proof_ref: String,
        assessment: Option<Box<HandoffAssessment>>,
    },
    /// The buyer confirmed receipt. Starts the release hold.
    ConfirmReceipt,
    /// Release the held funds to the seller. The buyer may do this early; the
    /// timer does it when the hold elapses.
    ReleaseFunds,
    OpenDispute { reason: String },
    ResolveDispute { outcome: DisputeOutcome, finding: String },
    Cancel { reason: String },
    /// A window elapsed. Only the system issues these.
    Expire { window: &'static str },
}

impl DealCommand {
    pub fn name(&self) -> &'static str {
        use DealCommand::*;
        match self {
            ProposeExtraction { .. } => "propose_extraction",
            CorrectTerms { .. } => "correct_terms",
            ConfirmTerms { .. } => "confirm_terms",
            Fund => "fund",
            SubmitHandoffProof { .. } => "submit_handoff_proof",
            ConfirmReceipt => "confirm_receipt",
            ReleaseFunds => "release_funds",
            OpenDispute { .. } => "open_dispute",
            ResolveDispute { .. } => "resolve_dispute",
            Cancel { .. } => "cancel",
            Expire { .. } => "expire",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimerKind {
    AgreementExpiry,
    FundingExpiry,
    HandoffDeadline,
    ReceiptWindow,
    ReleaseHold,
}

impl TimerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TimerKind::AgreementExpiry => "agreement_expiry",
            TimerKind::FundingExpiry => "funding_expiry",
            TimerKind::HandoffDeadline => "handoff_deadline",
            TimerKind::ReceiptWindow => "receipt_window",
            TimerKind::ReleaseHold => "release_hold",
        }
    }

    pub fn dedup_key(self, deal_id: DealId) -> String {
        format!("deal:{}:{}", deal_id, self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TimerRequest {
    Set {
        kind: TimerKind,
        due_at: OffsetDateTime,
        dedup_key: String,
    },
    Cancel {
        dedup_key: String,
    },
}

impl TimerRequest {
    fn set(deal_id: DealId, kind: TimerKind, due_at: OffsetDateTime) -> Self {
        TimerRequest::Set {
            kind,
            due_at,
            dedup_key: kind.dedup_key(deal_id),
        }
    }
    fn cancel(deal_id: DealId, kind: TimerKind) -> Self {
        TimerRequest::Cancel {
            dedup_key: kind.dedup_key(deal_id),
        }
    }
}

/// What the app layer should ask the settlement provider to do. The domain never
/// learns what a payment processor is — only that value should move.
#[derive(Debug, Clone, PartialEq)]
pub enum SettlementIntent {
    Hold { amount: Money },
    Release,
    Refund,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEvent {
    pub kind: String,
    pub payload: serde_json::Value,
}

impl DomainEvent {
    fn new(kind: &str, payload: serde_json::Value) -> Self {
        Self {
            kind: kind.to_string(),
            payload,
        }
    }
}

/// The complete result of one command. State and history move together.
#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    pub deal: Deal,
    pub events: Vec<DomainEvent>,
    pub attestation: Option<AttestationDraft>,
    pub timers: Vec<TimerRequest>,
    pub settlement: Option<SettlementIntent>,
}

// ---------------------------------------------------------------------------
// The transition function
// ---------------------------------------------------------------------------

pub fn transition(
    deal: &Deal,
    cmd: DealCommand,
    actor: Actor,
    now: OffsetDateTime,
) -> Result<Transition, DomainError> {
    let illegal = || DomainError::IllegalTransition {
        state: deal.state.as_str(),
        command: cmd.name(),
    };

    // A terminal deal accepts nothing at all. There is no "one more update"
    // after a receipt is final.
    if deal.state.is_terminal() {
        return Err(illegal());
    }

    let mut next = deal.clone();
    next.version = deal.version + 1;
    let mut events = Vec::new();
    let mut timers = Vec::new();
    let mut settlement = None;
    let attestation;

    match (&deal.state, &cmd) {
        // -- capture ---------------------------------------------------------
        (
            DealState::Draft,
            DealCommand::ProposeExtraction {
                extraction,
                transcript_hash,
                audio,
            },
        ) => {
            if !extraction.is_proposable() {
                return Err(DomainError::Invalid(
                    "the witness did not find a completed agreement in this conversation".into(),
                ));
            }
            let terms = extraction
                .to_candidate_terms()
                .ok_or(DomainError::NoAgreedPrice)?;
            terms.validate()?;

            next.state = DealState::PendingAgreement;
            next.terms = Some(terms.clone());
            next.terms_revision = 0;
            next.buyer_confirmed = None;
            next.seller_confirmed = None;

            timers.push(TimerRequest::set(
                deal.id,
                TimerKind::AgreementExpiry,
                now + AGREEMENT_WINDOW,
            ));
            events.push(DomainEvent::new(
                "witness.proposed",
                json!({ "summary": terms.summary(), "confidence": extraction.confidence }),
            ));
            attestation = Some(AttestationDraft::new(
                AttestationAction::WitnessProposed,
                actor,
                json!({
                    "deal_id": deal.id.to_string(),
                    "transcript_hash": transcript_hash,
                    "audio": audio.as_ref().map(|a| &**a),
                    "extraction": &**extraction,
                    "proposed_at": rfc3339(now),
                }),
            ));
        }

        // -- correcting the reading -----------------------------------------
        (DealState::PendingAgreement, DealCommand::CorrectTerms { terms }) => {
            let party = require_party(actor)?;
            terms.validate()?;

            next.terms = Some((**terms).clone());
            next.terms_revision = deal.terms_revision + 1;
            // A correction invalidates both confirmations, including the
            // corrector's own. You confirm what is on the table now, not what
            // was on the table before you changed it.
            next.buyer_confirmed = None;
            next.seller_confirmed = None;

            events.push(DomainEvent::new(
                "terms.corrected",
                json!({ "by": party.as_str(), "revision": next.terms_revision }),
            ));
            attestation = Some(AttestationDraft::new(
                AttestationAction::TermsCorrected,
                actor,
                json!({
                    "deal_id": deal.id.to_string(),
                    "revision": next.terms_revision,
                    "terms": &**terms,
                    "corrected_at": rfc3339(now),
                }),
            ));
        }

        // -- confirming ------------------------------------------------------
        (
            DealState::PendingAgreement,
            DealCommand::ConfirmTerms {
                revision,
                same_device,
            },
        ) => {
            let party = require_party(actor)?;
            if *revision != deal.terms_revision {
                return Err(DomainError::StaleTermsRevision {
                    confirmed: *revision,
                    current: deal.terms_revision,
                });
            }
            let terms = deal
                .terms
                .as_ref()
                .ok_or_else(|| DomainError::Invalid("no terms to confirm".into()))?;

            match party {
                Party::Buyer => next.buyer_confirmed = Some(*revision),
                Party::Seller => next.seller_confirmed = Some(*revision),
            }

            if next.both_confirmed() {
                // Freeze. From here the terms are immutable and hashed.
                let hash = crate::canonical::canonical_hash(terms)?;
                next.state = DealState::Agreed;
                next.terms_hash = Some(hash.clone());
                next.frozen_at = Some(now);

                timers.push(TimerRequest::cancel(deal.id, TimerKind::AgreementExpiry));
                timers.push(TimerRequest::set(
                    deal.id,
                    TimerKind::FundingExpiry,
                    now + FUNDING_WINDOW,
                ));
                events.push(DomainEvent::new(
                    "terms.frozen",
                    json!({ "terms_hash": hash, "summary": terms.summary() }),
                ));
                attestation = Some(AttestationDraft::new(
                    AttestationAction::TermsFrozen,
                    actor,
                    json!({
                        "deal_id": deal.id.to_string(),
                        "revision": revision,
                        "terms": terms,
                        "terms_hash": hash,
                        "confirmed_by": ["buyer", "seller"],
                        "same_device": same_device,
                        "frozen_at": rfc3339(now),
                    }),
                ));
            } else {
                events.push(DomainEvent::new(
                    "terms.confirmed",
                    json!({ "by": party.as_str(), "revision": revision, "same_device": same_device }),
                ));
                attestation = Some(AttestationDraft::new(
                    AttestationAction::TermsConfirmed,
                    actor,
                    json!({
                        "deal_id": deal.id.to_string(),
                        "revision": revision,
                        "by": party.as_str(),
                        "same_device": same_device,
                        "confirmed_at": rfc3339(now),
                    }),
                ));
            }
        }

        // -- funding ---------------------------------------------------------
        (DealState::Agreed, DealCommand::Fund) => {
            require_role(actor, Party::Buyer)?;
            let terms = deal
                .terms
                .as_ref()
                .ok_or_else(|| DomainError::Invalid("frozen deal has no terms".into()))?;

            next.state = DealState::Funded;
            settlement = Some(SettlementIntent::Hold {
                amount: terms.price.clone(),
            });

            timers.push(TimerRequest::cancel(deal.id, TimerKind::FundingExpiry));
            timers.push(TimerRequest::set(
                deal.id,
                TimerKind::HandoffDeadline,
                now + HANDOFF_WINDOW,
            ));
            events.push(DomainEvent::new(
                "funds.held",
                json!({ "amount": terms.price }),
            ));
            attestation = Some(AttestationDraft::new(
                AttestationAction::FundsHeld,
                actor,
                json!({
                    "deal_id": deal.id.to_string(),
                    "amount": terms.price,
                    "terms_hash": deal.terms_hash,
                    "held_at": rfc3339(now),
                }),
            ));
        }

        // -- handoff ---------------------------------------------------------
        (
            DealState::Funded,
            DealCommand::SubmitHandoffProof {
                proof_ref,
                assessment,
            },
        ) => {
            require_role(actor, Party::Seller)?;

            next.state = DealState::HandoffProved;
            timers.push(TimerRequest::cancel(deal.id, TimerKind::HandoffDeadline));
            timers.push(TimerRequest::set(
                deal.id,
                TimerKind::ReceiptWindow,
                now + RECEIPT_WINDOW,
            ));
            events.push(DomainEvent::new(
                "handoff.proved",
                json!({ "proof_ref": proof_ref }),
            ));
            attestation = Some(AttestationDraft::new(
                AttestationAction::HandoffProved,
                actor,
                json!({
                    "deal_id": deal.id.to_string(),
                    // The photo itself lives in object storage behind a vault
                    // reference; only its handle and the witness's reading of it
                    // are hashed into the public chain.
                    "proof_ref": proof_ref,
                    "assessment": assessment.as_ref().map(|a| &**a),
                    "proved_at": rfc3339(now),
                }),
            ));
        }

        // -- receipt ---------------------------------------------------------
        (DealState::HandoffProved, DealCommand::ConfirmReceipt) => {
            require_role(actor, Party::Buyer)?;
            let due = now + RELEASE_HOLD;

            next.state = DealState::Holding;
            next.release_due_at = Some(due);

            timers.push(TimerRequest::cancel(deal.id, TimerKind::ReceiptWindow));
            timers.push(TimerRequest::set(deal.id, TimerKind::ReleaseHold, due));
            events.push(DomainEvent::new(
                "receipt.confirmed",
                json!({ "release_due_at": rfc3339(due), "automatic": false }),
            ));
            attestation = Some(AttestationDraft::new(
                AttestationAction::ReceiptConfirmed,
                actor,
                json!({
                    "deal_id": deal.id.to_string(),
                    "automatic": false,
                    "release_due_at": rfc3339(due),
                    "confirmed_at": rfc3339(now),
                }),
            ));
        }

        // The buyer went quiet after the seller proved handoff. The deal cannot
        // hang forever, so the release clock starts anyway — but the receipt is
        // permanently labelled as unconfirmed, and the dispute path stays open
        // for the whole hold.
        (DealState::HandoffProved, DealCommand::Expire { window }) => {
            require_system(actor)?;
            let due = now + RELEASE_HOLD;

            next.state = DealState::Holding;
            next.release_due_at = Some(due);
            next.receipt_auto_confirmed = true;

            timers.push(TimerRequest::set(deal.id, TimerKind::ReleaseHold, due));
            events.push(DomainEvent::new(
                "receipt.auto_confirmed",
                json!({ "window": window, "release_due_at": rfc3339(due) }),
            ));
            attestation = Some(AttestationDraft::new(
                AttestationAction::ReceiptConfirmed,
                actor,
                json!({
                    "deal_id": deal.id.to_string(),
                    "automatic": true,
                    "window": window,
                    "release_due_at": rfc3339(due),
                    "confirmed_at": rfc3339(now),
                }),
            ));
        }

        // -- release ---------------------------------------------------------
        (DealState::Holding, DealCommand::ReleaseFunds) => {
            // The buyer may waive the remainder of the hold; the timer releases
            // it otherwise. The seller cannot release to themselves.
            match actor {
                Actor::System => {}
                Actor::Party {
                    party: Party::Buyer,
                } => {}
                Actor::Party {
                    party: Party::Seller,
                } => {
                    return Err(DomainError::WrongRole {
                        required: "buyer",
                        actual: "seller",
                    })
                }
                Actor::Mediator => {
                    return Err(DomainError::WrongRole {
                        required: "buyer",
                        actual: "mediator",
                    })
                }
            }
            let early = matches!(actor, Actor::Party { .. });

            next.state = DealState::Completed;
            next.terminal_at = Some(now);
            settlement = Some(SettlementIntent::Release);

            timers.push(TimerRequest::cancel(deal.id, TimerKind::ReleaseHold));
            events.push(DomainEvent::new("funds.released", json!({ "early": early })));
            attestation = Some(AttestationDraft::new(
                AttestationAction::FundsReleased,
                actor,
                json!({
                    "deal_id": deal.id.to_string(),
                    "early": early,
                    "amount": deal.price(),
                    "released_at": rfc3339(now),
                }),
            ));
        }

        // -- disputes --------------------------------------------------------
        (
            DealState::Funded | DealState::HandoffProved | DealState::Holding,
            DealCommand::OpenDispute { reason },
        ) => {
            let party = require_party(actor)?;

            next.state = DealState::Disputed;
            // Freezing the release is the whole point: while a dispute is open
            // there is no transfer to race against.
            timers.push(TimerRequest::cancel(deal.id, TimerKind::ReleaseHold));
            timers.push(TimerRequest::cancel(deal.id, TimerKind::ReceiptWindow));
            timers.push(TimerRequest::cancel(deal.id, TimerKind::HandoffDeadline));
            events.push(DomainEvent::new(
                "dispute.opened",
                json!({ "by": party.as_str() }),
            ));
            attestation = Some(AttestationDraft::new(
                AttestationAction::DisputeOpened,
                actor,
                json!({
                    "deal_id": deal.id.to_string(),
                    "by": party.as_str(),
                    "reason": reason,
                    "opened_at": rfc3339(now),
                }),
            ));
        }

        (DealState::Disputed, DealCommand::ResolveDispute { outcome, finding }) => {
            require_mediator(actor)?;

            match outcome {
                DisputeOutcome::ReleaseToSeller => {
                    next.state = DealState::Resolved { outcome: *outcome };
                    next.terminal_at = Some(now);
                    settlement = Some(SettlementIntent::Release);
                }
                DisputeOutcome::RefundToBuyer => {
                    next.state = DealState::Resolved { outcome: *outcome };
                    next.terminal_at = Some(now);
                    settlement = Some(SettlementIntent::Refund);
                }
                DisputeOutcome::Withdrawn => {
                    // Nothing was decided about the money, so the deal resumes
                    // where it was — with a full hold, not the remainder of the
                    // one the dispute interrupted.
                    let due = now + RELEASE_HOLD;
                    next.state = DealState::Holding;
                    next.release_due_at = Some(due);
                    timers.push(TimerRequest::set(deal.id, TimerKind::ReleaseHold, due));
                }
            }

            events.push(DomainEvent::new(
                "dispute.resolved",
                json!({ "outcome": outcome }),
            ));
            attestation = Some(AttestationDraft::new(
                AttestationAction::DisputeResolved,
                actor,
                json!({
                    "deal_id": deal.id.to_string(),
                    "outcome": outcome,
                    "finding": finding,
                    "resolved_at": rfc3339(now),
                }),
            ));
        }

        // -- unwinding -------------------------------------------------------
        (
            DealState::Draft | DealState::PendingAgreement | DealState::Agreed,
            DealCommand::Cancel { reason },
        ) => {
            let party = require_party(actor)?;

            next.state = DealState::Cancelled;
            next.terminal_at = Some(now);
            timers.push(TimerRequest::cancel(deal.id, TimerKind::AgreementExpiry));
            timers.push(TimerRequest::cancel(deal.id, TimerKind::FundingExpiry));
            events.push(DomainEvent::new(
                "deal.cancelled",
                json!({ "by": party.as_str() }),
            ));
            attestation = Some(AttestationDraft::new(
                AttestationAction::Cancelled,
                actor,
                json!({
                    "deal_id": deal.id.to_string(),
                    "by": party.as_str(),
                    "reason": reason,
                    "cancelled_at": rfc3339(now),
                }),
            ));
        }

        // Nobody funded, or nobody confirmed. No money moved, so nothing to
        // return — the deal simply lapses.
        (
            DealState::Draft | DealState::PendingAgreement | DealState::Agreed,
            DealCommand::Expire { window },
        ) => {
            require_system(actor)?;

            next.state = DealState::Expired;
            next.terminal_at = Some(now);
            timers.push(TimerRequest::cancel(deal.id, TimerKind::AgreementExpiry));
            timers.push(TimerRequest::cancel(deal.id, TimerKind::FundingExpiry));
            events.push(DomainEvent::new("deal.expired", json!({ "window": window })));
            attestation = Some(AttestationDraft::new(
                AttestationAction::Expired,
                actor,
                json!({
                    "deal_id": deal.id.to_string(),
                    "window": window,
                    "expired_at": rfc3339(now),
                }),
            ));
        }

        // The seller took the money and never handed anything over. Funds go
        // back; this is the one expiry that moves value.
        (DealState::Funded, DealCommand::Expire { window }) => {
            require_system(actor)?;

            next.state = DealState::Refunded;
            next.terminal_at = Some(now);
            settlement = Some(SettlementIntent::Refund);

            timers.push(TimerRequest::cancel(deal.id, TimerKind::HandoffDeadline));
            events.push(DomainEvent::new(
                "funds.refunded",
                json!({ "window": window }),
            ));
            attestation = Some(AttestationDraft::new(
                AttestationAction::FundsRefunded,
                actor,
                json!({
                    "deal_id": deal.id.to_string(),
                    "window": window,
                    "amount": deal.price(),
                    "refunded_at": rfc3339(now),
                }),
            ));
        }

        _ => return Err(illegal()),
    }

    Ok(Transition {
        deal: next,
        events,
        attestation,
        timers,
        settlement,
    })
}

fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn require_party(actor: Actor) -> Result<Party, DomainError> {
    // `ok_or_else`, not `ok_or`: the error value must not be built on the happy
    // path, or the `Party` arm below is evaluated for every successful call.
    actor.party().ok_or_else(|| DomainError::NotAParticipant {
        actor: match actor {
            Actor::System => "system".into(),
            Actor::Mediator => "mediator".into(),
            Actor::Party { party } => party.as_str().into(),
        },
    })
}

fn require_role(actor: Actor, required: Party) -> Result<(), DomainError> {
    match actor.party() {
        Some(p) if p == required => Ok(()),
        Some(p) => Err(DomainError::WrongRole {
            required: required.as_str(),
            actual: p.as_str(),
        }),
        None => Err(DomainError::WrongRole {
            required: required.as_str(),
            actual: "system",
        }),
    }
}

fn require_system(actor: Actor) -> Result<(), DomainError> {
    match actor {
        Actor::System => Ok(()),
        _ => Err(DomainError::WrongRole {
            required: "system",
            actual: "party",
        }),
    }
}

fn require_mediator(actor: Actor) -> Result<(), DomainError> {
    match actor {
        Actor::Mediator => Ok(()),
        _ => Err(DomainError::WrongRole {
            required: "mediator",
            actual: "party",
        }),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terms::{HandoffMethod, SettlementMethod};
    use crate::witness::Confidence;
    use time::macros::datetime;

    const T0: OffsetDateTime = datetime!(2026-08-16 12:00:00 UTC);

    fn buyer() -> Actor {
        Actor::Party {
            party: Party::Buyer,
        }
    }
    fn seller() -> Actor {
        Actor::Party {
            party: Party::Seller,
        }
    }

    fn extraction() -> WitnessExtraction {
        WitnessExtraction {
            item: "Fitbit".into(),
            item_detail: None,
            condition: Some("used".into()),
            agreed_price: Some(Money::usd(4000).unwrap()),
            buyer_speaker: "Nash".into(),
            seller_speaker: "Stella".into(),
            ladder: vec![],
            settlement: SettlementMethod::Escrow,
            handoff: HandoffMethod::InPerson,
            confidence: Confidence::High,
            ambiguities: vec![],
            agreement_detected: true,
            agreement_quote: Some("We have a deal".into()),
        }
    }

    fn propose(deal: &Deal) -> Deal {
        transition(
            deal,
            DealCommand::ProposeExtraction {
                extraction: Box::new(extraction()),
                transcript_hash: "deadbeef".into(),
                audio: None,
            },
            Actor::System,
            T0,
        )
        .unwrap()
        .deal
    }

    fn agreed() -> Deal {
        let d = Deal::new(DealId::new(), T0);
        let d = propose(&d);
        let d = transition(&d, DealCommand::ConfirmTerms { revision: 0, same_device: false }, buyer(), T0)
            .unwrap()
            .deal;
        transition(&d, DealCommand::ConfirmTerms { revision: 0, same_device: false }, seller(), T0)
            .unwrap()
            .deal
    }

    fn holding() -> Deal {
        let d = agreed();
        let d = transition(&d, DealCommand::Fund, buyer(), T0).unwrap().deal;
        let d = transition(
            &d,
            DealCommand::SubmitHandoffProof {
                proof_ref: "obj://proof/1".into(),
                assessment: None,
            },
            seller(),
            T0,
        )
        .unwrap()
        .deal;
        transition(&d, DealCommand::ConfirmReceipt, buyer(), T0)
            .unwrap()
            .deal
    }

    #[test]
    fn walks_the_nash_and_stella_story_end_to_end() {
        let d = holding();
        assert_eq!(d.state, DealState::Holding);
        assert_eq!(d.release_due_at, Some(T0 + RELEASE_HOLD));

        let t = transition(&d, DealCommand::ReleaseFunds, Actor::System, T0 + RELEASE_HOLD).unwrap();
        assert_eq!(t.deal.state, DealState::Completed);
        assert_eq!(t.settlement, Some(SettlementIntent::Release));
    }

    #[test]
    fn a_same_device_confirmation_is_recorded_not_refused() {
        // Two people around one phone is a legitimate way to handshake, and the
        // weaker evidence is labelled rather than rejected.
        let d = propose(&Deal::new(DealId::new(), T0));
        let t = transition(
            &d,
            DealCommand::ConfirmTerms {
                revision: 0,
                same_device: true,
            },
            buyer(),
            T0,
        )
        .unwrap();

        let payload = &t.attestation.as_ref().unwrap().payload;
        assert_eq!(payload["same_device"], serde_json::json!(true));

        // And it carries through to the freeze, which is what the receipt reads.
        let t2 = transition(
            &t.deal,
            DealCommand::ConfirmTerms {
                revision: 0,
                same_device: true,
            },
            seller(),
            T0,
        )
        .unwrap();
        assert_eq!(t2.deal.state, DealState::Agreed);
        assert_eq!(
            t2.attestation.as_ref().unwrap().payload["same_device"],
            serde_json::json!(true)
        );
    }

    #[test]
    fn one_confirmation_does_not_freeze_terms() {
        let d = propose(&Deal::new(DealId::new(), T0));
        let t = transition(&d, DealCommand::ConfirmTerms { revision: 0, same_device: false }, buyer(), T0).unwrap();
        assert_eq!(t.deal.state, DealState::PendingAgreement);
        assert!(t.deal.terms_hash.is_none());
    }

    #[test]
    fn freezing_hashes_the_terms_exactly_once() {
        let d = agreed();
        assert_eq!(d.state, DealState::Agreed);
        let hash = d.terms_hash.clone().unwrap();
        assert_eq!(hash.len(), 64);
        // The hash is over the terms, so an identical agreement hashes the same.
        assert_eq!(
            hash,
            crate::canonical::canonical_hash(d.terms.as_ref().unwrap()).unwrap()
        );
    }

    #[test]
    fn correcting_terms_revokes_both_confirmations() {
        let d = propose(&Deal::new(DealId::new(), T0));
        let d = transition(&d, DealCommand::ConfirmTerms { revision: 0, same_device: false }, buyer(), T0)
            .unwrap()
            .deal;
        assert!(d.confirmed_by(Party::Buyer));

        let mut corrected = d.terms.clone().unwrap();
        corrected.price = Money::usd(4500).unwrap();
        let d = transition(
            &d,
            DealCommand::CorrectTerms {
                terms: Box::new(corrected),
            },
            seller(),
            T0,
        )
        .unwrap()
        .deal;

        assert_eq!(d.terms_revision, 1);
        assert!(!d.confirmed_by(Party::Buyer));
        assert!(!d.confirmed_by(Party::Seller));
    }

    #[test]
    fn confirming_a_stale_revision_is_rejected() {
        let d = propose(&Deal::new(DealId::new(), T0));
        let mut corrected = d.terms.clone().unwrap();
        corrected.price = Money::usd(4500).unwrap();
        let d = transition(
            &d,
            DealCommand::CorrectTerms {
                terms: Box::new(corrected),
            },
            seller(),
            T0,
        )
        .unwrap()
        .deal;

        // Nash confirms the $40 he was looking at; the price is now $45.
        assert_eq!(
            transition(&d, DealCommand::ConfirmTerms { revision: 0, same_device: false }, buyer(), T0),
            Err(DomainError::StaleTermsRevision {
                confirmed: 0,
                current: 1
            })
        );
    }

    #[test]
    fn only_the_buyer_funds_and_only_the_seller_proves_handoff() {
        let d = agreed();
        assert!(matches!(
            transition(&d, DealCommand::Fund, seller(), T0),
            Err(DomainError::WrongRole { .. })
        ));

        let d = transition(&d, DealCommand::Fund, buyer(), T0).unwrap().deal;
        assert!(matches!(
            transition(
                &d,
                DealCommand::SubmitHandoffProof {
                    proof_ref: "x".into(),
                    assessment: None
                },
                buyer(),
                T0
            ),
            Err(DomainError::WrongRole { .. })
        ));
    }

    #[test]
    fn the_seller_cannot_release_funds_to_themselves() {
        let d = holding();
        assert!(matches!(
            transition(&d, DealCommand::ReleaseFunds, seller(), T0),
            Err(DomainError::WrongRole {
                required: "buyer",
                actual: "seller"
            })
        ));
    }

    #[test]
    fn the_buyer_may_waive_the_hold_and_release_early() {
        let d = holding();
        let t = transition(&d, DealCommand::ReleaseFunds, buyer(), T0).unwrap();
        assert_eq!(t.deal.state, DealState::Completed);
        assert_eq!(t.settlement, Some(SettlementIntent::Release));
    }

    #[test]
    fn a_dispute_freezes_the_release() {
        let d = holding();
        let t = transition(
            &d,
            DealCommand::OpenDispute {
                reason: "it is not the model described".into(),
            },
            buyer(),
            T0,
        )
        .unwrap();

        assert_eq!(t.deal.state, DealState::Disputed);
        assert!(t.settlement.is_none());
        assert!(t.timers.iter().any(|x| matches!(
            x,
            TimerRequest::Cancel { dedup_key } if dedup_key.ends_with("release_hold")
        )));

        // And with the release frozen, the timer firing cannot move money.
        assert!(matches!(
            transition(&t.deal, DealCommand::ReleaseFunds, Actor::System, T0),
            Err(DomainError::IllegalTransition { .. })
        ));
    }

    #[test]
    fn withdrawing_a_dispute_restarts_a_full_hold() {
        let d = holding();
        let d = transition(
            &d,
            DealCommand::OpenDispute {
                reason: "mistake".into(),
            },
            buyer(),
            T0,
        )
        .unwrap()
        .deal;

        let later = T0 + Duration::hours(6);
        let t = transition(
            &d,
            DealCommand::ResolveDispute {
                outcome: DisputeOutcome::Withdrawn,
                finding: "raiser withdrew".into(),
            },
            Actor::Mediator,
            later,
        )
        .unwrap();

        assert_eq!(t.deal.state, DealState::Holding);
        assert_eq!(t.deal.release_due_at, Some(later + RELEASE_HOLD));
        assert!(t.settlement.is_none());
    }

    #[test]
    fn an_upheld_dispute_refunds_the_buyer() {
        let d = holding();
        let d = transition(
            &d,
            DealCommand::OpenDispute {
                reason: "never arrived".into(),
            },
            buyer(),
            T0,
        )
        .unwrap()
        .deal;
        let t = transition(
            &d,
            DealCommand::ResolveDispute {
                outcome: DisputeOutcome::RefundToBuyer,
                finding: "no evidence of handoff".into(),
            },
            Actor::Mediator,
            T0,
        )
        .unwrap();

        assert_eq!(t.settlement, Some(SettlementIntent::Refund));
        assert!(t.deal.state.is_terminal());
    }

    #[test]
    fn a_silent_buyer_does_not_strand_the_sellers_money() {
        let d = agreed();
        let d = transition(&d, DealCommand::Fund, buyer(), T0).unwrap().deal;
        let d = transition(
            &d,
            DealCommand::SubmitHandoffProof {
                proof_ref: "obj://proof/1".into(),
                assessment: None,
            },
            seller(),
            T0,
        )
        .unwrap()
        .deal;

        let t = transition(
            &d,
            DealCommand::Expire {
                window: "receipt_window",
            },
            Actor::System,
            T0 + RECEIPT_WINDOW,
        )
        .unwrap();

        assert_eq!(t.deal.state, DealState::Holding);
        // The receipt is labelled forever, not quietly upgraded.
        assert!(t.deal.receipt_auto_confirmed);
    }

    #[test]
    fn a_silent_seller_returns_the_buyers_money() {
        let d = agreed();
        let d = transition(&d, DealCommand::Fund, buyer(), T0).unwrap().deal;
        let t = transition(
            &d,
            DealCommand::Expire {
                window: "handoff_deadline",
            },
            Actor::System,
            T0 + HANDOFF_WINDOW,
        )
        .unwrap();

        assert_eq!(t.deal.state, DealState::Refunded);
        assert_eq!(t.settlement, Some(SettlementIntent::Refund));
    }

    #[test]
    fn terminal_deals_accept_nothing() {
        let d = holding();
        let d = transition(&d, DealCommand::ReleaseFunds, Actor::System, T0)
            .unwrap()
            .deal;
        assert!(d.state.is_terminal());

        for cmd in [
            DealCommand::ReleaseFunds,
            DealCommand::ConfirmReceipt,
            DealCommand::OpenDispute {
                reason: "too late".into(),
            },
            DealCommand::Cancel {
                reason: "too late".into(),
            },
        ] {
            assert!(matches!(
                transition(&d, cmd, buyer(), T0),
                Err(DomainError::IllegalTransition { .. })
            ));
        }
    }

    #[test]
    fn a_conversation_without_agreement_is_not_a_deal() {
        let mut x = extraction();
        x.agreement_detected = false;
        let d = Deal::new(DealId::new(), T0);
        assert!(transition(
            &d,
            DealCommand::ProposeExtraction {
                extraction: Box::new(x),
                transcript_hash: "x".into(),
                audio: None,
            },
            Actor::System,
            T0
        )
        .is_err());
    }

    #[test]
    fn every_accepted_transition_bumps_the_version_and_attests() {
        let d = agreed();
        let before = d.version;
        let t = transition(&d, DealCommand::Fund, buyer(), T0).unwrap();
        assert_eq!(t.deal.version, before + 1);
        assert!(t.attestation.is_some());
    }

    #[test]
    fn funds_are_held_exactly_across_the_states_that_say_so() {
        assert!(!DealState::Agreed.funds_held());
        assert!(DealState::Funded.funds_held());
        assert!(DealState::HandoffProved.funds_held());
        assert!(DealState::Holding.funds_held());
        assert!(DealState::Disputed.funds_held());
        assert!(!DealState::Completed.funds_held());
    }
}
