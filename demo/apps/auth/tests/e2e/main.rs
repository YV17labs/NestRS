use auth::{AuthModule, IssuerConfig, RegisteredClient};
use base64::Engine as _;
use features::{Claims, Role};
use nest_rs_authn::{JwtConfig, JwtOptions, JwtService, hash_password};
use nest_rs_social::{GithubSocialConfig, GoogleSocialConfig};
use nest_rs_testing::{EphemeralDatabase, TestApp};
use poem::http::StatusCode;
use sea_orm::sea_query::{OnConflict, Query};
use sea_orm::{ConnectionTrait, DatabaseConnection, DeriveIden};
use serde_json::json;
use uuid::Uuid;

const ORG_ID: &str = "018f0000-0000-7000-8000-000000000000";
const LOGIN_EMAIL: &str = "alice@example.com";
const LOGIN_PASSWORD: &str = "correct-horse";
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
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("create + migrate a throwaway database");
    let app = TestApp::builder()
        .module::<AuthModule>()
        .with_test_telemetry()
        .provide_arc(db.connection())
        .provide(JwtConfig {
            private_key: Some(DEV_PRIVATE_KEY.into()),
            public_key: Some(DEV_PUBLIC_KEY.into()),
            ..Default::default()
        })
        .provide(GithubSocialConfig {
            client_id: "demo-github-client-id".into(),
            client_secret: "demo-github-client-secret".into(),
            redirect_url: "http://localhost:3001/social/github/callback".into(),
            scopes: vec![],
        })
        .provide(GoogleSocialConfig {
            client_id: "demo-google-client-id".into(),
            client_secret: "demo-google-client-secret".into(),
            redirect_url: "http://localhost:3001/social/google/callback".into(),
            scopes: vec![],
        })
        .provide(IssuerConfig {
            clients: vec![
                RegisteredClient {
                    client_id: CLIENT_ID.into(),
                    client_secret: CLIENT_SECRET.into(),
                    payload: ORG_ID.parse().expect("a valid org uuid"),
                    scopes: vec!["admin".into(), "user".into()],
                },
                RegisteredClient {
                    client_id: "limited-service".into(),
                    client_secret: "limited-secret".into(),
                    payload: ORG_ID.parse().expect("a valid org uuid"),
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

async fn seed_org_and_user(db: &DatabaseConnection) {
    #[derive(DeriveIden)]
    enum Org {
        Table,
        Id,
        Name,
    }
    #[derive(DeriveIden)]
    enum User {
        Table,
        Id,
        OrgId,
        Name,
        Email,
        Role,
        PasswordHash,
    }

    let org_id = Uuid::parse_str(ORG_ID).expect("valid org uuid");
    let org_stmt = Query::insert()
        .into_table(Org::Table)
        .columns([Org::Id, Org::Name])
        .values_panic([org_id.into(), "Demo".into()])
        .on_conflict(OnConflict::column(Org::Id).do_nothing().to_owned())
        .to_owned();
    db.execute(&org_stmt).await.expect("seed org");

    let hash = hash_password(LOGIN_PASSWORD).expect("hash password");
    let user_stmt = Query::insert()
        .into_table(User::Table)
        .columns([
            User::Id,
            User::OrgId,
            User::Name,
            User::Email,
            User::Role,
            User::PasswordHash,
        ])
        .values_panic([
            Uuid::now_v7().into(),
            org_id.into(),
            "Alice".into(),
            LOGIN_EMAIL.into(),
            "admin".into(),
            hash.into(),
        ])
        .to_owned();
    db.execute(&user_stmt).await.expect("seed login user");
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
        .header(
            "authorization",
            basic_auth("limited-service", "limited-secret"),
        )
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
        .header(
            "authorization",
            basic_auth("limited-service", "limited-secret"),
        )
        .content_type("application/x-www-form-urlencoded")
        .body("grant_type=client_credentials")
        .send()
        .await;
    resp.assert_status_is_ok();

    let json = resp.json().await;
    let token = json
        .value()
        .object()
        .get("access_token")
        .string()
        .to_owned();
    let claims: Claims = resource_server_verifier()
        .verify(&token)
        .expect("the public key verifies the privately-signed token");
    assert_eq!(claims.org_id.to_string(), ORG_ID);
    assert_eq!(claims.roles, vec![Role::User]);
}

#[tokio::test]
async fn the_social_authorize_endpoint_redirects_to_the_provider() {
    let (_db, app) = boot().await;
    let resp = app.http().get("/social/github/authorize").send().await;
    resp.assert_status(StatusCode::FOUND);
    resp.assert_header_exist("location");
    resp.assert_header_exist("set-cookie");
    let location = resp.0.headers().get("location").expect("location header");
    assert!(
        location
            .to_str()
            .expect("ascii location")
            .starts_with("https://github.com/login/oauth/authorize"),
        "redirect must hit GitHub, got {location:?}",
    );
}

#[tokio::test]
async fn the_provider_path_segment_is_case_insensitive() {
    let (_db, app) = boot().await;
    let resp = app.http().get("/social/GitHub/authorize").send().await;
    resp.assert_status(StatusCode::FOUND);
    let location = resp.0.headers().get("location").expect("location header");
    assert!(
        location
            .to_str()
            .expect("ascii location")
            .starts_with("https://github.com/login/oauth/authorize"),
        "case-insensitive provider must still hit GitHub, got {location:?}",
    );
}

#[tokio::test]
async fn a_configured_provider_that_is_not_imported_is_unknown() {
    let (_db, app) = boot().await;
    app.http()
        .get("/social/gitlab/authorize")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_social_callback_rejects_a_forged_state() {
    let (_db, app) = boot().await;
    app.http()
        .get("/social/github/callback?code=abc&state=forged")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_issues_a_token_the_public_key_verifies() {
    let (db, app) = boot().await;
    seed_org_and_user(db.connection().as_ref()).await;

    let resp = app
        .http()
        .post("/login")
        .body_json(&json!({
            "email": LOGIN_EMAIL,
            "password": LOGIN_PASSWORD,
        }))
        .send()
        .await;
    resp.assert_status_is_ok();

    let json = resp.json().await;
    let token = json
        .value()
        .object()
        .get("access_token")
        .string()
        .to_owned();
    let claims: Claims = resource_server_verifier()
        .verify(&token)
        .expect("the public key verifies the privately-signed token");
    assert_eq!(claims.org_id.to_string(), ORG_ID);
    assert!(claims.sub.is_some());
    assert!(claims.roles.contains(&Role::Admin));
}

#[tokio::test]
async fn login_rejects_bad_credentials() {
    let (db, app) = boot().await;
    seed_org_and_user(db.connection().as_ref()).await;

    app.http()
        .post("/login")
        .body_json(&json!({
            "email": LOGIN_EMAIL,
            "password": "wrong-password",
        }))
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_rejects_unknown_email() {
    let (_db, app) = boot().await;

    app.http()
        .post("/login")
        .body_json(&json!({
            "email": "nobody@example.com",
            "password": LOGIN_PASSWORD,
        }))
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}
