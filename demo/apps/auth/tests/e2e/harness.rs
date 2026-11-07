use auth::{AuthModule, ClientPayload, SignInConfig};
use base64::Engine as _;
use features::Role;
use features::authz::constants;
use features::orgs::ActiveModel as OrgActive;
use features::testing::connection;
use features::users::{ActiveModel as UserActive, UserRole};
use nest_rs::authn::{AuthnConfig, JwtOptions, JwtService, hash_password};
use nest_rs::oauth::server::{OAuthServerConfig, RegisteredClient};
use nest_rs::testing::{TestApp, TestResponse};
use sea_orm::{ActiveModelTrait, Set};
use uuid::Uuid;

pub(crate) use features::testing::{API, ASSISTANT, CLIENT_ID, ORG_ID};

pub(crate) const LOGIN_EMAIL: &str = "alice@example.com";
pub(crate) const LOGIN_PASSWORD: &str = "correct-horse";
pub(crate) const CLIENT_SECRET: &str = "demo-service-secret";
pub(crate) const READER_ID: &str = "reader-service";
pub(crate) const READER_SECRET: &str = "reader-secret";
pub(crate) const FOREIGN_MEDIA_TYPE: &str = "text/x-quoted-marker";

pub(crate) fn basic_auth(client_id: &str, client_secret: &str) -> String {
    let raw = format!("{client_id}:{client_secret}");
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(raw)
    )
}

pub(crate) async fn boot() -> TestApp {
    TestApp::builder()
        .module::<AuthModule>()
        .ephemeral_database::<migrations::Migrator>()
        .provide(OAuthServerConfig {
            clients: vec![
                RegisteredClient::new(
                    CLIENT_ID,
                    CLIENT_SECRET,
                    constants::all(),
                    [API, ASSISTANT],
                    payload(Role::Admin),
                ),
                RegisteredClient::new(
                    READER_ID,
                    READER_SECRET,
                    [constants::POSTS_READ],
                    [API],
                    payload(Role::User),
                ),
            ],
        })
        .provide(SignInConfig {
            resources: vec![API.into(), ASSISTANT.into()],
            default_org_id: ORG_ID,
            ..SignInConfig::default()
        })
        .build()
        .await
        .expect("the auth app boots signing with the key its own file names")
}

fn payload(role: Role) -> ClientPayload {
    ClientPayload {
        org_id: ORG_ID,
        roles: vec![role],
    }
}

pub(crate) async fn access_token(resp: TestResponse) -> String {
    resp.json()
        .await
        .value()
        .object()
        .get("access_token")
        .string()
        .to_owned()
}

pub(crate) fn resource_server_verifier(app: &TestApp, resource: &str) -> JwtService {
    let issuer = app
        .container()
        .get::<AuthnConfig>()
        .expect("the auth app reads its key material");
    let mut options = JwtOptions::eddsa_verify(
        issuer
            .public_key
            .clone()
            .expect("the issuer's public key, which every resource server verifies with"),
    );
    options.audience = Some(resource.into());
    JwtService::new(options).expect("the issuer's public key parses")
}

pub(crate) async fn seed_org_and_user(app: &TestApp) {
    let conn = connection(app.container());
    OrgActive {
        id: Set(ORG_ID),
        name: Set("Demo".into()),
        ..Default::default()
    }
    .insert(conn.as_ref())
    .await
    .expect("seed org");
    UserActive {
        id: Set(Uuid::now_v7()),
        org_id: Set(ORG_ID),
        name: Set("Alice".into()),
        email: Set(LOGIN_EMAIL.into()),
        role: Set(UserRole::Admin),
        password_hash: Set(Some(hash_password(LOGIN_PASSWORD).expect("hash password"))),
        ..Default::default()
    }
    .insert(conn.as_ref())
    .await
    .expect("seed login user");
}
