use auth::{AuthModule, ClientPayload, SignInConfig};
use base64::Engine as _;
use features::Role;
use features::authz::constants;
use nest_rs::authn::{AuthnConfig, JwtOptions, JwtService, hash_password};
use nest_rs::oauth::server::{OAuthServerConfig, RegisteredClient};
use nest_rs::social::{GithubSocialConfig, GoogleSocialConfig};
use nest_rs::testing::{EphemeralDatabase, TestApp, TestResponse};
use sea_orm::sea_query::{OnConflict, Query};
use sea_orm::{ConnectionTrait, DatabaseConnection, DeriveIden};
use uuid::Uuid;

pub(crate) use features::testing::{API, ASSISTANT, DEV_PRIVATE_KEY, DEV_PUBLIC_KEY, ORG_ID};

pub(crate) const LOGIN_EMAIL: &str = "alice@example.com";
pub(crate) const LOGIN_PASSWORD: &str = "correct-horse";
pub(crate) const CLIENT_ID: &str = "demo-service";
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

pub(crate) async fn boot() -> (EphemeralDatabase, TestApp) {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("create + migrate a throwaway database");
    let app = TestApp::builder()
        .module::<AuthModule>()
        .with_test_telemetry()
        .provide_arc(db.connection())
        .provide(AuthnConfig {
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
            default_org_id: ORG_ID.parse().expect("a valid org uuid"),
        })
        .build()
        .await
        .expect("the auth app boots");
    (db, app)
}

fn payload(role: Role) -> ClientPayload {
    ClientPayload {
        org_id: ORG_ID.parse().expect("a valid org uuid"),
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

pub(crate) fn resource_server_verifier(resource: &str) -> JwtService {
    let mut options = JwtOptions::eddsa_verify(DEV_PUBLIC_KEY);
    options.audience = Some(resource.into());
    JwtService::new(options).expect("the dev public key parses")
}

pub(crate) async fn seed_org_and_user(db: &DatabaseConnection) {
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
            hash.expose_secret().clone().into(),
        ])
        .to_owned();
    db.execute(&user_stmt).await.expect("seed login user");
}
