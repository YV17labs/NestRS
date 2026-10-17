//! `OccurrenceLockError` carries a backend's failure as a link the scheduler's
//! line can read, so a decode failure in it is said without its value.

use nest_rs_core::__private::serde::de::Error as _;
use nest_rs_core::__private::serde::de::value::Error as ValueError;
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

/// anyhow's own box hides the error it holds from `source()`.
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
