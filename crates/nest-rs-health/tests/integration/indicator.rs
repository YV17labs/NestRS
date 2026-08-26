//! The decorator's expansion, **executed** — a real `#[indicators]` host, booted,
//! answering through the probe body a caller reads back.
//!
//! Neither umbrella witness reaches this. `nest-rs-macro-hygiene` proves the
//! expansion *compiles* under one dependency and never runs it; the composition
//! witness in `controller.rs` boots the documented import with no indicator in
//! the module, so the three routes it asserts answer an empty registry. Between
//! them sat the whole developer-facing surface of this capability — the `run`
//! thunk that resolves the host and invokes the method, the `name` / `kind` /
//! `origin` the expansion fills, and the two return shapes it adapts — asserted
//! by nothing in either workspace.
//!
//! The host lives here rather than in `src/`'s `#[cfg(test)]` module for the
//! reason recorded on `run_indicators`: `inventory` is process-wide, so a
//! submitted fixture joins every other probe in the process. It is safe *here*
//! because module gating is what these entries are filtered by — `Sensors` is
//! reachable only from this file's `AppModule`, so the sibling suites' probes
//! never see it.

use nest_rs_core::{injectable, module};
use nest_rs_health::{HealthModule, indicators};
use nest_rs_testing::TestApp;

#[injectable]
#[derive(Default)]
struct Sensors;

#[indicators]
impl Sensors {
    /// `Ok` is up.
    #[readiness]
    async fn upstream_reachable(&self) -> Result<(), std::io::Error> {
        Ok(())
    }

    /// `Err` is down — and the error's own text is what must **not** reach the
    /// body, since a readiness probe answers whatever can open the port.
    #[readiness]
    async fn credentials_valid(&self) -> Result<(), std::io::Error> {
        Err(std::io::Error::other(
            "dsn=postgres://user:hunter2@10.0.0.4",
        ))
    }

    /// A method returning nothing is the expansion's other return shape
    /// (`ReturnType::Default`): reaching the end of the body is the `Ok`.
    #[liveness]
    async fn process_responsive(&self) {}
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

/// The decorated methods reach the report under the names the expansion wrote,
/// and one failing check takes the probe down.
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

/// The `#[liveness]` method is on the liveness probe and **not** on readiness —
/// the per-method `kind` the expansion fills is what routes it, and routing it
/// wrongly would put a process check behind a dependency check.
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

/// The indicator's own error never reaches the body — this is the half the
/// unit tests assert on a hand-built entry, now asserted through the decorator
/// that real callers use, on a body served over the wire.
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
