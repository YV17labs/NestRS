//! The indicator `RedisModule::for_root` attaches for each instance it opens,
//! booted beside `HealthModule` alone over scripted servers answering `PING`:
//! the default instance answers the readiness and startup probes as `redis`,
//! a named one as `redis:cache` — opened at the root or by a feature the app
//! imports, once however many modules open it — and liveness waits on
//! neither. An app without `HealthModule` boots as it did.

use std::time::Duration;

use nest_rs_core::{App, module};
use nest_rs_health::{HealthModule, HealthService, IndicatorStatus, ProbeKind};
use nest_rs_redis::{RedisConfig, RedisModule};

use crate::connection::DemotedRedis;
use crate::harness::tls::trusting_the_test_authority;
use crate::module::{CACHE_URL, Cache, cache_config};

/// The default instance's config, on `server`: the suite's own
/// `<PREFIX>_REDIS__URL` would outrank a pin.
fn default_on(server: &DemotedRedis) -> RedisConfig {
    RedisConfig {
        url: server.url(),
        connect_timeout: Duration::from_secs(2),
        tls: trusting_the_test_authority(),
    }
}

/// The names `kind`'s report carries, each `up`.
async fn answering_up(health: &HealthService, kind: ProbeKind) -> Vec<String> {
    let report = health.probe(kind).await;
    assert_eq!(report.status, IndicatorStatus::Up, "{kind:?}: {report:#?}");
    report.details.keys().map(ToString::to_string).collect()
}

#[module(imports = [
    HealthModule,
    RedisModule::for_root(None),
    RedisModule::for_root((Cache, cache_config())),
])]
struct TwoOpened;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_instance_answers_the_readiness_and_startup_probes_under_its_own_name() {
    let primary = DemotedRedis::start().await;
    let cache = DemotedRedis::start().await;
    CACHE_URL.set(cache.url()).expect("set once");
    let app = App::builder()
        .provide(default_on(&primary))
        .module::<TwoOpened>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("both instances boot: {refused:#}"));
    let health = app
        .container()
        .get::<HealthService>()
        .expect("HealthModule provides it");

    for kind in [ProbeKind::Readiness, ProbeKind::Startup] {
        assert_eq!(
            answering_up(&health, kind).await,
            ["redis", "redis:cache"],
            "{kind:?}"
        );
    }
    assert!(
        health.probe(ProbeKind::Liveness).await.details.is_empty(),
        "a Valkey outage is no reason to restart the process"
    );
}

/// A feature opening the cache with the base it pins.
#[module(imports = [RedisModule::for_root((Cache, cache_config()))])]
struct CachingFeature;

/// A second feature opening the same instance, bare.
#[module(imports = [RedisModule::for_root((Cache, None))])]
struct CacheReadingFeature;

/// An app naming no Redis of its own: its features open the cache.
#[module(imports = [HealthModule, CachingFeature, CacheReadingFeature])]
struct FeaturesOpenTheCache;

#[tokio::test]
async fn an_instance_features_open_answers_once_in_an_app_that_never_names_it() {
    let cache = DemotedRedis::start().await;
    CACHE_URL.set(cache.url()).expect("set once");
    let app = App::builder()
        .module::<FeaturesOpenTheCache>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("two openers of one instance boot: {refused:#}"));
    let health = app
        .container()
        .get::<HealthService>()
        .expect("HealthModule provides it");

    for kind in [ProbeKind::Readiness, ProbeKind::Startup] {
        assert_eq!(
            answering_up(&health, kind).await,
            ["redis:cache"],
            "{kind:?}"
        );
    }
}

#[module(imports = [RedisModule::for_root(None)])]
struct OpenedUnprobed;

#[tokio::test]
async fn without_health_module_an_app_opening_an_instance_boots_and_serves_no_probe() {
    let primary = DemotedRedis::start().await;
    let app = App::builder()
        .provide(default_on(&primary))
        .module::<OpenedUnprobed>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("boots: {refused:#}"));
    assert!(
        app.container().get::<HealthService>().is_none(),
        "the indicator imports no probe of its own"
    );
}
