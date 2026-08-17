//! HTTP surface.
//!
//! Conventions: bearer token per party, RFC 9457 problem-details errors, and a
//! `server_time` on every response so clients anchor countdowns to our clock
//! rather than theirs. Every mutating response returns the attestation id its
//! transition produced — the UI is not permitted to say "accepted" until the
//! chain says so.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use serde::Deserialize;
use th_app::{AppError, Handshake, ImageBytes};
use th_domain::{
    DealId, DisputeOutcome, SessionId, SpeakerBinding, SpeakerIdentification, Terms, Utterance,
};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::views::*;

#[derive(Clone)]
pub struct AppState {
    pub handshake: Arc<Handshake>,
    pub public_base_url: String,
    /// Guards the mediator endpoints. Absent means mediation is disabled.
    pub mediator_token: Option<String>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

pub struct ApiError(AppError);

impl From<AppError> for ApiError {
    fn from(e: AppError) -> Self {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

        // A 409 is a designed state, not a failure: the body carries enough for
        // the client to refetch and show what changed.
        let mut body = serde_json::json!({
            "type": format!("https://true-handshake.example/errors/{}", self.0.code()),
            "title": self.0.code(),
            "status": status.as_u16(),
            "detail": self.0.to_string(),
        });
        if let AppError::VersionConflict { expected, current } = &self.0 {
            body["expected_version"] = (*expected).into();
            body["current_version"] = (*current).into();
        }

        if status.is_server_error() {
            tracing::error!(error = %self.0, "request failed");
        }

        let mut resp = (status, Json(body)).into_response();
        resp.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/problem+json"),
        );
        resp
    }
}

type ApiResult<T> = std::result::Result<T, ApiError>;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn bearer(headers: &HeaderMap) -> Result<String, ApiError> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or(ApiError(AppError::Unauthorized))
}

fn deal_id(raw: &str) -> Result<DealId, ApiError> {
    Uuid::parse_str(raw)
        .map(DealId)
        .map_err(|_| ApiError(AppError::Invalid("malformed deal id".into())))
}

fn session_id(raw: &str) -> Result<SessionId, ApiError> {
    Uuid::parse_str(raw)
        .map(SessionId)
        .map_err(|_| ApiError(AppError::Invalid("malformed session id".into())))
}

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

// ---------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct StartSessionBody {
    pub buyer_name: String,
    pub seller_name: String,
}

async fn start_session(
    State(st): State<AppState>,
    Json(body): Json<StartSessionBody>,
) -> ApiResult<Json<StartedView>> {
    let started = st
        .handshake
        .start_session(body.buyer_name, body.seller_name)
        .await?;

    Ok(Json(StartedView {
        buyer_link: format!(
            "{}/deal/{}?t={}",
            st.public_base_url, started.deal_id, started.buyer_token
        ),
        seller_link: format!(
            "{}/deal/{}?t={}",
            st.public_base_url, started.deal_id, started.seller_token
        ),
        session_id: started.session_id.to_string(),
        deal_id: started.deal_id.to_string(),
        buyer_token: started.buyer_token,
        seller_token: started.seller_token,
    }))
}

#[derive(Debug, Deserialize)]
pub struct AppendBody {
    pub utterances: Vec<Utterance>,
}

async fn append_utterances(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AppendBody>,
) -> ApiResult<Json<TranscriptView>> {
    let sid = session_id(&id)?;
    st.handshake.append_utterances(sid, body.utterances).await?;
    let session = st.handshake.transcript(sid).await?;

    Ok(Json(TranscriptView {
        session_id: session.id.to_string(),
        deal_id: session.deal_id.to_string(),
        closed: session.closed,
        utterances: session.transcript.utterances,
        server_time: now()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
    }))
}

async fn get_transcript(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<TranscriptView>> {
    let session = st.handshake.transcript(session_id(&id)?).await?;
    Ok(Json(TranscriptView {
        session_id: session.id.to_string(),
        deal_id: session.deal_id.to_string(),
        closed: session.closed,
        utterances: session.transcript.utterances,
        server_time: now()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
    }))
}

/// Run the witness over the conversation. The result lands in
/// `pending_agreement` — proposed, and binding on nobody.
async fn propose(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<CommandView>> {
    let result = st.handshake.propose_from_session(session_id(&id)?).await?;
    Ok(Json(CommandView::build(&result, now())))
}

#[derive(Debug, Deserialize)]
pub struct AudioBody {
    pub media_type: String,
    pub data_b64: String,
    #[serde(default)]
    pub duration_ms: Option<i64>,
}

/// Attach the recording the transcript came from.
///
/// The digest is computed server-side and lands in the proposal attestation, so
/// the receipt commits to the sound in the room and not merely to our reading of
/// it. The bytes stay outside the chain and can be destroyed on request.
async fn attach_audio(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AudioBody>,
) -> ApiResult<Json<serde_json::Value>> {
    if !body.media_type.starts_with("audio/") && !body.media_type.starts_with("video/") {
        return Err(ApiError(AppError::Invalid(format!(
            "unsupported recording type {}",
            body.media_type
        ))));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body.data_b64.as_bytes())
        .map_err(|_| ApiError(AppError::Invalid("recording was not valid base64".into())))?;

    let evidence = st
        .handshake
        .attach_audio(session_id(&id)?, body.media_type, bytes, body.duration_ms)
        .await?;

    Ok(Json(serde_json::json!({
        "sha256": evidence.sha256,
        "size_bytes": evidence.size_bytes,
        "media_type": evidence.media_type,
        "duration_ms": evidence.duration_ms,
    })))
}

/// Read who is who from the opening exchange. Advisory — a human confirms it.
async fn identify_speakers(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<SpeakerIdentification>> {
    Ok(Json(
        st.handshake.identify_speakers(session_id(&id)?).await?,
    ))
}

#[derive(Debug, Deserialize)]
pub struct ConfirmSpeakersBody {
    pub bindings: Vec<SpeakerBinding>,
}

async fn confirm_speakers(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ConfirmSpeakersBody>,
) -> ApiResult<Json<SpeakerIdentification>> {
    let identification = SpeakerIdentification {
        bindings: body.bindings,
        unbound: vec![],
        // A human looked at it, which is the only confidence that matters here.
        confidence: th_domain::Confidence::High,
        note: None,
    };
    Ok(Json(
        st.handshake
            .confirm_speakers(session_id(&id)?, identification)
            .await?,
    ))
}

// ---------------------------------------------------------------------------
// Deals
// ---------------------------------------------------------------------------

async fn get_deal(
    State(st): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<DealView>> {
    let did = deal_id(&id)?;
    let token = bearer(&headers)?;
    let role = st.handshake.role_for(did, &token).await?;

    let record = st.handshake.load(did).await?;
    let events = st.handshake.timeline(did).await?;
    let attestations = st.handshake.attestations(did).await?;

    Ok(Json(DealView::build(
        &record,
        role,
        events,
        &attestations,
        now(),
    )))
}

#[derive(Debug, Deserialize)]
pub struct CorrectBody {
    pub terms: Terms,
}

async fn correct_terms(
    State(st): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CorrectBody>,
) -> ApiResult<Json<CommandView>> {
    let token = bearer(&headers)?;
    let result = st
        .handshake
        .correct_terms(deal_id(&id)?, &token, body.terms)
        .await?;
    Ok(Json(CommandView::build(&result, now())))
}

#[derive(Debug, Deserialize)]
pub struct ConfirmBody {
    /// The revision the party actually looked at. A mismatch is a 409, never a
    /// silent confirmation of something that changed underneath them.
    pub revision: u32,
    /// Set when this device also holds the other party's credentials.
    #[serde(default)]
    pub same_device: bool,
}

async fn confirm_terms(
    State(st): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ConfirmBody>,
) -> ApiResult<Json<CommandView>> {
    let token = bearer(&headers)?;
    let result = st
        .handshake
        .confirm_terms(deal_id(&id)?, &token, body.revision, body.same_device)
        .await?;
    Ok(Json(CommandView::build(&result, now())))
}

async fn fund(
    State(st): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<CommandView>> {
    let token = bearer(&headers)?;
    let result = st.handshake.fund(deal_id(&id)?, &token).await?;
    Ok(Json(CommandView::build(&result, now())))
}

#[derive(Debug, Deserialize)]
pub struct HandoffBody {
    #[serde(default)]
    pub images: Vec<InlineImage>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct InlineImage {
    pub media_type: String,
    pub data_b64: String,
}

async fn submit_handoff(
    State(st): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<HandoffBody>,
) -> ApiResult<Json<CommandView>> {
    let token = bearer(&headers)?;

    let mut images = Vec::with_capacity(body.images.len());
    for img in body.images {
        if !matches!(
            img.media_type.as_str(),
            "image/jpeg" | "image/png" | "image/webp" | "image/gif"
        ) {
            return Err(ApiError(AppError::Invalid(format!(
                "unsupported image type {}",
                img.media_type
            ))));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(img.data_b64.as_bytes())
            .map_err(|_| ApiError(AppError::Invalid("image was not valid base64".into())))?;
        images.push(ImageBytes {
            media_type: img.media_type,
            bytes,
        });
    }

    let result = st
        .handshake
        .submit_handoff_proof(deal_id(&id)?, &token, images, body.note)
        .await?;
    Ok(Json(CommandView::build(&result, now())))
}

async fn confirm_receipt(
    State(st): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<CommandView>> {
    let token = bearer(&headers)?;
    let result = st.handshake.confirm_receipt(deal_id(&id)?, &token).await?;
    Ok(Json(CommandView::build(&result, now())))
}

async fn release_now(
    State(st): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<CommandView>> {
    let token = bearer(&headers)?;
    let result = st.handshake.release_now(deal_id(&id)?, &token).await?;
    Ok(Json(CommandView::build(&result, now())))
}

#[derive(Debug, Deserialize)]
pub struct ReasonBody {
    pub reason: String,
}

async fn open_dispute(
    State(st): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ReasonBody>,
) -> ApiResult<Json<CommandView>> {
    let token = bearer(&headers)?;
    let result = st
        .handshake
        .open_dispute(deal_id(&id)?, &token, body.reason)
        .await?;
    Ok(Json(CommandView::build(&result, now())))
}

async fn cancel(
    State(st): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ReasonBody>,
) -> ApiResult<Json<CommandView>> {
    let token = bearer(&headers)?;
    let result = st
        .handshake
        .cancel(deal_id(&id)?, &token, body.reason)
        .await?;
    Ok(Json(CommandView::build(&result, now())))
}

#[derive(Debug, Deserialize)]
pub struct ResolveBody {
    pub outcome: String,
    pub finding: String,
}

async fn resolve_dispute(
    State(st): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ResolveBody>,
) -> ApiResult<Json<CommandView>> {
    let expected = st
        .mediator_token
        .as_ref()
        .ok_or_else(|| ApiError(AppError::Invalid("mediation is not enabled".into())))?;
    if &bearer(&headers)? != expected {
        return Err(ApiError(AppError::Unauthorized));
    }

    let outcome = match body.outcome.as_str() {
        "release_to_seller" => DisputeOutcome::ReleaseToSeller,
        "refund_to_buyer" => DisputeOutcome::RefundToBuyer,
        "withdrawn" => DisputeOutcome::Withdrawn,
        other => {
            return Err(ApiError(AppError::Invalid(format!(
                "unknown outcome {other}"
            ))))
        }
    };

    let result = st
        .handshake
        .resolve_dispute(deal_id(&id)?, outcome, body.finding)
        .await?;
    Ok(Json(CommandView::build(&result, now())))
}

// ---------------------------------------------------------------------------
// Public receipt — no authentication
// ---------------------------------------------------------------------------

async fn public_receipt(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<th_app::Receipt>> {
    let id = id.trim_end_matches(".json");
    let uuid = Uuid::parse_str(id)
        .map_err(|_| ApiError(AppError::Invalid("malformed receipt id".into())))?;
    Ok(Json(st.handshake.receipt(DealId(uuid)).await?))
}

async fn keys(State(st): State<AppState>) -> Json<KeyDocument> {
    Json(KeyDocument::new(
        st.handshake.signer.key_id(),
        st.handshake.signer.public_key_b64(),
    ))
}

async fn health() -> &'static str {
    "ok"
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/.well-known/true-handshake-keys.json", get(keys))
        // Capture
        .route("/v1/sessions", post(start_session))
        .route("/v1/sessions/{id}", get(get_transcript))
        .route("/v1/sessions/{id}/utterances", post(append_utterances))
        .route("/v1/sessions/{id}/audio", post(attach_audio))
        .route("/v1/sessions/{id}/identify", post(identify_speakers))
        .route("/v1/sessions/{id}/speakers", post(confirm_speakers))
        .route("/v1/sessions/{id}/propose", post(propose))
        // Agreement
        .route("/v1/deals/{id}", get(get_deal))
        .route("/v1/deals/{id}/terms", post(correct_terms))
        .route("/v1/deals/{id}/confirm", post(confirm_terms))
        // Money and goods
        .route("/v1/deals/{id}/fund", post(fund))
        .route("/v1/deals/{id}/handoff", post(submit_handoff))
        .route("/v1/deals/{id}/receipt", post(confirm_receipt))
        .route("/v1/deals/{id}/release", post(release_now))
        .route("/v1/deals/{id}/dispute", post(open_dispute))
        .route("/v1/deals/{id}/cancel", post(cancel))
        .route("/v1/deals/{id}/resolve", post(resolve_dispute))
        // Public
        .route("/v/{id}", get(public_receipt))
        .with_state(state)
}
