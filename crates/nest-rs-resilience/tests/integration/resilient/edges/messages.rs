//! A decorated message of a `#[messages]` block, refused, and attempted again
//! within its one unit.

use std::sync::Arc;

use nest_rs_core::module;
use nest_rs_core::operation_log::{OK, TARGET as OPERATIONS};
use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
use nest_rs_testing::{LogCapture, TestApp};
use nest_rs_ws::{WsModule, gateway, messages};

use super::{Attempts, UpstreamDown, opening_on_the_first_failure, retried_once, retrying_at_once};

#[gateway(path = "/ws/decorated")]
struct DecoratedGateway {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
}

#[messages]
impl DecoratedGateway {
    #[subscribe_message("decorated")]
    #[public]
    #[resilient(upstream)]
    async fn decorated(&self) -> Result<String, ResilienceError<UpstreamDown>> {
        Err(UpstreamDown)
    }
}

#[module(
    imports = [WsModule, ResilienceModule::for_root(opening_on_the_first_failure())],
    providers = [DecoratedGateway],
)]
struct DecoratedWsModule;

#[tokio::test]
async fn a_message_answers_its_refusal_unavailable_with_the_breakers_wait() {
    let app = TestApp::builder()
        .module::<DecoratedWsModule>()
        .build_ws()
        .await
        .expect("a decorated gateway boots on a real port");
    let upstream = app
        .container()
        .get::<ResiliencePolicy>()
        .expect("the module built the policy");
    let mut socket = app.socket("/ws/decorated").connect().await;

    socket.send("decorated", serde_json::Value::Null).await;
    let failed = socket.next_envelope().await;
    assert_eq!(failed["data"]["code"], "INTERNAL", "{failed}");

    let most = crate::breaker_wait(&upstream);
    socket.send("decorated", serde_json::Value::Null).await;
    let refused = socket.next_envelope().await;
    let least = crate::breaker_wait(&upstream);
    assert_eq!(refused["data"]["code"], "UNAVAILABLE", "{refused}");
    let told = refused["data"]["retryAfterSeconds"].as_u64();
    assert!(
        told.is_some_and(|told| (least..=most).contains(&told)),
        "the breaker's wait as it refused, {least} to {most} s: {refused}"
    );

    app.shutdown().await.expect("the transport stops cleanly");
}

#[gateway(path = "/ws/retried")]
struct RetriedGateway {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    attempts: Arc<Attempts>,
}

#[messages]
impl RetriedGateway {
    #[subscribe_message("track")]
    #[public]
    #[resilient(upstream, idempotent)]
    async fn track(&self) -> Result<String, ResilienceError<UpstreamDown>> {
        if self.attempts.first() {
            return Err(UpstreamDown);
        }
        Ok("in transit".to_owned())
    }
}

#[module(
    imports = [WsModule, ResilienceModule::for_root(retrying_at_once())],
    providers = [RetriedGateway, Attempts],
)]
struct RetriedWsModule;

/// On the real clock, as the socket is: the retry comes a millisecond or
/// two after the failure.
#[tokio::test]
async fn a_message_failing_once_answers_on_its_retry_within_its_one_unit() {
    let logs = LogCapture::install();
    let app = TestApp::builder()
        .module::<RetriedWsModule>()
        .build_ws()
        .await
        .expect("a retried gateway boots on a real port");
    let mut socket = app.socket("/ws/retried").connect().await;

    socket.send("track", serde_json::Value::Null).await;
    let answer = socket.next_envelope().await;
    assert_eq!(answer["data"], "in transit", "{answer}");

    let attempts = app.container().get::<Attempts>().expect("provided");
    assert_eq!(attempts.made(), 2, "the failed attempt and its retry");
    retried_once(&logs, "RetriedGateway", "track");
    let message = logs.expect_one(OPERATIONS, nest_rs_ws::unit::MESSAGE.name());
    assert_eq!(message.field("outcome").as_deref(), Some(OK), "{message:?}");

    app.shutdown().await.expect("the transport stops cleanly");
}
