//! Covers `src/jwt/service.rs` — `JwtService` sign/verify and decode error mapping.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, EncodingKey, Header, get_current_timestamp};
use nest_rs_authn::{AuthError, JwtOptions, JwtService};
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
fn audience_omitted_is_rejected_when_configured() {
    // Regression: a configured audience must be *mandatory*. A validly-signed
    // token that omits `aud` entirely was silently accepted (set_audience only
    // compares when the claim is present); it must now fail closed.
    let mut options = JwtOptions::new("aud-required-secret");
    options.audience = Some("api".into());
    let jwt = JwtService::new(options).expect("service");

    // `TestClaims.aud` is `skip_serializing_if = Option::is_none`, so `None`
    // produces a token with no `aud` claim at all.
    let omitted = claims(jwt.expiry(), None);
    assert!(omitted.aud.is_none());
    let token = jwt.sign(&omitted).expect("sign");
    assert!(matches!(
        jwt.verify::<TestClaims>(&token),
        Err(AuthError::InvalidToken)
    ));

    // A token that carries the matching audience is still accepted.
    let mut present = claims(jwt.expiry(), None);
    present.aud = Some("api".into());
    let token = jwt.sign(&present).expect("sign");
    assert!(jwt.verify::<TestClaims>(&token).is_ok());
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
fn unsigned_alg_none_token_is_rejected() {
    // The classic alg-confusion / "unsecured JWT" attack: an attacker forges a
    // token whose header declares `alg: none` and ships an empty signature,
    // hoping the verifier skips signature checking. `JwtService` must reject it
    // — an unsigned token is never authentic. jsonwebtoken has no `none` in its
    // `Algorithm` enum and its encoder cannot emit one, so we hand-craft the
    // token (base64url header + payload + empty signature) to prove the service
    // refuses it rather than relying on the encoder to produce the attack.
    let jwt = service("alg-none-secret");
    // A valid, non-expired `exp` so rejection can only be due to `alg: none`,
    // never an incidental claim failure.
    let exp = get_current_timestamp() + 3600;
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"sub":"alice","exp":{exp}}}"#));
    // `header.payload.` — three segments with an empty signature (RFC 7519 §6.1).
    let token = format!("{header}.{payload}.");
    assert!(
        jwt.verify::<TestClaims>(&token).is_err(),
        "an alg=none unsigned token must never verify",
    );
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
