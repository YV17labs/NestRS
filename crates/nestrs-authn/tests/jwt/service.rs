//! Covers `src/jwt/service.rs` — `JwtService` sign/verify and decode error mapping.

use jsonwebtoken::{Algorithm, EncodingKey, Header, get_current_timestamp};
use nestrs_authn::{AuthError, JwtOptions, JwtService};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct TestClaims {
    sub: String,
    exp: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    aud: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nbf: Option<u64>,
}

fn service(secret: &str) -> JwtService {
    JwtService::new(JwtOptions::new(secret)).expect("HMAC service")
}

fn claims(exp: u64, nbf: Option<u64>) -> TestClaims {
    TestClaims {
        sub: "alice".into(),
        exp,
        aud: None,
        nbf,
    }
}

#[test]
fn sign_and_verify_round_trip() {
    let jwt = service("round-trip-secret");
    let token = jwt.sign(&claims(jwt.expiry(), None)).expect("sign");
    let decoded: TestClaims = jwt.verify(&token).expect("verify");
    assert_eq!(decoded.sub, "alice");
}

#[test]
fn expired_token_is_rejected() {
    let jwt = service("expired-secret");
    let past = get_current_timestamp().saturating_sub(3600);
    let token = jwt.sign(&claims(past, None)).expect("sign");
    assert!(matches!(
        jwt.verify::<TestClaims>(&token),
        Err(AuthError::Expired)
    ));
}

#[test]
fn not_yet_valid_token_is_rejected() {
    let jwt = service("nbf-secret");
    let now = get_current_timestamp();
    let token = jwt
        .sign(&claims(now + 7200, Some(now + 3600)))
        .expect("sign");
    assert!(matches!(
        jwt.verify::<TestClaims>(&token),
        Err(AuthError::NotYetValid)
    ));
}

#[test]
fn invalid_signature_is_rejected() {
    let issuer = service("issuer-secret");
    let verifier = service("other-secret");
    let token = issuer.sign(&claims(issuer.expiry(), None)).expect("sign");
    assert!(matches!(
        verifier.verify::<TestClaims>(&token),
        Err(AuthError::InvalidSignature)
    ));
}

#[test]
fn verify_only_service_cannot_sign() {
    let jwt = JwtService::new(JwtOptions::eddsa_verify(crate::common::DEV_PUBLIC_KEY))
        .expect("verify-only");
    assert!(matches!(
        jwt.sign(&claims(jwt.expiry(), None)),
        Err(AuthError::Failed(_))
    ));
}

#[test]
fn invalid_pem_fails_at_construction() {
    assert!(matches!(
        JwtService::new(JwtOptions::eddsa_verify("not-a-pem")),
        Err(AuthError::Failed(_))
    ));
}

#[test]
fn audience_must_match_when_configured() {
    let mut options = JwtOptions::new("aud-secret");
    options.audience = Some("api".into());
    let jwt = JwtService::new(options).expect("service");
    let mut ok = claims(jwt.expiry(), None);
    ok.aud = Some("api".into());
    let token = jwt.sign(&ok).expect("sign");
    assert!(jwt.verify::<TestClaims>(&token).is_ok());

    let mut bad = claims(jwt.expiry(), None);
    bad.aud = Some("other".into());
    let token = jwt.sign(&bad).expect("sign");
    assert!(matches!(
        jwt.verify::<TestClaims>(&token),
        Err(AuthError::InvalidToken)
    ));
}

#[test]
fn invalid_algorithm_is_rejected() {
    let jwt = service("alg-secret");
    let header = Header::new(Algorithm::HS384);
    let key = EncodingKey::from_secret(b"alg-secret");
    let token = jsonwebtoken::encode(&header, &claims(jwt.expiry(), None), &key)
        .expect("encode with mismatched alg");
    assert!(matches!(
        jwt.verify::<TestClaims>(&token),
        Err(AuthError::InvalidAlgorithm)
    ));
}

#[test]
fn eddsa_sign_and_verify_round_trip() {
    let jwt = JwtService::new(JwtOptions::eddsa(
        crate::common::DEV_PRIVATE_KEY,
        crate::common::DEV_PUBLIC_KEY,
    ))
    .expect("EdDSA service");
    let token = jwt.sign(&claims(jwt.expiry(), None)).expect("sign");
    let decoded: TestClaims = jwt.verify(&token).expect("verify");
    assert_eq!(decoded.sub, "alice");
}
