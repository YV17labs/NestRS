use auth::{AppModule, IssuerConfig, RegisteredClient};
use base64::Engine as _;
use domain::{Claims, Role};
use nestrs_authn::{JwtConfig, JwtOptions, JwtService, OAuth2Config};
use nestrs_testing::{EphemeralDatabase, TestApp};
use poem::http::StatusCode;

const ORG_ID: &str = "018f0000-0000-7000-8000-000000000000";
const CLIENT_ID: &str = "demo-service";
const CLIENT_SECRET: &str = "demo-service-secret";

fn basic_auth(client_id: &str, client_secret: &str) -> String {
    let raw = format!("{client_id}:{client_secret}");
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(raw)
    )
}

const DEV_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIEYTRN4vmCuIfaUslO5G9pKyxkDJn3q3t9WDHo2FCfw3\n-----END PRIVATE KEY-----\n";
const DEV_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAHfPOjd2Y3m1BLM5nBJBMZFAlfWt69WL1NY8XyYeGfeo=\n-----END PUBLIC KEY-----\n";

async fn boot() -> (EphemeralDatabase, TestApp) {
    let db = EphemeralDatabase::create::<db::Migrator>()
        .await
        .expect("create + migrate a throwaway database");
    let app = TestApp::builder()
        .module::<AppModule>()
        .with_test_telemetry()
        .provide_arc(db.connection())
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
        .provide(IssuerConfig {
            clients: vec![
                RegisteredClient {
                    client_id: CLIENT_ID.into(),
                    client_secret: CLIENT_SECRET.into(),
                    org_id: ORG_ID.parse().expect("a valid org uuid"),
                    scopes: vec!["admin".into(), "user".into()],
                },
                RegisteredClient {
                    client_id: "limited-service".into(),
                    client_secret: "limited-secret".into(),
                    org_id: ORG_ID.parse().expect("a valid org uuid"),
                    scopes: vec!["user".into()],
                },
            ],
            default_org_id: ORG_ID.parse().expect("a valid org uuid"),
        })
        .build()
        .await
        .expect("the auth app boots");
    (db, app)
}

fn resource_server_verifier() -> JwtService {
    JwtService::new(JwtOptions::eddsa_verify(DEV_PUBLIC_KEY)).expect("the dev public key parses")
}

#[tokio::test]
async fn token_endpoint_issues_a_token_the_public_key_verifies() {
    let (_db, app) = boot().await;

    let resp = app
        .http()
        .post("/token")
        .header("authorization", basic_auth(CLIENT_ID, CLIENT_SECRET))
        .content_type("application/x-www-form-urlencoded")
        .body("grant_type=client_credentials&scope=admin+user")
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
    let (_db, app) = boot().await;
    app.http()
        .post("/token")
        .header("authorization", basic_auth(CLIENT_ID, CLIENT_SECRET))
        .content_type("application/x-www-form-urlencoded")
        .body("grant_type=password")
        .send()
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn token_endpoint_rejects_an_unauthenticated_client() {
    let (_db, app) = boot().await;
    app.http()
        .post("/token")
        .content_type("application/x-www-form-urlencoded")
        .body("grant_type=client_credentials&scope=user")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn token_endpoint_rejects_a_bad_client_secret() {
    let (_db, app) = boot().await;
    app.http()
        .post("/token")
        .header("authorization", basic_auth(CLIENT_ID, "wrong-secret"))
        .content_type("application/x-www-form-urlencoded")
        .body("grant_type=client_credentials&scope=user")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn token_endpoint_rejects_a_scope_beyond_the_client_grant() {
    let (_db, app) = boot().await;
    app.http()
        .post("/token")
        .header("authorization", basic_auth("limited-service", "limited-secret"))
        .content_type("application/x-www-form-urlencoded")
        .body("grant_type=client_credentials&scope=admin")
        .send()
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn token_endpoint_derives_the_org_from_the_authenticated_client() {
    let (_db, app) = boot().await;
    let resp = app
        .http()
        .post("/token")
        .header("authorization", basic_auth("limited-service", "limited-secret"))
        .content_type("application/x-www-form-urlencoded")
        .body("grant_type=client_credentials")
        .send()
        .await;
    resp.assert_status_is_ok();

    let json = resp.json().await;
    let token = json.value().object().get("access_token").string().to_owned();
    let claims: Claims = resource_server_verifier()
        .verify(&token)
        .expect("the public key verifies the privately-signed token");
    assert_eq!(claims.org_id.to_string(), ORG_ID);
    assert_eq!(claims.roles, vec![Role::User]);
}

#[tokio::test]
async fn the_oauth_authorize_endpoint_redirects_to_the_provider() {
    let (_db, app) = boot().await;
    let resp = app.http().get("/authorize").send().await;
    resp.assert_status(StatusCode::FOUND);
    resp.assert_header_exist("location");
    resp.assert_header_exist("set-cookie");
}
