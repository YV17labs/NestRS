//! The namespaced-config flow end to end: a `#[config]` struct loaded by
//! `ConfigModule::for_feature`, injected into a service as `Arc<C>`, booted
//! through the real DI graph. Also proves the testability promise — a seeded
//! config wins over the environment-reading factory.

use std::sync::Arc;

use nestrs_config::{config, ConfigModule};
use nestrs_core::{injectable, module};
use nestrs_testing::TestApp;
use serde::Deserialize;
use validator::Validate;

#[config(namespace = "demoapp")]
#[derive(Clone, Debug, Deserialize, Validate)]
struct DemoConfig {
    url: String,
    #[validate(range(min = 1))]
    max_connections: u32,
}

#[injectable]
struct DemoService {
    #[inject]
    cfg: Arc<DemoConfig>,
}

impl DemoService {
    fn url(&self) -> &str {
        &self.cfg.url
    }
    fn max_connections(&self) -> u32 {
        self.cfg.max_connections
    }
}

#[module(
    imports = [ConfigModule::for_feature::<DemoConfig>()],
    providers = [DemoService],
)]
struct DemoModule;

// One test with two phases rather than two parallel tests: the env vars are
// process-global, so a single sequential test keeps the two boots from racing
// on `NESTRS_DEMOAPP__*`.
#[tokio::test]
async fn for_feature_loads_injects_and_a_seed_overrides_the_environment() {
    std::env::set_var("NESTRS_DEMOAPP__URL", "postgres://from-env/app");
    std::env::set_var("NESTRS_DEMOAPP__MAX_CONNECTIONS", "7");

    // Phase 1: the factory loads `DemoConfig` from the namespace prefix, and the
    // service injects it as `Arc<DemoConfig>` — the `ConfigType` + `.KEY` collapse.
    let app = TestApp::builder()
        .module::<DemoModule>()
        .build_headless()
        .await
        .expect("the config-backed module boots");
    let svc = app
        .container()
        .get::<DemoService>()
        .expect("DemoService is registered");
    assert_eq!(svc.url(), "postgres://from-env/app");
    assert_eq!(svc.max_connections(), 7);
    // The loaded config is global infrastructure — injectable / resolvable anywhere.
    assert!(
        app.container().get::<DemoConfig>().is_some(),
        "the loaded config is a factory output, present in the container"
    );

    // Phase 2: a seeded config wins over the env-reading factory (seed-wins), so a
    // test pins configuration without touching the environment.
    let app = TestApp::builder()
        .module::<DemoModule>()
        .provide(DemoConfig {
            url: "postgres://seeded/app".into(),
            max_connections: 99,
        })
        .build_headless()
        .await
        .expect("the seeded config boots");
    let svc = app.container().get::<DemoService>().unwrap();
    assert_eq!(svc.url(), "postgres://seeded/app", "the seed wins over env");
    assert_eq!(svc.max_connections(), 99);

    std::env::remove_var("NESTRS_DEMOAPP__URL");
    std::env::remove_var("NESTRS_DEMOAPP__MAX_CONNECTIONS");
}
