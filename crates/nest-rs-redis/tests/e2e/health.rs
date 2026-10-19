//! The indicator `RedisHealthModule` attaches, against the suite's Valkey: on
//! every topology the default instance answers the readiness and startup
//! probes as `redis`; on one server, an instance whose server is cut drops
//! readiness to `503` naming it, while its sibling reads up and liveness
//! waits on neither.

use nest_rs_core::module;
use nest_rs_health::HealthModule;
use nest_rs_redis::{RedisHealthModule, RedisModule};
use nest_rs_testing::TestApp;

use crate::{at_default_budget, redis_config};

#[module(imports = [
    HealthModule,
    RedisModule::for_root(at_default_budget(redis_config())),
    RedisHealthModule,
])]
struct Probed;

#[tokio::test]
async fn the_default_instance_answers_the_readiness_and_startup_probes_as_redis() {
    let app = TestApp::for_module::<Probed>()
        .await
        .unwrap_or_else(|refused| panic!("boots: {refused:#}"));
    for probe in ["/health/ready", "/health/startup"] {
        let answer = app.http().get(probe).send().await;
        answer.assert_status_is_ok();
        let body = answer.json().await;
        body.value()
            .object()
            .get("info")
            .object()
            .get("redis")
            .object()
            .get("status")
            .assert_string("up");
    }
}

/// What holds on one server alone: the proxy the test severs fronts that
/// server.
mod standalone {
    use std::sync::OnceLock;
    use std::time::Duration;

    use nest_rs_http::StatusCode;
    use nest_rs_redis::RedisConfig;
    use nest_rs_testing::wait_for;

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
        RedisHealthModule,
        RedisModule::for_root((Cache, cache_config())),
        RedisHealthModule::<Cache>,
    ])]
    struct CacheBehindAProxy;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_instance_whose_server_is_cut_drops_readiness_naming_it_and_leaves_liveness_up() {
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

        let ready = app.http().get("/health/ready").send().await;
        ready.assert_status(StatusCode::SERVICE_UNAVAILABLE);
        let body = ready.json().await;
        let report = body.value().object();
        report
            .get("error")
            .object()
            .get("redis:cache")
            .object()
            .get("status")
            .assert_string("down");
        report
            .get("info")
            .object()
            .get("redis")
            .object()
            .get("status")
            .assert_string("up");
        app.http()
            .get("/health/live")
            .send()
            .await
            .assert_status_is_ok();
    }
}
