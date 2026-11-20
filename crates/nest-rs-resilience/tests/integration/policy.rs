//! Covers `src/policy.rs` through the HTTP edge: an attempt ends by the
//! request's deadline, before the policy's own timeout, so its client is told
//! by the policy and its breaker counts the attempt.

use std::sync::Arc;
use std::time::Duration;

use http::StatusCode;
use nest_rs_core::{Problem, ToProblem, module};
use nest_rs_http::{HttpConfig, HttpModule, controller, routes};
use nest_rs_resilience::{ResilienceConfig, ResilienceError, ResilienceModule, ResiliencePolicy};
use nest_rs_testing::TestApp;

#[derive(Debug, thiserror::Error)]
#[error("the upstream never answers")]
struct Silent;

impl ToProblem for Silent {
    fn to_problem(&self) -> Option<Problem> {
        None
    }
}

#[controller(path = "/slow")]
struct SlowController {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
}

#[routes]
impl SlowController {
    #[get("/")]
    #[public]
    async fn wait(&self) -> Result<String, ResilienceError<Silent>> {
        self.upstream
            .run(|_| std::future::pending::<Result<String, Silent>>())
            .await
    }
}

#[module(
    imports = [
        HttpModule::for_root(HttpConfig {
            request_timeout: Some(Duration::from_secs(1)),
            ..HttpConfig::default()
        }),
        ResilienceModule::for_root(ResilienceConfig {
            timeout: Duration::from_secs(10),
            retries: None,
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: Duration::from_secs(30),
            ..ResilienceConfig::default()
        }),
    ],
    providers = [SlowController],
)]
struct SlowModule;

#[tokio::test(start_paused = true)]
async fn an_attempt_ends_by_the_requests_deadline_and_its_breaker_counts_it() {
    let app = TestApp::for_module::<SlowModule>()
        .await
        .expect("a route over a policy boots");

    let started = tokio::time::Instant::now();
    let given_up = app.http().get("/slow").send().await;
    given_up.assert_status(StatusCode::SERVICE_UNAVAILABLE);
    given_up.assert_header_is_not_exist("retry-after");
    assert_eq!(
        started.elapsed(),
        Duration::from_secs(1),
        "the request's deadline, not the policy's 10 s"
    );

    let refused = app.http().get("/slow").send().await;
    refused.assert_status(StatusCode::SERVICE_UNAVAILABLE);
    refused.assert_header("retry-after", "30");
}
