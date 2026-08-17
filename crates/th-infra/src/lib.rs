//! # True Handshake — infrastructure
//!
//! Concrete implementations of the ports in `th-app`: PostgreSQL repositories
//! and the durable timer queue, the Claude-backed witness, the mock escrow
//! ledger, and Ed25519 signing.
//!
//! Nothing above this crate names any of these types. `th-api` wires them once
//! at startup and then talks only to traits.

pub mod claude;
pub mod offline_witness;
pub mod pg;
pub mod settlement;
pub mod signer;

pub use claude::ClaudeWitness;
pub use offline_witness::OfflineWitness;
pub use pg::{connect, migrate, PgAudioStore, PgDealRepo, PgProofStore, PgSessionRepo, PgTaskQueue};
pub use settlement::MockEscrow;
pub use signer::Ed25519Signer;
