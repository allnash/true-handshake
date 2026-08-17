//! Response shapes.
//!
//! Two things every deal response carries, both load-bearing for the UI:
//!
//! * `server_time` — the client anchors its countdowns to this and tracks the
//!   offset, so a phone with a wrong clock still shows the right time remaining
//!   and never decides on its own that a window closed.
//! * `version` — the client sends it back on the next command, so a stale tab
//!   gets a 409 with the current state instead of silently applying to a deal
//!   that moved.

use serde::Serialize;
use th_app::DealRecord;
use th_domain::{Attestation, DomainEvent, Party, Terms};
use time::OffsetDateTime;

fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[derive(Debug, Serialize)]
pub struct TimelineEntry {
    pub at: String,
    pub kind: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct ChainLink {
    pub seq: u32,
    pub action: String,
    pub actor: serde_json::Value,
    pub at: String,
    pub chain_hash: String,
}

#[derive(Debug, Serialize)]
pub struct DealView {
    pub deal_id: String,
    pub state: String,
    pub version: u32,
    pub terms_revision: u32,

    /// Which side the caller is on.
    pub your_role: String,
    pub buyer_name: String,
    pub seller_name: String,

    pub terms: Option<Terms>,
    pub terms_hash: Option<String>,
    pub summary: Option<String>,

    pub you_confirmed: bool,
    pub they_confirmed: bool,

    /// What the witness could not resolve, surfaced verbatim so the parties fix
    /// it rather than discovering it later.
    pub ambiguities: Vec<String>,
    pub witness_confidence: Option<String>,

    pub release_due_at: Option<String>,
    pub receipt_auto_confirmed: bool,
    pub evidence_tier: String,

    pub timeline: Vec<TimelineEntry>,
    pub chain: Vec<ChainLink>,

    pub server_time: String,
}

impl DealView {
    pub fn build(
        record: &DealRecord,
        role: Party,
        events: Vec<(OffsetDateTime, DomainEvent)>,
        attestations: &[Attestation],
        now: OffsetDateTime,
    ) -> Self {
        let deal = &record.deal;
        let (you_confirmed, they_confirmed) = match role {
            Party::Buyer => (
                deal.confirmed_by(Party::Buyer),
                deal.confirmed_by(Party::Seller),
            ),
            Party::Seller => (
                deal.confirmed_by(Party::Seller),
                deal.confirmed_by(Party::Buyer),
            ),
        };

        // The witness's caveats live in the payload of the proposal attestation.
        let proposal = attestations
            .iter()
            .find(|a| a.action == th_domain::AttestationAction::WitnessProposed);
        let ambiguities = proposal
            .and_then(|a| a.payload.get("extraction"))
            .and_then(|e| e.get("ambiguities"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let witness_confidence = proposal
            .and_then(|a| a.payload.get("extraction"))
            .and_then(|e| e.get("confidence"))
            .and_then(|v| v.as_str())
            .map(String::from);

        Self {
            deal_id: deal.id.to_string(),
            state: deal.state.as_str().to_string(),
            version: deal.version,
            terms_revision: deal.terms_revision,
            your_role: role.as_str().to_string(),
            buyer_name: record.parties.buyer_name.clone(),
            seller_name: record.parties.seller_name.clone(),
            summary: deal.terms.as_ref().map(|t| t.summary()),
            terms: deal.terms.clone(),
            terms_hash: deal.terms_hash.clone(),
            you_confirmed,
            they_confirmed,
            ambiguities,
            witness_confidence,
            release_due_at: deal.release_due_at.map(rfc3339),
            receipt_auto_confirmed: deal.receipt_auto_confirmed,
            evidence_tier: match deal.evidence_tier {
                th_domain::EvidenceTier::Attested => "attested".into(),
                th_domain::EvidenceTier::Observed => "observed".into(),
            },
            timeline: events
                .into_iter()
                .map(|(at, e)| TimelineEntry {
                    at: rfc3339(at),
                    kind: e.kind,
                    payload: e.payload,
                })
                .collect(),
            chain: attestations
                .iter()
                .map(|a| ChainLink {
                    seq: a.seq,
                    action: a.action.as_str().to_string(),
                    actor: serde_json::to_value(a.actor).unwrap_or(serde_json::Value::Null),
                    at: rfc3339(a.at),
                    chain_hash: a.chain_hash.clone(),
                })
                .collect(),
            server_time: rfc3339(now),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct TranscriptView {
    pub session_id: String,
    pub deal_id: String,
    pub closed: bool,
    pub utterances: Vec<th_domain::Utterance>,
    pub server_time: String,
}

#[derive(Debug, Serialize)]
pub struct StartedView {
    pub session_id: String,
    pub deal_id: String,
    pub buyer_token: String,
    pub seller_token: String,
    /// Ready-made links: one for each phone.
    pub buyer_link: String,
    pub seller_link: String,
}

#[derive(Debug, Serialize)]
pub struct CommandView {
    pub deal_id: String,
    pub state: String,
    pub version: u32,
    /// Proof the chain recorded this, returned on every mutating response.
    pub attestation_id: String,
    pub chain_hash: String,
    pub server_time: String,
}

impl CommandView {
    pub fn build(result: &th_app::CommandResult, now: OffsetDateTime) -> Self {
        Self {
            deal_id: result.deal.id.to_string(),
            state: result.deal.state.as_str().to_string(),
            version: result.deal.version,
            attestation_id: result.attestation_id.to_string(),
            chain_hash: result.chain_hash.clone(),
            server_time: rfc3339(now),
        }
    }
}

/// The published key document. Everything needed to verify a receipt without
/// holding an account or asking us anything.
#[derive(Debug, Serialize)]
pub struct KeyDocument {
    pub keys: Vec<PublishedKey>,
    pub algorithm: &'static str,
    pub canonicalization: &'static str,
    pub signing_domain: &'static str,
    pub genesis_domain: &'static str,
    pub procedure: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct PublishedKey {
    pub key_id: String,
    pub public_key_b64: String,
}

impl KeyDocument {
    pub fn new(key_id: String, public_key_b64: String) -> Self {
        Self {
            keys: vec![PublishedKey {
                key_id,
                public_key_b64,
            }],
            algorithm: "Ed25519",
            canonicalization: "RFC 8785 (JCS), integers only",
            signing_domain: th_domain::chain::SIGNATURE_DOMAIN,
            genesis_domain: th_domain::chain::GENESIS_DOMAIN,
            procedure: vec![
                "payload_hash[n] = hex(SHA-256(canonical_json(payload[n])))",
                "prev_chain_hash[0] = hex(SHA-256(genesis_domain || deal_id))",
                "prev_chain_hash[n] = chain_hash[n-1]",
                "chain_hash[n] = hex(SHA-256(prev_chain_hash[n] || payload_hash[n]))",
                "verify Ed25519(public_key, signing_domain || chain_hash[n], signature[n])",
            ],
        }
    }
}
