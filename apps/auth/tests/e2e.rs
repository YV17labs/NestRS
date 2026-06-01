use auth::AppModule;
use identity::{Claims, Role};
use nestrs_authn::{JwtConfig, JwtOptions, JwtService, OAuth2Config};
use nestrs_testing::TestApp;
use poem::http::StatusCode;

const ORG_ID: &str = "018f0000-0000-7000-8000-000000000000";

/// Non-secret sample EdDSA keypair, test-only — so the issuer can sign without
/// `NESTRS_AUTHN__*` in the environment, and the matching public key verifies.
const DEV_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIEYTRN4vmCuIfaUslO5G9pKyxkDJn3q3t9WDHo2FCfw3\n-----END PRIVATE KEY-----\n";
const DEV_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAHfPOjd2Y3m1BLM5nBJBMZFAlfWt69WL1NY8XyYeGfeo=\n-----END PUBLIC KEY-----\n";

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<AppModule>()
        .with_test_telemetry()
        // Seed the signing config + OAuth provider so the boot needs no
        // `NESTRS_AUTHN__*` in the environment (seed wins over the env factory).
        .provide(JwtConfig {
            private_key: Some(DEV_PRIVATE_KEY.into()),
            public_key: Some(DEV_PUBLIC_KEY.into()),
            ..Default::default()
        })
        .provide(OAuth2Config {
            client_id: "demo-client-id".into(),
            client_secret: "demo-client-secret".into(),
            auth_url: "https://github.com/login/oauth/authorize".into(),
            token_url: "https://github.com/login/oauth/access_token".into(),
            userinfo_url: "https://api.github.com/user".into(),
            redirect_url: "http://localhost:3002/callback".into(),
            scopes: vec!["read:user".into()],
        })
        .build()
        .await
        .expect("the auth app boots")
}

fn resource_server_verifier() -> JwtService {
    JwtService::new(JwtOptions::eddsa_verify(DEV_PUBLIC_KEY)).expect("the dev public key parses")
}

#[tokio::test]
async fn token_endpoint_issues_a_token_the_public_key_verifies() {
    let app = boot().await;

    let resp = app
        .http()
        .post("/token")
        .content_type("application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=client_credentials&org_id={ORG_ID}&scope=admin+user"
        ))
        .send()
        .await;
    resp.assert_status_is_ok();

    let json = resp.json().await;
    let obj = json.value().object();
    assert_eq!(obj.get("token_type").string(), "Bearer");
    assert!(obj.get("expires_in").i64() > 0);
    let token = obj.get("access_token").string().to_owned();

    let claims: Claims = resource_server_verifier()
        .verify(&token)
        .expect("the public key verifies the privately-signed token");
    assert_eq!(claims.org_id.to_string(), ORG_ID);
    assert!(claims.roles.contains(&Role::Admin));
}

#[tokio::test]
async fn token_endpoint_rejects_an_unsupported_grant() {
    let app = boot().await;
    app.http()
        .post("/token")
        .content_type("application/x-www-form-urlencoded")
        .body(format!("grant_type=password&org_id={ORG_ID}"))
        .send()
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_oauth_authorize_endpoint_redirects_to_the_provider() {
    let app = boot().await;
    let resp = app.http().get("/authorize").send().await;
    resp.assert_status(StatusCode::FOUND);
    resp.assert_header_exist("location");
    resp.assert_header_exist("set-cookie");
}
