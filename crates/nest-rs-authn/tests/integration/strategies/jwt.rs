//! Covers `src/strategies/jwt.rs`.

use std::sync::Arc;

use jsonwebtoken::get_current_timestamp;
use nest_rs_authn::{AuthError, JwtOptions, JwtService, JwtStrategy, PrincipalIdentity, Strategy};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct TestClaims {
    sub: String,
    client_id: String,
    exp: u64,
}

impl nest_rs_core::AmbientValue for TestClaims {}

impl PrincipalIdentity for TestClaims {
    fn actor_id(&self) -> Option<String> {
        Some(self.sub.clone())
    }
}

#[tokio::test]
async fn authenticates_with_valid_bearer_token() {
    let jwt = Arc::new(
        JwtService::new(crate::named(JwtOptions::new(
            "strategy-secret-padded-to-thirty-two",
        )))
        .expect("jwt"),
    );
    let strategy = JwtStrategy::<TestClaims>::new(jwt.clone());
    let token = jwt
        .sign(&TestClaims {
            sub: "alice".into(),
            client_id: "ci".into(),
            exp: get_current_timestamp() + 3600,
        })
        .expect("sign");
    let mut req = crate::request(&[("Authorization", &format!("Bearer {token}"))]);

    let claims = strategy.authenticate(&mut req).await.expect("authenticate");
    assert_eq!(claims.sub, "alice");
}

#[tokio::test]
async fn missing_bearer_credentials_are_rejected() {
    let jwt = Arc::new(
        JwtService::new(crate::named(JwtOptions::new(
            "strategy-secret-padded-to-thirty-two",
        )))
        .expect("jwt"),
    );
    let strategy = JwtStrategy::<TestClaims>::new(jwt);
    let mut req = crate::request(&[]);

    assert!(matches!(
        strategy.authenticate(&mut req).await,
        Err(AuthError::MissingCredentials)
    ));
}

#[tokio::test]
async fn invalid_bearer_token_is_rejected() {
    let jwt = Arc::new(
        JwtService::new(crate::named(JwtOptions::new(
            "strategy-secret-padded-to-thirty-two",
        )))
        .expect("jwt"),
    );
    let strategy = JwtStrategy::<TestClaims>::new(jwt);
    let mut req = crate::request(&[("Authorization", "Bearer not-a-jwt")]);

    assert!(matches!(
        strategy.authenticate(&mut req).await,
        Err(AuthError::InvalidToken)
    ));
}

/// RFC 9068 §4: a resource server refuses a token whose `iss` is not its
/// issuer's or whose `aud` does not name it — one naming neither included.
#[tokio::test]
async fn a_token_naming_another_or_no_issuer_or_audience_is_refused() {
    use crate::{AUDIENCE, ISSUER};

    const SECRET: &str = "strategy-secret-padded-to-thirty-two";
    let strategy = JwtStrategy::<TestClaims>::new(Arc::new(
        JwtService::new(crate::named(JwtOptions::new(SECRET))).expect("jwt"),
    ));
    let minted = |claims: serde_json::Value| {
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
        header.typ = Some("at+jwt".into());
        let token = jsonwebtoken::encode(
            &header,
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
        )
        .expect("encode");
        crate::request(&[("Authorization", &format!("Bearer {token}"))])
    };
    let exp = get_current_timestamp() + 3600;

    let mut named = minted(serde_json::json!({
        "sub": "alice", "client_id": "ci", "exp": exp, "iss": ISSUER, "aud": AUDIENCE,
    }));
    strategy
        .authenticate(&mut named)
        .await
        .expect("a token naming both verifies");
    for (label, claims) in [
        (
            "no issuer",
            serde_json::json!({ "sub": "alice", "client_id": "ci", "exp": exp, "aud": AUDIENCE }),
        ),
        (
            "no audience",
            serde_json::json!({ "sub": "alice", "client_id": "ci", "exp": exp, "iss": ISSUER }),
        ),
        (
            "neither",
            serde_json::json!({ "sub": "alice", "client_id": "ci", "exp": exp }),
        ),
        (
            "another issuer",
            serde_json::json!({ "sub": "alice", "client_id": "ci", "exp": exp, "iss": "https://evil.example", "aud": AUDIENCE }),
        ),
        (
            "another audience",
            serde_json::json!({ "sub": "alice", "client_id": "ci", "exp": exp, "iss": ISSUER, "aud": "https://billing.example" }),
        ),
    ] {
        assert!(
            matches!(
                strategy.authenticate(&mut minted(claims)).await,
                Err(AuthError::InvalidToken)
            ),
            "a token naming {label} must be refused"
        );
    }
}
