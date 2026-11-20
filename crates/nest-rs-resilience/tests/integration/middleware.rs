//! Covers `src/middleware.rs`: an app's own reqwest client under a policy sends
//! again what the HTTP method or an `Idempotency-Key` declares idempotent, each
//! attempt a client operation numbered by its resend count, never sends again
//! what it does not, and is refused at once — never sent — while the breaker
//! is open.

use std::convert::Infallible;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use nest_rs_core::client_operation::{ClientOperation, HTTP_REQUEST_RESEND_COUNT, SERVER_PORT};
use nest_rs_core::{Correlation, Problem, code};
use nest_rs_reqwest::ReqwestMiddleware;
use nest_rs_resilience::{
    ResilienceConfig, ResilienceError, ResilienceMiddleware, ResiliencePolicy,
};
use nest_rs_testing::{HttpsPeer, PeerAnswer, TestAuthority, TestCertificate};
use reqwest::StatusCode;
use reqwest_middleware::ClientWithMiddleware;

use crate::{policy, quiet};

/// The authority the peer's certificate chains to, and the client trusts.
static AUTHORITY: LazyLock<TestAuthority> = LazyLock::new(TestAuthority::new);

static CERTIFICATE: LazyLock<TestCertificate> = LazyLock::new(|| AUTHORITY.server(&["127.0.0.1"]));

/// What every request asks the peer for.
const PARCEL: &str = "/parcels/42";

/// A peer answering `script` in turn, then `200` to every request after it.
async fn answering<const N: usize>(script: [PeerAnswer; N]) -> HttpsPeer {
    HttpsPeer::answering(
        &CERTIFICATE,
        script.into_iter().chain([PeerAnswer::status(200)]),
    )
    .await
}

/// The value of the header `name` on the request `peer` read `nth`.
fn header(peer: &HttpsPeer, nth: usize, name: &str) -> Option<String> {
    peer.requests()[nth].header(name).map(str::to_owned)
}

/// A client trusting the peer's authority, under `policy`, each attempt traced.
fn client(policy: Arc<ResiliencePolicy>) -> ClientWithMiddleware {
    let authority =
        reqwest::Certificate::from_pem(AUTHORITY.pem().as_bytes()).expect("the authority");
    let client = reqwest::Client::builder()
        .tls_certs_only([authority])
        .build()
        .expect("a client");
    reqwest_middleware::ClientBuilder::new(client)
        .with(ResilienceMiddleware::new(policy))
        .with(ReqwestMiddleware::new())
        .build()
}

/// Two retries, each waiting a millisecond or two: the subject is what is sent
/// again, never how long it waits.
fn retrying() -> ResilienceConfig {
    ResilienceConfig {
        retries: Some(2),
        backoff_first: Duration::from_millis(1),
        backoff_ceiling: Duration::from_millis(2),
        ..quiet()
    }
}

/// The policy's refusal a request ended on.
fn refusal(error: &reqwest_middleware::Error) -> &ResilienceError<Infallible> {
    let reqwest_middleware::Error::Middleware(error) = error else {
        panic!("the policy refused: {error:?}");
    };
    error
        .downcast_ref::<ResilienceError<Infallible>>()
        .expect("the policy's refusal")
}

/// Each request's server port and resend count, as the trace middleware
/// records them.
static RECORDED: Mutex<Vec<(Option<String>, Option<String>)>> = Mutex::new(Vec::new());

fn capture(operation: &ClientOperation<'_>) {
    let attribute = |name: &str| {
        operation
            .attributes()
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.to_string())
    };
    RECORDED
        .lock()
        .expect("the recorded")
        .push((attribute(SERVER_PORT), attribute(HTTP_REQUEST_RESEND_COUNT)));
}

#[tokio::test]
async fn an_idempotent_request_is_sent_again_each_attempt_numbered_by_its_resend_count() {
    nest_rs_core::__private::set_client_recorder(capture, true);
    let peer = answering([PeerAnswer::status(503), PeerAnswer::status(502)]).await;
    let (_app, policy) = policy(retrying()).await;
    let unit = Correlation::minted(None);

    let answered = nest_rs_core::with_request_scope(None, unit, async {
        client(policy).get(peer.url(PARCEL)).send().await
    })
    .await
    .expect("answered");

    assert_eq!(
        answered.status(),
        StatusCode::OK,
        "the third attempt answered"
    );
    assert_eq!(peer.requests().len(), 3);
    let parents: Vec<_> = (0..3)
        .map(|nth| header(&peer, nth, "traceparent").expect("a traceparent"))
        .collect();
    assert!(
        parents[0] != parents[1] && parents[1] != parents[2],
        "each attempt is a span of its own: {parents:?}"
    );
    let port = peer.port().to_string();
    let resent: Vec<Option<String>> = RECORDED
        .lock()
        .expect("the recorded")
        .iter()
        .filter(|(server, _)| server.as_deref() == Some(port.as_str()))
        .map(|(_, resent)| resent.clone())
        .collect();
    assert_eq!(resent, [None, Some("1".to_owned()), Some("2".to_owned())]);
}

#[tokio::test]
async fn a_request_not_declared_idempotent_is_sent_once_and_its_answer_returned() {
    let peer = answering([PeerAnswer::status(503)]).await;
    let (_app, policy) = policy(retrying()).await;

    let answered = client(policy)
        .post(peer.url(PARCEL))
        .body("a parcel")
        .send()
        .await
        .expect("answered");

    assert_eq!(answered.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        peer.requests().len(),
        1,
        "a POST could create the parcel twice"
    );
}

#[tokio::test]
async fn a_request_carrying_an_idempotency_key_is_sent_again_with_its_body() {
    let peer = answering([PeerAnswer::status(500)]).await;
    let (_app, policy) = policy(retrying()).await;

    let answered = client(policy)
        .post(peer.url(PARCEL))
        .header(
            "idempotency-key",
            "\"8e03978e-40d5-43e8-bc93-6894a57f9324\"",
        )
        .body("a parcel")
        .send()
        .await
        .expect("answered");

    assert_eq!(answered.status(), StatusCode::OK);
    assert_eq!(peer.requests().len(), 2);
    assert_eq!(
        header(&peer, 1, "content-length").as_deref(),
        Some("8"),
        "the body is sent again whole"
    );
    assert!(
        peer.requests()[1].line.starts_with("POST "),
        "the method is kept"
    );
}

#[tokio::test]
async fn a_query_is_idempotent_and_sent_again() {
    let peer = answering([PeerAnswer::status(502)]).await;
    let (_app, policy) = policy(retrying()).await;

    let answered = client(policy)
        .request(reqwest::Method::QUERY, peer.url(PARCEL))
        .body("status:shipped")
        .send()
        .await
        .expect("answered");

    assert_eq!(answered.status(), StatusCode::OK);
    assert_eq!(peer.requests().len(), 2);
}

#[tokio::test]
async fn a_wait_asked_past_the_backoffs_ceiling_ends_the_run_with_that_answer() {
    let peer = answering([PeerAnswer::status(429).header("retry-after", "3600")]).await;
    let (_app, policy) = policy(retrying()).await;

    let answered = client(policy)
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect("answered");

    assert_eq!(answered.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        peer.requests().len(),
        1,
        "the peer asked an hour; the policy waits 2 ms at most"
    );
}

#[tokio::test]
async fn a_client_error_is_the_dependencys_answer_and_never_sent_again() {
    let peer = answering([PeerAnswer::status(404)]).await;
    let (_app, policy) = policy(retrying()).await;

    let answered = client(policy)
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect("answered");

    assert_eq!(answered.status(), StatusCode::NOT_FOUND);
    assert_eq!(peer.requests().len(), 1);
}

#[tokio::test]
async fn an_open_breaker_refuses_the_request_unsent_with_the_503_every_edge_answers() {
    let peer = answering([PeerAnswer::status(500)]).await;
    let (_app, policy) = policy(ResilienceConfig {
        breaker_window: Some(1),
        breaker_failures: 1,
        breaker_open: Duration::from_secs(30),
        ..quiet()
    })
    .await;
    let client = client(policy);

    let failed = client.get(peer.url(PARCEL)).send().await.expect("answered");
    assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let refused = client
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect_err("the breaker opened on the 500");
    assert!(
        matches!(refusal(&refused), ResilienceError::Open { .. }),
        "{refused:?}"
    );
    let problem = nest_rs_core::__private::find_in_chain::<Problem>(&refused)
        .expect("the problem rides the chain");
    assert_eq!(
        (problem.status(), problem.code(), problem.retry_after()),
        (503, code::UNAVAILABLE, Some(30))
    );
    assert_eq!(peer.requests().len(), 1, "a refused request is never sent");
}

#[tokio::test]
async fn a_peer_that_never_answers_is_given_up_at_the_policys_timeout() {
    let peer = HttpsPeer::answering(&CERTIFICATE, [PeerAnswer::Silent]).await;
    let (_app, policy) = policy(ResilienceConfig {
        timeout: Duration::from_millis(200),
        ..quiet()
    })
    .await;

    let given_up = client(policy)
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect_err("no answer");

    assert!(
        matches!(
            refusal(&given_up),
            ResilienceError::TimedOut { timeout, .. } if *timeout == Duration::from_millis(200)
        ),
        "{given_up:?}"
    );
}

#[tokio::test]
async fn a_middleware_added_after_the_trace_refuses_every_request_unsent_saying_so() {
    let peer = answering([]).await;
    let (_app, policy) = policy(retrying()).await;
    let authority =
        reqwest::Certificate::from_pem(AUTHORITY.pem().as_bytes()).expect("the authority");
    let misordered = reqwest_middleware::ClientBuilder::new(
        reqwest::Client::builder()
            .tls_certs_only([authority])
            .build()
            .expect("a client"),
    )
    .with(ReqwestMiddleware::new())
    .with(ResilienceMiddleware::new(policy))
    .build();

    let refused = misordered
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect_err("the order is refused");

    assert!(
        refused
            .to_string()
            .contains("add it before, so each attempt is a client span of its own"),
        "{refused}"
    );
    assert_eq!(peer.requests().len(), 0, "nothing is sent");
}
