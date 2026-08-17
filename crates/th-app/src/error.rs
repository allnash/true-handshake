use th_domain::DomainError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error(transparent)]
    Domain(#[from] DomainError),

    #[error("not found")]
    NotFound,

    #[error("this deal moved while you were looking at it (you had version {expected}, it is now {current})")]
    VersionConflict { expected: u32, current: u32 },

    #[error("not authorized for this deal")]
    Unauthorized,

    #[error("storage: {0}")]
    Storage(String),

    #[error("the witness could not read this conversation: {0}")]
    Witness(String),

    #[error("settlement: {0}")]
    Settlement(String),

    #[error("{0}")]
    Invalid(String),
}

impl AppError {
    /// The HTTP status this maps to. Kept next to the error rather than in the
    /// API crate so a new variant cannot silently default to 500.
    pub fn status_code(&self) -> u16 {
        match self {
            AppError::Domain(DomainError::IllegalTransition { .. }) => 422,
            AppError::Domain(DomainError::WrongRole { .. }) => 403,
            AppError::Domain(DomainError::NotAParticipant { .. }) => 403,
            AppError::Domain(DomainError::StaleTermsRevision { .. }) => 409,
            AppError::Domain(_) => 422,
            AppError::NotFound => 404,
            AppError::VersionConflict { .. } => 409,
            AppError::Unauthorized => 401,
            AppError::Storage(_) => 503,
            AppError::Witness(_) => 502,
            AppError::Settlement(_) => 502,
            AppError::Invalid(_) => 400,
        }
    }

    /// Stable machine-readable code for the RFC 9457 `type` field.
    pub fn code(&self) -> &'static str {
        match self {
            AppError::Domain(DomainError::IllegalTransition { .. }) => "illegal_transition",
            AppError::Domain(DomainError::WrongRole { .. }) => "wrong_role",
            AppError::Domain(DomainError::NotAParticipant { .. }) => "not_a_participant",
            AppError::Domain(DomainError::StaleTermsRevision { .. }) => "stale_terms_revision",
            AppError::Domain(_) => "invalid_terms",
            AppError::NotFound => "not_found",
            AppError::VersionConflict { .. } => "version_conflict",
            AppError::Unauthorized => "unauthorized",
            AppError::Storage(_) => "storage_unavailable",
            AppError::Witness(_) => "witness_unavailable",
            AppError::Settlement(_) => "settlement_unavailable",
            AppError::Invalid(_) => "invalid_request",
        }
    }
}
