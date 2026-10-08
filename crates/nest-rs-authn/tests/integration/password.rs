//! Covers `src/password.rs`.

use nest_rs_authn::{PasswordError, burn_verify, hash_password, verify_password};

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
        Err(PasswordError::InvalidHash(_))
    ));
}

/// A PHC string that parses but names a hash this hasher cannot run is an
/// unusable record, not a wrong password: `Ok(false)` would lock its owner out
/// with nothing in the logs to say why.
#[test]
fn a_parsable_hash_the_hasher_cannot_run_is_invalid_not_a_mismatch() {
    let scrypt =
        "$scrypt$ln=16,r=8,p=1$aM15713r3Xsvxbi31lqr1Q$nFNh2CVHVjNldFVKDHDlm4CbdRSCdEBsjjJxD+iCs5E";
    let error = verify_password(scrypt, "password").expect_err("an scrypt hash cannot be verified");
    assert!(
        matches!(error, PasswordError::InvalidHash(_)),
        "got {error:?}"
    );
    assert!(
        std::error::Error::source(&error).is_some(),
        "the cause says why the record is unusable",
    );
}

#[test]
fn burn_verify_runs_without_panic() {
    burn_verify("any-password");
}

#[test]
fn hash_pins_argon2id() {
    // A PHC string leads with its algorithm id: a switch away from Argon2id
    // surfaces here.
    let encoded = hash_password("pin-the-algorithm").expect("hash");
    assert!(
        encoded.starts_with("$argon2id$"),
        "stored password hashes must be Argon2id, got: {encoded}",
    );
}
