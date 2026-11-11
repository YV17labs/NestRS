//! `TestApp::bearer`: a token signed with the key `ephemeral_signing_key`
//! installs, which the app verifies as it verifies any — for its audience,
//! from its issuer — whatever key material its configuration names, and
//! whether or not the app signs.

use nest_rs_authn::__private::sign_unchecked;
use nest_rs_authn::{AuthnConfig, AuthnModule, JwtOptions, JwtService, PrincipalIdentity};
use nest_rs_core::{Secret, module};
use nest_rs_oauth_resource::{OAuthResourceConfig, OAuthResourceModule};
use nest_rs_testing::TestApp;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Serialize, PrincipalIdentity)]
struct Claims {
    #[principal(actor)]
    sub: String,
}

/// What a verified token says, its registered claims included.
#[derive(Deserialize)]
struct Read {
    sub: String,
    aud: String,
    iss: String,
    exp: u64,
}

#[module(imports = [AuthnModule])]
struct ResourceModule;

#[module(imports = [OAuthResourceModule])]
struct ProtectedModule;

#[module]
struct UnauthenticatedModule;

const AUDIENCE: &str = "https://api.example.com";
const ISSUER: &str = "https://auth.example.com";

/// The public half of an Ed25519 pair whose private half the test never holds.
const PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAHfPOjd2Y3m1BLM5nBJBMZFAlfWt69WL1NY8XyYeGfeo=\n-----END PUBLIC KEY-----\n";

/// `key` for a resource server's own audience, from one issuer.
fn named(key: AuthnConfig) -> AuthnConfig {
    AuthnConfig {
        audience: Some(AUDIENCE.into()),
        issuer: Some(ISSUER.into()),
        ..key
    }
}

/// A resource server verifying `key`, for its own audience, from one issuer.
async fn verifying(key: AuthnConfig) -> TestApp {
    TestApp::builder()
        .module::<ResourceModule>()
        .provide(named(key))
        .ephemeral_signing_key()
        .build()
        .await
        .expect("the app boots verifying the test's key")
}

fn ada() -> Claims {
    Claims { sub: "ada".into() }
}

fn token(authorization: &str) -> &str {
    authorization
        .strip_prefix("Bearer ")
        .expect("an Authorization value of the Bearer scheme")
}

fn jwt(app: &TestApp) -> std::sync::Arc<JwtService> {
    app.container()
        .get::<JwtService>()
        .expect("the app provides its JwtService")
}

#[tokio::test]
async fn a_verify_only_app_admits_the_bearer_the_test_signs() {
    let app = verifying(AuthnConfig {
        public_key: Some(PUBLIC_KEY.into()),
        ..AuthnConfig::default()
    })
    .await;
    let jwt = jwt(&app);

    let read: Read = jwt
        .verify(token(&app.bearer(&ada())))
        .await
        .expect("the app verifies the test's token");

    assert_eq!(read.sub, "ada");
    assert_eq!(read.aud, AUDIENCE, "signed for the app's audience");
    assert_eq!(read.iss, ISSUER, "signed as the app's issuer");
    assert!(
        read.exp.abs_diff(jwt.expiry_in(jwt.ttl_secs())) <= 1,
        "the claims left `exp` out, and the bearer carries the app's lifetime",
    );
}

#[tokio::test]
async fn a_verify_only_app_stays_verify_only() {
    let app = verifying(AuthnConfig {
        public_key: Some(PUBLIC_KEY.into()),
        ..AuthnConfig::default()
    })
    .await;

    assert!(
        jwt(&app).sign(&ada()).is_err(),
        "the test's private key is the test's: the app signs in its suite only when it signs in production",
    );
}

#[tokio::test]
async fn an_app_verifying_an_issuers_key_set_admits_the_bearer_without_reaching_it() {
    let app = verifying(AuthnConfig {
        jwks_uri: Some("https://issuer.invalid/.well-known/jwks.json".into()),
        ..AuthnConfig::default()
    })
    .await;

    let read: Read = jwt(&app)
        .verify(token(&app.bearer(&ada())))
        .await
        .expect("the app verifies the test's token, fetching no key set");
    assert_eq!(read.sub, "ada");
}

#[tokio::test]
async fn an_app_that_signs_signs_with_the_tests_key() {
    let app = verifying(AuthnConfig {
        secret: Some(Secret::new("a-test-secret-of-thirty-two-bytes!".into())),
        ..AuthnConfig::default()
    })
    .await;
    let jwt = jwt(&app);

    let own =
        sign_unchecked(&jwt, &json!({ "sub": "grace" })).expect("an app that signs keeps signing");
    let read: Read = jwt.verify(&own).await.expect("its own token verifies");
    assert_eq!(read.sub, "grace");
    jwt.verify::<Read>(token(&app.bearer(&ada())))
        .await
        .expect("and so does the test's, one key signing both");
}

#[tokio::test]
async fn an_app_whose_configuration_names_no_key_verifies_the_tests_alone() {
    let app = TestApp::builder()
        .module::<ResourceModule>()
        .provide(named(AuthnConfig::default()))
        .ephemeral_signing_key()
        .build()
        .await
        .expect("the key is the deployment's, and the test's stands in for it");
    let jwt = jwt(&app);

    let read: serde_json::Value = jwt
        .verify(token(&app.bearer(&ada())))
        .await
        .expect("the app verifies the test's token");
    assert_eq!(read["sub"], "ada");
    assert!(
        jwt.sign(&ada()).is_err(),
        "nothing says this app signs, so it does not",
    );
}

#[tokio::test]
async fn a_resource_server_whose_configuration_names_no_key_verifies_the_tests_alone() {
    let app = TestApp::builder()
        .module::<ProtectedModule>()
        .provide(OAuthResourceConfig {
            resource: Some(AUDIENCE.into()),
            authorization_servers: vec![ISSUER.into()],
            ..OAuthResourceConfig::default()
        })
        .ephemeral_signing_key()
        .build()
        .await
        .expect("the protected resource boots with no key of its own");

    let read: Read = jwt(&app)
        .verify(token(&app.bearer(&ada())))
        .await
        .expect("the bound verifier admits it");
    assert_eq!(read.aud, AUDIENCE);
    assert_eq!(read.iss, ISSUER);
}

#[tokio::test]
async fn a_configuration_the_boot_refuses_is_refused_with_a_key_of_the_tests() {
    let Err(refused) = TestApp::builder()
        .module::<ResourceModule>()
        .provide(AuthnConfig {
            private_key: Some(Secret::new("a private key without its public key".into())),
            ..AuthnConfig::default()
        })
        .ephemeral_signing_key()
        .build()
        .await
    else {
        panic!("a private key alone booted because the test holds a key");
    };
    let refused = format!("{refused:#}");
    assert!(refused.contains("is set without"), "{refused}");
}

#[tokio::test]
async fn a_claim_the_claims_carry_is_kept() {
    let app = verifying(AuthnConfig {
        public_key: Some(PUBLIC_KEY.into()),
        ..AuthnConfig::default()
    })
    .await;
    let jwt = jwt(&app);

    let soon = jwt.expiry_in(60);
    let read: Read = jwt
        .verify(token(&app.bearer(&json!({ "sub": "ada", "exp": soon }))))
        .await
        .expect("verified");
    assert_eq!(read.exp, soon, "an `exp` the claims carry is the token's");

    for (claim, value) in [
        ("aud", "https://other.example.com"),
        ("iss", "https://elsewhere.example.com"),
    ] {
        let refused = jwt
            .verify::<Read>(token(&app.bearer(&json!({ "sub": "ada", claim: value }))))
            .await;
        assert!(
            refused.is_err(),
            "a token whose `{claim}` names another is refused"
        );
    }
}

#[tokio::test]
async fn under_a_resource_binding_the_bearer_names_the_resource_and_its_server() {
    let app = TestApp::builder()
        .module::<ProtectedModule>()
        .provide(OAuthResourceConfig {
            resource: Some(AUDIENCE.into()),
            authorization_servers: vec![ISSUER.into(), "https://login.example.com".into()],
            ..OAuthResourceConfig::default()
        })
        .provide(AuthnConfig {
            public_key: Some(PUBLIC_KEY.into()),
            ..AuthnConfig::default()
        })
        .ephemeral_signing_key()
        .build()
        .await
        .expect("the protected resource boots");

    let read: Read = jwt(&app)
        .verify(token(&app.bearer(&ada())))
        .await
        .expect("the bound verifier admits it");
    assert_eq!(read.aud, AUDIENCE, "the resource is the audience");
    assert_eq!(
        read.iss, ISSUER,
        "the first authorization server is the issuer"
    );
}

#[tokio::test]
async fn an_app_verifying_no_token_refuses_a_key_of_the_tests() {
    let Err(refused) = TestApp::builder()
        .module::<UnauthenticatedModule>()
        .ephemeral_signing_key()
        .build()
        .await
    else {
        panic!("an app with no JwtService booted with a key nothing verifies with");
    };
    let refused = format!("{refused:#}");
    assert!(
        refused.contains("ephemeral_signing_key") && refused.contains("AuthnModule"),
        "{refused}"
    );
}

#[tokio::test]
async fn an_app_whose_verifier_was_seeded_refuses_a_key_of_the_tests() {
    let Err(refused) = TestApp::builder()
        .module::<ResourceModule>()
        .provide({
            let mut options = JwtOptions::eddsa_verify(PUBLIC_KEY);
            options.audience = Some(AUDIENCE.into());
            options.issuer = Some(ISSUER.into());
            JwtService::new(options).expect("the public key parses")
        })
        .ephemeral_signing_key()
        .build()
        .await
    else {
        panic!("an app verifying a seeded key booted with a key it refuses");
    };
    let refused = format!("{refused:#}");
    assert!(
        refused.contains("refuses a token of its ephemeral signing key"),
        "{refused}"
    );
}

#[tokio::test]
#[should_panic(expected = "ephemeral_signing_key")]
async fn an_app_booted_without_a_key_of_the_tests_signs_no_bearer() {
    let app = TestApp::builder()
        .module::<ResourceModule>()
        .provide(named(AuthnConfig {
            public_key: Some(PUBLIC_KEY.into()),
            ..AuthnConfig::default()
        }))
        .build()
        .await
        .expect("the app boots verifying its configured key");
    let _ = app.bearer(&ada());
}

#[tokio::test]
async fn a_socket_app_signs_as_the_in_process_one_does() {
    let app = TestApp::builder()
        .module::<ResourceModule>()
        .provide(named(AuthnConfig {
            public_key: Some(PUBLIC_KEY.into()),
            ..AuthnConfig::default()
        }))
        .ephemeral_signing_key()
        .build_ws()
        .await
        .expect("the app serves on a real port");
    let jwt = app
        .container()
        .get::<JwtService>()
        .expect("the app provides its JwtService");

    let read: serde_json::Value = jwt
        .verify(token(&app.bearer(&ada())))
        .await
        .expect("the app verifies the socket app's token");
    assert_eq!(read["sub"], "ada");
    app.close().await;
}
