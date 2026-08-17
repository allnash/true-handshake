//! Everything the application needs from the outside world, as traits.
//!
//! The escrow upgrade path exists because of this file. `SettlementProvider` is
//! implemented today by a mock ledger that moves numbers in Postgres; replacing
//! it with a PSP adapter that custodies real funds changes `evidence_tier()` from
//! `Attested` to `Observed` and changes nothing else. The domain never learns
//! what a payment processor is.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use th_domain::{
    Attestation, AudioEvidence, Deal, DealId, DomainEvent, HandoffAssessment, Money, SessionId,
    SpeakerIdentification, Terms, TimerKind, TimerRequest, Transcript, WitnessExtraction,
};
use time::OffsetDateTime;

use crate::error::AppError;

pub type Result<T> = std::result::Result<T, AppError>;

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

/// A product made almost entirely of deadlines cannot have `OffsetDateTime::now`
/// scattered through it and stay testable.
pub trait Clock: Send + Sync {
    fn now(&self) -> OffsetDateTime;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

// ---------------------------------------------------------------------------
// Identity (deliberately thin in v1)
// ---------------------------------------------------------------------------

/// How the two sides of a handshake are addressed.
///
/// v1 identifies each party by an unguessable per-deal token handed out when the
/// witness session starts. That is enough for two people standing next to each
/// other with two phones, and it is honestly *not* authentication: there are no
/// accounts, no passkeys, and no verification tiers yet. Swapping this for real
/// sessions is a change to this struct and the middleware that reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartyBinding {
    pub buyer_name: String,
    pub seller_name: String,
    pub buyer_token: String,
    pub seller_token: String,
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DealRecord {
    pub deal: Deal,
    pub parties: PartyBinding,
    /// Opaque handle from the settlement provider, once funds are held.
    pub settlement_handle: Option<String>,
    pub session_id: Option<SessionId>,
}

#[derive(Debug, Clone)]
pub struct ChainHead {
    pub next_seq: u32,
    pub prev_chain_hash: String,
}

/// One atomic write: the new deal state, its event, its attestation, and its
/// timers. All of it commits together or none of it does — there is no window in
/// which a deal has advanced but its history has not.
#[derive(Debug, Clone)]
pub struct Commit {
    pub expected_version: u32,
    pub deal: Deal,
    pub events: Vec<DomainEvent>,
    pub attestation: Attestation,
    pub timers: Vec<TimerRequest>,
    /// Set when this transition established a settlement handle.
    pub settlement_handle: Option<String>,
}

#[async_trait]
pub trait DealRepo: Send + Sync {
    async fn create(&self, deal: &Deal, parties: &PartyBinding, session: SessionId) -> Result<()>;
    async fn load(&self, id: DealId) -> Result<Option<DealRecord>>;
    async fn chain_head(&self, id: DealId) -> Result<ChainHead>;
    /// Fails with `AppError::VersionConflict` if the deal moved underneath us.
    async fn commit(&self, commit: Commit) -> Result<()>;
    async fn attestations(&self, id: DealId) -> Result<Vec<Attestation>>;
    async fn events(&self, id: DealId) -> Result<Vec<(OffsetDateTime, DomainEvent)>>;
    async fn list_for_token(&self, token: &str) -> Result<Vec<DealRecord>>;
    /// Party names are display labels, so they can be filled in from what people
    /// called themselves. Only legal before terms are frozen.
    async fn rename_parties(&self, id: DealId, buyer: &str, seller: &str) -> Result<()>;
}

// ---------------------------------------------------------------------------
// Witness capture
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct WitnessSession {
    pub id: SessionId,
    pub deal_id: DealId,
    pub transcript: Transcript,
    pub started_at: OffsetDateTime,
    pub closed: bool,
    /// The recording this transcript came from, once one has been uploaded.
    pub audio: Option<AudioEvidence>,
    pub audio_ref: Option<String>,
    /// Which voice is whom, confirmed by a human before negotiating.
    pub speakers: Option<SpeakerIdentification>,
}

#[async_trait]
pub trait SessionRepo: Send + Sync {
    async fn create(&self, session: &WitnessSession) -> Result<()>;
    async fn load(&self, id: SessionId) -> Result<Option<WitnessSession>>;
    async fn append(&self, id: SessionId, transcript: &Transcript) -> Result<()>;
    async fn close(&self, id: SessionId) -> Result<()>;
    async fn attach_audio(
        &self,
        id: SessionId,
        reference: &str,
        evidence: &AudioEvidence,
    ) -> Result<()>;
    async fn set_speakers(&self, id: SessionId, speakers: &SpeakerIdentification) -> Result<()>;
}

/// Recordings live here, referenced by handle. The bytes never enter the hash
/// chain — only their digest does — so a recording can be destroyed on request
/// without invalidating a single receipt.
#[async_trait]
pub trait AudioStore: Send + Sync {
    /// Returns the handle and the digest, both computed server-side. A
    /// client-supplied hash would let anyone claim a recording says whatever
    /// they liked.
    async fn put(
        &self,
        deal_id: DealId,
        media_type: &str,
        bytes: Vec<u8>,
        duration_ms: Option<i64>,
    ) -> Result<(String, AudioEvidence)>;
    async fn get(&self, reference: &str) -> Result<(String, Vec<u8>)>;
}

// ---------------------------------------------------------------------------
// The witness itself
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct WitnessContext {
    /// Speaker labels present in the transcript, in first-appearance order.
    pub speakers: Vec<String>,
    pub currency: String,
}

/// Reads a negotiation and proposes what was agreed. Never authoritative.
#[async_trait]
pub trait Witness: Send + Sync {
    async fn extract(
        &self,
        transcript: &Transcript,
        ctx: &WitnessContext,
    ) -> Result<WitnessExtraction>;

    /// Read the opening moments of a session and work out which voice belongs to
    /// whom, from what each said about themselves.
    ///
    /// This is deliberately a separate call from `extract`. Telling voices apart
    /// is signal processing; deciding which one is Stella is a reading of the
    /// words — and it has to be settled *before* anyone names a price, because
    /// an inverted mapping swaps the buyer and the seller.
    async fn identify_speakers(&self, opening: &Transcript) -> Result<SpeakerIdentification>;
}

/// Looks at handoff photos and says what it sees. Also never authoritative —
/// a poor assessment annotates a deal, it does not block one.
#[async_trait]
pub trait VisionWitness: Send + Sync {
    async fn assess_handoff(
        &self,
        terms: &Terms,
        images: &[ImageBytes],
    ) -> Result<HandoffAssessment>;
}

#[derive(Debug, Clone)]
pub struct ImageBytes {
    pub media_type: String,
    pub bytes: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Evidence storage
// ---------------------------------------------------------------------------

/// Handoff photos never enter the hash chain — only an opaque reference does, so
/// the image can be destroyed on request without breaking a receipt.
#[async_trait]
pub trait ProofStore: Send + Sync {
    async fn put(&self, deal_id: DealId, images: &[ImageBytes]) -> Result<String>;
    async fn get(&self, reference: &str) -> Result<Vec<ImageBytes>>;
}

// ---------------------------------------------------------------------------
// Settlement
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettlementState {
    /// No funds involved; the deal records claims only.
    Declared,
    Held,
    Released,
    Refunded,
}

/// The seam that makes real escrow an adapter rather than a rewrite.
#[async_trait]
pub trait SettlementProvider: Send + Sync {
    fn id(&self) -> &'static str;
    /// `Attested` for the mock ledger; `Observed` once a processor actually
    /// watches the money move.
    fn evidence_tier(&self) -> th_domain::EvidenceTier;

    async fn hold(&self, deal_id: DealId, amount: &Money) -> Result<String>;
    async fn release(&self, handle: &str) -> Result<SettlementState>;
    async fn refund(&self, handle: &str) -> Result<SettlementState>;
    async fn state(&self, handle: &str) -> Result<SettlementState>;
}

// ---------------------------------------------------------------------------
// Signing
// ---------------------------------------------------------------------------

pub trait Signer: Send + Sync {
    fn key_id(&self) -> String;
    /// Base64 (standard, padded) Ed25519 signature.
    fn sign(&self, message: &[u8]) -> String;
    /// Base64 public key, published at `/.well-known/true-handshake-keys.json`.
    fn public_key_b64(&self) -> String;
}

// ---------------------------------------------------------------------------
// Durable timers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DueTask {
    pub id: th_domain::TaskId,
    pub deal_id: DealId,
    pub kind: TimerKind,
    pub due_at: OffsetDateTime,
}

#[async_trait]
pub trait TaskQueue: Send + Sync {
    async fn apply(&self, deal_id: DealId, requests: &[TimerRequest]) -> Result<()>;
    /// Claim up to `limit` tasks whose logical due time has passed.
    async fn claim_due(&self, now: OffsetDateTime, limit: i64) -> Result<Vec<DueTask>>;
    async fn complete(&self, id: th_domain::TaskId) -> Result<()>;
    async fn fail(&self, id: th_domain::TaskId, error: &str) -> Result<()>;
}
