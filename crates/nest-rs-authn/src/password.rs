//! Argon2id password hashing. Lookup, lockout and registration policy belong to
//! whoever owns the user table, not here.

use std::sync::OnceLock;

use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};

use crate::error::PasswordError;

static TIMING_DUMMY_HASH: OnceLock<String> = OnceLock::new();

// The work factor, pinned rather than inherited. Source: the OWASP Password
// Storage Cheat Sheet's Argon2id recommendation — m = 19 MiB, t = 2, p = 1 —
// which `Argon2::default()` happens to equal today. Riding that default put a
// security-critical parameter on a value an upstream release can lower without
// a line changing here, which is the same argument `JwtService::new` makes for
// pinning `validate_exp`. Pinned, a downstream change is a visible diff.
/// Memory cost in KiB (19 MiB).
const ARGON2_M_COST: u32 = 19 * 1024;
/// Iteration count.
const ARGON2_T_COST: u32 = 2;
/// Degree of parallelism.
const ARGON2_P_COST: u32 = 1;

/// The pinned Argon2id hasher. `Params::new` validates the three constants
/// above, so this can only fail if one of them is edited out of range — a
/// `HashFailed` rather than a panic, since nothing here may `unwrap` on a hot
/// path.
fn argon2() -> Result<Argon2<'static>, PasswordError> {
    let params = Params::new(ARGON2_M_COST, ARGON2_T_COST, ARGON2_P_COST, None)
        .map_err(|_| PasswordError::HashFailed)?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

/// Hash `password` for storage (Argon2id, random salt, the pinned OWASP work
/// factor).
pub fn hash_password(password: &str) -> Result<String, PasswordError> {
    let salt = SaltString::generate(&mut OsRng);
    argon2()?
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| PasswordError::HashFailed)
}

/// Returns `true` when `password` matches `encoded_hash`.
///
/// Verification reads the parameters **out of the stored hash**, not from the
/// pinned ones — which is what lets [`ARGON2_M_COST`] and its siblings move
/// without invalidating every credential already in the database.
pub fn verify_password(encoded_hash: &str, password: &str) -> Result<bool, PasswordError> {
    let parsed = PasswordHash::new(encoded_hash).map_err(|_| PasswordError::InvalidHash)?;
    Ok(argon2()?
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
                target: crate::TARGET,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_work_factor_is_pinned_at_the_owasp_recommendation() {
        // The values, asserted where a downstream `argon2` release cannot move
        // them: OWASP Password Storage Cheat Sheet, Argon2id — m = 19 MiB
        // (19456 KiB), t = 2, p = 1.
        let hasher = argon2().expect("the pinned parameters are in range");
        assert_eq!(hasher.params().m_cost(), 19_456);
        assert_eq!(hasher.params().t_cost(), 2);
        assert_eq!(hasher.params().p_cost(), 1);
    }

    #[test]
    fn a_stored_hash_records_the_pinned_parameters() {
        // The proof that reaches the database: a PHC string carries the cost
        // parameters it was produced with, so this is what a later reader (and
        // `verify_password`) actually sees.
        let encoded = hash_password("correct horse battery staple").expect("hash");
        assert!(
            encoded.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
            "the stored hash names Argon2id and the pinned work factor: {encoded}",
        );
    }

    #[test]
    fn a_hash_made_with_another_work_factor_still_verifies() {
        // Verification reads the stored parameters, so pinning ours never
        // invalidates credentials hashed before the pin.
        let salt = SaltString::generate(&mut OsRng);
        let weaker = Argon2::new(
            Algorithm::Argon2id,
            Version::V0x13,
            Params::new(8 * 1024, 1, 1, None).expect("params"),
        )
        .hash_password(b"legacy", &salt)
        .expect("hash")
        .to_string();
        assert!(verify_password(&weaker, "legacy").expect("verify"));
        assert!(!verify_password(&weaker, "wrong").expect("verify"));
    }
}
