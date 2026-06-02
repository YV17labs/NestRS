//! Covers `src/passport/strategies/jwt.rs`.

use std::sync::Arc;

use jsonwebtoken::get_current_timestamp;
use nestrs_authn::{AuthError, JwtOptions, JwtService, JwtStrategy, Outcome, Strategy};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct TestClaims {
    sub: String,
    exp: u64,
}

#[tokio::test]
async fn authenticates_with_valid_bearer_token() {
    let jwt = Arc::new(JwtService::new(JwtOptions::new("strategy-secret")).expect("jwt"));
    let strategy = JwtStrategy::<TestClaims>::new(jwt.clone());
    let token = jwt
        .sign(&TestClaims {
            sub: "alice".into(),
            exp: get_current_timestamp() + 3600,
        })
        .expect("sign");
    let mut req = crate::common::request(&[("Authorization", &format!("Bearer {token}"))]);

    match strategy.authenticate(&mut req).await.expect("authenticate") {
        Outcome::Authenticated(claims) => assert_eq!(claims.sub, "alice"),
        Outcome::Challenge(_) => panic!("expected authenticated principal"),
    }
}

#[tokio::test]
async fn missing_bearer_credentials_are_rejected() {
    let jwt = Arc::new(JwtService::new(JwtOptions::new("strategy-secret")).expect("jwt"));
    let strategy = JwtStrategy::<TestClaims>::new(jwt);
    let mut req = crate::common::request(&[]);

    assert!(matches!(
        strategy.authenticate(&mut req).await,
        Err(AuthError::MissingCredentials)
    ));
}

#[tokio::test]
async fn invalid_bearer_token_is_rejected() {
    let jwt = Arc::new(JwtService::new(JwtOptions::new("strategy-secret")).expect("jwt"));
    let strategy = JwtStrategy::<TestClaims>::new(jwt);
    let mut req = crate::common::request(&[("Authorization", "Bearer not-a-jwt")]);

    assert!(matches!(
        strategy.authenticate(&mut req).await,
        Err(AuthError::InvalidToken)
    ));
}
