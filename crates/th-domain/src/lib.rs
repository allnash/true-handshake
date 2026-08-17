//! # True Handshake — domain
//!
//! Pure types, the deal state machine, the attestation chain, and the canonical
//! encoding the public receipt spec is written against.
//!
//! This crate has no async runtime, no database, no network, and no clock. Time
//! arrives as a parameter. That is what lets a full lifecycle — propose, confirm,
//! fund, hand off, hold for 24 hours, release — be tested deterministically in
//! microseconds, and it is why the trust-bearing logic can be reviewed without
//! reading any I/O code.
//!
//! ## The rule this crate exists to enforce
//!
//! > The witness proposes; the humans attest; the chain records.
//!
//! An AI reading of a conversation is never binding on its own. It becomes
//! `Terms` only when both parties have confirmed the same revision, at which
//! point it is frozen, hashed, and countersigned. Everything downstream — escrow,
//! handoff, release — hangs off that frozen artifact.

pub mod canonical;
pub mod chain;
pub mod error;
pub mod ids;
pub mod money;
pub mod state;
pub mod terms;
pub mod witness;

pub use error::DomainError;
pub use ids::{AccountId, AttestationId, DealId, DisputeId, ProofId, ReceiptId, SessionId, TaskId};
pub use money::Money;
pub use state::{
    transition, Deal, DealCommand, DealState, DisputeOutcome, DomainEvent, SettlementIntent,
    TimerKind, TimerRequest, Transition, AGREEMENT_WINDOW, FUNDING_WINDOW, HANDOFF_WINDOW,
    RECEIPT_WINDOW, RELEASE_HOLD,
};
pub use terms::{EvidenceTier, HandoffMethod, Offer, OfferKind, Party, SettlementMethod, Terms};
pub use witness::{
    AudioEvidence, Confidence, HandoffAssessment, SpeakerBinding, SpeakerIdentification,
    Transcript, Utterance, WitnessExtraction,
};

pub use chain::{Actor, Attestation, AttestationAction, AttestationDraft};
