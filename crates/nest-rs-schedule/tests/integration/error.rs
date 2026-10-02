//! `OccurrenceLockError` carries a backend's failure as a link the scheduler's
//! line can read: a decode failure in it is said without its value, even when
//! its sentence is the decoder's own words, which only the exact reading of the
//! chain reaches.

use nest_rs_core::serde::de::Error as _;
use nest_rs_core::serde::de::value::Error as ValueError;
use nest_rs_schedule::OccurrenceLockError;

const SECRET: &str = "sk_live_51HsecretTOKEN";

/// A lease record that did not decode, refused in a type's own words — the
/// shape no reading of serde's wording recognises.
fn refused() -> ValueError {
    ValueError::custom(format!("lease token {SECRET} is not ours"))
}

#[test]
fn a_backend_s_decode_failure_is_said_without_its_value() {
    let said = nest_rs_core::error_message(&OccurrenceLockError::new(refused()));
    assert!(!said.contains(SECRET), "{said}");
}

/// The documented backend shape: `anyhow::Result` and `?`. anyhow's own box
/// hides the error it holds from `source()`, so the lock error has to be built
/// from the error itself for the line to find it.
#[test]
fn a_decode_failure_in_an_anyhow_chain_is_said_without_its_value() {
    let said =
        nest_rs_core::error_message(&OccurrenceLockError::new(anyhow::Error::new(refused())));
    assert!(!said.contains(SECRET), "{said}");

    let said = nest_rs_core::error_message(&OccurrenceLockError::new(
        anyhow::Error::new(refused()).context("reading the lease"),
    ));
    assert!(!said.contains(SECRET), "{said}");
    assert!(said.starts_with("reading the lease: "), "{said}");
}
