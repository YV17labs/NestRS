//! Covers `src/service.rs` — `JwtService` sign/verify and decode error mapping.

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
    // Pad to ≥ 32 bytes so the HS256 min-secret guard (SEC-F3) passes, while the
    // label keeps each test's secret distinct.
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
    // SEC-F3: the ≥256-bit rule must hold at the derivation point
    // (`JwtService::new`), not only on the config-env path — so the documented
    // honest-API constructor `JwtOptions::new` can't mint a forgeable-key service.
    // `JwtService` has no `Debug` (secrets must not leak), so match rather than
    // `.expect_err`.
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

#[test]
fn audience_must_match_when_configured() {
    let mut options = JwtOptions::new("aud-secret-padded-to-thirty-two-bytes");
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
    let secret = "aud-required-secret-padded-to-32-bytes";
    let mut options = JwtOptions::new(secret);
    options.audience = Some("api".into());
    let jwt = JwtService::new(options).expect("service");

    // Forged with the raw encoder — another holder of the shared key minting a
    // token that omits `aud`. `set_audience` alone only *compares* a present
    // claim, so this is the case that must fail closed.
    let omitted = claims(jwt.expiry(), None);
    assert!(omitted.aud.is_none());
    let forged = jsonwebtoken::encode(
        &at_jwt_header(Algorithm::HS256),
        &omitted,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("encode");
    assert!(matches!(
        jwt.verify::<TestClaims>(&forged),
        Err(AuthError::InvalidToken)
    ));

    // Our own signer stamps the configured audience, so a claims struct that
    // leaves `aud` unset still mints a token this service accepts — the app
    // never has to restate the scoping its config already declares.
    let token = jwt.sign(&claims(jwt.expiry(), None)).expect("sign");
    let round_tripped: TestClaims = jwt.verify(&token).expect("stamped aud verifies");
    assert_eq!(round_tripped.aud.as_deref(), Some("api"));

    // An explicit audience in the claims is never overwritten.
    let mut present = claims(jwt.expiry(), None);
    present.aud = Some("api".into());
    let token = jwt.sign(&present).expect("sign");
    assert!(jwt.verify::<TestClaims>(&token).is_ok());
}

#[test]
fn a_configured_issuer_is_stamped_and_required() {
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
    let back: IssClaims = jwt.verify(&token).expect("stamped iss verifies");
    assert_eq!(back.iss.as_deref(), Some("auth"));

    // The same claims encoded without the stamp are refused.
    let forged = jsonwebtoken::encode(
        &at_jwt_header(Algorithm::HS256),
        &minted,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("encode");
    assert!(matches!(
        jwt.verify::<IssClaims>(&forged),
        Err(AuthError::InvalidToken)
    ));
}

#[test]
fn invalid_algorithm_is_rejected() {
    let jwt = service("alg-secret");
    let header = at_jwt_header(Algorithm::HS384);
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
        crate::DEV_PRIVATE_KEY,
        crate::DEV_PUBLIC_KEY,
    ))
    .expect("EdDSA service");
    let token = jwt.sign(&claims(jwt.expiry(), None)).expect("sign");
    let decoded: TestClaims = jwt.verify(&token).expect("verify");
    assert_eq!(decoded.sub, "alice");
}

#[test]
fn a_token_for_another_service_is_rejected_when_no_audience_is_configured() {
    // The confused deputy, and the regression this file exists to hold: the
    // default path (`AuthnModule::for_root(None)` with no audience configured)
    // switched jsonwebtoken's `validate_aud` off, so a token the shared issuer
    // minted *for a sibling service* verified here. RFC 7519 §4.1.3 says a
    // principal that does not identify itself with a value in a present `aud`
    // MUST reject the JWT — and it binds a verifier naming no audience too.
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
            jwt.verify::<TestClaims>(&token),
            Err(AuthError::InvalidToken)
        ),
        "a validly-signed token minted for another audience must not verify here",
    );
}

#[test]
fn an_audience_less_token_still_verifies_when_no_audience_is_configured() {
    // The other half of §4.1.3, and why the clause costs nothing: it fires only
    // when the claim is *present*. A deployment that names no audience and whose
    // issuer stamps none is untouched.
    let jwt = service("no-aud-anywhere-secret");
    let token = jwt.sign(&claims(jwt.expiry(), None)).expect("sign");
    let decoded: TestClaims = jwt.verify(&token).expect("verify");
    assert!(decoded.aud.is_none());
}

#[test]
fn allow_any_audience_is_the_named_opt_out_and_reports_itself() {
    // The permissive behaviour survives, but only as something a deployment
    // wrote down — and it says so once per boot, naming the variable, so an
    // operator can find who disabled the check.
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
        jwt.verify::<TestClaims>(&token).is_ok(),
        "the opt-out is what restores the old permissive reading",
    );
}

#[test]
fn allow_any_audience_beside_a_configured_audience_is_refused() {
    // Two fields stating opposite policies. Letting either win silently is how a
    // deployment comes to believe the stricter one is in force.
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
/// value". §2.1 names the attack this closes: an OpenID Connect ID Token,
/// signed by the same issuer with the same key, must not be spendable as an
/// access token here.
#[test]
fn a_token_typed_as_anything_else_is_refused() {
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
                jwt.verify::<TestClaims>(&token),
                Err(nest_rs_authn::AuthError::InvalidToken)
            ),
            "typ={typ} must not verify as an access token",
        );
    }
}

/// §4 names both spellings, and RFC 9110 §8.3.1 makes a media type
/// case-insensitive — so the long form and an odd casing both verify.
#[test]
fn the_long_media_type_and_odd_casing_both_verify() {
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
        assert!(jwt.verify::<TestClaims>(&token).is_ok(), "typ={typ}");
    }
}

/// The opt-out exists for an issuer that predates the profile, and it is the
/// only way a plain `typ: JWT` verifies.
#[test]
fn explicit_typing_can_be_turned_off_for_a_legacy_issuer() {
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
    assert!(jwt.verify::<TestClaims>(&token).is_ok());
}
