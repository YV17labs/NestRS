//! The `#[indicators]` expansion, executed through the probe body.
//!
//! `inventory` is process-wide; `Sensors` is reachable only from this file's
//! `AppModule`, so the sibling suites' probes never run it.

use nest_rs_core::{injectable, module};
use nest_rs_health::{HealthModule, indicators};
use nest_rs_testing::TestApp;

#[injectable]
#[derive(Default)]
struct Sensors;

#[indicators]
impl Sensors {
    #[readiness]
    async fn upstream_reachable(&self) -> Result<(), std::io::Error> {
        Ok(())
    }

    #[readiness]
    async fn credentials_valid(&self) -> Result<(), std::io::Error> {
        Err(std::io::Error::other(
            "dsn=postgres://user:hunter2@10.0.0.4",
        ))
    }

    #[liveness]
    async fn process_responsive(&self) {}

    #[expect(
        clippy::needless_arbitrary_self_type,
        reason = "the spelled-out receiver is the shape under test"
    )]
    #[liveness]
    async fn heartbeat(self: &Self) -> () {}

    #[liveness]
    async fn r#loop(&self) {}

    #[liveness]
    async fn through_its_arc(self: &std::sync::Arc<Self>) {}

    #[cfg(any())]
    #[readiness]
    async fn compiled_out(&self) {}
}

#[module(imports = [HealthModule], providers = [Sensors])]
struct AppModule;

async fn probe(path: &str) -> (u16, String) {
    let app = TestApp::for_module::<AppModule>()
        .await
        .expect("a module with a decorated indicator host boots");
    let response = app.http().get(path).send().await.0;
    let status = response.status().as_u16();
    let body = response
        .into_body()
        .into_string()
        .await
        .expect("the probe answers with a body");
    (status, body)
}

#[tokio::test]
async fn a_decorated_host_reports_through_the_probe_body() {
    let (status, body) = probe("/health/ready").await;

    assert_eq!(
        status, 503,
        "one indicator is down, so the probe is: {body}"
    );
    assert!(
        body.contains("upstream_reachable") && body.contains("credentials_valid"),
        "the method name is the indicator's key in the body: {body}",
    );
}

#[tokio::test]
async fn each_method_answers_only_its_own_probe() {
    let (live_status, live_body) = probe("/health/live").await;
    assert_eq!(
        live_status, 200,
        "the only liveness check passes: {live_body}"
    );
    assert!(
        live_body.contains("process_responsive"),
        "the returns-nothing shape reports up: {live_body}",
    );
    assert!(
        !live_body.contains("credentials_valid"),
        "a readiness indicator must not answer the liveness probe: {live_body}",
    );
}

#[tokio::test]
async fn the_hosts_own_error_never_reaches_the_body() {
    let (_, body) = probe("/health/ready").await;

    for leaked in ["hunter2", "postgres://", "10.0.0.4", "dsn"] {
        assert!(
            !body.contains(leaked),
            "a readiness body is served to whatever can reach the port, and it carried \
             {leaked:?}: {body}",
        );
    }
}

#[tokio::test]
async fn a_spelled_out_unit_return_and_receiver_report_up() {
    let (status, body) = probe("/health/live").await;
    assert_eq!(status, 200, "{body}");
    assert!(
        body.contains("heartbeat") && body.contains("through_its_arc"),
        "{body}"
    );
    assert!(body.contains("\"loop\"") && !body.contains("r#"), "{body}");
}
