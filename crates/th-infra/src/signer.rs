//! Ed25519 signing for the attestation chain.
//!
//! In production the private key belongs in a KMS or HSM, with only the digest
//! crossing the boundary. This implementation holds the key in process, which is
//! the honest shape for local development and explicitly not the shape for
//! anything holding real money — a compromised process here can forge history.
//!
//! The public half is published at `/.well-known/true-handshake-keys.json` so
//! anyone can verify a receipt without asking us anything.

use base64::Engine;
use ed25519_dalek::{Signer as _, SigningKey};
use sha2::{Digest, Sha256};
use th_app::Signer;

pub struct Ed25519Signer {
    key: SigningKey,
    key_id: String,
}

impl Ed25519Signer {
    /// Load from a base64 32-byte seed, or mint a fresh one.
    ///
    /// A generated key means previously issued receipts stop verifying, so this
    /// warns loudly rather than doing it silently.
    pub fn from_seed_b64(seed: Option<&str>) -> anyhow::Result<Self> {
        let key = match seed {
            Some(s) if !s.trim().is_empty() => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(s.trim())
                    .map_err(|e| anyhow::anyhow!("signing seed is not valid base64: {e}"))?;
                let arr: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
                    anyhow::anyhow!("signing seed must be exactly 32 bytes, got {}", bytes.len())
                })?;
                SigningKey::from_bytes(&arr)
            }
            _ => {
                tracing::warn!(
                    "no TH_SIGNING_SEED set; generating an ephemeral key. Receipts signed \
                     by this process will not verify after a restart."
                );
                SigningKey::generate(&mut rand::rngs::OsRng)
            }
        };

        let key_id = Self::derive_key_id(&key);
        Ok(Self { key, key_id })
    }

    /// Deterministic from the public key, so the same key always has the same id.
    fn derive_key_id(key: &SigningKey) -> String {
        let digest = Sha256::digest(key.verifying_key().as_bytes());
        format!(
            "th-{}",
            digest
                .iter()
                .take(8)
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        )
    }

    /// Print a fresh seed for operators to paste into their environment.
    pub fn generate_seed_b64() -> String {
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        base64::engine::general_purpose::STANDARD.encode(key.to_bytes())
    }
}

impl Signer for Ed25519Signer {
    fn key_id(&self) -> String {
        self.key_id.clone()
    }

    fn sign(&self, message: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.key.sign(message).to_bytes())
    }

    fn public_key_b64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.key.verifying_key().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    use th_domain::chain;

    #[test]
    fn a_signature_verifies_against_the_published_public_key() {
        let seed = Ed25519Signer::generate_seed_b64();
        let signer = Ed25519Signer::from_seed_b64(Some(&seed)).unwrap();

        let message = chain::signing_message("a3f1c2");
        let sig_b64 = signer.sign(&message);

        // Exactly what a third-party verifier does, with nothing but the
        // receipt and the well-known key document.
        let pk_bytes = base64::engine::general_purpose::STANDARD
            .decode(signer.public_key_b64())
            .unwrap();
        let vk = VerifyingKey::from_bytes(&pk_bytes.try_into().unwrap()).unwrap();
        let sig_bytes: [u8; 64] = base64::engine::general_purpose::STANDARD
            .decode(&sig_b64)
            .unwrap()
            .try_into()
            .unwrap();

        assert!(vk.verify(&message, &Signature::from_bytes(&sig_bytes)).is_ok());
        // And a signature over a different chain hash must not verify.
        assert!(vk
            .verify(
                &chain::signing_message("different"),
                &Signature::from_bytes(&sig_bytes)
            )
            .is_err());
    }

    #[test]
    fn the_same_seed_always_yields_the_same_key_id() {
        let seed = Ed25519Signer::generate_seed_b64();
        let a = Ed25519Signer::from_seed_b64(Some(&seed)).unwrap();
        let b = Ed25519Signer::from_seed_b64(Some(&seed)).unwrap();
        assert_eq!(a.key_id(), b.key_id());
        assert_eq!(a.public_key_b64(), b.public_key_b64());
    }

    #[test]
    fn a_malformed_seed_is_rejected_rather_than_silently_replaced() {
        assert!(Ed25519Signer::from_seed_b64(Some("not base64!!")).is_err());
        assert!(Ed25519Signer::from_seed_b64(Some("c2hvcnQ=")).is_err());
    }
}
