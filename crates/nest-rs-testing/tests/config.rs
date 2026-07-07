//! Namespaced-config flow end-to-end: load via `ConfigModule::for_feature`,
//! inject as `Arc<C>`, and verify a seed wins over the env-reading factory.

use std::sync::Arc;

use nest_rs_config::{Config, ConfigModule, ConfigService, MapSource, config};
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

// `from_env` reads its namespaced vars hermetically from a `MapSource` — no
// process env, no `.env`, no `unsafe`. This is the config-read half of the
// wiring; the module boot below covers load/inject/seed-override.
#[test]
fn from_env_maps_each_namespaced_var_from_its_source() {
    let service = ConfigService::with_source(
        "demoapp",
        Arc::new(MapSource::from_iter([
            ("NESTRS_DEMOAPP__URL", "postgres://from-env/app"),
            ("NESTRS_DEMOAPP__MAX_CONNECTIONS", "7"),
        ])),
    );
    let cfg = DemoConfig::from_env(&service).expect("reads from the source");
    assert_eq!(cfg.url, "postgres://from-env/app");
    assert_eq!(cfg.max_connections, 7);
}

#[tokio::test]
async fn for_feature_loads_injects_and_a_seed_overrides_the_factory() {
    // No env set: the `for_feature` factory loads the in-code defaults and
    // injects the config as `Arc<DemoConfig>`.
    let app = TestApp::builder()
        .module::<DemoModule>()
        .build_headless()
        .await
        .expect("the config-backed module boots");
    let svc = app
        .container()
        .get::<DemoService>()
        .expect("DemoService is registered");
    assert_eq!(svc.url(), "", "unset ⇒ the config's in-code default");
    assert_eq!(svc.max_connections(), 10);
    assert!(
        app.container().get::<DemoConfig>().is_some(),
        "the loaded config is a factory output, present in the container"
    );

    // A seeded config wins over the env-reading factory.
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
    assert_eq!(
        svc.url(),
        "postgres://seeded/app",
        "the seed wins over the factory"
    );
    assert_eq!(svc.max_connections(), 99);
}
