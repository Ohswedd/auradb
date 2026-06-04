//! Static-token authentication primitives.
//!
//! Tokens are never stored or compared in plaintext. A token is hashed with
//! Argon2id into a PHC string (which embeds the algorithm, parameters, and a
//! random salt), and verification recomputes the hash with Argon2's
//! constant-time comparison. This module is used both by the server (to verify
//! client tokens) and by the CLI (`auradb auth hash-token`).

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use auradb_core::{Error, Result};
use rand::rngs::OsRng;

/// Hash a token with Argon2id, returning a PHC string that embeds the random
/// salt and parameters. The returned string is safe to store in configuration.
pub fn hash_token(token: &str) -> Result<String> {
    if token.is_empty() {
        return Err(Error::InvalidRequest("token must not be empty".into()));
    }
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(token.as_bytes(), &salt)
        .map_err(|e| Error::Internal(format!("argon2 hashing failed: {e}")))?;
    Ok(hash.to_string())
}

/// Verify a candidate token against a stored Argon2 PHC hash.
///
/// Returns `Ok(true)` on a match, `Ok(false)` on a mismatch, and `Err` only if
/// the stored hash is malformed. The comparison is constant-time. Neither the
/// token nor the hash is logged or included in error messages.
pub fn verify_token(stored_hash: &str, candidate: &str) -> Result<bool> {
    let parsed = PasswordHash::new(stored_hash)
        .map_err(|_| Error::Config("stored token hash is malformed".into()))?;
    match Argon2::default().verify_password(candidate.as_bytes(), &parsed) {
        Ok(()) => Ok(true),
        Err(argon2::password_hash::Error::Password) => Ok(false),
        Err(e) => Err(Error::Internal(format!("argon2 verification failed: {e}"))),
    }
}

/// Validate that a string is a well-formed Argon2 PHC hash. Used at config
/// validation time so the server fails closed on a malformed token hash rather
/// than rejecting every client at runtime.
pub fn validate_hash(stored_hash: &str) -> Result<()> {
    let parsed = PasswordHash::new(stored_hash)
        .map_err(|_| Error::Config("auth.token_hash is not a valid PHC hash".into()))?;
    if !parsed.algorithm.as_str().starts_with("argon2") {
        return Err(Error::Config(format!(
            "auth.token_hash uses unsupported algorithm '{}', expected argon2id",
            parsed.algorithm
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_not_plaintext() {
        let hash = hash_token("super-secret").unwrap();
        assert!(!hash.contains("super-secret"));
        assert!(hash.starts_with("$argon2id$"));
    }

    #[test]
    fn valid_token_verifies() {
        let hash = hash_token("correct horse battery staple").unwrap();
        assert!(verify_token(&hash, "correct horse battery staple").unwrap());
    }

    #[test]
    fn invalid_token_rejected() {
        let hash = hash_token("the-right-token").unwrap();
        assert!(!verify_token(&hash, "the-wrong-token").unwrap());
    }

    #[test]
    fn distinct_hashes_for_same_token() {
        // Random salts mean the same token hashes to different PHC strings.
        let a = hash_token("same").unwrap();
        let b = hash_token("same").unwrap();
        assert_ne!(a, b);
        assert!(verify_token(&a, "same").unwrap());
        assert!(verify_token(&b, "same").unwrap());
    }

    #[test]
    fn malformed_hash_is_error() {
        assert!(verify_token("not-a-hash", "x").is_err());
        assert!(validate_hash("not-a-hash").is_err());
    }

    #[test]
    fn empty_token_rejected() {
        assert!(hash_token("").is_err());
    }
}
