//! Argon2id password hashing. Lookup, lockout, and registration policy live in `product`.

use std::sync::OnceLock;

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use thiserror::Error;

static TIMING_DUMMY_HASH: OnceLock<String> = OnceLock::new();

/// Failure from the Argon2id hashing helpers.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PasswordError {
    /// Argon2 could not produce a hash (e.g. an internal/parameter error).
    #[error("password hashing failed")]
    HashFailed,
    /// A stored hash string could not be parsed — corrupt or wrong format.
    #[error("stored password hash is invalid")]
    InvalidHash,
}

/// Hash `password` for storage (Argon2id, random salt).
pub fn hash_password(password: &str) -> Result<String, PasswordError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| PasswordError::HashFailed)
}

/// Returns `true` when `password` matches `encoded_hash`.
pub fn verify_password(encoded_hash: &str, password: &str) -> Result<bool, PasswordError> {
    let parsed = PasswordHash::new(encoded_hash).map_err(|_| PasswordError::InvalidHash)?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

/// Run a verify against a dummy hash — call when the account is absent so the
/// work factor matches a real login attempt.
///
/// The dummy initializes on first use; hashing a constant can only fail on an
/// Argon2 internal error. That failure degrades to a logged no-op burn rather
/// than a request panic — the timing equalization is best-effort hardening,
/// not a correctness gate.
pub fn burn_verify(password: &str) {
    let dummy = TIMING_DUMMY_HASH.get_or_init(|| match hash_password("nestrs-timing-dummy") {
        Ok(hash) => hash,
        Err(error) => {
            tracing::error!(
                target: "nest_rs::authn",
                %error,
                "timing dummy hash failed to initialize — absent-account burn degrades to no-op",
            );
            String::new()
        }
    });
    if !dummy.is_empty() {
        let _ = verify_password(dummy, password);
    }
}
