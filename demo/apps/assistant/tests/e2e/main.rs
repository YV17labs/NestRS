#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]
mod authorization;
mod machine_client;
mod posts_tool;
mod shared_endpoint;
mod tool;

use assistant::AssistantModule;
use features::Role;
use features::posts::{ActiveModel as PostActive, PostStatus};
use features::users::UserRole;
use nest_rs::testing::TestApp;
use sea_orm::{ActiveModelTrait, Set};
use uuid::Uuid;

use features::testing::{ORG_ID, claims, connection, delete_transcode, seed_org, seed_user, user};

pub(crate) const SHARED_ENDPOINT: &str = "/mcp";

pub(crate) const POSTS_ENDPOINT: &str = "/mcp/posts";

pub(crate) async fn boot() -> TestApp {
    TestApp::builder()
        .module::<AssistantModule>()
        .ephemeral_database::<migrations::Migrator>()
        .ephemeral_signing_key()
        .build()
        .await
        .expect("AssistantModule boots against a throwaway database")
}

pub(crate) async fn seed_org_with_post(app: &TestApp, org_name: &str, title: &str) -> Uuid {
    let conn = connection(app.container());
    let org = seed_org(&conn, org_name).await;
    let author = seed_user(
        &conn,
        org,
        &format!("{org_name} author"),
        &format!("{}@example.com", org.simple()),
        UserRole::Admin,
    )
    .await;
    PostActive {
        id: Set(Uuid::now_v7()),
        org_id: Set(org),
        author_id: Set(author),
        title: Set(title.into()),
        body: Set("seeded".into()),
        status: Set(PostStatus::Draft),
        ..Default::default()
    }
    .insert(conn.as_ref())
    .await
    .expect("seed post");
    org
}

pub(crate) fn sign(app: &TestApp, org_id: Uuid) -> String {
    app.bearer(&claims(org_id, Role::Admin, user()))
}
