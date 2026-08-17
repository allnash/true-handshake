use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

macro_rules! id_type {
    ($name:ident, $prefix:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
            pub fn as_uuid(&self) -> Uuid {
                self.0
            }
            /// Prefixed form used in URLs and logs, e.g. `deal_2f1c…`.
            pub fn prefixed(&self) -> String {
                format!("{}_{}", $prefix, self.0.simple())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl From<Uuid> for $name {
            fn from(u: Uuid) -> Self {
                Self(u)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

id_type!(DealId, "deal");
id_type!(SessionId, "wsess");
id_type!(AccountId, "acct");
id_type!(AttestationId, "att");
id_type!(ProofId, "proof");
id_type!(DisputeId, "disp");
id_type!(TaskId, "task");

/// A receipt id is just the deal id, rendered for public URLs. Kept as its own
/// type so a receipt link can never be confused with an internal handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReceiptId(pub Uuid);

impl From<DealId> for ReceiptId {
    fn from(d: DealId) -> Self {
        ReceiptId(d.0)
    }
}

impl fmt::Display for ReceiptId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.simple())
    }
}
