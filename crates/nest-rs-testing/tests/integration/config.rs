//! Namespaced config end-to-end: `ConfigModule::for_feature`, a seed winning
//! over the factory and held to the checks a read passes, and a config pinned
//! in code overridden per field.

use nest_rs_config::var_name;
use std::any::TypeId;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use nest_rs_config::{Config, ConfigError, ConfigModule, ConfigService, MapSource, config};
use nest_rs_core::{Collecting, ContainerBuilder, DynamicModule, injectable, module};
use nest_rs_http::HttpConfig;
use nest_rs_testing::HeadlessApp;

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
            url: env.get("URL")?.unwrap_or(base.url),
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
    let app = HeadlessApp::builder()
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

    let app = HeadlessApp::builder()
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
/// to `ConfigModule::provide_feature`.
struct DemoPinnedSetup(DemoConfig);

impl DynamicModule for DemoPinnedSetup {
    fn module() -> TypeId {
        TypeId::of::<Self>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        ConfigModule::provide_feature(Some(self.0.clone()), builder)
    }
}

fn pinned_module() -> DemoPinnedSetup {
    DemoPinnedSetup(DemoConfig {
        max_connections: 42,
        ..Default::default()
    })
}

#[module(imports = [pinned_module()], providers = [DemoService])]
struct DemoPinnedModule;

#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn a_pinned_config_still_lets_the_environment_override_each_field() {
    // `Jail` returns nothing, and `#[tokio::test]` would already be running a
    // runtime, so the boot runs inside and reports through `seen`.
    let mut seen: Option<(String, u32)> = None;
    figment::Jail::expect_with(|jail| {
        jail.set_env(var_name("demoapp", "URL"), "postgres://from-deployment/app");
        seen = Some(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(async {
                    let app = HeadlessApp::builder()
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

#[tokio::test]
async fn a_pinned_field_survives_an_environment_that_says_nothing() {
    let app = HeadlessApp::builder()
        .module::<DemoPinnedModule>()
        .build_headless()
        .await
        .expect("the pinned-config module boots");
    let svc = app.container().get::<DemoService>().unwrap();
    assert_eq!(svc.max_connections(), 42, "the pin is the base");
    assert_eq!(svc.url(), "", "and an unpinned field keeps its own default");
}

#[tokio::test]
async fn a_seed_outside_the_validation_its_read_passes_fails_the_boot_naming_the_namespace() {
    let Err(refused) = HeadlessApp::builder()
        .module::<DemoModule>()
        .provide(DemoConfig {
            url: String::new(),
            max_connections: 0,
        })
        .build_headless()
        .await
    else {
        panic!("a seed no read would accept boots");
    };
    let config = refused
        .downcast_ref::<ConfigError>()
        .unwrap_or_else(|| panic!("not the config's refusal: {refused:#}"));
    assert!(
        matches!(config, ConfigError::Validation { namespace, .. } if namespace == "demoapp"),
        "{refused:#}"
    );
}

static PROBE_BUILT: AtomicBool = AtomicBool::new(false);

struct Probe;

/// A factory reading the seeded `HttpConfig`, as a pool reads its own.
struct ProbeSetup;

impl DynamicModule for ProbeSetup {
    fn module() -> TypeId {
        TypeId::of::<Self>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        ConfigModule::provide_feature(None::<HttpConfig>, builder)
            .provide_factory_after::<Probe, (HttpConfig,), _, _>(|_| async {
                PROBE_BUILT.store(true, Ordering::SeqCst);
                Ok(Probe)
            })
    }
}

fn probe() -> ProbeSetup {
    ProbeSetup
}

#[module(imports = [probe()])]
struct ProbeModule;

#[tokio::test]
async fn a_seed_past_a_bound_fails_the_boot_before_any_factory_reads_it() {
    let Err(refused) = HeadlessApp::builder()
        .module::<ProbeModule>()
        .provide(HttpConfig {
            shutdown_timeout: Duration::MAX,
            ..HttpConfig::default()
        })
        .build_headless()
        .await
    else {
        panic!("a seed past its bound boots");
    };
    assert!(
        refused.to_string().contains("HttpConfig::shutdown_timeout"),
        "{refused:#}"
    );
    assert!(
        !PROBE_BUILT.load(Ordering::SeqCst),
        "the factory reading the seed never ran"
    );
}

#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn a_valid_seed_is_kept_as_seeded_whatever_the_environment_says() {
    let mut seen: Option<String> = None;
    figment::Jail::expect_with(|jail| {
        jail.set_env(var_name("demoapp", "URL"), "postgres://from-deployment/app");
        seen = Some(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(async {
                    let app = HeadlessApp::builder()
                        .module::<DemoModule>()
                        .provide(DemoConfig {
                            url: "postgres://seeded/app".into(),
                            max_connections: 3,
                        })
                        .build_headless()
                        .await
                        .expect("a valid seed boots");
                    app.container()
                        .get::<DemoService>()
                        .unwrap()
                        .url()
                        .to_owned()
                }),
        );
        Ok(())
    });
    assert_eq!(seen.as_deref(), Some("postgres://seeded/app"));
}
