//! e2e suite root: the module list plus the fixtures the siblings share.

mod authorization;
mod posts_tool;
mod shared_endpoint;
mod tool;

use std::sync::Arc;
use std::time::Duration;

use assistant::AssistantModule;
use nest_rs::authn::JwtConfig;
use nest_rs::config::Config;
use nest_rs::storage::{Storage, StorageConfig};
use nest_rs::testing::{EphemeralDatabase, TestApp};
use sea_orm::sea_query::Query;
use sea_orm::{ConnectionTrait, DatabaseConnection, DeriveIden};
use uuid::Uuid;

use features::testing::{AUDIENCE, DEV_PUBLIC_KEY, ORG_ID};

pub(crate) async fn boot() -> (EphemeralDatabase, TestApp) {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("create + migrate a throwaway database");
    let app = TestApp::builder()
        .module::<AssistantModule>()
        .provide_arc(db.connection())
        .provide(JwtConfig {
            public_key: Some(DEV_PUBLIC_KEY.into()),
            audience: Some(AUDIENCE.into()),
            ..Default::default()
        })
        .build()
        .await
        .expect("AssistantModule boots against the throwaway database");
    (db, app)
}

#[derive(DeriveIden)]
enum Org {
    Table,
    Id,
    Name,
}

#[derive(DeriveIden)]
enum Post {
    Table,
    Id,
    OrgId,
    AuthorId,
    Title,
    Body,
    Status,
}

#[derive(DeriveIden)]
enum User {
    Table,
    Id,
    OrgId,
    Name,
    Email,
    Role,
}

pub(crate) async fn seed_org_with_post(
    db: &DatabaseConnection,
    org_name: &str,
    title: &str,
) -> Uuid {
    let org_id = Uuid::now_v7();
    let author_id = Uuid::now_v7();

    let org = Query::insert()
        .into_table(Org::Table)
        .columns([Org::Id, Org::Name])
        .values_panic([org_id.into(), org_name.into()])
        .to_owned();
    db.execute(&org).await.expect("seed org");

    let user = Query::insert()
        .into_table(User::Table)
        .columns([User::Id, User::OrgId, User::Name, User::Email, User::Role])
        .values_panic([
            author_id.into(),
            org_id.into(),
            format!("{org_name} author").into(),
            format!("{}@example.com", org_id.simple()).into(),
            "admin".into(),
        ])
        .to_owned();
    db.execute(&user).await.expect("seed author");

    let post = Query::insert()
        .into_table(Post::Table)
        .columns([
            Post::Id,
            Post::OrgId,
            Post::AuthorId,
            Post::Title,
            Post::Body,
            Post::Status,
        ])
        .values_panic([
            Uuid::now_v7().into(),
            org_id.into(),
            author_id.into(),
            title.into(),
            "seeded".into(),
            "draft".into(),
        ])
        .to_owned();
    db.execute(&post).await.expect("seed post");

    org_id
}

pub(crate) fn bearer_for(org_id: &str) -> String {
    format!(
        "Bearer {}",
        features::testing::token_for(org_id, "admin", None)
    )
}

pub(crate) fn bearer() -> String {
    bearer_for(ORG_ID)
}

pub(crate) fn storage_client() -> Storage {
    let config = StorageConfig::load().expect("storage config parses from env");
    Storage::new(Arc::new(config))
}

pub(crate) async fn ensure_bucket() {
    if let Ok(url) = storage_client()
        .presign_put("", Duration::from_secs(60))
        .await
    {
        let _ = reqwest::Client::new().put(&url).send().await;
    }
}
