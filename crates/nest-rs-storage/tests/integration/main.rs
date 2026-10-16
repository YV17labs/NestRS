//! `nest-rs-storage`'s suite in process: an app importing `StorageModule` over
//! `memory:///`, which boots with no server and holds every object in memory.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a suite fails by panicking; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod client;
mod module;

use std::sync::Arc;

use nest_rs_core::injectable;
use nest_rs_storage::{Storage, StorageConfig, StorageModule};
use nest_rs_testing::{HeadlessApp, TestApp};

/// A feature service injecting the client, as an app's would.
#[injectable]
struct UploadsService {
    #[inject]
    storage: Arc<Storage>,
}

#[nest_rs_core::module(
    imports = [StorageModule::for_root(StorageConfig {
        url: "memory:///uploads".into(),
        ..StorageConfig::default()
    })],
    providers = [UploadsService],
)]
struct UploadsModule;

/// The app's client, booted over the in-process store.
async fn booted() -> (HeadlessApp, Arc<Storage>) {
    let app = TestApp::builder()
        .module::<UploadsModule>()
        .build_headless()
        .await
        .expect("memory boots with no server");
    let storage = Arc::clone(
        &app.container()
            .get::<UploadsService>()
            .expect("the service is provided")
            .storage,
    );
    (app, storage)
}
