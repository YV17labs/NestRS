//! Covers `src/module.rs` — a `#[liveness]`, `#[readiness]` or `#[startup]`
//! host the app reaches without `HealthModule` is named at run, with the import
//! that serves it, and one the probes serve is not.

use nest_rs_core::{App, AppLifecycle, injectable, module, target};
use nest_rs_health::{HealthModule, indicators};
use nest_rs_testing::LogCapture;

/// What `App::run` files for an indicator's host the probes do not serve.
const UNSERVED: &str = "host not served: the health probes are not mounted";

#[injectable]
#[derive(Default)]
struct CacheProbe;

#[indicators]
impl CacheProbe {
    #[readiness]
    async fn cache(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[module(providers = [CacheProbe])]
struct CacheProbeModule;

#[tokio::test]
async fn an_indicator_the_app_reaches_without_the_health_module_is_named_at_run() {
    let logs = LogCapture::install();

    App::builder()
        .module::<CacheProbeModule>()
        .build()
        .await
        .expect("the module boots")
        .run()
        .await
        .expect("an app serving nothing stops cleanly");

    let unserved = logs.expect_one(target::APP, UNSERVED);
    assert_eq!(unserved.level, "warn");
    assert_eq!(unserved.field("edge"), None, "health is no edge");
    assert_eq!(unserved.field("host").as_deref(), Some("CacheProbe"));
    assert_eq!(
        unserved.field("module").as_deref(),
        Some("CacheProbeModule")
    );
    assert_eq!(
        unserved.field("import").as_deref(),
        Some("nest_rs::health::HealthModule")
    );
}

#[module(
    imports = [
        HealthModule,
        nest_rs_http::HttpModule::for_root(nest_rs_http::HttpConfig {
            host: "127.0.0.1".into(),
            port: 0,
            ..nest_rs_http::HttpConfig::default()
        }),
    ],
    providers = [CacheProbe],
)]
struct ServedCacheProbeModule;

#[tokio::test]
async fn an_indicator_the_probes_serve_is_not_named_at_run() {
    let logs = LogCapture::install();
    let app = App::builder()
        .module::<ServedCacheProbeModule>()
        .build()
        .await
        .expect("the probes mount");
    let lifecycle = app
        .container()
        .get::<AppLifecycle>()
        .expect("every app is seeded its lifecycle");

    // `App::run` names an unserved host before it turns ready, so the run is
    // dropped there.
    let running = tokio::spawn(app.run());
    assert!(lifecycle.ready().await, "the app serves");
    running.abort();

    logs.expect_one(target::APP, "app ready");
    logs.expect_none(target::APP, UNSERVED);
}
