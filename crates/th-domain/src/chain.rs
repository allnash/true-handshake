//! The attestation chain: an append-only, hash-linked record of every consent
//! and every state change on one deal.
//!
//! ## The published verification procedure
//!
//! A third party with a receipt JSON, and no access to our database, verifies it
//! like this:
//!
//! ```text
//! payload_hash[n] = hex(SHA-256(canonical_json(payload[n])))
//!
//! prev_chain_hash[0] = hex(SHA-256("true-handshake/v1/genesis:" || deal_id))
//! prev_chain_hash[n] = chain_hash[n-1]
//!
//! chain_hash[n]   = hex(SHA-256(prev_chain_hash[n] || payload_hash[n]))   // ASCII hex concat
//! signature[n]    = Ed25519(platform_key, "true-handshake/v1/attestation:" || chain_hash[n])
//! ```
//!
//! Two deliberate choices there:
//!
//! - **The genesis link is domain-separated by deal id.** Without it, two deals
//!   whose first payloads happened to be identical would produce identical chain
//!   hashes and therefore identical signatures, letting an attestation be
//!   transplanted from one deal to another.
//! - **Hashes are concatenated as lowercase ASCII hex, not raw bytes.** Slightly
//!   wasteful, and deliberately so: an independent verifier in JavaScript,
//!   Python, or Go reimplements string concatenation without a single
//!   byte-order or padding question.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use crate::canonical::{canonical_hash, hex};
use crate::ids::{AttestationId, DealId};
use crate::terms::Party;

pub const GENESIS_DOMAIN: &str = "true-handshake/v1/genesis:";
pub const SIGNATURE_DOMAIN: &str = "true-handshake/v1/attestation:";

/// Who caused an attestation. `System` covers timer-driven transitions — a
/// deadline firing is still a recorded act, not an invisible one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Actor {
    Party { party: Party },
    System,
    Mediator,
}

impl Actor {
    pub fn party(&self) -> Option<Party> {
        match self {
            Actor::Party { party } => Some(*party),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttestationAction {
    /// A witness session produced a reading of the conversation.
    WitnessProposed,
    /// A party edited the reading before confirming.
    TermsCorrected,
    /// A party confirmed the reading as accurate.
    TermsConfirmed,
    /// Both parties confirmed; terms are frozen from here.
    TermsFrozen,
    /// Buyer's funds entered escrow.
    FundsHeld,
    /// Seller submitted evidence of handing the item over.
    HandoffProved,
    /// Buyer confirmed they received it. Starts the release clock.
    ReceiptConfirmed,
    /// Funds moved to the seller.
    FundsReleased,
    /// Funds returned to the buyer.
    FundsRefunded,
    DisputeOpened,
    DisputeResolved,
    Cancelled,
    Expired,
}

impl AttestationAction {
    pub fn as_str(self) -> &'static str {
        use AttestationAction::*;
        match self {
            WitnessProposed => "witness_proposed",
            TermsCorrected => "terms_corrected",
            TermsConfirmed => "terms_confirmed",
            TermsFrozen => "terms_frozen",
            FundsHeld => "funds_held",
            HandoffProved => "handoff_proved",
            ReceiptConfirmed => "receipt_confirmed",
            FundsReleased => "funds_released",
            FundsRefunded => "funds_refunded",
            DisputeOpened => "dispute_opened",
            DisputeResolved => "dispute_resolved",
            Cancelled => "cancelled",
            Expired => "expired",
        }
    }
}

/// What the domain asks the app layer to append. Unhashed and unsigned — the
/// domain has no key material and no clock of its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttestationDraft {
    pub action: AttestationAction,
    pub actor: Actor,
    /// Structured, already free of personal data — this gets hashed into the
    /// public chain. Anything identifying belongs behind a vault reference.
    pub payload: serde_json::Value,
}

impl AttestationDraft {
    pub fn new(action: AttestationAction, actor: Actor, payload: serde_json::Value) -> Self {
        Self {
            action,
            actor,
            payload,
        }
    }
}

/// A sealed link in the chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attestation {
    pub id: AttestationId,
    pub deal_id: DealId,
    pub seq: u32,
    pub action: AttestationAction,
    pub actor: Actor,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub payload: serde_json::Value,
    pub payload_hash: String,
    pub prev_chain_hash: String,
    pub chain_hash: String,
    pub key_id: String,
    /// Base64 (standard alphabet, padded) Ed25519 signature over the signing
    /// message. Absent only in tests that exercise hashing alone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// The first `prev_chain_hash` of a deal's chain.
pub fn genesis_hash(deal_id: DealId) -> String {
    let mut h = Sha256::new();
    h.update(GENESIS_DOMAIN.as_bytes());
    h.update(deal_id.to_string().as_bytes());
    hex(&h.finalize())
}

/// `SHA-256(prev_chain_hash || payload_hash)` over lowercase ASCII hex.
pub fn link_hash(prev_chain_hash: &str, payload_hash: &str) -> String {
    let mut h = Sha256::new();
    h.update(prev_chain_hash.as_bytes());
    h.update(payload_hash.as_bytes());
    hex(&h.finalize())
}

/// The exact bytes the platform key signs. Domain-separated so a True Handshake
/// signature can never be replayed as a signature over anything else.
pub fn signing_message(chain_hash: &str) -> Vec<u8> {
    let mut m = Vec::with_capacity(SIGNATURE_DOMAIN.len() + chain_hash.len());
    m.extend_from_slice(SIGNATURE_DOMAIN.as_bytes());
    m.extend_from_slice(chain_hash.as_bytes());
    m
}

/// Compute the hashes for the next link. The caller supplies the id, clock, and
/// signature; this function owns the part that must match the published spec.
pub fn seal(
    id: AttestationId,
    deal_id: DealId,
    seq: u32,
    prev_chain_hash: &str,
    draft: AttestationDraft,
    at: OffsetDateTime,
    key_id: impl Into<String>,
) -> Result<Attestation, crate::error::DomainError> {
    let payload_hash = canonical_hash(&draft.payload)?;
    let chain_hash = link_hash(prev_chain_hash, &payload_hash);
    Ok(Attestation {
        id,
        deal_id,
        seq,
        action: draft.action,
        actor: draft.actor,
        at,
        payload: draft.payload,
        payload_hash,
        prev_chain_hash: prev_chain_hash.to_string(),
        chain_hash,
        key_id: key_id.into(),
        signature: None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainError {
    OutOfOrder { expected: u32, found: u32 },
    BrokenLink { seq: u32 },
    PayloadTampered { seq: u32 },
    WrongGenesis,
}

impl std::fmt::Display for ChainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChainError::OutOfOrder { expected, found } => {
                write!(f, "attestation out of order: expected seq {expected}, found {found}")
            }
            ChainError::BrokenLink { seq } => {
                write!(f, "chain hash at seq {seq} does not follow from its predecessor")
            }
            ChainError::PayloadTampered { seq } => {
                write!(f, "payload at seq {seq} does not match its recorded hash")
            }
            ChainError::WrongGenesis => write!(f, "chain does not start at this deal's genesis"),
        }
    }
}

impl std::error::Error for ChainError {}

/// Verify hash linkage over a whole chain. Signature checking is separate: it
/// needs the public key, which lives outside the domain.
pub fn verify_chain(deal_id: DealId, chain: &[Attestation]) -> Result<(), ChainError> {
    let mut expected_prev = genesis_hash(deal_id);
    for (i, a) in chain.iter().enumerate() {
        let seq = i as u32;
        if a.seq != seq {
            return Err(ChainError::OutOfOrder {
                expected: seq,
                found: a.seq,
            });
        }
        if i == 0 && a.prev_chain_hash != expected_prev {
            return Err(ChainError::WrongGenesis);
        }
        if a.prev_chain_hash != expected_prev {
            return Err(ChainError::BrokenLink { seq });
        }
        let recomputed_payload =
            canonical_hash(&a.payload).map_err(|_| ChainError::PayloadTampered { seq })?;
        if recomputed_payload != a.payload_hash {
            return Err(ChainError::PayloadTampered { seq });
        }
        if link_hash(&a.prev_chain_hash, &a.payload_hash) != a.chain_hash {
            return Err(ChainError::BrokenLink { seq });
        }
        expected_prev = a.chain_hash.clone();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use time::macros::datetime;

    fn build(deal_id: DealId, n: u32) -> Vec<Attestation> {
        let mut prev = genesis_hash(deal_id);
        let mut out = Vec::new();
        for seq in 0..n {
            let a = seal(
                AttestationId::new(),
                deal_id,
                seq,
                &prev,
                AttestationDraft::new(
                    AttestationAction::TermsConfirmed,
                    Actor::Party {
                        party: Party::Buyer,
                    },
                    json!({ "seq": seq }),
                ),
                datetime!(2026-08-16 12:00:00 UTC),
                "test-key",
            )
            .unwrap();
            prev = a.chain_hash.clone();
            out.push(a);
        }
        out
    }

    #[test]
    fn a_well_formed_chain_verifies() {
        let deal = DealId::new();
        assert!(verify_chain(deal, &build(deal, 4)).is_ok());
    }

    #[test]
    fn genesis_is_domain_separated_by_deal() {
        // Identical first payloads on two deals must not collide, or an
        // attestation could be transplanted between them.
        let a = build(DealId::new(), 1);
        let b = build(DealId::new(), 1);
        assert_eq!(a[0].payload_hash, b[0].payload_hash);
        assert_ne!(a[0].chain_hash, b[0].chain_hash);
    }

    #[test]
    fn tampering_with_a_payload_is_detected() {
        let deal = DealId::new();
        let mut chain = build(deal, 3);
        chain[1].payload = json!({ "seq": 99 });
        assert_eq!(
            verify_chain(deal, &chain),
            Err(ChainError::PayloadTampered { seq: 1 })
        );
    }

    #[test]
    fn removing_a_link_breaks_the_chain() {
        let deal = DealId::new();
        let mut chain = build(deal, 3);
        chain.remove(1);
        assert!(verify_chain(deal, &chain).is_err());
    }

    #[test]
    fn a_chain_from_another_deal_is_rejected() {
        let deal = DealId::new();
        let chain = build(deal, 2);
        assert_eq!(
            verify_chain(DealId::new(), &chain),
            Err(ChainError::WrongGenesis)
        );
    }

    #[test]
    fn signing_message_is_domain_separated() {
        let m = signing_message("abc");
        assert_eq!(m, b"true-handshake/v1/attestation:abc".to_vec());
    }
}
