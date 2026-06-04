//! Covers `src/password/hash.rs`.

use nestrs_authn::{PasswordError, burn_verify, hash_password, verify_password};

#[test]
fn hash_and_verify_round_trip() {
    let encoded = hash_password("correct-horse-battery-staple").expect("hash");
    assert!(verify_password(&encoded, "correct-horse-battery-staple").expect("verify"));
    assert!(!verify_password(&encoded, "wrong").expect("verify"));
}

#[test]
fn invalid_stored_hash_returns_error() {
    assert!(matches!(
        verify_password("not-a-phc-string", "password"),
        Err(PasswordError::InvalidHash)
    ));
}

#[test]
fn burn_verify_runs_without_panic() {
    burn_verify("any-password");
}
