//! `OutboxModule::for_root`, the relay's one seam, booted as an app writes it.

use std::time::Duration;

use nest_rs_core::module;
use nest_rs_queue::{OutboxConfig, OutboxModule};
use nest_rs_testing::HeadlessApp;

#[module(imports = [OutboxModule::for_root(OutboxConfig {
    sweep_interval: Duration::from_millis(250),
    batch_size: 10,
    stall_warning: Duration::from_secs(60),
})])]
struct PinnedApp;

#[tokio::test]
async fn a_pinned_config_reaches_the_relay_it_attaches() {
    let app = HeadlessApp::builder()
        .module::<PinnedApp>()
        .build_headless()
        .await
        .expect("the app boots");
    let config = app
        .container()
        .get::<OutboxConfig>()
        .expect("the relay's settings are resolved");
    assert_eq!(config.sweep_interval, Duration::from_millis(250));
    assert_eq!(config.batch_size, 10);
    assert_eq!(config.stall_warning, Duration::from_secs(60));
    assert_eq!(app.attached_transports(), ["outbox"]);
}
