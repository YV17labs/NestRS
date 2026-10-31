use api::ApiModule;
use features::Role;
use features::authz::constants;
use features::testing::{ASSISTANT, claims, client, signed_for};
use nest_rs::authn::AuthnConfig;
use nest_rs::http::{StatusCode, header};
use nest_rs::schedule::Scheduler;
use nest_rs::testing::{EphemeralDatabase, HeadlessApp, TestApp, TestResponse};

use crate::{API, DEV_PUBLIC_KEY, ORG_ID};

#[tokio::test]
async fn the_api_requires_its_own_audience_read_from_its_own_file() {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("create + migrate a throwaway database");
    let app = TestApp::builder()
        .module::<ApiModule>()
        .provide_arc(db.connection())
        .build()
        .await
        .expect("ApiModule boots on the configuration `nestrs run dev` reads");
    let claims = claims(
        ORG_ID.parse().expect("a valid org uuid"),
        vec![Role::Admin],
        client(),
        constants::all(),
    );

    list_users(&app, &signed_for(ASSISTANT, &claims))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    list_users(&app, &signed_for(API, &claims))
        .await
        .assert_status_is_ok();
}

async fn list_users(app: &TestApp, token: &str) -> TestResponse {
    app.http()
        .get("/users")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .send()
        .await
}

#[tokio::test]
async fn api_app_binds_the_lock_its_one_replica_transcode_seed_claims_through() {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("create + migrate a throwaway database");
    let api = HeadlessApp::builder()
        .module::<ApiModule>()
        .provide_arc(db.connection())
        .provide(AuthnConfig {
            public_key: Some(DEV_PUBLIC_KEY.into()),
            audience: Some(API.into()),
            ..Default::default()
        })
        .build_headless()
        .await
        .expect("ApiModule boots against the throwaway database and Redis");
    let scheduler = api.spawn_transport(Scheduler::new()).await.expect(
        "the scheduler configures: the transcode seed declared replicas = \"one\" has a lock",
    );
    scheduler
        .shutdown()
        .await
        .expect("the scheduler stops cleanly");
}
