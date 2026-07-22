use auth::{AuthModule, IssuerConfig, RegisteredClient};
use base64::Engine as _;
use nest_rs_authn::{JwtConfig, JwtOptions, JwtService, hash_password};
use nest_rs_social::{GithubSocialConfig, GoogleSocialConfig};
use nest_rs_testing::{EphemeralDatabase, TestApp};
use sea_orm::sea_query::{OnConflict, Query};
use sea_orm::{ConnectionTrait, DatabaseConnection, DeriveIden};
use uuid::Uuid;

pub(crate) use features::testing::{DEV_PRIVATE_KEY, DEV_PUBLIC_KEY, ORG_ID};

pub(crate) const LOGIN_EMAIL: &str = "alice@example.com";
pub(crate) const LOGIN_PASSWORD: &str = "correct-horse";
pub(crate) const CLIENT_ID: &str = "demo-service";
pub(crate) const CLIENT_SECRET: &str = "demo-service-secret";

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

pub(crate) fn resource_server_verifier() -> JwtService {
    JwtService::new(JwtOptions::eddsa_verify(DEV_PUBLIC_KEY)).expect("the dev public key parses")
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
            hash.into(),
        ])
        .to_owned();
    db.execute(&user_stmt).await.expect("seed login user");
}
