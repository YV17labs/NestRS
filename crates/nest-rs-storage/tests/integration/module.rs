//! `StorageModule` (`src/module.rs`) booted over `memory:///`: the store opens
//! with no network, and the client reads its URL back; and a client a test
//! overrides, whose store the boot never opens.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::injectable;
use nest_rs_storage::{Storage, StorageConfig, StorageModule};
use nest_rs_testing::HeadlessApp;

use crate::booted;

#[tokio::test]
async fn memory_boots_with_no_network_and_shows_its_url() {
    let (_app, storage) = booted().await;
    assert_eq!(storage.url(), "memory:///uploads");
}

/// A feature service injecting the client a test overrides.
#[injectable]
struct ReportsService {
    #[inject]
    storage: Arc<Storage>,
}

#[nest_rs_core::module(
    imports = [StorageModule::for_root(StorageConfig {
        url: "s3://unreached".into(),
        endpoint: Some("http://127.0.0.1:9".into()),
        allow_http: true,
        access_key: "AKIAUNREACHED".into(),
        secret_key: "unreached-secret".into(),
        operation_timeout: Duration::from_secs(1),
        ..StorageConfig::default()
    })],
    providers = [ReportsService],
)]
struct ReportsModule;

#[tokio::test]
async fn an_overridden_client_boots_without_opening_the_store_it_replaces() {
    let builder = HeadlessApp::builder();
    let double = Storage::connect(&StorageConfig {
        url: "memory:///double".into(),
        ..StorageConfig::default()
    })
    .await
    .expect("memory opens with no network");
    let app = builder
        .override_value(double)
        .module::<ReportsModule>()
        .build_headless()
        .await
        .expect("an overridden client's store is never opened");
    let service = app
        .container()
        .get::<ReportsService>()
        .expect("the service is provided");
    assert_eq!(service.storage.url(), "memory:///double");
}
