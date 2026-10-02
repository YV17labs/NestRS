use api::ApiModule;
use nest_rs::authn::AuthnConfig;
use nest_rs::schedule::Scheduler;
use nest_rs::testing::{EphemeralDatabase, TestApp};

use crate::{AUDIENCE, DEV_PUBLIC_KEY};

#[tokio::test]
async fn api_app_binds_the_lock_its_one_replica_transcode_seed_claims_through() {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("create + migrate a throwaway database");
    let api = TestApp::builder()
        .module::<ApiModule>()
        .provide_arc(db.connection())
        .provide(AuthnConfig {
            public_key: Some(DEV_PUBLIC_KEY.into()),
            audience: Some(AUDIENCE.into()),
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
