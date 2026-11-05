use api::ApiModule;
use features::Role;
use features::testing::{API, ASSISTANT, with_claim};
use nest_rs::http::{StatusCode, header};
use nest_rs::schedule::Scheduler;
use nest_rs::testing::{HeadlessApp, TestApp, TestResponse};

use crate::{ORG_ID, boot, claims, client};

#[tokio::test]
async fn the_api_requires_its_own_audience_read_from_its_own_file() {
    let app = boot().await;
    let for_resource = |resource: &str| {
        app.bearer(&with_claim(
            claims(ORG_ID, Role::Admin, client()),
            "aud",
            resource,
        ))
    };

    list_users(&app, &for_resource(ASSISTANT))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    list_users(&app, &for_resource(API))
        .await
        .assert_status_is_ok();
}

async fn list_users(app: &TestApp, authorization: &str) -> TestResponse {
    app.http()
        .get("/users")
        .header(header::AUTHORIZATION, authorization)
        .send()
        .await
}

#[tokio::test]
async fn api_app_binds_the_lock_its_one_replica_transcode_seed_claims_through() {
    let api = HeadlessApp::builder()
        .module::<ApiModule>()
        .ephemeral_database::<migrations::Migrator>()
        .build_headless()
        .await
        .expect("ApiModule boots against a throwaway database and Redis");
    let scheduler = api.spawn_transport(Scheduler::new()).await.expect(
        "the scheduler configures: the transcode seed declared replicas = \"one\" has a lock",
    );
    scheduler
        .shutdown()
        .await
        .expect("the scheduler stops cleanly");
}
