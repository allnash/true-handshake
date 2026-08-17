//! Money is integer minor units plus an ISO-4217 currency code. There are no
//! floats anywhere in this system: a float in the terms would make the canonical
//! JSON encoding implementation-dependent, and the whole receipt rests on two
//! independent implementations hashing to the same bytes.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::error::DomainError;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Money {
    /// ISO 4217, uppercase.
    pub currency: String,
    /// Minor units (cents for USD). Never negative in a deal.
    pub minor_units: i64,
}

impl Money {
    pub fn new(currency: impl Into<String>, minor_units: i64) -> Result<Self, DomainError> {
        let currency = currency.into().to_uppercase();
        if currency.len() != 3 || !currency.chars().all(|c| c.is_ascii_uppercase()) {
            return Err(DomainError::InvalidCurrency(currency));
        }
        if minor_units < 0 {
            return Err(DomainError::NegativeAmount(minor_units));
        }
        Ok(Self {
            currency,
            minor_units,
        })
    }

    pub fn usd(minor_units: i64) -> Result<Self, DomainError> {
        Self::new("USD", minor_units)
    }

    pub fn is_zero(&self) -> bool {
        self.minor_units == 0
    }

    /// Public receipts show a band, never the exact figure, unless both parties
    /// opt in. This is the band computation.
    pub fn band(&self) -> String {
        let major = self.minor_units / 100;
        let (lo, hi) = match major {
            0..=24 => (0, 25),
            25..=49 => (25, 50),
            50..=99 => (50, 100),
            100..=249 => (100, 250),
            250..=499 => (250, 500),
            500..=999 => (500, 1_000),
            1_000..=4_999 => (1_000, 5_000),
            _ => (5_000, i64::MAX),
        };
        if hi == i64::MAX {
            format!("{}5,000+", symbol(&self.currency))
        } else {
            format!("{}{}–{}{}", symbol(&self.currency), lo, symbol(&self.currency), hi)
        }
    }
}

fn symbol(currency: &str) -> &'static str {
    match currency {
        "USD" => "$",
        "EUR" => "€",
        "GBP" => "£",
        _ => "",
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let major = self.minor_units / 100;
        let minor = (self.minor_units % 100).abs();
        write!(f, "{}{}.{:02}", symbol(&self.currency), major, minor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_currency_and_negatives() {
        assert!(Money::new("usdd", 100).is_err());
        assert!(Money::new("US", 100).is_err());
        assert!(Money::new("USD", -1).is_err());
        assert_eq!(Money::new("usd", 4000).unwrap().currency, "USD");
    }

    #[test]
    fn formats_and_bands() {
        let m = Money::usd(4000).unwrap();
        assert_eq!(m.to_string(), "$40.00");
        assert_eq!(m.band(), "$25–$50");
        assert_eq!(Money::usd(500_000).unwrap().band(), "$5,000+");
    }
}
