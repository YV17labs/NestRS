//! Covers `src/algorithm.rs` — the JWS `alg` values a verifier runs, read and
//! said as RFC 7518 spells them.

use nest_rs_authn::{Algorithm, UnknownAlgorithmError};

/// Every `alg` value the verifier runs, as RFC 7518 §3.1 and RFC 8037 §3.1
/// spell them.
const NAMES: [&str; 12] = [
    "HS256", "HS384", "HS512", "RS256", "RS384", "RS512", "PS256", "PS384", "PS512", "ES256",
    "ES384", "EdDSA",
];

#[test]
fn every_name_round_trips_as_the_rfc_spells_it() {
    for name in NAMES {
        let algorithm: Algorithm = name.parse().unwrap_or_else(|_| panic!("{name} parses"));
        assert_eq!(algorithm.to_string(), name);
        assert_eq!(algorithm.as_str(), name);
    }
}

/// `alg` is case-sensitive (RFC 7515 §4.1.1), `none` is no algorithm a
/// verifier runs, and a refusal never repeats what it was given.
#[test]
fn a_name_the_verifier_does_not_run_is_refused_without_repeating_it() {
    for refused in ["none", "rs256", "", "ES512", "eddsa"] {
        let error: UnknownAlgorithmError = refused
            .parse::<Algorithm>()
            .expect_err("not an algorithm this verifier runs");
        let said = error.to_string();
        assert_eq!(said, "not a JWS algorithm this verifier runs");
        if !refused.is_empty() {
            assert!(!said.contains(refused), "{said}");
        }
    }
}
