//! Shared boot + token helpers for this suite.

use std::sync::Arc;
use std::time::Duration;

use assistant::AssistantModule;
use nest_rs_authn::JwtConfig;
use nest_rs_config::{Config, ConfigService};
use nest_rs_storage::{Storage, StorageConfig};
use nest_rs_testing::TestApp;
use serde_json::json;

use features::testing::{DEV_PUBLIC_KEY, ORG_ID};

pub(crate) async fn boot() -> TestApp {
    TestApp::builder()
        .module::<AssistantModule>()
        .with_test_telemetry()
        .provide(JwtConfig {
            public_key: Some(DEV_PUBLIC_KEY.into()),
            ..Default::default()
        })
        .build()
        .await
        .expect("AssistantModule boots")
}

pub(crate) fn bearer() -> String {
    format!(
        "Bearer {}",
        features::testing::token_for(ORG_ID, "admin", None)
    )
}

pub(crate) fn storage_client() -> Storage {
    // The real config loader (`NESTRS_STORAGE__*` + in-code defaults) — no
    // hand-copied env override list to drift from it.
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

pub(crate) fn initialize_request() -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "nestrs-e2e", "version": "0" }
        }
    })
}
