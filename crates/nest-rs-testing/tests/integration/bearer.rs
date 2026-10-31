//! `TestApp::bearer`: a token the app's own `JwtService` signs over a claims
//! value, stamping its lifetime, which that service then verifies.

use std::time::{SystemTime, UNIX_EPOCH};

use nest_rs_authn::{AuthnConfig, AuthnModule, JwtService};
use nest_rs_core::{Secret, module};
use nest_rs_testing::TestApp;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct Claims {
    sub: String,
}

#[derive(Deserialize)]
struct Verified {
    sub: String,
    exp: u64,
}

#[module(imports = [AuthnModule])]
struct SigningModule;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("after the epoch")
        .as_secs()
}

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

    let authorization = app.bearer(&Claims { sub: "ada".into() });

    let token = authorization
        .strip_prefix("Bearer ")
        .expect("an Authorization value of the Bearer scheme");
    let claims: Verified = jwt
        .verify(token)
        .await
        .expect("the app's service verifies it");
    assert_eq!(claims.sub, "ada");
    assert!(
        claims.exp.abs_diff(now() + jwt.ttl_secs()) <= 1,
        "the token carries the expiry the service stamps",
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
    let _ = app.bearer(&Claims { sub: "ada".into() });
}

/// The public half of an Ed25519 pair, with no private key beside it.
const PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAHfPOjd2Y3m1BLM5nBJBMZFAlfWt69WL1NY8XyYeGfeo=\n-----END PUBLIC KEY-----\n";
