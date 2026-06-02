//! Covers `src/passport/credentials.rs`.

use base64::Engine as _;
use nestrs_authn::{basic_credentials, bearer_token};

#[test]
fn bearer_token_extracts_non_empty_value() {
    let req = crate::common::request(&[("Authorization", "Bearer token-123")]);
    assert_eq!(bearer_token(&req), Some("token-123"));
}

#[test]
fn bearer_token_rejects_missing_blank_and_malformed() {
    assert_eq!(bearer_token(&crate::common::request(&[])), None);
    assert_eq!(
        bearer_token(&crate::common::request(&[("Authorization", "Bearer   ")])),
        None
    );
    assert_eq!(
        bearer_token(&crate::common::request(&[("Authorization", "Basic abc")])),
        None
    );
}

#[test]
fn basic_credentials_decodes_id_and_secret() {
    let encoded = base64::engine::general_purpose::STANDARD.encode(b"client-id:client-secret");
    let req = crate::common::request(&[("Authorization", &format!("Basic {encoded}"))]);
    assert_eq!(
        basic_credentials(&req),
        Some(("client-id".into(), "client-secret".into()))
    );
}

#[test]
fn basic_credentials_allows_colons_in_secret() {
    let encoded = base64::engine::general_purpose::STANDARD.encode(b"id:sec:ret:with:colons");
    let req = crate::common::request(&[("Authorization", &format!("Basic {encoded}"))]);
    assert_eq!(
        basic_credentials(&req),
        Some(("id".into(), "sec:ret:with:colons".into()))
    );
}
