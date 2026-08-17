//! # True Handshake — application
//!
//! Use cases and the ports they depend on. This crate knows there is a database,
//! a witness, and a settlement provider, but not which ones: everything crossing
//! the boundary is a trait implemented in `th-infra`.

pub mod error;
pub mod ports;
pub mod service;

pub use error::AppError;
pub use ports::*;
pub use service::{role_of, CommandResult, Handshake, Receipt, StartedSession};
