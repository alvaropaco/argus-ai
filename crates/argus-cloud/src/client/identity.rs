//! The installation's asymmetric identity.
//!
//! An installation proves who it is with an Ed25519 key pair: it sends the
//! public key once at enrollment and signs the cloud's per-connection challenge
//! on every connection (ADR-0024). The private seed never leaves the daemon —
//! it is persisted by the caller in the secret store at mode `0600` and is only
//! ever touched through this type.
//!
//! Encodings are fixed by ADR-0024 §3 and are asserted against a cross-language
//! vector so a drift fails here and in the cloud's TypeScript verifier:
//!
//! - public key: `base64url` (no padding) of the 44-byte DER `SubjectPublicKeyInfo`
//! - signature: `base64url` (no padding) of the raw 64-byte Ed25519 signature
//! - signed message: the UTF-8 bytes of the challenge string

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signer, SigningKey};
use rand_core::OsRng;

/// Length of an Ed25519 private seed.
pub const SEED_LEN: usize = 32;

/// Length of the DER `SubjectPublicKeyInfo` for an Ed25519 public key.
pub const SPKI_LEN: usize = 44;

/// DER prefix of an Ed25519 `SubjectPublicKeyInfo` (RFC 8410): a fixed
/// `AlgorithmIdentifier` for id-Ed25519 followed by a 32-byte `BIT STRING`.
const SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// Why a persisted seed could not be turned back into a key.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    #[error("the installation key seed is not valid base64url")]
    NotBase64,

    #[error("the installation key seed must be {SEED_LEN} bytes, got {0}")]
    WrongLength(usize),
}

/// The installation's Ed25519 key pair.
///
/// Deliberately does not derive `Debug`: the seed is secret, and a stray `{:?}`
/// must not be able to write it to a log. Callers may only observe the public
/// key and produce signatures.
pub struct InstallationKey {
    signing: SigningKey,
}

impl InstallationKey {
    /// Generates a fresh key pair from the operating system CSPRNG.
    pub fn generate() -> Self {
        Self {
            signing: SigningKey::generate(&mut OsRng),
        }
    }

    /// Rebuilds a key from its 32-byte seed.
    pub fn from_seed_bytes(seed: &[u8; SEED_LEN]) -> Self {
        Self {
            signing: SigningKey::from_bytes(seed),
        }
    }

    /// Rebuilds a key from the base64url seed form the secret store holds.
    pub fn from_seed_b64(seed_b64: &str) -> Result<Self, IdentityError> {
        let decoded = URL_SAFE_NO_PAD
            .decode(seed_b64.trim())
            .map_err(|_| IdentityError::NotBase64)?;
        let seed: [u8; SEED_LEN] = decoded
            .as_slice()
            .try_into()
            .map_err(|_| IdentityError::WrongLength(decoded.len()))?;
        Ok(Self::from_seed_bytes(&seed))
    }

    /// The seed, base64url, for persisting in the secret store.
    ///
    /// Named for what it exposes: reading secret key material.
    pub fn seed_b64(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.signing.to_bytes())
    }

    /// The public key, base64url DER, as sent to the cloud.
    pub fn public_key_b64(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.spki_der())
    }

    /// Signs the cloud's challenge, returning the base64url signature.
    ///
    /// Ed25519 is PureEdDSA: the message is signed directly, with no prehash.
    pub fn sign_challenge(&self, challenge: &str) -> String {
        URL_SAFE_NO_PAD.encode(self.signing.sign(challenge.as_bytes()).to_bytes())
    }

    fn spki_der(&self) -> [u8; SPKI_LEN] {
        let mut der = [0u8; SPKI_LEN];
        der[..SPKI_PREFIX.len()].copy_from_slice(&SPKI_PREFIX);
        der[SPKI_PREFIX.len()..].copy_from_slice(&self.signing.verifying_key().to_bytes());
        der
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::messages::{
        MAX_CHALLENGE_SIGNATURE_LEN, MAX_PUBLIC_KEY_LEN, MIN_CHALLENGE_SIGNATURE_LEN,
        MIN_PUBLIC_KEY_LEN,
    };
    use ed25519_dalek::{Signature, Verifier};

    /// Pinned by ADR-0024 §"Cross-Language Test Vector"; the TypeScript verifier
    /// asserts the same values.
    const VECTOR_SEED: [u8; SEED_LEN] = [7u8; SEED_LEN];
    const VECTOR_CHALLENGE: &str = "test-challenge-nonce";
    const VECTOR_PUBLIC_KEY: &str = "MCowBQYDK2VwAyEA6kpsY-KcUgq-9VB7Ey7F-ZVHdq6-vnuSQh7qaRRG0iw";
    const VECTOR_SIGNATURE: &str =
        "ub_IfDKsfEELPjLDfWLy9xzCJIrbJzD3jZN2KZEbXT8kwS079k76yWLqrJWFPd2vQSEdgf3-tbskRdB6lvFoBQ";

    fn vector_key() -> InstallationKey {
        InstallationKey::from_seed_bytes(&VECTOR_SEED)
    }

    #[test]
    fn the_cross_language_vector_reproduces_exactly() {
        let key = vector_key();
        assert_eq!(key.public_key_b64(), VECTOR_PUBLIC_KEY);
        assert_eq!(key.sign_challenge(VECTOR_CHALLENGE), VECTOR_SIGNATURE);
    }

    #[test]
    fn the_vector_signature_verifies_and_a_tampered_message_does_not() {
        let key = vector_key();
        let signature: [u8; 64] = URL_SAFE_NO_PAD
            .decode(VECTOR_SIGNATURE)
            .unwrap()
            .try_into()
            .unwrap();

        assert!(
            key.signing
                .verifying_key()
                .verify(
                    VECTOR_CHALLENGE.as_bytes(),
                    &Signature::from_bytes(&signature)
                )
                .is_ok()
        );
        assert!(
            key.signing
                .verifying_key()
                .verify(b"some-other-challenge", &Signature::from_bytes(&signature))
                .is_err()
        );
    }

    #[test]
    fn the_seed_round_trips_through_its_persisted_form() {
        let key = vector_key();
        let rebuilt = InstallationKey::from_seed_b64(&key.seed_b64()).expect("valid seed");
        assert_eq!(rebuilt.public_key_b64(), key.public_key_b64());
    }

    #[test]
    fn a_generated_key_is_well_formed_and_self_consistent() {
        let key = InstallationKey::generate();
        assert_eq!(key.spki_der().len(), SPKI_LEN);
        assert_eq!(&key.spki_der()[..SPKI_PREFIX.len()], &SPKI_PREFIX);
        assert_eq!(
            InstallationKey::from_seed_b64(&key.seed_b64())
                .unwrap()
                .public_key_b64(),
            key.public_key_b64()
        );
    }

    #[test]
    fn the_persisted_seed_is_distinct_from_the_public_key() {
        let key = vector_key();
        assert_eq!(
            URL_SAFE_NO_PAD.decode(key.seed_b64()).unwrap().len(),
            SEED_LEN
        );
        assert_ne!(
            key.seed_b64(),
            key.public_key_b64(),
            "the persisted seed must never be the public key"
        );
    }

    #[test]
    fn a_bad_seed_is_rejected() {
        assert!(matches!(
            InstallationKey::from_seed_b64("!!!not base64!!!"),
            Err(IdentityError::NotBase64)
        ));
        assert!(matches!(
            InstallationKey::from_seed_b64(&URL_SAFE_NO_PAD.encode([1u8; 16])),
            Err(IdentityError::WrongLength(16))
        ));
    }

    #[test]
    fn the_wire_values_fit_the_protocol_bounds() {
        let key = vector_key();
        let public = key.public_key_b64();
        let signature = key.sign_challenge(VECTOR_CHALLENGE);

        assert!(public.len() >= MIN_PUBLIC_KEY_LEN && public.len() <= MAX_PUBLIC_KEY_LEN);
        assert!(
            signature.len() >= MIN_CHALLENGE_SIGNATURE_LEN
                && signature.len() <= MAX_CHALLENGE_SIGNATURE_LEN
        );
    }
}
