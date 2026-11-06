//! The indicator `RedisModule::for_root` attaches for each instance it opens,
//! against the suite's Valkey: on every topology the default instance answers
//! the readiness and startup probes as `redis`; on one server, an instance
//! whose server is cut drops readiness and startup to `503` naming it, while
//! its sibling reads up and liveness waits on neither; and on Sentinel and on a
//! Cluster, each `failover` module asks them while its primary is frozen.

use std::time::Duration;

use nest_rs_core::module;
use nest_rs_health::HealthModule;
use nest_rs_http::StatusCode;
use nest_rs_redis::RedisModule;
use nest_rs_testing::{TestApp, TestJson, wait_for};

use crate::{at_default_budget, redis_config};

#[module(imports = [
    HealthModule,
    RedisModule::for_root(at_default_budget(redis_config())),
])]
struct Probed;

/// That `report` lists `indicator` in `bucket`, reading `status`.
fn assert_reads(report: &TestJson, bucket: &str, indicator: &str, status: &str) {
    report
        .value()
        .object()
        .get(bucket)
        .object()
        .get(indicator)
        .object()
        .get("status")
        .assert_string(status);
}

#[tokio::test]
async fn the_default_instance_answers_the_readiness_and_startup_probes_as_redis() {
    let app = TestApp::for_module::<Probed>()
        .await
        .unwrap_or_else(|refused| panic!("boots: {refused:#}"));
    for probe in ["/health/ready", "/health/startup"] {
        let answer = app.http().get(probe).send().await;
        answer.assert_status_is_ok();
        assert_reads(&answer.json().await, "info", "redis", "up");
    }
}

#[module(imports = [HealthModule, RedisModule::for_root(redis_config())])]
struct ProbedWithinTheBudget;

/// An app reading its readiness off the suite's Valkey within the suite's budget,
/// so a `failover` test reads it down while the node it froze still sleeps.
pub(crate) async fn probing() -> TestApp {
    TestApp::for_module::<ProbedWithinTheBudget>()
        .await
        .unwrap_or_else(|refused| panic!("boots: {refused:#}"))
}

/// While a node of the instance is frozen, its readiness answers `503` naming
/// `redis`, within what is left of the freeze, and liveness waits on nothing.
pub(crate) async fn assert_down_naming_redis_and_alive(app: &TestApp) {
    wait_for(Duration::from_secs(5), || async {
        let answer = app.http().get("/health/ready").send().await;
        if answer.0.status() != StatusCode::SERVICE_UNAVAILABLE {
            return false;
        }
        assert_reads(&answer.json().await, "error", "redis", "down");
        true
    })
    .await;
    app.http()
        .get("/health/live")
        .send()
        .await
        .assert_status_is_ok();
}

/// Readiness comes back once the failover ended, within the wait the test gave
/// the failover itself.
pub(crate) async fn assert_ready_again(app: &TestApp) {
    wait_for(Duration::from_secs(30), || async {
        app.http().get("/health/ready").send().await.0.status() == StatusCode::OK
    })
    .await;
}

/// What holds on one server alone: the proxy the test severs fronts that
/// server.
mod standalone {
    use std::sync::OnceLock;

    use nest_rs_redis::RedisConfig;

    use super::*;
    use crate::module::Cache;
    use crate::{SeveringProxy, through};

    /// The cache instance's URL, through the proxy: nextest runs each test in a
    /// process of its own.
    static CACHE_URL: OnceLock<String> = OnceLock::new();

    fn cache_config() -> RedisConfig {
        at_default_budget(through(CACHE_URL.get().cloned().unwrap_or_default()))
    }

    #[module(imports = [
        HealthModule,
        RedisModule::for_root(at_default_budget(redis_config())),
        RedisModule::for_root((Cache, cache_config())),
    ])]
    struct CacheBehindAProxy;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_instance_whose_server_is_cut_drops_readiness_and_startup_naming_it_not_liveness() {
        let proxy = SeveringProxy::start().await;
        CACHE_URL.set(proxy.url_on(0)).expect("set once");
        let app = TestApp::for_module::<CacheBehindAProxy>()
            .await
            .unwrap_or_else(|refused| panic!("both instances boot: {refused:#}"));
        app.http()
            .get("/health/ready")
            .send()
            .await
            .assert_status_is_ok();

        proxy.sever();
        wait_for(Duration::from_secs(5), || {
            let app = &app;
            async move {
                app.http().get("/health/ready").send().await.0.status()
                    == StatusCode::SERVICE_UNAVAILABLE
            }
        })
        .await;

        for probe in ["/health/ready", "/health/startup"] {
            let answer = app.http().get(probe).send().await;
            answer.assert_status(StatusCode::SERVICE_UNAVAILABLE);
            let report = answer.json().await;
            assert_reads(&report, "error", "redis:cache", "down");
            assert_reads(&report, "info", "redis", "up");
        }
        app.http()
            .get("/health/live")
            .send()
            .await
            .assert_status_is_ok();
    }
}
