use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DomainError {
    #[error("not a valid ISO-4217 currency code: {0}")]
    InvalidCurrency(String),

    #[error("amount cannot be negative: {0}")]
    NegativeAmount(i64),

    #[error("canonical JSON cannot encode a non-integer number ({0}); money must be minor units")]
    NonIntegerNumber(String),

    #[error("illegal transition: cannot apply {command} while deal is {state}")]
    IllegalTransition {
        state: &'static str,
        command: &'static str,
    },

    #[error("actor {actor} is not a participant in this deal")]
    NotAParticipant { actor: String },

    #[error("this command requires the {required} role; actor holds {actual}")]
    WrongRole {
        required: &'static str,
        actual: &'static str,
    },

    #[error("terms revision mismatch: confirmed {confirmed} but current is {current}")]
    StaleTermsRevision { confirmed: u32, current: u32 },

    #[error("the negotiation produced no agreed price")]
    NoAgreedPrice,

    #[error("a deal needs two distinct parties")]
    PartiesNotDistinct,

    #[error("{0}")]
    Invalid(String),
}
