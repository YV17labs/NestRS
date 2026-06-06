//! Namespaced-config flow end-to-end: load via `ConfigModule::for_feature`,
//! inject as `Arc<C>`, and verify a seed wins over the env-reading factory.

use std::sync::Arc;

use nest_rs_config::{Config, ConfigModule, ConfigService, config};
use nest_rs_core::{injectable, module};
use nest_rs_testing::TestApp;
use validator::Validate;

#[config(namespace = "demoapp")]
#[derive(Clone, Debug, Validate)]
struct DemoConfig {
    url: String,
    #[validate(range(min = 1))]
    max_connections: u32,
}

impl Config for DemoConfig {
    fn from_env(env: &ConfigService) -> nest_rs_config::Result<Self> {
        Ok(Self {
            url: env.get("URL").unwrap_or_default(),
            max_connections: env.parse("MAX_CONNECTIONS")?.unwrap_or(10),
        })
    }
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

// One sequential test (two boots) so concurrent env mutation doesn't race.
#[tokio::test]
async fn for_feature_loads_injects_and_a_seed_overrides_the_environment() {
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("NESTRS_DEMOAPP__URL", "postgres://from-env/app") };
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("NESTRS_DEMOAPP__MAX_CONNECTIONS", "7") };

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
    assert!(
        app.container().get::<DemoConfig>().is_some(),
        "the loaded config is a factory output, present in the container"
    );

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

    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("NESTRS_DEMOAPP__URL") };
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("NESTRS_DEMOAPP__MAX_CONNECTIONS") };
}
