//! `OutboxModule::for_root`, the relay's one seam, booted as an app writes it.

use std::time::Duration;

use nest_rs_core::{App, Discovery, TransportContribution, module};
use nest_rs_queue::{OutboxConfig, OutboxModule};

#[module(imports = [OutboxModule::for_root(OutboxConfig {
    sweep_interval: Duration::from_millis(250),
    batch_size: 10,
    stall_warning: Duration::from_secs(60),
})])]
struct PinnedApp;

#[tokio::test]
async fn a_pinned_config_reaches_the_relay_it_attaches() {
    let app = App::builder()
        .module::<PinnedApp>()
        .build()
        .await
        .expect("the app boots");
    let config = app
        .container()
        .get::<OutboxConfig>()
        .expect("the relay's settings are resolved");
    assert_eq!(config.sweep_interval, Duration::from_millis(250));
    assert_eq!(config.batch_size, 10);
    assert_eq!(config.stall_warning, Duration::from_secs(60));
    let attached: Vec<String> = Discovery::new(app.container())
        .meta::<TransportContribution>()
        .iter()
        .map(|contribution| contribution.meta.name().to_owned())
        .collect();
    assert_eq!(attached, ["OutboxRelay"]);
}
