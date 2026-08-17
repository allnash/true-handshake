//! PostgreSQL adapters.
//!
//! Queries are built at runtime rather than with `sqlx::query!`, so the workspace
//! compiles without a live database. That costs compile-time SQL checking and
//! buys a `cargo build` that works on a fresh clone; the integration tests are
//! where the SQL is actually exercised.

use async_trait::async_trait;
use sqlx::{postgres::PgRow, PgPool, Row};
use th_app::{
    AppError, AudioStore, ChainHead, Commit, DealRecord, DealRepo, DueTask, ImageBytes,
    PartyBinding, ProofStore, Result, SessionRepo, TaskQueue, WitnessSession,
};
use th_domain::{
    Actor, Attestation, AttestationAction, AudioEvidence, Deal, DealId, DealState, DisputeOutcome,
    DomainEvent, EvidenceTier, SessionId, SpeakerIdentification, TaskId, Terms, TimerKind,
    TimerRequest, Transcript,
};
use time::OffsetDateTime;
use uuid::Uuid;

fn storage<E: std::fmt::Display>(e: E) -> AppError {
    AppError::Storage(e.to_string())
}

pub async fn connect(url: &str, max_connections: u32) -> anyhow::Result<PgPool> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(url)
        .await?;
    Ok(pool)
}

pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("../../migrations").run(pool).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Serialization helpers
// ---------------------------------------------------------------------------

fn state_to_columns(state: &DealState) -> (String, Option<String>) {
    match state {
        DealState::Resolved { outcome } => (
            "resolved".into(),
            Some(
                match outcome {
                    DisputeOutcome::ReleaseToSeller => "release_to_seller",
                    DisputeOutcome::RefundToBuyer => "refund_to_buyer",
                    DisputeOutcome::Withdrawn => "withdrawn",
                }
                .into(),
            ),
        ),
        other => (other.as_str().into(), None),
    }
}

fn state_from_columns(state: &str, outcome: Option<&str>) -> Result<DealState> {
    Ok(match state {
        "draft" => DealState::Draft,
        "pending_agreement" => DealState::PendingAgreement,
        "agreed" => DealState::Agreed,
        "funded" => DealState::Funded,
        "handoff_proved" => DealState::HandoffProved,
        "holding" => DealState::Holding,
        "completed" => DealState::Completed,
        "refunded" => DealState::Refunded,
        "cancelled" => DealState::Cancelled,
        "expired" => DealState::Expired,
        "disputed" => DealState::Disputed,
        "resolved" => DealState::Resolved {
            outcome: match outcome {
                Some("release_to_seller") => DisputeOutcome::ReleaseToSeller,
                Some("refund_to_buyer") => DisputeOutcome::RefundToBuyer,
                Some("withdrawn") => DisputeOutcome::Withdrawn,
                other => {
                    return Err(AppError::Storage(format!(
                        "resolved deal has unknown outcome {other:?}"
                    )))
                }
            },
        },
        other => {
            return Err(AppError::Storage(format!(
                "unknown persisted deal state {other:?}"
            )))
        }
    })
}

fn timer_kind_from_str(s: &str) -> Result<TimerKind> {
    Ok(match s {
        "agreement_expiry" => TimerKind::AgreementExpiry,
        "funding_expiry" => TimerKind::FundingExpiry,
        "handoff_deadline" => TimerKind::HandoffDeadline,
        "receipt_window" => TimerKind::ReceiptWindow,
        "release_hold" => TimerKind::ReleaseHold,
        other => return Err(AppError::Storage(format!("unknown timer kind {other:?}"))),
    })
}

fn action_from_str(s: &str) -> Result<AttestationAction> {
    use AttestationAction::*;
    Ok(match s {
        "witness_proposed" => WitnessProposed,
        "terms_corrected" => TermsCorrected,
        "terms_confirmed" => TermsConfirmed,
        "terms_frozen" => TermsFrozen,
        "funds_held" => FundsHeld,
        "handoff_proved" => HandoffProved,
        "receipt_confirmed" => ReceiptConfirmed,
        "funds_released" => FundsReleased,
        "funds_refunded" => FundsRefunded,
        "dispute_opened" => DisputeOpened,
        "dispute_resolved" => DisputeResolved,
        "cancelled" => Cancelled,
        "expired" => Expired,
        other => {
            return Err(AppError::Storage(format!(
                "unknown attestation action {other:?}"
            )))
        }
    })
}

fn deal_from_row(row: &PgRow) -> Result<DealRecord> {
    let state = state_from_columns(
        row.try_get::<String, _>("state").map_err(storage)?.as_str(),
        row.try_get::<Option<String>, _>("dispute_outcome")
            .map_err(storage)?
            .as_deref(),
    )?;

    let terms: Option<Terms> = row
        .try_get::<Option<serde_json::Value>, _>("terms")
        .map_err(storage)?
        .map(serde_json::from_value)
        .transpose()
        .map_err(storage)?;

    let evidence_tier = match row
        .try_get::<String, _>("evidence_tier")
        .map_err(storage)?
        .as_str()
    {
        "observed" => EvidenceTier::Observed,
        _ => EvidenceTier::Attested,
    };

    let deal = Deal {
        id: DealId(row.try_get::<Uuid, _>("id").map_err(storage)?),
        state,
        version: row.try_get::<i32, _>("version").map_err(storage)? as u32,
        terms_revision: row.try_get::<i32, _>("terms_revision").map_err(storage)? as u32,
        buyer_confirmed: row
            .try_get::<Option<i32>, _>("buyer_confirmed")
            .map_err(storage)?
            .map(|v| v as u32),
        seller_confirmed: row
            .try_get::<Option<i32>, _>("seller_confirmed")
            .map_err(storage)?
            .map(|v| v as u32),
        terms,
        terms_hash: row.try_get("terms_hash").map_err(storage)?,
        evidence_tier,
        receipt_auto_confirmed: row.try_get("receipt_auto_confirmed").map_err(storage)?,
        created_at: row.try_get("created_at").map_err(storage)?,
        frozen_at: row.try_get("frozen_at").map_err(storage)?,
        release_due_at: row.try_get("release_due_at").map_err(storage)?,
        terminal_at: row.try_get("terminal_at").map_err(storage)?,
    };

    Ok(DealRecord {
        deal,
        parties: PartyBinding {
            buyer_name: row.try_get("buyer_name").map_err(storage)?,
            seller_name: row.try_get("seller_name").map_err(storage)?,
            buyer_token: row.try_get("buyer_token").map_err(storage)?,
            seller_token: row.try_get("seller_token").map_err(storage)?,
        },
        settlement_handle: row.try_get("settlement_handle").map_err(storage)?,
        session_id: row
            .try_get::<Option<Uuid>, _>("session_id")
            .map_err(storage)?
            .map(SessionId),
    })
}

const DEAL_COLUMNS: &str = "id, state, dispute_outcome, version, terms_revision, buyer_confirmed, \
     seller_confirmed, terms, terms_hash, evidence_tier, receipt_auto_confirmed, created_at, \
     frozen_at, release_due_at, terminal_at, buyer_name, seller_name, buyer_token, seller_token, \
     settlement_handle, session_id";

// ---------------------------------------------------------------------------
// Deals
// ---------------------------------------------------------------------------

pub struct PgDealRepo {
    pool: PgPool,
}

impl PgDealRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl DealRepo for PgDealRepo {
    async fn create(&self, deal: &Deal, parties: &PartyBinding, session: SessionId) -> Result<()> {
        let (state, outcome) = state_to_columns(&deal.state);
        sqlx::query(
            "insert into deals (id, state, dispute_outcome, version, terms_revision, \
             evidence_tier, receipt_auto_confirmed, created_at, buyer_name, seller_name, \
             buyer_token, seller_token, session_id) \
             values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
        )
        .bind(deal.id.as_uuid())
        .bind(state)
        .bind(outcome)
        .bind(deal.version as i32)
        .bind(deal.terms_revision as i32)
        .bind("attested")
        .bind(deal.receipt_auto_confirmed)
        .bind(deal.created_at)
        .bind(&parties.buyer_name)
        .bind(&parties.seller_name)
        .bind(&parties.buyer_token)
        .bind(&parties.seller_token)
        .bind(session.as_uuid())
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        Ok(())
    }

    async fn load(&self, id: DealId) -> Result<Option<DealRecord>> {
        let row = sqlx::query(&format!("select {DEAL_COLUMNS} from deals where id = $1"))
            .bind(id.as_uuid())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?;
        row.as_ref().map(deal_from_row).transpose()
    }

    async fn chain_head(&self, id: DealId) -> Result<ChainHead> {
        let row = sqlx::query(
            "select seq, chain_hash from attestations where deal_id = $1 \
             order by seq desc limit 1",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?;

        Ok(match row {
            Some(r) => ChainHead {
                next_seq: r.try_get::<i32, _>("seq").map_err(storage)? as u32 + 1,
                prev_chain_hash: r.try_get("chain_hash").map_err(storage)?,
            },
            None => ChainHead {
                next_seq: 0,
                prev_chain_hash: th_domain::chain::genesis_hash(id),
            },
        })
    }

    /// State, events, and the attestation commit together. There is no window in
    /// which a deal has advanced but its history has not.
    async fn commit(&self, commit: Commit) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(storage)?;
        let deal = &commit.deal;
        let (state, outcome) = state_to_columns(&deal.state);

        // The version predicate is the optimistic lock: zero rows updated means
        // somebody else moved this deal while we were deciding.
        let updated = sqlx::query(
            "update deals set state = $1, dispute_outcome = $2, version = $3, \
             terms_revision = $4, buyer_confirmed = $5, seller_confirmed = $6, terms = $7, \
             terms_hash = $8, receipt_auto_confirmed = $9, frozen_at = $10, \
             release_due_at = $11, terminal_at = $12, \
             settlement_handle = coalesce($13, settlement_handle) \
             where id = $14 and version = $15",
        )
        .bind(state)
        .bind(outcome)
        .bind(deal.version as i32)
        .bind(deal.terms_revision as i32)
        .bind(deal.buyer_confirmed.map(|v| v as i32))
        .bind(deal.seller_confirmed.map(|v| v as i32))
        .bind(
            deal.terms
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(storage)?,
        )
        .bind(&deal.terms_hash)
        .bind(deal.receipt_auto_confirmed)
        .bind(deal.frozen_at)
        .bind(deal.release_due_at)
        .bind(deal.terminal_at)
        .bind(&commit.settlement_handle)
        .bind(deal.id.as_uuid())
        .bind(commit.expected_version as i32)
        .execute(&mut *tx)
        .await
        .map_err(storage)?;

        if updated.rows_affected() == 0 {
            let current: Option<i32> = sqlx::query_scalar("select version from deals where id = $1")
                .bind(deal.id.as_uuid())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?;
            return Err(match current {
                Some(v) => AppError::VersionConflict {
                    expected: commit.expected_version,
                    current: v as u32,
                },
                None => AppError::NotFound,
            });
        }

        let a = &commit.attestation;
        sqlx::query(
            "insert into attestations (id, deal_id, seq, action, actor, at, payload, \
             payload_hash, prev_chain_hash, chain_hash, key_id, signature) \
             values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
        )
        .bind(a.id.as_uuid())
        .bind(a.deal_id.as_uuid())
        .bind(a.seq as i32)
        .bind(a.action.as_str())
        .bind(serde_json::to_value(a.actor).map_err(storage)?)
        .bind(a.at)
        .bind(&a.payload)
        .bind(&a.payload_hash)
        .bind(&a.prev_chain_hash)
        .bind(&a.chain_hash)
        .bind(&a.key_id)
        .bind(&a.signature)
        .execute(&mut *tx)
        .await
        .map_err(storage)?;

        let mut next_event_seq: i32 = sqlx::query_scalar(
            "select coalesce(max(seq), -1) + 1 from deal_events where deal_id = $1",
        )
        .bind(deal.id.as_uuid())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;

        for event in &commit.events {
            sqlx::query(
                "insert into deal_events (deal_id, seq, kind, payload, occurred_at) \
                 values ($1,$2,$3,$4,$5)",
            )
            .bind(deal.id.as_uuid())
            .bind(next_event_seq)
            .bind(&event.kind)
            .bind(&event.payload)
            .bind(a.at)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            next_event_seq += 1;
        }

        tx.commit().await.map_err(storage)?;
        Ok(())
    }

    async fn attestations(&self, id: DealId) -> Result<Vec<Attestation>> {
        let rows = sqlx::query(
            "select id, deal_id, seq, action, actor, at, payload, payload_hash, \
             prev_chain_hash, chain_hash, key_id, signature \
             from attestations where deal_id = $1 order by seq asc",
        )
        .bind(id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;

        rows.into_iter()
            .map(|r| {
                let actor: Actor =
                    serde_json::from_value(r.try_get("actor").map_err(storage)?).map_err(storage)?;
                Ok(Attestation {
                    id: th_domain::AttestationId(r.try_get::<Uuid, _>("id").map_err(storage)?),
                    deal_id: DealId(r.try_get::<Uuid, _>("deal_id").map_err(storage)?),
                    seq: r.try_get::<i32, _>("seq").map_err(storage)? as u32,
                    action: action_from_str(
                        r.try_get::<String, _>("action").map_err(storage)?.as_str(),
                    )?,
                    actor,
                    at: r.try_get("at").map_err(storage)?,
                    payload: r.try_get("payload").map_err(storage)?,
                    payload_hash: r.try_get("payload_hash").map_err(storage)?,
                    prev_chain_hash: r.try_get("prev_chain_hash").map_err(storage)?,
                    chain_hash: r.try_get("chain_hash").map_err(storage)?,
                    key_id: r.try_get("key_id").map_err(storage)?,
                    signature: r.try_get("signature").map_err(storage)?,
                })
            })
            .collect()
    }

    async fn events(&self, id: DealId) -> Result<Vec<(OffsetDateTime, DomainEvent)>> {
        let rows = sqlx::query(
            "select kind, payload, occurred_at from deal_events where deal_id = $1 order by seq asc",
        )
        .bind(id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;

        rows.into_iter()
            .map(|r| {
                Ok((
                    r.try_get("occurred_at").map_err(storage)?,
                    DomainEvent {
                        kind: r.try_get("kind").map_err(storage)?,
                        payload: r.try_get("payload").map_err(storage)?,
                    },
                ))
            })
            .collect()
    }

    async fn rename_parties(&self, id: DealId, buyer: &str, seller: &str) -> Result<()> {
        // Guarded in SQL rather than in a check-then-write: once terms are
        // frozen the names are part of what was hashed, and renaming would
        // silently invalidate the receipt.
        let updated = sqlx::query(
            "update deals set buyer_name = $1, seller_name = $2 \
             where id = $3 and terms_hash is null",
        )
        .bind(buyer)
        .bind(seller)
        .bind(id.as_uuid())
        .execute(&self.pool)
        .await
        .map_err(storage)?;

        if updated.rows_affected() == 0 {
            return Err(AppError::Invalid(
                "these terms are already frozen; the names are part of what was signed".into(),
            ));
        }
        Ok(())
    }

    async fn list_for_token(&self, token: &str) -> Result<Vec<DealRecord>> {
        let rows = sqlx::query(&format!(
            "select {DEAL_COLUMNS} from deals \
             where buyer_token = $1 or seller_token = $1 order by created_at desc limit 100"
        ))
        .bind(token)
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.iter().map(deal_from_row).collect()
    }
}

// ---------------------------------------------------------------------------
// Witness sessions
// ---------------------------------------------------------------------------

pub struct PgSessionRepo {
    pool: PgPool,
}

impl PgSessionRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl SessionRepo for PgSessionRepo {
    async fn create(&self, session: &WitnessSession) -> Result<()> {
        sqlx::query(
            "insert into witness_sessions (id, deal_id, transcript, started_at, closed) \
             values ($1,$2,$3,$4,$5)",
        )
        .bind(session.id.as_uuid())
        .bind(session.deal_id.as_uuid())
        .bind(serde_json::to_value(&session.transcript).map_err(storage)?)
        .bind(session.started_at)
        .bind(session.closed)
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        Ok(())
    }

    async fn load(&self, id: SessionId) -> Result<Option<WitnessSession>> {
        let row = sqlx::query(
            "select id, deal_id, transcript, started_at, closed, audio_ref, speaker_bindings \
             from witness_sessions where id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?;

        let Some(r) = row else { return Ok(None) };

        let audio_ref: Option<String> = r.try_get("audio_ref").map_err(storage)?;
        let speakers: Option<SpeakerIdentification> = r
            .try_get::<Option<serde_json::Value>, _>("speaker_bindings")
            .map_err(storage)?
            .map(serde_json::from_value)
            .transpose()
            .map_err(storage)?;

        // The digest is what the attestation commits to, so it is read back
        // from the stored object rather than cached on the session row — one
        // source of truth for what the recording actually is.
        let audio = match &audio_ref {
            Some(reference) => sqlx::query(
                "select media_type, sha256, duration_ms, octet_length(bytes) as size_bytes \
                 from audio_objects where reference = $1",
            )
            .bind(reference)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .map(|a| -> Result<AudioEvidence> {
                Ok(AudioEvidence {
                    media_type: a.try_get("media_type").map_err(storage)?,
                    sha256: a.try_get("sha256").map_err(storage)?,
                    size_bytes: a.try_get::<Option<i32>, _>("size_bytes").map_err(storage)?
                        .unwrap_or(0) as i64,
                    duration_ms: a
                        .try_get::<Option<i32>, _>("duration_ms")
                        .map_err(storage)?
                        .map(|d| d as i64),
                })
            })
            .transpose()?,
            None => None,
        };

        Ok(Some(WitnessSession {
            id: SessionId(r.try_get::<Uuid, _>("id").map_err(storage)?),
            deal_id: DealId(r.try_get::<Uuid, _>("deal_id").map_err(storage)?),
            transcript: serde_json::from_value::<Transcript>(
                r.try_get("transcript").map_err(storage)?,
            )
            .map_err(storage)?,
            started_at: r.try_get("started_at").map_err(storage)?,
            closed: r.try_get("closed").map_err(storage)?,
            audio,
            audio_ref,
            speakers,
        }))
    }

    async fn append(&self, id: SessionId, transcript: &Transcript) -> Result<()> {
        sqlx::query("update witness_sessions set transcript = $1 where id = $2 and closed = false")
            .bind(serde_json::to_value(transcript).map_err(storage)?)
            .bind(id.as_uuid())
            .execute(&self.pool)
            .await
            .map_err(storage)?;
        Ok(())
    }

    async fn close(&self, id: SessionId) -> Result<()> {
        sqlx::query("update witness_sessions set closed = true where id = $1")
            .bind(id.as_uuid())
            .execute(&self.pool)
            .await
            .map_err(storage)?;
        Ok(())
    }

    async fn attach_audio(
        &self,
        id: SessionId,
        reference: &str,
        evidence: &AudioEvidence,
    ) -> Result<()> {
        sqlx::query(
            "update witness_sessions set audio_ref = $1, audio_sha256 = $2 \
             where id = $3 and closed = false",
        )
        .bind(reference)
        .bind(&evidence.sha256)
        .bind(id.as_uuid())
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        Ok(())
    }

    async fn set_speakers(&self, id: SessionId, speakers: &SpeakerIdentification) -> Result<()> {
        sqlx::query("update witness_sessions set speaker_bindings = $1 where id = $2")
            .bind(serde_json::to_value(speakers).map_err(storage)?)
            .bind(id.as_uuid())
            .execute(&self.pool)
            .await
            .map_err(storage)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Recordings
// ---------------------------------------------------------------------------

/// Stores recordings in Postgres. Object storage is the production answer; the
/// point of the port is that swapping it changes nothing above.
pub struct PgAudioStore {
    pool: PgPool,
}

impl PgAudioStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl AudioStore for PgAudioStore {
    async fn put(
        &self,
        deal_id: DealId,
        media_type: &str,
        bytes: Vec<u8>,
        duration_ms: Option<i64>,
    ) -> Result<(String, AudioEvidence)> {
        use sha2::{Digest, Sha256};

        // Hashed here, from the bytes we actually received. Accepting a
        // client-supplied digest would let a party commit the receipt to a
        // recording they never made.
        let sha256 = th_domain::canonical::hex(&Sha256::digest(&bytes));
        let reference = format!("audio:{}", Uuid::new_v4().simple());
        let size_bytes = bytes.len() as i64;

        sqlx::query(
            "insert into audio_objects \
             (reference, deal_id, media_type, sha256, bytes, duration_ms, created_at) \
             values ($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(&reference)
        .bind(deal_id.as_uuid())
        .bind(media_type)
        .bind(&sha256)
        .bind(&bytes)
        .bind(duration_ms.map(|d| d as i32))
        .bind(OffsetDateTime::now_utc())
        .execute(&self.pool)
        .await
        .map_err(storage)?;

        Ok((
            reference,
            AudioEvidence {
                sha256,
                media_type: media_type.to_string(),
                size_bytes,
                duration_ms,
            },
        ))
    }

    async fn get(&self, reference: &str) -> Result<(String, Vec<u8>)> {
        let row = sqlx::query("select media_type, bytes from audio_objects where reference = $1")
            .bind(reference)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or(AppError::NotFound)?;
        Ok((
            row.try_get("media_type").map_err(storage)?,
            row.try_get("bytes").map_err(storage)?,
        ))
    }
}

// ---------------------------------------------------------------------------
// Durable timers
// ---------------------------------------------------------------------------

pub struct PgTaskQueue {
    pool: PgPool,
    lease: time::Duration,
}

impl PgTaskQueue {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            lease: time::Duration::minutes(5),
        }
    }
}

#[async_trait]
impl TaskQueue for PgTaskQueue {
    async fn apply(&self, deal_id: DealId, requests: &[TimerRequest]) -> Result<()> {
        for req in requests {
            match req {
                TimerRequest::Set {
                    kind,
                    due_at,
                    dedup_key,
                } => {
                    // `dedup_key` is unique, so a retried command re-arms the
                    // same timer instead of creating a second one.
                    sqlx::query(
                        "insert into scheduled_tasks (id, deal_id, kind, due_at, state, dedup_key) \
                         values ($1,$2,$3,$4,'pending',$5) \
                         on conflict (dedup_key) do update \
                         set due_at = excluded.due_at, state = 'pending', \
                             locked_until = null, attempts = 0",
                    )
                    .bind(Uuid::new_v4())
                    .bind(deal_id.as_uuid())
                    .bind(kind.as_str())
                    .bind(*due_at)
                    .bind(dedup_key)
                    .execute(&self.pool)
                    .await
                    .map_err(storage)?;
                }
                TimerRequest::Cancel { dedup_key } => {
                    sqlx::query(
                        "update scheduled_tasks set state = 'cancelled' \
                         where dedup_key = $1 and state = 'pending'",
                    )
                    .bind(dedup_key)
                    .execute(&self.pool)
                    .await
                    .map_err(storage)?;
                }
            }
        }
        Ok(())
    }

    async fn claim_due(&self, now: OffsetDateTime, limit: i64) -> Result<Vec<DueTask>> {
        // SKIP LOCKED lets several worker replicas drain the same queue without
        // coordinating and without double-firing a deadline.
        let rows = sqlx::query(
            "update scheduled_tasks set locked_until = $1, attempts = attempts + 1 \
             where id in ( \
                 select id from scheduled_tasks \
                 where state = 'pending' and due_at <= $2 \
                   and (locked_until is null or locked_until < $2) \
                 order by due_at asc \
                 for update skip locked \
                 limit $3 \
             ) returning id, deal_id, kind, due_at",
        )
        .bind(now + self.lease)
        .bind(now)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;

        rows.into_iter()
            .map(|r| {
                Ok(DueTask {
                    id: TaskId(r.try_get::<Uuid, _>("id").map_err(storage)?),
                    deal_id: DealId(r.try_get::<Uuid, _>("deal_id").map_err(storage)?),
                    kind: timer_kind_from_str(
                        r.try_get::<String, _>("kind").map_err(storage)?.as_str(),
                    )?,
                    // The *logical* due time, not the time we happened to pick it
                    // up. A worker resuming late still acts as of the right
                    // instant.
                    due_at: r.try_get("due_at").map_err(storage)?,
                })
            })
            .collect()
    }

    async fn complete(&self, id: TaskId) -> Result<()> {
        sqlx::query("update scheduled_tasks set state = 'done', locked_until = null where id = $1")
            .bind(id.as_uuid())
            .execute(&self.pool)
            .await
            .map_err(storage)?;
        Ok(())
    }

    async fn fail(&self, id: TaskId, error: &str) -> Result<()> {
        // Back to pending so it retries; `attempts` is already incremented, and
        // a task that keeps failing stays visible rather than vanishing.
        sqlx::query(
            "update scheduled_tasks set state = 'pending', locked_until = null, last_error = $1 \
             where id = $2",
        )
        .bind(error)
        .bind(id.as_uuid())
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Handoff photos
// ---------------------------------------------------------------------------

/// Stores proof images in Postgres. Object storage is the production answer;
/// the point of the port is that swapping it changes nothing above.
pub struct PgProofStore {
    pool: PgPool,
}

impl PgProofStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredImage {
    media_type: String,
    data_b64: String,
}

#[async_trait]
impl ProofStore for PgProofStore {
    async fn put(&self, deal_id: DealId, images: &[ImageBytes]) -> Result<String> {
        use base64::Engine;
        let reference = format!("proof:{}", Uuid::new_v4().simple());
        let stored: Vec<StoredImage> = images
            .iter()
            .map(|i| StoredImage {
                media_type: i.media_type.clone(),
                data_b64: base64::engine::general_purpose::STANDARD.encode(&i.bytes),
            })
            .collect();

        sqlx::query(
            "insert into proof_objects (reference, deal_id, images, created_at) \
             values ($1,$2,$3,$4)",
        )
        .bind(&reference)
        .bind(deal_id.as_uuid())
        .bind(serde_json::to_value(&stored).map_err(storage)?)
        .bind(OffsetDateTime::now_utc())
        .execute(&self.pool)
        .await
        .map_err(storage)?;

        Ok(reference)
    }

    async fn get(&self, reference: &str) -> Result<Vec<ImageBytes>> {
        use base64::Engine;
        let row = sqlx::query("select images from proof_objects where reference = $1")
            .bind(reference)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or(AppError::NotFound)?;

        let stored: Vec<StoredImage> =
            serde_json::from_value(row.try_get("images").map_err(storage)?).map_err(storage)?;

        stored
            .into_iter()
            .map(|s| {
                Ok(ImageBytes {
                    media_type: s.media_type,
                    bytes: base64::engine::general_purpose::STANDARD
                        .decode(s.data_b64.as_bytes())
                        .map_err(storage)?,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deal_state_survives_a_round_trip_through_columns() {
        let states = [
            DealState::Draft,
            DealState::PendingAgreement,
            DealState::Agreed,
            DealState::Funded,
            DealState::HandoffProved,
            DealState::Holding,
            DealState::Completed,
            DealState::Refunded,
            DealState::Cancelled,
            DealState::Expired,
            DealState::Disputed,
            DealState::Resolved {
                outcome: DisputeOutcome::ReleaseToSeller,
            },
            DealState::Resolved {
                outcome: DisputeOutcome::RefundToBuyer,
            },
            DealState::Resolved {
                outcome: DisputeOutcome::Withdrawn,
            },
        ];

        for state in states {
            let (s, o) = state_to_columns(&state);
            assert_eq!(state_from_columns(&s, o.as_deref()).unwrap(), state);
        }
    }

    #[test]
    fn every_timer_kind_round_trips() {
        for kind in [
            TimerKind::AgreementExpiry,
            TimerKind::FundingExpiry,
            TimerKind::HandoffDeadline,
            TimerKind::ReceiptWindow,
            TimerKind::ReleaseHold,
        ] {
            assert_eq!(timer_kind_from_str(kind.as_str()).unwrap(), kind);
        }
    }

    #[test]
    fn every_attestation_action_round_trips() {
        use AttestationAction::*;
        for a in [
            WitnessProposed,
            TermsCorrected,
            TermsConfirmed,
            TermsFrozen,
            FundsHeld,
            HandoffProved,
            ReceiptConfirmed,
            FundsReleased,
            FundsRefunded,
            DisputeOpened,
            DisputeResolved,
            Cancelled,
            Expired,
        ] {
            assert_eq!(action_from_str(a.as_str()).unwrap(), a);
        }
    }

    #[test]
    fn unknown_persisted_values_are_errors_not_defaults() {
        assert!(state_from_columns("something_new", None).is_err());
        assert!(state_from_columns("resolved", None).is_err());
        assert!(timer_kind_from_str("nope").is_err());
    }
}
