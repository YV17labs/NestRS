//! Argon2id password hashing. Lookup, lockout, and registration policy live in `product`.

use std::sync::OnceLock;

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use thiserror::Error;

static TIMING_DUMMY_HASH: OnceLock<String> = OnceLock::new();

#[derive(Debug, Error)]
pub enum PasswordError {
    #[error("password hashing failed")]
    HashFailed,
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
pub fn burn_verify(password: &str) {
    let dummy = TIMING_DUMMY_HASH
        .get_or_init(|| hash_password("nestrs-timing-dummy").expect("dummy hash initializes once"));
    let _ = verify_password(dummy, password);
}
