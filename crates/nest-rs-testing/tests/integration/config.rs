//! Namespaced-config flow end-to-end: load via `ConfigModule::for_feature`,
//! inject as `Arc<C>`, verify a seed wins over the env-reading factory, and —
//! the dual-path rule the framework promises — verify a config **pinned in
//! code** still lets the environment override it **per field**.

use nest_rs_config::var_name;
use std::sync::Arc;

use nest_rs_config::{Config, ConfigModule, ConfigService, MapSource, config};
use nest_rs_core::{ContainerBuilder, DynamicModule, injectable, module};
use nest_rs_testing::TestApp;

#[config(namespace = "demoapp")]
#[derive(Clone, Debug)]
struct DemoConfig {
    url: String,
    #[validate(range(min = 1))]
    max_connections: u32,
}

impl Default for DemoConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            max_connections: 10,
        }
    }
}

impl Config for DemoConfig {
    fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
        Ok(Self {
            url: env.get("URL").unwrap_or(base.url),
            max_connections: env
                .parse("MAX_CONNECTIONS")?
                .unwrap_or(base.max_connections),
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
            (var_name("demoapp", "URL"), "postgres://from-env/app"),
            (var_name("demoapp", "MAX_CONNECTIONS"), "7"),
        ])),
    );
    let cfg = DemoConfig::from_env(&service, Default::default()).expect("reads from the source");
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

/// What every configurable module's `for_root(cfg)` does: hand the pinned struct
/// to `ConfigModule::provide_feature`. Standing in for `HttpModule::for_root`
/// here keeps the test on the seam all of them share rather than on one crate's
/// spelling of it.
struct DemoPinnedSetup(DemoConfig);

impl DynamicModule for DemoPinnedSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        ConfigModule::provide_feature(Some(self.0.clone()), builder)
    }
}

fn pinned_module() -> DemoPinnedSetup {
    // The scaffold's shape: pin one field, let `..Default::default()` fill the
    // rest. That is exactly the case that used to freeze the whole namespace.
    DemoPinnedSetup(DemoConfig {
        max_connections: 42,
        ..Default::default()
    })
}

#[module(imports = [pinned_module()], providers = [DemoService])]
struct DemoPinnedModule;

/// The finding this closes: `provide_feature(Some(cfg))` registered the struct
/// verbatim, so **every** `NESTRS_DEMOAPP__*` variable went inert — a deployment
/// setting one of them got silence, not an override. Pinning one field must
/// leave the others live, and a set variable must beat the pin.
#[test]
#[allow(clippy::result_large_err)] // figment::Jail's fixed closure signature
fn a_pinned_config_still_lets_the_environment_override_each_field() {
    // `Jail` isolates the real process env (and reverts it), which is what the
    // deployment tier means — no `unsafe { set_var }` in this crate. It hands
    // nothing back out, so the boot's observations land in `seen`, and the
    // runtime is built inside rather than via `#[tokio::test]` (which would
    // already be running one).
    let mut seen: Option<(String, u32)> = None;
    figment::Jail::expect_with(|jail| {
        jail.set_env(var_name("demoapp", "URL"), "postgres://from-deployment/app");
        seen = Some(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(async {
                    let app = TestApp::builder()
                        .module::<DemoPinnedModule>()
                        .build_headless()
                        .await
                        .expect("the pinned-config module boots");
                    let svc = app.container().get::<DemoService>().unwrap();
                    (svc.url().to_owned(), svc.max_connections())
                }),
        );
        Ok(())
    });

    let (url, max_connections) = seen.expect("the jailed boot ran");
    assert_eq!(
        url, "postgres://from-deployment/app",
        "a deployment variable must reach a module configured in code",
    );
    assert_eq!(
        max_connections, 42,
        "and the field the call site actually pinned must survive",
    );
}

/// The other half: with nothing in the environment, the pin is the answer. Guards
/// against "fixing" the override by dropping the pinned value on the floor.
#[tokio::test]
async fn a_pinned_field_survives_an_environment_that_says_nothing() {
    let app = TestApp::builder()
        .module::<DemoPinnedModule>()
        .build_headless()
        .await
        .expect("the pinned-config module boots");
    let svc = app.container().get::<DemoService>().unwrap();
    assert_eq!(svc.max_connections(), 42, "the pin is the base");
    assert_eq!(svc.url(), "", "and an unpinned field keeps its own default");
}
