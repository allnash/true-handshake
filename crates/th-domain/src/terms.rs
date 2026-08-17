//! The frozen agreement, and the negotiation ladder that produced it.
//!
//! The ladder is the interesting part. A conventional escrow records "both parties
//! agreed to $40". True Handshake records that Stella asked $50, Nash countered
//! $30, Stella countered $40, and Nash said "we have a deal" — each step attributed
//! to a speaker and anchored to the verbatim words they said. That history is what
//! makes the receipt worth more than a checkbox.

use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::money::Money;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Party {
    Buyer,
    Seller,
}

impl Party {
    pub fn other(self) -> Self {
        match self {
            Party::Buyer => Party::Seller,
            Party::Seller => Party::Buyer,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Party::Buyer => "buyer",
            Party::Seller => "seller",
        }
    }
}

/// What a single step in the negotiation was doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OfferKind {
    /// Background that is not itself a price on the table ("I got it for $80").
    Context,
    /// A price named by the seller.
    Ask,
    /// A price named by the buyer.
    Offer,
    /// A price that responds to a previous one.
    Counter,
    /// The moment of agreement.
    Accept,
}

/// One rung of the negotiation ladder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub seq: u16,
    pub by: Party,
    pub kind: OfferKind,
    /// Absent for `Accept` and for `Context` rungs that name no figure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<Money>,
    /// The words actually spoken, verbatim. This is the evidence; the structured
    /// fields above are our reading of it.
    pub quote: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SettlementMethod {
    /// v1 default: funds held by the platform's mock ledger behind the
    /// SettlementProvider port, released on a timer after receipt.
    Escrow,
    Cash,
    BankTransfer,
    PeerToPeerApp { app: String },
    Other { description: String },
}

impl SettlementMethod {
    pub fn holds_funds(&self) -> bool {
        matches!(self, SettlementMethod::Escrow)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffMethod {
    InPerson,
    Shipped,
    Digital,
}

/// How much the platform observed versus merely recorded a claim about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceTier {
    /// Both parties signed claims; nothing was independently observed.
    Attested,
    /// A payment processor observed the transfer. Unlocked when a real PSP
    /// adapter replaces the mock ledger — no domain change required.
    Observed,
}

/// The agreement, frozen. Once hashed and countersigned this is immutable; a
/// correction produces a new revision before freezing, never after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Terms {
    pub item: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    pub price: Money,
    pub buyer_name: String,
    pub seller_name: String,
    pub settlement: SettlementMethod,
    pub handoff: HandoffMethod,
    /// The full negotiation, in order.
    pub ladder: Vec<Offer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl Terms {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.item.trim().is_empty() {
            return Err(DomainError::Invalid("terms must name an item".into()));
        }
        if self.price.is_zero() {
            return Err(DomainError::NoAgreedPrice);
        }
        if self.buyer_name.trim().is_empty() || self.seller_name.trim().is_empty() {
            return Err(DomainError::Invalid("both parties must be named".into()));
        }
        if self.buyer_name.trim().eq_ignore_ascii_case(self.seller_name.trim()) {
            return Err(DomainError::PartiesNotDistinct);
        }
        Ok(())
    }

    /// A one-line human summary used in confirmation dialogs and receipts.
    pub fn summary(&self) -> String {
        format!(
            "{} buys \"{}\" from {} for {}",
            self.buyer_name, self.item, self.seller_name, self.price
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms() -> Terms {
        Terms {
            item: "Fitbit".into(),
            item_detail: Some("Charge 5, black".into()),
            condition: Some("used".into()),
            price: Money::usd(4000).unwrap(),
            buyer_name: "Nash".into(),
            seller_name: "Stella".into(),
            settlement: SettlementMethod::Escrow,
            handoff: HandoffMethod::InPerson,
            ladder: vec![],
            notes: None,
        }
    }

    #[test]
    fn validates_a_well_formed_agreement() {
        assert!(terms().validate().is_ok());
    }

    #[test]
    fn rejects_zero_price_and_same_party() {
        let mut t = terms();
        t.price = Money::usd(0).unwrap();
        assert_eq!(t.validate(), Err(DomainError::NoAgreedPrice));

        let mut t = terms();
        t.seller_name = "nash".into();
        assert_eq!(t.validate(), Err(DomainError::PartiesNotDistinct));
    }

    #[test]
    fn summary_reads_like_a_sentence() {
        assert_eq!(
            terms().summary(),
            "Nash buys \"Fitbit\" from Stella for $40.00"
        );
    }
}
