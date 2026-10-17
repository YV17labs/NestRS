//! Covers `src/credentials.rs`.

use base64::Engine as _;
use nest_rs_authn::{basic_credentials, bearer_token};

#[test]
fn bearer_token_extracts_non_empty_value() {
    let req = crate::request(&[("Authorization", "Bearer token-123")]);
    assert_eq!(bearer_token(&req), Some("token-123"));
}

#[test]
fn bearer_token_rejects_missing_blank_and_malformed() {
    assert_eq!(bearer_token(&crate::request(&[])), None);
    assert_eq!(
        bearer_token(&crate::request(&[("Authorization", "Bearer   ")])),
        None
    );
    assert_eq!(
        bearer_token(&crate::request(&[("Authorization", "Basic abc")])),
        None
    );
}

#[test]
fn basic_credentials_decodes_id_and_secret() {
    let encoded = base64::engine::general_purpose::STANDARD.encode(b"client-id:client-secret");
    let req = crate::request(&[("Authorization", &format!("Basic {encoded}"))]);
    assert_eq!(
        basic_credentials(&req),
        Some(("client-id".into(), "client-secret".into()))
    );
}

#[test]
fn basic_credentials_matches_scheme_case_insensitively() {
    // RFC 7235: auth schemes are case-insensitive.
    let encoded = base64::engine::general_purpose::STANDARD.encode(b"client-id:client-secret");
    for scheme in ["basic", "BASIC", "BaSiC"] {
        let req = crate::request(&[("Authorization", &format!("{scheme} {encoded}"))]);
        assert_eq!(
            basic_credentials(&req),
            Some(("client-id".into(), "client-secret".into())),
            "scheme `{scheme}` must be accepted",
        );
    }
    // A different scheme still refuses.
    let req = crate::request(&[("Authorization", &format!("Bearer {encoded}"))]);
    assert_eq!(basic_credentials(&req), None);
}

#[test]
fn basic_credentials_allows_colons_in_secret() {
    let encoded = base64::engine::general_purpose::STANDARD.encode(b"id:sec:ret:with:colons");
    let req = crate::request(&[("Authorization", &format!("Basic {encoded}"))]);
    assert_eq!(
        basic_credentials(&req),
        Some(("id".into(), "sec:ret:with:colons".into()))
    );
}

/// RFC 6749 §2.3.1 / OAuth 2.1 §2.4.1: both halves of a `Basic` credential
/// arrive `application/x-www-form-urlencoded`.
#[test]
fn basic_credentials_form_urldecode_both_halves() {
    // `a b+c%d` is what the deployment stored; `a+b%2Bc%25d` is what a
    // conforming client puts on the wire for it.
    let encoded = base64::engine::general_purpose::STANDARD.encode("cli%20ent:a+b%2Bc%25d");
    let req = crate::request(&[("Authorization", &format!("Basic {encoded}"))]);
    assert_eq!(
        basic_credentials(&req),
        Some(("cli ent".into(), "a b+c%d".into())),
    );
}

/// A credential carrying neither escape is returned byte-for-byte: the common
/// case must not be rewritten by a decoder it never went through.
#[test]
fn a_credential_with_no_escapes_is_untouched() {
    let encoded = base64::engine::general_purpose::STANDARD.encode("demo-service:s3cr3t-value");
    let req = crate::request(&[("Authorization", &format!("Basic {encoded}"))]);
    assert_eq!(
        basic_credentials(&req),
        Some(("demo-service".into(), "s3cr3t-value".into())),
    );
}

/// A malformed escape is left verbatim rather than refused — this runs on a
/// credential, and a truncated `%` must fail as the wrong secret it is, not as
/// a different error.
#[test]
fn a_malformed_escape_is_left_verbatim() {
    let encoded = base64::engine::general_purpose::STANDARD.encode("id:100%");
    let req = crate::request(&[("Authorization", &format!("Basic {encoded}"))]);
    assert_eq!(basic_credentials(&req), Some(("id".into(), "100%".into())));
}

/// RFC 3986 §2.1: `pct-encoded = "%" HEXDIG HEXDIG`. A sign is no hex digit, so
/// `%+1` must not decode to `\x01`.
#[test]
fn a_signed_escape_is_not_a_hex_escape() {
    // The `%` is malformed, so it stays; the `+` that follows is still a space.
    for (raw, expected) in [
        ("%+1", "% 1"),
        ("%+a", "% a"),
        ("%+F", "% F"),
        ("%-1", "%-1"),
        ("% 1", "% 1"),
    ] {
        let encoded = base64::engine::general_purpose::STANDARD.encode(format!("id:{raw}"));
        let req = crate::request(&[("Authorization", &format!("Basic {encoded}"))]);
        let (_, secret) = basic_credentials(&req).expect("credentials");
        assert_eq!(secret, expected, "{raw} must not decode as a hex escape");
        assert_ne!(
            secret, "\u{1}",
            "{raw} must never alias the byte `%01` encodes",
        );
    }
}
