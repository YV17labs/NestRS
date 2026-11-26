//! A decorated handler and `#[sse]` route of a `#[routes]` block, failing, then
//! refused, and each attempted again within its one request.

use std::sync::Arc;

use http::StatusCode;
use nest_rs_core::operation_log::TARGET as OPERATIONS;
use nest_rs_core::{code, module};
use nest_rs_http::futures_util::stream;
use nest_rs_http::{SseEvent, SseStream, controller, routes};
use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
use nest_rs_testing::{LogCapture, TestApp};

use super::{Attempts, UpstreamDown, opening_on_the_first_failure, retried_once, retrying_once};

#[controller(path = "/decorated")]
struct DecoratedController {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    attempts: Arc<Attempts>,
}

#[routes]
impl DecoratedController {
    #[get("/")]
    #[public]
    #[resilient(upstream)]
    async fn preview(&self) -> Result<String, ResilienceError<UpstreamDown>> {
        Err(UpstreamDown)
    }

    /// A stream whose opening calls the upstream.
    #[sse("/feed")]
    #[public]
    #[resilient(upstream)]
    async fn feed(&self) -> Result<SseStream, ResilienceError<UpstreamDown>> {
        if self.attempts.first() {
            return Err(UpstreamDown);
        }
        Ok(SseStream::new(stream::iter([SseEvent::message("moved")])))
    }
}

#[module(
    imports = [ResilienceModule::for_root(opening_on_the_first_failure())],
    providers = [DecoratedController, Attempts],
)]
struct DecoratedHttpModule;

#[tokio::test(start_paused = true)]
async fn a_route_answers_its_refusal_503_with_the_breakers_wait() {
    let app = TestApp::for_module::<DecoratedHttpModule>()
        .await
        .expect("a decorated route boots");

    let failed = app.http().get("/decorated").send().await;
    failed.assert_status(StatusCode::INTERNAL_SERVER_ERROR);

    let refused = app.http().get("/decorated").send().await;
    refused.assert_status(StatusCode::SERVICE_UNAVAILABLE);
    refused.assert_header("retry-after", "30");
    refused
        .json()
        .await
        .value()
        .object()
        .get("code")
        .assert_string(code::UNAVAILABLE.as_str());
}

#[tokio::test(start_paused = true)]
async fn an_sse_route_answers_its_refusal_503_with_the_breakers_wait() {
    let app = TestApp::for_module::<DecoratedHttpModule>()
        .await
        .expect("a decorated stream boots");

    let failed = app.http().get("/decorated/feed").send().await;
    failed.assert_status(StatusCode::INTERNAL_SERVER_ERROR);

    let refused = app.http().get("/decorated/feed").send().await;
    refused.assert_status(StatusCode::SERVICE_UNAVAILABLE);
    refused.assert_header("retry-after", "30");
    refused
        .json()
        .await
        .value()
        .object()
        .get("code")
        .assert_string(code::UNAVAILABLE.as_str());
    let attempts = app.container().get::<Attempts>().expect("provided");
    assert_eq!(
        attempts.made(),
        1,
        "sent once, and the opening the breaker refused never"
    );
}

#[controller(path = "/retried")]
struct RetriedController {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    attempts: Arc<Attempts>,
}

#[routes]
impl RetriedController {
    #[get("/")]
    #[public]
    #[resilient(upstream, idempotent)]
    async fn preview(&self) -> Result<String, ResilienceError<UpstreamDown>> {
        if self.attempts.first() {
            return Err(UpstreamDown);
        }
        Ok("previewed".to_owned())
    }

    /// A stream whose opening calls the upstream, again after a failure.
    #[sse("/feed")]
    #[public]
    #[resilient(upstream, idempotent)]
    async fn feed(&self) -> Result<SseStream, ResilienceError<UpstreamDown>> {
        if self.attempts.first() {
            return Err(UpstreamDown);
        }
        Ok(SseStream::new(stream::iter([SseEvent::message("moved")])))
    }
}

#[module(
    imports = [ResilienceModule::for_root(retrying_once())],
    providers = [RetriedController, Attempts],
)]
struct RetriedHttpModule;

#[tokio::test(start_paused = true)]
async fn an_sse_route_failing_once_answers_on_its_retry() {
    let app = TestApp::for_module::<RetriedHttpModule>()
        .await
        .expect("a retried stream boots");

    let answered = app.http().get("/retried/feed").send().await;
    answered.assert_status_is_ok();
    let body = answered
        .0
        .into_body()
        .into_string()
        .await
        .expect("the stream is text");
    assert!(body.contains("data: moved"), "the stream opens: {body:?}");
    let attempts = app.container().get::<Attempts>().expect("provided");
    assert_eq!(attempts.made(), 2, "the failed attempt and its retry");
}

#[tokio::test(start_paused = true)]
async fn a_route_failing_once_answers_on_its_retry_within_its_one_request() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<RetriedHttpModule>()
        .await
        .expect("a retried route boots");

    let answered = app.http().get("/retried").send().await;
    answered.assert_status_is_ok();
    answered.assert_text("previewed").await;

    let attempts = app.container().get::<Attempts>().expect("provided");
    assert_eq!(attempts.made(), 2, "the failed attempt and its retry");
    retried_once(&logs, "RetriedController", "preview");
    let request = logs.expect_one(OPERATIONS, nest_rs_http::unit::REQUEST.name());
    assert_eq!(
        request.field("status").as_deref(),
        Some("200"),
        "{request:?}"
    );
}
