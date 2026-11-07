//! Covers `src/service.rs` — `JwtService` sign/verify and decode error mapping.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, EncodingKey, Header, get_current_timestamp};
use nest_rs_authn::__private::sign_unchecked;
use nest_rs_authn::{AuthError, JwtOptions, JwtService, PrincipalIdentity};
use serde::{Deserialize, Serialize};

/// The header a conformant issuer stamps (RFC 9068 §2.1). Fixtures below mint
/// tokens by hand to exercise *other* checks; without this they would all be
/// refused by the `typ` check instead, which is not what any of them asserts.
fn at_jwt_header(alg: Algorithm) -> Header {
    let mut header = Header::new(alg);
    header.typ = Some("at+jwt".into());
    header
}

#[derive(Debug, Serialize, Deserialize, PartialEq, PrincipalIdentity)]
struct TestClaims {
    #[principal(actor)]
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

/// An `exp` an hour from now, for claims naming their own.
fn in_an_hour() -> u64 {
    get_current_timestamp() + 3600
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
        matches!(&err, AuthError::Failed { .. })
            && nest_rs_core::error_message(&err).contains("at least 32 bytes"),
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
    let with = |mut options: JwtOptions, algorithm: nest_rs_authn::Algorithm| {
        options.algorithms = vec![algorithm];
        options
    };
    let cases = [
        (
            "an HMAC secret with RS256",
            with(JwtOptions::new(secret), nest_rs_authn::Algorithm::RS256),
        ),
        (
            "an EdDSA pair with HS256",
            with(
                JwtOptions::eddsa(crate::DEV_PRIVATE_KEY, crate::DEV_PUBLIC_KEY),
                nest_rs_authn::Algorithm::HS256,
            ),
        ),
        (
            "a verify-only EdDSA key with ES256",
            with(
                JwtOptions::eddsa_verify(crate::DEV_PUBLIC_KEY),
                nest_rs_authn::Algorithm::ES256,
            ),
        ),
    ];
    for (label, options) in cases {
        let Err(message @ AuthError::Failed { .. }) = JwtService::new(options) else {
            panic!("{label} must be refused at construction")
        };
        let message = nest_rs_core::error_message(&message);
        assert!(
            message.contains("algorithm cannot be used"),
            "{label}: {message}"
        );
    }
    // RFC 7518 §3.2: HS512 takes a key of at least its 64-byte hash.
    JwtService::new(with(
        JwtOptions::new(secret.repeat(2)),
        nest_rs_authn::Algorithm::HS512,
    ))
    .expect("another HMAC algorithm fits an HMAC secret of its hash's size");
}

#[tokio::test]
async fn sign_and_verify_round_trip() {
    let jwt = service("round-trip-secret");
    let token = jwt.sign(&claims(in_an_hour(), None)).expect("sign");
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
    let token = issuer.sign(&claims(in_an_hour(), None)).expect("sign");
    assert!(matches!(
        verifier.verify::<TestClaims>(&token).await,
        Err(AuthError::InvalidSignature)
    ));
}

#[test]
fn verify_only_service_cannot_sign() {
    let jwt =
        JwtService::new(JwtOptions::eddsa_verify(crate::DEV_PUBLIC_KEY)).expect("verify-only");
    let refused = jwt
        .sign(&claims(in_an_hour(), None))
        .expect_err("no key to sign with");
    let said = nest_rs_core::error_message(&refused);
    assert!(said.contains("verify-only"), "{said}");
}

#[test]
fn invalid_pem_fails_at_construction() {
    assert!(matches!(
        JwtService::new(JwtOptions::eddsa_verify("not-a-pem")),
        Err(AuthError::Failed { .. })
    ));
}

#[tokio::test]
async fn audience_must_match_when_configured() {
    let mut options = JwtOptions::new("aud-secret-padded-to-thirty-two-bytes");
    options.audience = Some("api".into());
    let jwt = JwtService::new(options).expect("service");
    let mut ok = claims(in_an_hour(), None);
    ok.aud = Some("api".into());
    let token = jwt.sign(&ok).expect("sign");
    assert!(jwt.verify::<TestClaims>(&token).await.is_ok());

    let mut bad = claims(in_an_hour(), None);
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
    let omitted = claims(in_an_hour(), None);
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
    let token = jwt.sign(&claims(in_an_hour(), None)).expect("sign");
    let round_tripped: TestClaims = jwt.verify(&token).await.expect("stamped aud verifies");
    assert_eq!(round_tripped.aud.as_deref(), Some("api"));

    // An explicit audience in the claims is never overwritten.
    let mut present = claims(in_an_hour(), None);
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

    #[derive(Serialize, Deserialize, PrincipalIdentity)]
    struct IssClaims {
        #[principal(actor)]
        sub: String,
        exp: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        iss: Option<String>,
    }

    let minted = IssClaims {
        sub: "alice".into(),
        exp: in_an_hour(),
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
    let token = jsonwebtoken::encode(&header, &claims(in_an_hour(), None), &key)
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
    let token = jwt.sign(&claims(in_an_hour(), None)).expect("sign");
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
    let Err(message @ nest_rs_authn::AuthError::Failed { .. }) = refused else {
        panic!("keys from two pairs must not build a service")
    };
    let message = nest_rs_core::error_message(&message);
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
    let token = jwt.sign(&claims(in_an_hour(), None)).expect("sign");
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
    let msg @ AuthError::Failed { .. } = &err else {
        panic!("expected Failed, got {err:?}");
    };
    let msg = nest_rs_core::error_message(msg);
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

/// RFC 9068 §2.2 requires `iat` and `jti` in a JWT access token: the signer
/// knows when it signs and draws an identifier no other token shares, so it
/// stamps both, and a claims type's own are kept.
#[tokio::test]
async fn a_minted_token_carries_its_issue_time_and_an_identifier_of_its_own() {
    let jwt = service("iat-jti-secret");
    let mint = || jwt.sign(&claims(in_an_hour(), None)).expect("sign");
    let before = get_current_timestamp();
    let first: serde_json::Value = jwt.verify(&mint()).await.expect("verify");
    let second: serde_json::Value = jwt.verify(&mint()).await.expect("verify");
    let after = get_current_timestamp();

    let issued = first["iat"].as_u64().expect("`iat` is a NumericDate");
    assert!((before..=after).contains(&issued), "{first}");
    let id = first["jti"].as_str().expect("`jti` is a string");
    assert!(!id.is_empty(), "{first}");
    assert_ne!(Some(id), second["jti"].as_str(), "each token names its own");

    #[derive(Serialize, PrincipalIdentity)]
    struct OwnClaims {
        #[principal(actor)]
        sub: &'static str,
        exp: u64,
        iat: u64,
        jti: &'static str,
    }
    let own = OwnClaims {
        sub: "alice",
        exp: in_an_hour(),
        iat: 7,
        jti: "chosen-by-the-issuer",
    };
    let kept: serde_json::Value = jwt
        .verify(&jwt.sign(&own).expect("sign"))
        .await
        .expect("verify");
    assert_eq!(kept["iat"], 7);
    assert_eq!(kept["jti"], "chosen-by-the-issuer");

    #[derive(Serialize, PrincipalIdentity)]
    struct NullClaims {
        #[principal(actor)]
        sub: &'static str,
        exp: u64,
        jti: Option<&'static str>,
    }
    let null = NullClaims {
        sub: "alice",
        exp: in_an_hour(),
        jti: None,
    };
    let filled: serde_json::Value = jwt
        .verify(&jwt.sign(&null).expect("sign"))
        .await
        .expect("verify");
    assert!(
        filled["jti"].as_str().is_some_and(|id| !id.is_empty()),
        "a claim serialized as null is no claim: {filled}"
    );
}

/// RFC 9068 §2.2 requires `exp`, and the signer alone knows the lifetime it
/// grants: claims leaving `exp` out, or serializing it as `null`, expire
/// `expires_in` after the token's `iat` — through either signing path — and
/// claims naming their own keep it.
#[tokio::test]
async fn a_minted_token_expires_its_signer_s_lifetime_after_it_was_issued() {
    let mut options = JwtOptions::new("exp-stamp-secret-padded-to-32-bytes");
    options.expires_in = Duration::from_secs(900);
    let jwt = JwtService::new(options).expect("service");

    #[derive(Serialize, PrincipalIdentity)]
    struct Anonymous {
        #[principal(actor)]
        sub: &'static str,
    }
    #[derive(Serialize, PrincipalIdentity)]
    struct NullExpiry {
        #[principal(actor)]
        sub: &'static str,
        exp: Option<u64>,
    }
    for token in [
        jwt.sign(&Anonymous { sub: "alice" }).expect("sign"),
        jwt.sign_for("https://api.example", &Anonymous { sub: "alice" })
            .expect("sign for a resource"),
        jwt.sign(&NullExpiry {
            sub: "alice",
            exp: None,
        })
        .expect("sign"),
    ] {
        let minted = payload(&token);
        let issued = minted["iat"].as_u64().expect("`iat` is a NumericDate");
        assert_eq!(minted["exp"].as_u64(), Some(issued + 900), "{minted}");
    }

    let own = claims(get_current_timestamp() + 60, None);
    let kept: TestClaims = jwt
        .verify(&jwt.sign(&own).expect("sign"))
        .await
        .expect("verify");
    assert_eq!(
        kept.exp, own.exp,
        "a claims type's own `exp` is never overwritten"
    );
}

/// A handshake lives the time its flow names, never the access-token
/// lifetime: claims leaving `exp` out, or serializing it as `null`, are
/// refused naming `expiry_in`, and claims naming it keep it.
#[test]
fn a_handshake_naming_no_exp_is_refused_rather_than_given_the_access_token_lifetime() {
    let jwt = service("handshake-exp");

    #[derive(Serialize)]
    struct Transaction {
        csrf: &'static str,
        exp: Option<u64>,
    }
    for claims in [
        serde_json::json!({ "csrf": "state" }),
        serde_json::to_value(Transaction {
            csrf: "state",
            exp: None,
        })
        .expect("serialize"),
    ] {
        let refused = jwt
            .sign_handshake("oauth-tx", &claims)
            .expect_err("a handshake carrying no `exp` is refused");
        assert!(
            nest_rs_core::error_message(&refused).contains("expiry_in"),
            "{refused:?}"
        );
    }

    let ten_minutes = jwt.expiry_in(600);
    let token = jwt
        .sign_handshake(
            "oauth-tx",
            &Transaction {
                csrf: "state",
                exp: Some(ten_minutes),
            },
        )
        .expect("a handshake naming its lifetime signs");
    let back: serde_json::Value = jwt
        .verify_handshake("oauth-tx", &token)
        .expect("and verifies");
    assert_eq!(back["exp"].as_u64(), Some(ten_minutes), "{back}");
}

/// The claims `token` carries, read without verifying it.
fn payload(token: &str) -> serde_json::Value {
    let encoded = token.split('.').nth(1).expect("a compact JWS");
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded).expect("base64url"))
        .expect("a JSON payload")
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
        let Err(refused @ AuthError::Failed { .. }) = JwtService::new(options) else {
            panic!("{field} out of range must be refused");
        };
        let refused = nest_rs_core::error_message(&refused);
        assert!(refused.contains(field), "{refused}");
    }
}

/// The one arithmetic a caller controls saturates: a wrapped sum would mint a
/// token already expired.
#[test]
fn an_expiry_past_the_end_of_time_saturates() {
    let jwt = service("saturate");
    assert_eq!(jwt.expiry_in(u64::MAX), u64::MAX);
}

/// RFC 8707 §2: a token minted for a resource names that resource as its
/// audience — whatever audience the signer requires of its own — so only a
/// verifier serving that resource accepts it.
#[tokio::test]
async fn a_token_signed_for_a_resource_names_it_and_verifies_only_there() {
    let secret = "resource-bound-secret-padded-32-b";
    let with_audience = |audience: &str| {
        let mut options = JwtOptions::new(secret);
        options.audience = Some(audience.into());
        JwtService::new(options).expect("service")
    };
    let issuer = with_audience("https://auth.example");
    let api = with_audience("https://api.example");
    let assistant = with_audience("https://assistant.example");

    let token = issuer
        .sign_for("https://api.example", &claims(in_an_hour(), None))
        .expect("sign for the api");

    let decoded: TestClaims = api.verify(&token).await.expect("the api accepts it");
    assert_eq!(decoded.aud.as_deref(), Some("https://api.example"));
    for elsewhere in [&assistant, &issuer] {
        assert!(
            matches!(
                elsewhere.verify::<TestClaims>(&token).await,
                Err(AuthError::InvalidToken)
            ),
            "a token minted for the api is refused by every other audience",
        );
    }
}

/// A token for a resource names it and nothing else: a blank resource names
/// none, and claims carrying an `aud` of their own would contradict it.
#[test]
fn signing_for_a_resource_refuses_a_blank_one_and_claims_naming_another() {
    let jwt = service("resource-refusals");
    for blank in ["", "  "] {
        let refused = jwt
            .sign_for(blank, &claims(in_an_hour(), None))
            .expect_err("a blank resource must be refused");
        assert!(
            nest_rs_core::error_message(&refused).contains("names no resource"),
            "{refused:?}"
        );
    }

    let mut naming_another = claims(in_an_hour(), None);
    naming_another.aud = Some("https://billing.example".into());
    let refused = jwt
        .sign_for("https://api.example", &naming_another)
        .expect_err("claims naming another audience must be refused");
    let refused = nest_rs_core::error_message(&refused);
    assert!(refused.contains("`aud` of their own"), "{refused}");
    assert!(!refused.contains("billing"), "{refused}");
}

/// The signer's own issuer, issue time and identifier are stamped on a token
/// minted for a resource as on one minted for itself.
#[tokio::test]
async fn a_token_signed_for_a_resource_carries_the_signers_stamps() {
    let secret = "resource-stamps-secret-padded-32b";
    let mut options = JwtOptions::new(secret);
    options.issuer = Some("https://auth.example".into());
    let issuer = JwtService::new(options).expect("issuer");
    let mut options = JwtOptions::new(secret);
    options.audience = Some("https://api.example".into());
    options.issuer = Some("https://auth.example".into());
    let api = JwtService::new(options).expect("api");

    let token = issuer
        .sign_for("https://api.example", &claims(in_an_hour(), None))
        .expect("sign");
    let stamped: serde_json::Value = api.verify(&token).await.expect("verify");
    assert_eq!(stamped["iss"], "https://auth.example");
    assert!(stamped["iat"].is_u64(), "{stamped}");
    assert!(stamped["jti"].as_str().is_some_and(|id| !id.is_empty()));
}

/// Every key a `JwtKey` holds prints as `<redacted>` and a JWK Set's URI
/// without its userinfo or query; `JwtOptions` prints its key the same way.
#[test]
fn debug_prints_no_key_material_and_no_credential_in_the_jwks_uri() {
    use nest_rs_authn::JwtKey;

    let secret = "an-hmac-secret-padded-to-thirty-two-bytes";
    let keys = [
        JwtKey::Hmac(secret.into()),
        JwtKey::Pem {
            private_pem: Some(crate::DEV_PRIVATE_KEY.into()),
            public_pem: crate::DEV_PUBLIC_KEY.into(),
        },
        JwtKey::Jwks {
            uri: "https://user:URLSECRET1@idp.example/jwks?key=URLSECRET2".into(),
            tls: nest_rs_config::ClientTls::default(),
        },
    ];
    let shown = [
        "Hmac(<redacted>)",
        "private_pem: Some(<redacted>)",
        "uri: \"https://idp.example/jwks\"",
    ];
    for (key, shown) in keys.into_iter().zip(shown) {
        let mut options = JwtOptions::new(secret);
        options.key = key.clone();
        for printed in [format!("{key:?}"), format!("{options:?}")] {
            for material in [secret, "PRIVATE KEY", "URLSECRET"] {
                assert!(!printed.contains(material), "{material} in {printed}");
            }
            assert!(printed.contains(shown), "{shown} in {printed}");
        }
    }
}

/// A claims type's subject, read by its role wherever the type holds it: no
/// `sub` field is no reason to mint a token naming no one (RFC 9068 §2.2).
#[derive(Serialize, PrincipalIdentity)]
struct NamedByItsActor {
    #[principal(actor)]
    user_id: Option<String>,
}

/// The claim they carry is theirs: an empty one names no one either.
#[derive(Serialize, PrincipalIdentity)]
struct CarriesItsSub {
    sub: String,
    #[principal(actor)]
    actor: Option<String>,
}

#[test]
fn a_sub_the_claims_carry_is_kept_and_an_empty_one_is_the_actors() {
    let jwt = service("carried-sub");

    let kept = jwt
        .sign(&CarriesItsSub {
            sub: "alice".into(),
            actor: Some("bob".into()),
        })
        .expect("sign");
    assert_eq!(payload(&kept)["sub"], "alice");

    let empty = jwt
        .sign(&CarriesItsSub {
            sub: String::new(),
            actor: Some("bob".into()),
        })
        .expect("sign");
    assert_eq!(payload(&empty)["sub"], "bob");

    assert!(
        jwt.sign(&CarriesItsSub {
            sub: String::new(),
            actor: None,
        })
        .is_err()
    );
}

/// `sign_for` refuses a blank resource by `trim`; a blank subject names no one
/// as surely, and the actor standing in for it is held to the same.
#[test]
fn a_blank_sub_is_the_actors_and_a_blank_actor_names_no_one() {
    let jwt = service("blank-sub");

    let carried = CarriesItsSub {
        sub: "   ".into(),
        actor: Some("bob".into()),
    };
    for token in [
        jwt.sign(&carried).expect("sign"),
        jwt.sign_for("https://api.example", &carried)
            .expect("sign for a resource"),
    ] {
        assert_eq!(payload(&token)["sub"], "bob");
    }

    let carried_blank = CarriesItsSub {
        sub: "\t ".into(),
        actor: Some("  ".into()),
    };
    let actor_blank = NamedByItsActor {
        user_id: Some("  ".into()),
    };
    for refused in [
        jwt.sign(&carried_blank),
        jwt.sign_for("https://api.example", &carried_blank),
        jwt.sign(&actor_blank),
        jwt.sign_for("https://api.example", &actor_blank),
    ] {
        let said = nest_rs_core::error_message(&refused.expect_err("minted naming no one"));
        assert!(said.contains("name no subject"), "{said}");
    }
}

/// The actor of a claims type holding its `sub` as a number.
#[derive(Serialize, PrincipalIdentity)]
struct NumberedSub {
    #[principal(actor)]
    sub: u64,
}

/// What the claims carry under `sub`, whatever its JSON type, beside a good actor.
#[derive(Serialize, PrincipalIdentity)]
struct ShapedSub {
    sub: serde_json::Value,
    #[principal(actor)]
    id: String,
}

/// RFC 7519 §4.1.2 makes `sub` a string, which RFC 9068 §2.2 requires: a verifier
/// reading one refuses anything else, so the signer does not mint it. A claim the
/// claims carry is never overwritten, so the actor does not stand in, and the
/// refusal quotes nothing they hold.
#[tokio::test]
async fn a_sub_that_is_no_string_mints_no_token() {
    let jwt = service("no-string");

    let said = nest_rs_core::error_message(
        &jwt.sign(&NumberedSub { sub: 4242 })
            .expect_err("minted a numeric `sub`"),
    );
    assert!(said.contains("is no string"), "{said}");
    assert!(!said.contains("4242"), "{said}");

    for shaped in [
        serde_json::json!(4242),
        serde_json::json!(true),
        serde_json::json!({ "user": "ada" }),
        serde_json::json!(["ada"]),
    ] {
        let claims = ShapedSub {
            sub: shaped.clone(),
            id: "actor-id".into(),
        };
        for refused in [
            jwt.sign(&claims),
            jwt.sign_for("https://api.example", &claims),
        ] {
            let said = nest_rs_core::error_message(&refused.expect_err("minted a shaped `sub`"));
            assert!(said.contains("is no string"), "{shaped}: {said}");
            for value in ["4242", "ada", "actor-id"] {
                assert!(!said.contains(value), "{shaped}: {said}");
            }
        }
    }

    let forged = sign_unchecked(&jwt, &NumberedSub { sub: 4242 }).expect("a suite forges it");
    assert_eq!(payload(&forged)["sub"], 4242);
}

/// An integer id under another name names its subject as its decimal string,
/// which is what the JWT page tells a claims type holding one to do.
#[test]
fn a_numeric_actor_under_another_name_is_stamped_as_its_string() {
    #[derive(Serialize, PrincipalIdentity)]
    struct NumberedActor {
        #[principal(actor)]
        user_id: u64,
    }

    let jwt = service("numeric-actor");

    let token = jwt.sign(&NumberedActor { user_id: 42 }).expect("sign");

    assert_eq!(payload(&token)["sub"], "42");
    assert_eq!(payload(&token)["user_id"], 42);
}

/// A suite forging what a verifier must refuse has its one way past the bound.
#[test]
fn only_the_unchecked_signer_mints_claims_naming_no_subject() {
    let jwt = service("unchecked");
    let forged = sign_unchecked(&jwt, &serde_json::json!({})).expect("sign");
    assert!(payload(&forged).get("sub").is_none());
}

#[test]
fn a_claims_type_with_no_sub_field_is_stamped_its_actor_as_sub() {
    let jwt = service("stamped-sub");
    let named = NamedByItsActor {
        user_id: Some("alice".into()),
    };
    for token in [
        jwt.sign(&named).expect("sign"),
        jwt.sign_for("https://api.example", &named)
            .expect("sign for a resource"),
    ] {
        assert_eq!(payload(&token)["sub"], "alice");
    }
}

#[test]
fn a_claims_type_naming_no_actor_mints_no_token() {
    let jwt = service("no-subject");
    let nobody = NamedByItsActor { user_id: None };
    let empty = NamedByItsActor {
        user_id: Some(String::new()),
    };
    for claims in [nobody, empty] {
        for refused in [
            jwt.sign(&claims),
            jwt.sign_for("https://api.example", &claims),
        ] {
            let said = nest_rs_core::error_message(&refused.expect_err("minted naming no one"));
            assert!(said.contains("name no subject"), "{said}");
        }
    }
}

/// RFC 7800 §3's `cnf` binds the token to a key the presenter proves it holds:
/// read as a plain bearer, a stolen one would pass with no proof. Any
/// confirmation member is refused, whichever method it names (RFC 9449 §6.1's
/// `jkt`, RFC 8705 §3.1's `x5t#S256`); a `null` confirms nothing.
#[tokio::test]
async fn a_token_bound_to_a_key_is_refused_as_a_bearer() {
    let jwt = service("sender-constrained");
    for confirmation in [
        serde_json::json!({ "jkt": "0ZcOCORZNYy-DWpqq30jZyJGHTN0d2HglBV3uiguA4I" }),
        serde_json::json!({ "x5t#S256": "bwcK0esc3ACC3DB2Y5_lESsXE8o9ltc05O89jdN-dg2" }),
        serde_json::json!({}),
    ] {
        let token = sign_unchecked(
            &jwt,
            &serde_json::json!({ "sub": "alice", "cnf": confirmation }),
        )
        .expect("sign");

        let refused = jwt.verify::<serde_json::Value>(&token).await;

        assert!(
            matches!(refused, Err(AuthError::SenderConstrained)),
            "{confirmation}: {refused:?}"
        );
    }
    let token =
        sign_unchecked(&jwt, &serde_json::json!({ "sub": "alice", "cnf": null })).expect("sign");
    jwt.verify::<serde_json::Value>(&token)
        .await
        .expect("a `null` confirms no key");
}

/// What the signer stamps sits in a JSON object: claims serializing to
/// anything else cannot carry one, and are refused rather than signed bare.
#[test]
fn claims_that_are_no_json_object_are_refused() {
    let jwt = service("no-object");
    for refused in [
        jwt.sign_handshake("oauth-tx", &7_u32),
        sign_unchecked(&jwt, &7_u32),
    ] {
        let said = nest_rs_core::error_message(&refused.expect_err("signed a number"));
        assert!(said.contains("no JSON object"), "{said}");
    }
}

/// Claims carrying, beside a good actor, whatever `claim` holds.
#[derive(Serialize, PrincipalIdentity)]
struct CarriesAClaim {
    #[principal(actor)]
    sub: String,
    #[serde(flatten)]
    claim: serde_json::Map<String, serde_json::Value>,
}

fn carrying(claim: &str, value: serde_json::Value) -> CarriesAClaim {
    CarriesAClaim {
        sub: "alice".into(),
        claim: serde_json::Map::from_iter([(claim.to_owned(), value)]),
    }
}

/// RFC 7519 §4.1 types each claim it registers, and RFC 9068 §2.2 the ones it
/// adds (`client_id`, `scope` through RFC 8693 §4, `auth_time`, `acr`, `amr`
/// through OpenID Connect Core §2): a verifier reading one as its type refuses
/// anything else, so the signer mints none of another type, on either path,
/// naming the claim and quoting nothing the claims hold.
#[test]
fn a_registered_claim_of_another_json_type_mints_no_token() {
    use serde_json::json;

    let jwt = service("registered-types");
    let not_a_string = [
        json!(4242),
        json!(true),
        json!({ "v": "never-shown" }),
        json!(["never-shown"]),
    ];
    let not_a_date = [
        json!("never-shown"),
        json!(true),
        json!({}),
        json!([4242]),
        json!(-4242),
    ];
    let refused: Vec<(&str, Vec<serde_json::Value>)> = vec![
        ("client_id", not_a_string.to_vec()),
        ("iss", not_a_string.to_vec()),
        ("jti", not_a_string.to_vec()),
        ("acr", not_a_string.to_vec()),
        ("exp", not_a_date.to_vec()),
        ("nbf", not_a_date.to_vec()),
        ("iat", not_a_date.to_vec()),
        ("auth_time", not_a_date.to_vec()),
        (
            "aud",
            vec![
                json!(4242),
                json!(true),
                json!({}),
                json!([4242]),
                json!(["a", 4242]),
            ],
        ),
        (
            "amr",
            vec![json!("never-shown"), json!(4242), json!({}), json!([4242])],
        ),
        (
            "scope",
            vec![
                json!(4242),
                json!(["never-shown"]),
                json!("never-shown  posts:read"),
                json!(" never-shown"),
                json!("never-shown "),
                json!("never\"shown"),
                json!("never\\shown"),
                json!("never-shown\tposts:read"),
            ],
        ),
    ];
    for (claim, values) in refused {
        for value in values {
            let claims = carrying(claim, value.clone());
            for minted in [
                jwt.sign(&claims),
                jwt.sign_for("https://api.example", &claims),
            ] {
                let said = nest_rs_core::error_message(
                    &minted.expect_err(&format!("minted `{claim}`: {value}")),
                );
                assert!(
                    said.contains(&format!("the claims' `{claim}` is no ")),
                    "{value}: {said}"
                );
                for quoted in ["never", "4242"] {
                    assert!(!said.contains(quoted), "{value}: {said}");
                }
            }
        }
    }
}

/// What each specification allows passes as written: a StringOrURI, an
/// audience of one or several, a NumericDate integral or not, the scope list
/// in RFC 6749 §3.3's grammar — empty when the token grants none.
#[test]
fn a_registered_claim_of_its_json_type_is_kept_as_written() {
    use serde_json::json;

    let jwt = service("registered-kept");
    let later = in_an_hour();
    for (claim, value) in [
        ("iss", json!("https://issuer.example")),
        ("jti", json!("a-token-of-its-own")),
        ("client_id", json!("ci")),
        ("acr", json!("urn:mace:incommon:iap:silver")),
        ("exp", json!(later)),
        ("exp", json!(later as f64 + 0.5)),
        ("nbf", json!(0)),
        ("iat", json!(later)),
        ("auth_time", json!(1_700_000_000)),
        ("aud", json!("https://api.example")),
        ("aud", json!(["https://api.example", "https://mcp.example"])),
        ("amr", json!(["pwd", "mfa"])),
        ("amr", json!([])),
        ("scope", json!("posts:read posts:write")),
        ("scope", json!("")),
    ] {
        let token = jwt.sign(&carrying(claim, value.clone())).expect("signs");
        assert_eq!(payload(&token)[claim], value, "{claim}");
    }
}

/// A registered claim the claims serialize as `null` names nothing, so the
/// token leaves it out rather than carry a value its verifier reads as none or
/// refuses.
#[test]
fn a_registered_claim_serialized_as_null_is_absent() {
    let jwt = service("registered-null");
    for claim in ["client_id", "acr", "amr", "auth_time", "scope", "nbf"] {
        let token = jwt
            .sign(&carrying(claim, serde_json::Value::Null))
            .expect("signs");
        assert!(
            payload(&token).get(claim).is_none(),
            "{claim}: {}",
            payload(&token)
        );
    }
}

/// A flow's handshake carries registered claims too, held to the same types.
#[test]
fn a_handshake_carrying_a_registered_claim_of_another_type_is_refused() {
    let jwt = service("handshake-types");
    let refused = jwt.sign_handshake(
        "oauth-tx",
        &serde_json::json!({ "exp": jwt.expiry_in(600), "iss": 4242 }),
    );
    let said = nest_rs_core::error_message(&refused.expect_err("minted a numeric `iss`"));
    assert!(said.contains("the claims' `iss` is no "), "{said}");
}
