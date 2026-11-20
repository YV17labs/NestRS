//! Covers `src/error.rs` through the HTTP edge: a client of a route whose
//! dependency's breaker is open is answered `503` with the `Retry-After` the
//! breaker knows, at once — whether the route answers the policy's error or a
//! domain error of its own handing its problem on — and a failed call is
//! answered opaquely.

use std::sync::Arc;
use std::time::Duration;

use http::StatusCode;
use nest_rs_core::{Problem, ToProblem, code, module};
use nest_rs_http::{controller, routes};
use nest_rs_resilience::{ResilienceConfig, ResilienceError, ResilienceModule, ResiliencePolicy};
use nest_rs_testing::TestApp;

/// What the dependency answers: a failure the route cannot describe.
#[derive(Debug, thiserror::Error)]
#[error("the upstream answered 502")]
struct UpstreamDown;

/// A route's own error, wrapping the policy's and answering what it answers.
#[derive(Debug, thiserror::Error)]
#[error("no preview")]
struct NoPreview(#[source] ResilienceError<UpstreamDown>);

impl ToProblem for NoPreview {
    fn to_problem(&self) -> Option<Problem> {
        self.0.to_problem()
    }
}

#[controller(path = "/previews")]
struct PreviewsController {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
}

#[routes]
impl PreviewsController {
    #[get("/direct")]
    #[public]
    async fn direct(&self) -> Result<String, ResilienceError<UpstreamDown>> {
        self.upstream
            .run(|_| async { Err::<String, _>(UpstreamDown) })
            .await
    }

    #[get("/wrapped")]
    #[public]
    async fn wrapped(&self) -> Result<String, NoPreview> {
        self.upstream
            .run(|_| async { Err::<String, _>(UpstreamDown) })
            .await
            .map_err(NoPreview)
    }
}

#[module(
    imports = [ResilienceModule::for_root(ResilienceConfig {
        retries: None,
        breaker_window: Some(1),
        breaker_failures: 1,
        breaker_open: Duration::from_secs(30),
        ..ResilienceConfig::default()
    })],
    providers = [PreviewsController],
)]
struct PreviewsModule;

#[tokio::test]
async fn an_open_breaker_is_answered_503_with_its_retry_after_and_a_failure_opaquely() {
    let app = TestApp::for_module::<PreviewsModule>()
        .await
        .expect("the documented wiring boots");

    let failed = app.http().get("/previews/direct").send().await;
    failed.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    failed.assert_header_is_not_exist("retry-after");
    let body = failed.json().await;
    body.value()
        .object()
        .get("code")
        .assert_string(code::INTERNAL.as_str());

    for route in ["/previews/direct", "/previews/wrapped"] {
        let refused = app.http().get(route).send().await;
        refused.assert_status(StatusCode::SERVICE_UNAVAILABLE);
        refused.assert_header("retry-after", "30");
        let body = refused.json().await;
        body.value()
            .object()
            .get("code")
            .assert_string(code::UNAVAILABLE.as_str());
    }
}
