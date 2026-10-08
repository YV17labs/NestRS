//! Covers `src/service.rs` — `JwtService` sign/verify and decode error mapping.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, EncodingKey, Header, get_current_timestamp};
use nest_rs_authn::{AuthError, JwtOptions, JwtService};
use serde::{Deserialize, Serialize};

/// The header a conformant issuer stamps (RFC 9068 §2.1). Fixtures below mint
/// tokens by hand to exercise *other* checks; without this they would all be
/// refused by the `typ` check instead, which is not what any of them asserts.
fn at_jwt_header(alg: Algorithm) -> Header {
    let mut header = Header::new(alg);
    header.typ = Some("at+jwt".into());
    header
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct TestClaims {
    sub: String,
    exp: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    aud: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nbf: Option<u64>,
}

fn service(label: &str) -> JwtService {
    // Pad to ≥ 32 bytes for the HS256 min-secret guard; the label keeps each
    // test's secret distinct.
    let secret = format!("{label}-padding-to-thirty-two-bytes-minimum");
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
fn short_hmac_secret_is_rejected_by_the_service_constructor() {
    // The ≥256-bit rule holds at `JwtService::new`, for `JwtOptions::new` too.
    // `JwtService` has no `Debug`, so match rather than `.expect_err`.
    let err = match JwtService::new(JwtOptions::new("too-short")) {
        Ok(_) => panic!("a sub-32-byte HS256 secret must be refused"),
        Err(e) => e,
    };
    assert!(
        matches!(&err, AuthError::Failed(msg) if msg.contains("at least 32 bytes")),
        "unexpected error: {err:?}",
    );

    // A 32-byte secret is accepted.
    let ok = "0123456789abcdef0123456789abcdef"; // exactly 32 bytes
    assert_eq!(ok.len(), 32);
    JwtService::new(JwtOptions::new(ok)).expect("a 32-byte secret is accepted");
}

/// A key and an algorithm that cannot work together are refused when the
/// service is built, not on the first sign or verify — which on a verifier is
/// the first request.
#[test]
fn an_algorithm_that_does_not_fit_the_key_is_refused_at_construction() {
    let secret = "0123456789abcdef0123456789abcdef";
    let with = |mut options: JwtOptions, algorithm: Algorithm| {
        options.algorithms = vec![algorithm];
        options
    };
    let cases = [
        (
            "an HMAC secret with RS256",
            with(JwtOptions::new(secret), Algorithm::RS256),
        ),
        (
            "an EdDSA pair with HS256",
            with(
                JwtOptions::eddsa(crate::DEV_PRIVATE_KEY, crate::DEV_PUBLIC_KEY),
                Algorithm::HS256,
            ),
        ),
        (
            "a verify-only EdDSA key with ES256",
            with(
                JwtOptions::eddsa_verify(crate::DEV_PUBLIC_KEY),
                Algorithm::ES256,
            ),
        ),
    ];
    for (label, options) in cases {
        let Err(AuthError::Failed(message)) = JwtService::new(options) else {
            panic!("{label} must be refused at construction")
        };
        assert!(
            message.contains("algorithm cannot be used"),
            "{label}: {message}"
        );
    }
    // RFC 7518 §3.2: HS512 takes a key of at least its 64-byte hash.
    JwtService::new(with(JwtOptions::new(secret.repeat(2)), Algorithm::HS512))
        .expect("another HMAC algorithm fits an HMAC secret of its hash's size");
}

#[tokio::test]
async fn sign_and_verify_round_trip() {
    let jwt = service("round-trip-secret");
    let token = jwt.sign(&claims(jwt.expiry(), None)).expect("sign");
    let decoded: TestClaims = jwt.verify(&token).await.expect("verify");
    assert_eq!(decoded.sub, "alice");
}

#[tokio::test]
async fn expired_token_is_rejected() {
    let jwt = service("expired-secret");
    let past = get_current_timestamp().saturating_sub(3600);
    let token = jwt.sign(&claims(past, None)).expect("sign");
    assert!(matches!(
        jwt.verify::<TestClaims>(&token).await,
        Err(AuthError::Expired)
    ));
}

#[tokio::test]
async fn not_yet_valid_token_is_rejected() {
    let jwt = service("nbf-secret");
    let now = get_current_timestamp();
    let token = jwt
        .sign(&claims(now + 7200, Some(now + 3600)))
        .expect("sign");
    assert!(matches!(
        jwt.verify::<TestClaims>(&token).await,
        Err(AuthError::NotYetValid)
    ));
}

#[tokio::test]
async fn invalid_signature_is_rejected() {
    let issuer = service("issuer-secret");
    let verifier = service("other-secret");
    let token = issuer.sign(&claims(issuer.expiry(), None)).expect("sign");
    assert!(matches!(
        verifier.verify::<TestClaims>(&token).await,
        Err(AuthError::InvalidSignature)
    ));
}

#[test]
fn verify_only_service_cannot_sign() {
    let jwt =
        JwtService::new(JwtOptions::eddsa_verify(crate::DEV_PUBLIC_KEY)).expect("verify-only");
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

#[tokio::test]
async fn audience_must_match_when_configured() {
    let mut options = JwtOptions::new("aud-secret-padded-to-thirty-two-bytes");
    options.audience = Some("api".into());
    let jwt = JwtService::new(options).expect("service");
    let mut ok = claims(jwt.expiry(), None);
    ok.aud = Some("api".into());
    let token = jwt.sign(&ok).expect("sign");
    assert!(jwt.verify::<TestClaims>(&token).await.is_ok());

    let mut bad = claims(jwt.expiry(), None);
    bad.aud = Some("other".into());
    let token = jwt.sign(&bad).expect("sign");
    assert!(matches!(
        jwt.verify::<TestClaims>(&token).await,
        Err(AuthError::InvalidToken)
    ));
}

#[tokio::test]
async fn audience_omitted_is_rejected_when_configured() {
    // A configured audience is mandatory: a validly-signed token omitting `aud` fails closed.
    let secret = "aud-required-secret-padded-to-32-bytes";
    let mut options = JwtOptions::new(secret);
    options.audience = Some("api".into());
    let jwt = JwtService::new(options).expect("service");

    // Forged with the raw encoder by another holder of the shared key.
    let omitted = claims(jwt.expiry(), None);
    assert!(omitted.aud.is_none());
    let forged = jsonwebtoken::encode(
        &at_jwt_header(Algorithm::HS256),
        &omitted,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("encode");
    assert!(matches!(
        jwt.verify::<TestClaims>(&forged).await,
        Err(AuthError::InvalidToken)
    ));

    // The signer stamps the configured audience onto claims that leave `aud` unset.
    let token = jwt.sign(&claims(jwt.expiry(), None)).expect("sign");
    let round_tripped: TestClaims = jwt.verify(&token).await.expect("stamped aud verifies");
    assert_eq!(round_tripped.aud.as_deref(), Some("api"));

    // An explicit audience in the claims is never overwritten.
    let mut present = claims(jwt.expiry(), None);
    present.aud = Some("api".into());
    let token = jwt.sign(&present).expect("sign");
    assert!(jwt.verify::<TestClaims>(&token).await.is_ok());
}

#[tokio::test]
async fn a_configured_issuer_is_stamped_and_required() {
    let secret = "iss-required-secret-padded-to-32-bytes";
    let mut options = JwtOptions::new(secret);
    options.issuer = Some("auth".into());
    let jwt = JwtService::new(options).expect("service");

    #[derive(Serialize, Deserialize)]
    struct IssClaims {
        sub: String,
        exp: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        iss: Option<String>,
    }

    let minted = IssClaims {
        sub: "alice".into(),
        exp: jwt.expiry(),
        iss: None,
    };
    let token = jwt.sign(&minted).expect("sign");
    let back: IssClaims = jwt.verify(&token).await.expect("stamped iss verifies");
    assert_eq!(back.iss.as_deref(), Some("auth"));

    // The same claims encoded without the stamp are refused.
    let forged = jsonwebtoken::encode(
        &at_jwt_header(Algorithm::HS256),
        &minted,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("encode");
    assert!(matches!(
        jwt.verify::<IssClaims>(&forged).await,
        Err(AuthError::InvalidToken)
    ));
}

#[tokio::test]
async fn invalid_algorithm_is_rejected() {
    let jwt = service("alg-secret");
    let header = at_jwt_header(Algorithm::HS384);
    let key = EncodingKey::from_secret(b"alg-secret");
    let token = jsonwebtoken::encode(&header, &claims(jwt.expiry(), None), &key)
        .expect("encode with mismatched alg");
    assert!(matches!(
        jwt.verify::<TestClaims>(&token).await,
        Err(AuthError::InvalidAlgorithm)
    ));
}

#[tokio::test]
async fn unsigned_alg_none_token_is_rejected() {
    // `alg: none` with an empty signature. jsonwebtoken cannot emit one, so the
    // token is hand-crafted.
    let jwt = service("alg-none-secret");
    // A valid, non-expired `exp` so rejection can only be due to `alg: none`,
    // never an incidental claim failure.
    let exp = get_current_timestamp() + 3600;
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"sub":"alice","exp":{exp}}}"#));
    // `header.payload.` — three segments with an empty signature (RFC 7519 §6.1).
    let token = format!("{header}.{payload}.");
    assert!(
        jwt.verify::<TestClaims>(&token).await.is_err(),
        "an alg=none unsigned token must never verify",
    );
}

#[tokio::test]
async fn eddsa_sign_and_verify_round_trip() {
    let jwt = JwtService::new(JwtOptions::eddsa(
        crate::DEV_PRIVATE_KEY,
        crate::DEV_PUBLIC_KEY,
    ))
    .expect("EdDSA service");
    let token = jwt.sign(&claims(jwt.expiry(), None)).expect("sign");
    let decoded: TestClaims = jwt.verify(&token).await.expect("verify");
    assert_eq!(decoded.sub, "alice");
}

/// A private key beside the public key of another pair signs tokens its own
/// service refuses, and verifies the tokens that other pair's private key signs
/// — so the service is refused before anything is served.
#[test]
fn a_private_key_beside_another_pairs_public_key_is_refused() {
    const ANOTHER_PAIRS_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\n\
         MCowBQYDK2VwAyEAK0Be2/q0Wt0AT7wXt1zGeT9ViWrp+Z2uMwAofcgKlnk=\n\
         -----END PUBLIC KEY-----\n";
    let refused = JwtService::new(JwtOptions::eddsa(
        crate::DEV_PRIVATE_KEY,
        ANOTHER_PAIRS_PUBLIC_KEY,
    ));
    let Err(nest_rs_authn::AuthError::Failed(message)) = refused else {
        panic!("keys from two pairs must not build a service")
    };
    assert!(message.contains("not one pair"), "{message}");
    for setting in ["PRIVATE_KEY_FILE", "PUBLIC_KEY_FILE"] {
        assert!(
            message.contains(&nest_rs_config::var_name("authn", setting)),
            "names the pair as either spelling may have set it: {message}"
        );
    }
}

#[tokio::test]
async fn a_token_for_another_service_is_rejected_when_no_audience_is_configured() {
    // The confused deputy: with no audience configured, a token the shared
    // issuer minted for a sibling service is still refused (RFC 7519 §4.1.3).
    let secret = "no-aud-configured-secret-padded-32b";
    let jwt = JwtService::new(JwtOptions::new(secret)).expect("service");
    assert!(
        JwtOptions::new(secret).audience.is_none(),
        "the default configures no audience — the path this asserts about",
    );

    let mut for_someone_else = claims(get_current_timestamp() + 3600, None);
    for_someone_else.aud = Some("https://billing.example".into());
    let token = jsonwebtoken::encode(
        &at_jwt_header(Algorithm::HS256),
        &for_someone_else,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("encode");

    assert!(
        matches!(
            jwt.verify::<TestClaims>(&token).await,
            Err(AuthError::InvalidToken)
        ),
        "a validly-signed token minted for another audience must not verify here",
    );
}

#[tokio::test]
async fn an_audience_less_token_still_verifies_when_no_audience_is_configured() {
    // §4.1.3 fires only when the claim is present: no audience configured and
    // none stamped verifies.
    let jwt = service("no-aud-anywhere-secret");
    let token = jwt.sign(&claims(jwt.expiry(), None)).expect("sign");
    let decoded: TestClaims = jwt.verify(&token).await.expect("verify");
    assert!(decoded.aud.is_none());
}

#[tokio::test]
async fn allow_any_audience_is_the_named_opt_out_and_reports_itself() {
    // The opt-out is written down and reported once per boot, naming the variable.
    let logs = nest_rs_testing::LogCapture::install();
    let secret = "any-aud-opt-in-secret-padded-32-by";
    let mut options = JwtOptions::new(secret);
    options.allow_any_audience = true;
    let jwt = JwtService::new(options).expect("service");

    let event = logs.expect_one(
        nest_rs_authn::TARGET,
        "audience validation is disabled — a token minted for another service verifies here",
    );
    assert_eq!(event.level, "warn");
    assert!(
        event
            .field("var")
            .is_some_and(|v| v.ends_with("AUTHN__ALLOW_ANY_AUDIENCE")),
        "the line names the variable that did it, built rather than spelled: {event:?}",
    );

    let mut foreign = claims(get_current_timestamp() + 3600, None);
    foreign.aud = Some("https://billing.example".into());
    let token = jsonwebtoken::encode(
        &at_jwt_header(Algorithm::HS256),
        &foreign,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("encode");
    assert!(
        jwt.verify::<TestClaims>(&token).await.is_ok(),
        "the opt-out is what restores the old permissive reading",
    );
}

#[test]
fn allow_any_audience_beside_a_configured_audience_is_refused() {
    let mut options = JwtOptions::new("contradiction-secret-padded-to-32b");
    options.audience = Some("api".into());
    options.allow_any_audience = true;
    let err = match JwtService::new(options) {
        Ok(_) => panic!("a contradictory audience policy must not build"),
        Err(e) => e,
    };
    let AuthError::Failed(msg) = &err else {
        panic!("expected Failed, got {err:?}");
    };
    assert!(
        msg.contains("ALLOW_ANY_AUDIENCE") && msg.contains("AUDIENCE"),
        "the refusal names both variables: {msg}",
    );
}

/// RFC 9068 §2.1: "JWT access tokens MUST include this media type in the `typ`
/// header parameter … the `typ` value used SHOULD be `at+jwt`."
#[test]
fn a_minted_token_carries_the_rfc9068_media_type() {
    let jwt =
        JwtService::new(JwtOptions::new("typ-fixture-secret-padded-32-byte")).expect("service");
    let token = jwt
        .sign(&claims(get_current_timestamp() + 3600, None))
        .expect("sign");
    let header = jsonwebtoken::decode_header(&token).expect("header");
    assert_eq!(header.typ.as_deref(), Some("at+jwt"));
}

/// RFC 9068 §4: the resource server "MUST verify that the `typ` header value is
/// `at+jwt` or `application/at+jwt` and reject tokens carrying any other
/// value". An ID Token signed by the same issuer and key must not be spendable here.
#[tokio::test]
async fn a_token_typed_as_anything_else_is_refused() {
    let secret = "typ-fixture-secret-padded-32-byte";
    let jwt = JwtService::new(JwtOptions::new(secret)).expect("service");

    for typ in ["JWT", "id_token+jwt", "at+jwtx"] {
        let mut header = Header::new(Algorithm::HS256);
        header.typ = Some(typ.into());
        let token = jsonwebtoken::encode(
            &header,
            &claims(get_current_timestamp() + 3600, None),
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .expect("encode");
        assert!(
            matches!(
                jwt.verify::<TestClaims>(&token).await,
                Err(nest_rs_authn::AuthError::InvalidToken)
            ),
            "typ={typ} must not verify as an access token",
        );
    }
}

/// §4 names both spellings, and RFC 9110 §8.3.1 makes a media type
/// case-insensitive — so the long form and an odd casing both verify.
#[tokio::test]
async fn the_long_media_type_and_odd_casing_both_verify() {
    let secret = "typ-fixture-secret-padded-32-byte";
    let jwt = JwtService::new(JwtOptions::new(secret)).expect("service");

    for typ in ["application/at+jwt", "AT+JWT"] {
        let mut header = Header::new(Algorithm::HS256);
        header.typ = Some(typ.into());
        let token = jsonwebtoken::encode(
            &header,
            &claims(get_current_timestamp() + 3600, None),
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .expect("encode");
        assert!(jwt.verify::<TestClaims>(&token).await.is_ok(), "typ={typ}");
    }
}

/// The opt-out exists for an issuer that predates the profile, and it is the
/// only way a plain `typ: JWT` verifies.
#[tokio::test]
async fn explicit_typing_can_be_turned_off_for_a_legacy_issuer() {
    let secret = "typ-fixture-secret-padded-32-byte";
    let mut options = JwtOptions::new(secret);
    options.explicit_typing = false;
    let jwt = JwtService::new(options).expect("service");

    let token = jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &claims(get_current_timestamp() + 3600, None),
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("encode");
    assert!(jwt.verify::<TestClaims>(&token).await.is_ok());
}

/// A `JwtOptions` built in code is held to the variables' ranges too, or the
/// lifetime and leeway arithmetic overflows.
#[test]
fn a_lifetime_or_a_leeway_built_in_code_outside_its_range_is_refused_at_construction() {
    let secret = "this-is-a-32-byte-test-secret!!!";
    for (expires_in, leeway, field) in [
        (
            Duration::ZERO,
            Duration::from_secs(30),
            "JwtOptions::expires_in",
        ),
        (
            Duration::from_secs(u64::MAX),
            Duration::from_secs(30),
            "JwtOptions::expires_in",
        ),
        (
            Duration::from_secs(3600),
            Duration::from_secs(301),
            "JwtOptions::leeway",
        ),
        (
            Duration::from_secs(3600),
            Duration::from_secs(u64::MAX),
            "JwtOptions::leeway",
        ),
    ] {
        let mut options = JwtOptions::new(secret);
        options.expires_in = expires_in;
        options.leeway = leeway;
        let Err(AuthError::Failed(refused)) = JwtService::new(options) else {
            panic!("{field} out of range must be refused");
        };
        assert!(refused.contains(field), "{refused}");
    }
}

/// The one arithmetic a caller controls saturates: a wrapped sum would mint a
/// token already expired.
#[test]
fn an_expiry_past_the_end_of_time_saturates() {
    let jwt = service("saturate");
    assert_eq!(jwt.expiry_in(u64::MAX), u64::MAX);
    assert!(jwt.expiry() > get_current_timestamp());
}
