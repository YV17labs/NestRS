use std::sync::Arc;
use std::time::Duration;

use assistant::AssistantModule;
use nest_rs_authn::JwtConfig;
use nest_rs_config::{Config, ConfigService};
use nest_rs_storage::{Storage, StorageConfig};
use nest_rs_testing::{EphemeralDatabase, TestApp};

use features::testing::{DEV_PUBLIC_KEY, ORG_ID};

pub(crate) async fn boot() -> (EphemeralDatabase, TestApp) {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("create + migrate a throwaway database");
    let app = TestApp::builder()
        .module::<AssistantModule>()
        .with_test_telemetry()
        .provide_arc(db.connection())
        .provide(JwtConfig {
            public_key: Some(DEV_PUBLIC_KEY.into()),
            ..Default::default()
        })
        .build()
        .await
        .expect("AssistantModule boots against the throwaway database");
    (db, app)
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
    let config = StorageConfig::from_env(&ConfigService::for_namespace("storage"))
        .expect("storage config parses from env");
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
