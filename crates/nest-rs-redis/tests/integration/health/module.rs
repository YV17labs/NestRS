//! `RedisHealthModule`, booted beside `HealthModule` over two instances, each a
//! scripted server answering `PING`: the default instance answers the readiness
//! and startup probes as `redis`, a named one as `redis:cache`, and liveness
//! waits on neither.

use std::sync::OnceLock;
use std::time::Duration;

use nest_rs_core::{App, module};
use nest_rs_health::{HealthModule, HealthService, IndicatorStatus, ProbeKind};
use nest_rs_redis::{RedisConfig, RedisHealthModule, RedisModule};

use crate::connection::DemotedRedis;
use crate::harness::tls::trusting_the_test_authority;
use crate::module::Cache;

/// The cache instance's server, set by the one test booting [`TwoProbed`]:
/// nextest runs each test in a process of its own.
static CACHE_URL: OnceLock<String> = OnceLock::new();

fn cache_config() -> RedisConfig {
    RedisConfig {
        url: CACHE_URL.get().cloned().unwrap_or_default(),
        connect_timeout: Duration::from_secs(2),
        tls: trusting_the_test_authority(),
    }
}

#[module(imports = [
    HealthModule,
    RedisModule::for_root(None),
    RedisModule::for_root((Cache, cache_config())),
    RedisHealthModule,
    RedisHealthModule::<Cache>,
])]
struct TwoProbed;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_instance_answers_the_readiness_and_startup_probes_under_its_own_name() {
    let primary = DemotedRedis::start().await;
    let cache = DemotedRedis::start().await;
    CACHE_URL.set(cache.url()).expect("set once");
    let app = App::builder()
        .provide(RedisConfig {
            url: primary.url(),
            connect_timeout: Duration::from_secs(2),
            tls: trusting_the_test_authority(),
        })
        .module::<TwoProbed>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("both instances boot: {refused:#}"));
    let health = app
        .container()
        .get::<HealthService>()
        .expect("HealthModule provides it");

    for kind in [ProbeKind::Readiness, ProbeKind::Startup] {
        let report = health.probe(kind).await;
        assert_eq!(report.status, IndicatorStatus::Up, "{kind:?}: {report:#?}");
        assert_eq!(
            report
                .details
                .keys()
                .map(|key| &**key)
                .collect::<Vec<&str>>(),
            ["redis", "redis:cache"],
            "{kind:?}: {report:#?}"
        );
    }
    assert!(
        health.probe(ProbeKind::Liveness).await.details.is_empty(),
        "a Valkey outage is no reason to restart the process"
    );
}

#[module(imports = [HealthModule, RedisHealthModule::<Cache>])]
struct ProbedWithoutItsConnection;

#[tokio::test]
async fn an_instance_no_module_opens_fails_the_boot_naming_the_import_that_opens_it() {
    let Err(refused) = App::builder()
        .module::<ProbedWithoutItsConnection>()
        .build()
        .await
    else {
        panic!("an indicator over no connection must not boot");
    };
    let sentence = format!("{refused:#}");
    assert!(
        sentence.contains("RedisHealthModule")
            && sentence.contains("RedisModule::for_root((Cache, None))"),
        "{sentence}"
    );
}
