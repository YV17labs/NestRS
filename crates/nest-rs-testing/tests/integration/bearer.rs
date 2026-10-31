//! `TestApp::bearer`: a token the app's own `JwtService` signs, over claims
//! built from the expiry it stamps, which that service then verifies.

use nest_rs_authn::{AuthnConfig, AuthnModule, JwtService};
use nest_rs_core::{Secret, module};
use nest_rs_testing::TestApp;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: u64,
}

#[module(imports = [AuthnModule])]
struct SigningModule;

const SECRET: &str = "a-test-secret-of-thirty-two-bytes!";

#[tokio::test]
async fn the_bearer_is_one_the_apps_own_service_verifies() {
    let app = TestApp::builder()
        .module::<SigningModule>()
        .provide(AuthnConfig {
            secret: Some(Secret::new(SECRET.into())),
            ..AuthnConfig::default()
        })
        .build()
        .await
        .expect("the app boots signing with its secret");
    let jwt = app.container().get::<JwtService>().expect("provided");

    let authorization = app.bearer(|exp| Claims {
        sub: "ada".into(),
        exp,
    });

    let token = authorization
        .strip_prefix("Bearer ")
        .expect("an Authorization value of the Bearer scheme");
    let claims: Claims = jwt
        .verify(token)
        .await
        .expect("the app's service verifies it");
    assert_eq!(claims.sub, "ada");
    assert!(
        claims.exp.abs_diff(jwt.expiry()) <= 1,
        "the claims carry the expiry the service stamps",
    );
}

#[tokio::test]
#[should_panic(expected = "the app's JwtService signs no token")]
async fn an_app_that_only_verifies_signs_no_bearer() {
    let app = TestApp::builder()
        .module::<SigningModule>()
        .provide(AuthnConfig {
            public_key: Some(PUBLIC_KEY.into()),
            ..AuthnConfig::default()
        })
        .build()
        .await
        .expect("the app boots verifying only");
    let _ = app.bearer(|exp| Claims {
        sub: "ada".into(),
        exp,
    });
}

/// The public half of an Ed25519 pair, with no private key beside it.
const PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAHfPOjd2Y3m1BLM5nBJBMZFAlfWt69WL1NY8XyYeGfeo=\n-----END PUBLIC KEY-----\n";
