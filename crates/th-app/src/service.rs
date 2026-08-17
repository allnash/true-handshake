//! Use cases. Every mutating path takes the same shape:
//!
//! 1. load the deal and resolve the caller's role,
//! 2. ask the domain what the command implies,
//! 3. seal and sign the attestation onto the chain head,
//! 4. commit state + event + attestation + timers in one transaction,
//! 5. drive the settlement provider.
//!
//! Step 5's position relative to step 4 differs by direction, and deliberately:
//! **holding** funds happens before the commit (so we never record "funded" for
//! money we failed to take), while **releasing** or **refunding** happens after
//! (so we never move money we failed to record). The mock provider is idempotent
//! per deal, which is what makes both orders safe under retry.

use std::sync::Arc;

use th_domain::{
    canonical, chain, transition, Actor, Attestation, AttestationDraft, AudioEvidence, Deal,
    DealCommand, DealId, DealState, DisputeOutcome, Money, Party, SessionId, SettlementIntent,
    SpeakerIdentification, Terms, TimerKind, Transcript, Transition, Utterance,
};
use time::OffsetDateTime;
use tracing::{info, warn};

use crate::error::AppError;
use crate::ports::*;

pub struct Handshake {
    pub clock: Arc<dyn Clock>,
    pub deals: Arc<dyn DealRepo>,
    pub sessions: Arc<dyn SessionRepo>,
    pub witness: Arc<dyn Witness>,
    pub vision: Arc<dyn VisionWitness>,
    pub proofs: Arc<dyn ProofStore>,
    pub audio: Arc<dyn AudioStore>,
    pub settlement: Arc<dyn SettlementProvider>,
    pub signer: Arc<dyn Signer>,
    pub tasks: Arc<dyn TaskQueue>,
    pub default_currency: String,
}

/// What a caller gets back after a successful command: enough to update a UI
/// without a second round trip, including the attestation id that proves the
/// chain actually recorded it.
#[derive(Debug, Clone)]
pub struct CommandResult {
    pub deal: Deal,
    pub attestation_id: th_domain::AttestationId,
    pub chain_hash: String,
}

/// The pair of tokens handed out when a handshake starts. Each party keeps one.
#[derive(Debug, Clone)]
pub struct StartedSession {
    pub session_id: SessionId,
    pub deal_id: DealId,
    pub buyer_token: String,
    pub seller_token: String,
}

impl Handshake {
    // -----------------------------------------------------------------------
    // Capture
    // -----------------------------------------------------------------------

    /// Open a witness session for two people about to negotiate.
    ///
    /// Roles are provisional: whoever ends up paying is decided by what the
    /// conversation actually says, and both parties confirm that reading before
    /// anything is binding. The names here are only speaker labels.
    pub async fn start_session(
        &self,
        buyer_name: String,
        seller_name: String,
    ) -> Result<StartedSession> {
        if buyer_name.trim().is_empty() || seller_name.trim().is_empty() {
            return Err(AppError::Invalid("both parties need a name".into()));
        }
        if buyer_name.trim().eq_ignore_ascii_case(seller_name.trim()) {
            return Err(AppError::Invalid(
                "the two parties need distinguishable names".into(),
            ));
        }

        let now = self.clock.now();
        let deal_id = DealId::new();
        let session_id = SessionId::new();
        let parties = PartyBinding {
            buyer_name: buyer_name.trim().to_string(),
            seller_name: seller_name.trim().to_string(),
            buyer_token: new_token(),
            seller_token: new_token(),
        };

        let deal = Deal::new(deal_id, now);
        self.deals.create(&deal, &parties, session_id).await?;
        self.sessions
            .create(&WitnessSession {
                id: session_id,
                deal_id,
                transcript: Transcript::default(),
                started_at: now,
                closed: false,
                audio: None,
                audio_ref: None,
                speakers: None,
            })
            .await?;

        info!(deal_id = %deal_id, session_id = %session_id, "witness session opened");

        Ok(StartedSession {
            session_id,
            deal_id,
            buyer_token: parties.buyer_token,
            seller_token: parties.seller_token,
        })
    }

    /// Append recognized speech. Called repeatedly as the browser recognizer
    /// finalizes phrases.
    pub async fn append_utterances(
        &self,
        session_id: SessionId,
        utterances: Vec<Utterance>,
    ) -> Result<Transcript> {
        let session = self
            .sessions
            .load(session_id)
            .await?
            .ok_or(AppError::NotFound)?;
        if session.closed {
            return Err(AppError::Invalid(
                "this session is closed; the reading has already been proposed".into(),
            ));
        }

        let mut transcript = session.transcript;
        let mut next_seq = transcript.utterances.len() as u32;
        for mut u in utterances {
            if u.text.trim().is_empty() {
                continue;
            }
            u.seq = next_seq;
            next_seq += 1;
            transcript.utterances.push(u);
        }

        self.sessions.append(session_id, &transcript).await?;
        Ok(transcript)
    }

    /// Run the witness over the captured conversation and put its reading in
    /// front of both parties. This is the only place the model's output touches
    /// the deal, and it lands in `PendingAgreement` — proposed, not agreed.
    pub async fn propose_from_session(&self, session_id: SessionId) -> Result<CommandResult> {
        let session = self
            .sessions
            .load(session_id)
            .await?
            .ok_or(AppError::NotFound)?;
        if session.transcript.is_empty() {
            return Err(AppError::Invalid("nothing has been said yet".into()));
        }

        let ctx = WitnessContext {
            speakers: session.transcript.speakers(),
            currency: self.default_currency.clone(),
        };
        let extraction = self.witness.extract(&session.transcript, &ctx).await?;
        let transcript_hash = canonical::canonical_hash(&session.transcript)?;

        let result = self
            .apply(
                session.deal_id,
                DealCommand::ProposeExtraction {
                    extraction: Box::new(extraction),
                    transcript_hash,
                    // Committing to the recording's digest is what lets a
                    // receipt say the transcript came from *this* conversation,
                    // rather than asking anyone to take our word for it.
                    audio: session.audio.clone().map(Box::new),
                },
                Actor::System,
            )
            .await?;

        self.sessions.close(session_id).await?;
        Ok(result)
    }

    /// Store a recording of the session and commit to its digest.
    ///
    /// The hash is computed here, from the bytes we actually received. A
    /// client-supplied digest would be worthless: the whole point is that the
    /// receipt commits to a specific recording, and a party who could name that
    /// digest freely could commit to a recording they never made.
    pub async fn attach_audio(
        &self,
        session_id: SessionId,
        media_type: String,
        bytes: Vec<u8>,
        duration_ms: Option<i64>,
    ) -> Result<AudioEvidence> {
        if bytes.is_empty() {
            return Err(AppError::Invalid("the recording is empty".into()));
        }
        let session = self
            .sessions
            .load(session_id)
            .await?
            .ok_or(AppError::NotFound)?;
        if session.closed {
            return Err(AppError::Invalid(
                "this session is closed; its recording is already committed".into(),
            ));
        }

        let (reference, evidence) = self
            .audio
            .put(session.deal_id, &media_type, bytes, duration_ms)
            .await?;
        self.sessions
            .attach_audio(session_id, &reference, &evidence)
            .await?;

        info!(
            deal_id = %session.deal_id,
            sha256 = %evidence.sha256,
            bytes = evidence.size_bytes,
            "recording attached"
        );
        Ok(evidence)
    }

    /// Work out which voice belongs to whom, from the opening exchange.
    ///
    /// Runs before any price is named, deliberately. An inverted mapping does
    /// not produce a slightly wrong receipt — it swaps the buyer and the seller,
    /// which inverts the entire deal.
    pub async fn identify_speakers(
        &self,
        session_id: SessionId,
    ) -> Result<SpeakerIdentification> {
        let session = self
            .sessions
            .load(session_id)
            .await?
            .ok_or(AppError::NotFound)?;
        if session.transcript.is_empty() {
            return Err(AppError::Invalid("nobody has said anything yet".into()));
        }

        let identification = self.witness.identify_speakers(&session.transcript).await?;
        // Persisted even when incomplete: the UI shows what was understood and
        // lets the parties finish the job by hand.
        self.sessions.set_speakers(session_id, &identification).await?;
        Ok(identification)
    }

    /// Accept a speaker mapping, after a human has looked at it.
    ///
    /// This is the same shape as confirming terms: the witness proposes, a
    /// person confirms, and only then does anything downstream rely on it.
    pub async fn confirm_speakers(
        &self,
        session_id: SessionId,
        identification: SpeakerIdentification,
    ) -> Result<SpeakerIdentification> {
        if !identification.is_complete() {
            return Err(AppError::Invalid(
                "a handshake needs exactly two voices with two distinct names".into(),
            ));
        }
        let session = self
            .sessions
            .load(session_id)
            .await?
            .ok_or(AppError::NotFound)?;

        self.sessions.set_speakers(session_id, &identification).await?;

        // Party names are display labels; now that people have said who they
        // are, use those. Which of them is buying is still decided later, by
        // what the conversation actually says.
        let first = &identification.bindings[0].name;
        let second = &identification.bindings[1].name;
        self.deals
            .rename_parties(session.deal_id, first, second)
            .await?;

        Ok(identification)
    }

    // -----------------------------------------------------------------------
    // Agreement
    // -----------------------------------------------------------------------

    pub async fn correct_terms(
        &self,
        deal_id: DealId,
        token: &str,
        terms: Terms,
    ) -> Result<CommandResult> {
        let actor = self.actor_for(deal_id, token).await?;
        self.apply(
            deal_id,
            DealCommand::CorrectTerms {
                terms: Box::new(terms),
            },
            actor,
        )
        .await
    }

    /// `same_device` is asserted by the client, and cannot be verified from
    /// here — two people around one phone look exactly like two phones. It is
    /// recorded because an honest client reporting it makes the receipt more
    /// accurate, and because the alternative is a receipt that quietly implies
    /// two independent confirmations when there was one device.
    pub async fn confirm_terms(
        &self,
        deal_id: DealId,
        token: &str,
        revision: u32,
        same_device: bool,
    ) -> Result<CommandResult> {
        let actor = self.actor_for(deal_id, token).await?;
        self.apply(
            deal_id,
            DealCommand::ConfirmTerms {
                revision,
                same_device,
            },
            actor,
        )
        .await
    }

    // -----------------------------------------------------------------------
    // Money and goods
    // -----------------------------------------------------------------------

    pub async fn fund(&self, deal_id: DealId, token: &str) -> Result<CommandResult> {
        let actor = self.actor_for(deal_id, token).await?;
        self.apply(deal_id, DealCommand::Fund, actor).await
    }

    /// The seller evidences handing the item over. The photo is optional: a deal
    /// where the item changed hands across a table does not always have one, and
    /// refusing to proceed without a picture would just push people off-platform.
    /// When a photo is supplied, the witness looks at it and its reading is
    /// recorded — as an annotation, never as a gate.
    pub async fn submit_handoff_proof(
        &self,
        deal_id: DealId,
        token: &str,
        images: Vec<ImageBytes>,
        note: Option<String>,
    ) -> Result<CommandResult> {
        let actor = self.actor_for(deal_id, token).await?;
        let record = self.load(deal_id).await?;
        let terms = record
            .deal
            .terms
            .clone()
            .ok_or_else(|| AppError::Invalid("this deal has no frozen terms".into()))?;

        let (proof_ref, assessment) = if images.is_empty() {
            (
                format!("note:{}", note.unwrap_or_else(|| "handed over".into())),
                None,
            )
        } else {
            let reference = self.proofs.put(deal_id, &images).await?;
            // A vision failure must not block a handoff — the deal proceeds with
            // the photo stored and no assessment attached.
            let assessment = match self.vision.assess_handoff(&terms, &images).await {
                Ok(a) => Some(Box::new(a)),
                Err(e) => {
                    warn!(deal_id = %deal_id, error = %e, "handoff assessment unavailable");
                    None
                }
            };
            (reference, assessment)
        };

        self.apply(
            deal_id,
            DealCommand::SubmitHandoffProof {
                proof_ref,
                assessment,
            },
            actor,
        )
        .await
    }

    pub async fn confirm_receipt(&self, deal_id: DealId, token: &str) -> Result<CommandResult> {
        let actor = self.actor_for(deal_id, token).await?;
        self.apply(deal_id, DealCommand::ConfirmReceipt, actor).await
    }

    /// The buyer waiving the remainder of the 24-hour hold.
    pub async fn release_now(&self, deal_id: DealId, token: &str) -> Result<CommandResult> {
        let actor = self.actor_for(deal_id, token).await?;
        self.apply(deal_id, DealCommand::ReleaseFunds, actor).await
    }

    pub async fn open_dispute(
        &self,
        deal_id: DealId,
        token: &str,
        reason: String,
    ) -> Result<CommandResult> {
        let actor = self.actor_for(deal_id, token).await?;
        self.apply(deal_id, DealCommand::OpenDispute { reason }, actor)
            .await
    }

    pub async fn resolve_dispute(
        &self,
        deal_id: DealId,
        outcome: DisputeOutcome,
        finding: String,
    ) -> Result<CommandResult> {
        self.apply(
            deal_id,
            DealCommand::ResolveDispute { outcome, finding },
            Actor::Mediator,
        )
        .await
    }

    pub async fn cancel(
        &self,
        deal_id: DealId,
        token: &str,
        reason: String,
    ) -> Result<CommandResult> {
        let actor = self.actor_for(deal_id, token).await?;
        self.apply(deal_id, DealCommand::Cancel { reason }, actor)
            .await
    }

    // -----------------------------------------------------------------------
    // Timers
    // -----------------------------------------------------------------------

    /// Fire one due timer. A timer invokes the same use case a human would, with
    /// a `System` actor — there is no privileged transition path.
    ///
    /// Late firing is safe: the command is evaluated against the deal's current
    /// state, so a worker that resumes hours behind either finds the transition
    /// still legal and applies it, or finds it moot and drops it.
    pub async fn fire_task(&self, task: &DueTask) -> Result<Option<CommandResult>> {
        let record = match self.deals.load(task.deal_id).await? {
            Some(r) => r,
            None => return Ok(None),
        };

        let cmd = match task.kind {
            TimerKind::ReleaseHold => DealCommand::ReleaseFunds,
            TimerKind::AgreementExpiry => DealCommand::Expire {
                window: "agreement_window",
            },
            TimerKind::FundingExpiry => DealCommand::Expire {
                window: "funding_window",
            },
            TimerKind::HandoffDeadline => DealCommand::Expire {
                window: "handoff_deadline",
            },
            TimerKind::ReceiptWindow => DealCommand::Expire {
                window: "receipt_window",
            },
        };

        match self.apply(task.deal_id, cmd, Actor::System).await {
            Ok(r) => Ok(Some(r)),
            // The deal moved on before the timer fired — a buyer released early,
            // a dispute froze the hold. That is the timer being moot, not an
            // error, so the task completes rather than retrying forever.
            Err(AppError::Domain(th_domain::DomainError::IllegalTransition { .. })) => {
                info!(
                    deal_id = %task.deal_id,
                    kind = task.kind.as_str(),
                    state = record.deal.state.as_str(),
                    "timer no longer applies; dropping"
                );
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    // -----------------------------------------------------------------------
    // Reads
    // -----------------------------------------------------------------------

    pub async fn load(&self, deal_id: DealId) -> Result<DealRecord> {
        self.deals.load(deal_id).await?.ok_or(AppError::NotFound)
    }

    pub async fn attestations(&self, deal_id: DealId) -> Result<Vec<Attestation>> {
        self.deals.attestations(deal_id).await
    }

    pub async fn timeline(&self, deal_id: DealId) -> Result<Vec<(OffsetDateTime, th_domain::DomainEvent)>> {
        self.deals.events(deal_id).await
    }

    pub async fn deals_for(&self, token: &str) -> Result<Vec<DealRecord>> {
        self.deals.list_for_token(token).await
    }

    pub async fn transcript(&self, session_id: SessionId) -> Result<WitnessSession> {
        self.sessions
            .load(session_id)
            .await?
            .ok_or(AppError::NotFound)
    }

    /// Resolve which side of the deal a bearer token speaks for.
    pub async fn role_for(&self, deal_id: DealId, token: &str) -> Result<Party> {
        let record = self.load(deal_id).await?;
        role_of(&record.parties, token).ok_or(AppError::Unauthorized)
    }

    // -----------------------------------------------------------------------
    // The common path
    // -----------------------------------------------------------------------

    async fn actor_for(&self, deal_id: DealId, token: &str) -> Result<Actor> {
        Ok(Actor::Party {
            party: self.role_for(deal_id, token).await?,
        })
    }

    async fn apply(
        &self,
        deal_id: DealId,
        cmd: DealCommand,
        actor: Actor,
    ) -> Result<CommandResult> {
        let record = self.load(deal_id).await?;
        let now = self.clock.now();
        let command_name = cmd.name();

        let t: Transition = transition(&record.deal, cmd, actor, now)?;

        // Take custody *before* recording custody. If this call succeeds and the
        // commit below fails, the caller retries and the provider's per-deal
        // idempotency returns the same handle rather than holding twice.
        let mut settlement_handle = None;
        if let Some(SettlementIntent::Hold { amount }) = &t.settlement {
            let handle = self.settlement.hold(deal_id, amount).await?;
            settlement_handle = Some(handle);
        }

        let attestation = self.seal(deal_id, &t, now).await?;

        self.deals
            .commit(Commit {
                expected_version: record.deal.version,
                deal: t.deal.clone(),
                events: t.events.clone(),
                attestation: attestation.clone(),
                timers: t.timers.clone(),
                settlement_handle,
            })
            .await?;

        self.tasks.apply(deal_id, &t.timers).await?;

        // Record before moving. A failure here leaves the deal saying "released"
        // with funds still held — visible, reconcilable, and far better than
        // money that moved with no record of why.
        match &t.settlement {
            Some(SettlementIntent::Release) => {
                let handle = record
                    .settlement_handle
                    .clone()
                    .ok_or_else(|| AppError::Settlement("no funds are held".into()))?;
                self.settlement.release(&handle).await?;
            }
            Some(SettlementIntent::Refund) => {
                let handle = record
                    .settlement_handle
                    .clone()
                    .ok_or_else(|| AppError::Settlement("no funds are held".into()))?;
                self.settlement.refund(&handle).await?;
            }
            _ => {}
        }

        info!(
            deal_id = %deal_id,
            command = command_name,
            from = record.deal.state.as_str(),
            to = t.deal.state.as_str(),
            seq = attestation.seq,
            "transition committed"
        );

        Ok(CommandResult {
            deal: t.deal,
            attestation_id: attestation.id,
            chain_hash: attestation.chain_hash,
        })
    }

    /// Hash the draft onto the chain head and sign it.
    async fn seal(
        &self,
        deal_id: DealId,
        t: &Transition,
        now: OffsetDateTime,
    ) -> Result<Attestation> {
        let draft: AttestationDraft = t
            .attestation
            .clone()
            .ok_or_else(|| AppError::Invalid("transition produced no attestation".into()))?;
        let head = self.deals.chain_head(deal_id).await?;

        let mut attestation = chain::seal(
            th_domain::AttestationId::new(),
            deal_id,
            head.next_seq,
            &head.prev_chain_hash,
            draft,
            now,
            self.signer.key_id(),
        )?;
        attestation.signature = Some(self.signer.sign(&chain::signing_message(&attestation.chain_hash)));
        Ok(attestation)
    }
}

pub fn role_of(parties: &PartyBinding, token: &str) -> Option<Party> {
    // Constant-time-ish comparison is overkill for a dev token, but comparing
    // full strings rather than prefixes is not.
    if !token.is_empty() && token == parties.buyer_token {
        Some(Party::Buyer)
    } else if !token.is_empty() && token == parties.seller_token {
        Some(Party::Seller)
    } else {
        None
    }
}

fn new_token() -> String {
    // Two v4 UUIDs' worth of entropy, url-safe.
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Convenience for building a `Money` in the platform's default currency.
pub fn money(currency: &str, minor_units: i64) -> Result<Money> {
    Ok(Money::new(currency, minor_units)?)
}

/// Everything a public receipt needs. Assembled here rather than in the API so
/// the CLI verifier and the HTTP endpoint serve byte-identical documents.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Receipt {
    pub receipt_id: String,
    pub deal_id: String,
    pub state: String,
    pub terms: Option<Terms>,
    pub terms_hash: Option<String>,
    pub amount_band: Option<String>,
    pub evidence_tier: th_domain::EvidenceTier,
    pub receipt_auto_confirmed: bool,
    pub attestations: Vec<Attestation>,
    pub key_id: String,
    pub public_key: String,
    pub verification: &'static str,
}

impl Handshake {
    pub async fn receipt(&self, deal_id: DealId) -> Result<Receipt> {
        let record = self.load(deal_id).await?;
        let attestations = self.deals.attestations(deal_id).await?;
        let deal = record.deal;

        Ok(Receipt {
            receipt_id: th_domain::ReceiptId::from(deal_id).to_string(),
            deal_id: deal_id.to_string(),
            state: deal.state.as_str().to_string(),
            amount_band: deal.terms.as_ref().map(|t| t.price.band()),
            // The public receipt carries the full frozen terms only once both
            // parties are bound by them; before that there is nothing agreed to
            // publish.
            terms: match deal.state {
                DealState::Draft | DealState::PendingAgreement => None,
                _ => deal.terms.clone(),
            },
            terms_hash: deal.terms_hash.clone(),
            evidence_tier: deal.evidence_tier,
            receipt_auto_confirmed: deal.receipt_auto_confirmed,
            attestations,
            key_id: self.signer.key_id(),
            public_key: self.signer.public_key_b64(),
            verification: "https://true-handshake.example/spec/v1/verification",
        })
    }
}
