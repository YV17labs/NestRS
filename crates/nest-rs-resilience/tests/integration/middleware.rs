//! Covers `src/middleware.rs`: an app's own reqwest client under a policy sends
//! again what the HTTP method or an `Idempotency-Key` declares idempotent, each
//! attempt a client operation numbered by its resend count, never sends again
//! what it does not, and is refused at once — never sent — while the breaker
//! is open or once its requests reach the rate limit, which counts a redirect
//! the client follows inside the attempt that met it, gives each attempt up at
//! its unit's deadline, and tells what an attempt failed on in the retry's
//! line and the breaker's opening: a status by its code, a transport failure
//! without the URL it was sent to. The response it hands back is reqwest's
//! own, its body read as it comes and holding the run's slot, the breaker's
//! verdict and what its head left of the attempt's bound until it ends: the
//! time its reads wait for the dependency, never the time its caller takes.

use std::convert::Infallible;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use nest_rs_core::client_operation::{ClientOperation, HTTP_REQUEST_RESEND_COUNT, SERVER_PORT};
use nest_rs_core::{Correlation, Problem, code};
use nest_rs_reqwest::ReqwestMiddleware;
use nest_rs_resilience::{
    BreakerState, ResilienceConfig, ResilienceError, ResilienceMiddleware, ResiliencePolicy,
};
use nest_rs_testing::{HttpsPeer, LogCapture, PeerAnswer, TestAuthority, TestCertificate};
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
    client_following(policy, reqwest::redirect::Policy::default())
}

/// [`client`], following the redirects `redirects` lets it follow.
fn client_following(
    policy: Arc<ResiliencePolicy>,
    redirects: reqwest::redirect::Policy,
) -> ClientWithMiddleware {
    let authority =
        reqwest::Certificate::from_pem(AUTHORITY.pem().as_bytes()).expect("the authority");
    let client = reqwest::Client::builder()
        .tls_certs_only([authority])
        .redirect(redirects)
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
    let client = client(Arc::clone(&policy));

    let failed = client.get(peer.url(PARCEL)).send().await.expect("answered");
    assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let most = crate::breaker_wait(&policy);
    let refused = client
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect_err("the breaker opened on the 500");
    let least = crate::breaker_wait(&policy);
    assert!(
        matches!(refusal(&refused), ResilienceError::Open { .. }),
        "{refused:?}"
    );
    let problem = nest_rs_core::__private::find_in_chain::<Problem>(&refused)
        .expect("the problem rides the chain");
    assert_eq!((problem.status(), problem.code()), (503, code::UNAVAILABLE));
    assert!(
        problem
            .retry_after()
            .is_some_and(|told| (least..=most).contains(&u64::from(told))),
        "the breaker's wait as it refused, {least} to {most} s: {problem:?}"
    );
    assert_eq!(peer.requests().len(), 1, "a refused request is never sent");
}

#[tokio::test]
async fn every_request_sent_counts_against_the_rate_limit_a_resend_included() {
    let peer = answering([PeerAnswer::status(503)]).await;
    let (_app, policy) = policy(ResilienceConfig {
        rate_limit: Some(2),
        ..retrying()
    })
    .await;
    let client = client(policy);

    let answered = client.get(peer.url(PARCEL)).send().await.expect("answered");
    assert_eq!(answered.status(), StatusCode::OK, "the resend answered");

    let refused = client
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect_err("the resend took the window's last request");
    assert!(
        matches!(refusal(&refused), ResilienceError::Limited { .. }),
        "{refused:?}"
    );
    let problem = nest_rs_core::__private::find_in_chain::<Problem>(&refused)
        .expect("the problem rides the chain");
    assert_eq!((problem.status(), problem.code()), (503, code::UNAVAILABLE));
    assert!(
        problem
            .retry_after()
            .is_some_and(|seconds| (1..=60).contains(&seconds)),
        "the time left of the minute's window the first request opened: {problem:?}"
    );
    assert_eq!(
        peer.requests().len(),
        2,
        "a request the rate refuses is never sent"
    );
}

/// Where the peer's redirect sends a request.
const MOVED: &str = "/parcels/43";

/// The rate counts attempts: a redirect the client follows goes out inside the
/// attempt that met it, so the dependency is sent two requests for the one
/// attempt counted.
#[tokio::test]
async fn a_redirect_the_client_follows_is_sent_inside_the_attempt_and_counted_with_it() {
    let peer = answering([PeerAnswer::status(307).header("location", MOVED)]).await;
    let (_app, policy) = policy(ResilienceConfig {
        rate_limit: Some(1),
        ..quiet()
    })
    .await;
    let client = client(policy);

    let answered = client.get(peer.url(PARCEL)).send().await.expect("answered");
    assert_eq!(answered.status(), StatusCode::OK, "the redirect, followed");
    let sent: Vec<String> = peer.requests().into_iter().map(|head| head.line).collect();
    assert_eq!(
        sent,
        [
            format!("GET {PARCEL} HTTP/1.1"),
            format!("GET {MOVED} HTTP/1.1")
        ],
        "the request and its redirect, in one attempt"
    );

    let refused = client
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect_err("the window's one attempt was taken");
    assert!(
        matches!(refusal(&refused), ResilienceError::Limited { .. }),
        "{refused:?}"
    );
    assert_eq!(peer.requests().len(), 2);
}

/// A client following no redirect hands it back as the dependency's answer, so
/// each request the dependency is sent is an attempt the rate counts.
#[tokio::test]
async fn a_client_following_no_redirect_sends_each_request_as_an_attempt_of_its_own() {
    let peer = answering([PeerAnswer::status(307).header("location", MOVED)]).await;
    let (_app, policy) = policy(ResilienceConfig {
        rate_limit: Some(1),
        ..quiet()
    })
    .await;
    let client = client_following(policy, reqwest::redirect::Policy::none());

    let moved = client.get(peer.url(PARCEL)).send().await.expect("answered");
    assert_eq!(moved.status(), StatusCode::TEMPORARY_REDIRECT, "the answer");
    assert_eq!(peer.requests().len(), 1);

    let refused = client
        .get(peer.url(MOVED))
        .send()
        .await
        .expect_err("the window's one attempt was taken");
    assert!(
        matches!(refusal(&refused), ResilienceError::Limited { .. }),
        "{refused:?}"
    );
    assert_eq!(
        peer.requests().len(),
        1,
        "the request the redirect asks for, refused unsent"
    );
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

/// What a client does before its first wait — a request built, a handshake
/// begun — as a middleware after the policy's, which then never answers.
struct Stalling;

#[async_trait::async_trait]
impl reqwest_middleware::Middleware for Stalling {
    async fn handle(
        &self,
        _req: reqwest::Request,
        _extensions: &mut http::Extensions,
        _next: reqwest_middleware::Next<'_>,
    ) -> reqwest_middleware::Result<reqwest::Response> {
        tokio::time::advance(Duration::from_millis(3)).await;
        std::future::pending().await
    }
}

/// Each attempt of the middleware ends at its unit's deadline however long its
/// request takes to first wait, whether the request may be sent again or not:
/// the policy gives it up there, before the edge cutting the unit at that
/// deadline, and its breaker counts it.
#[tokio::test(start_paused = true)]
async fn an_attempt_slow_to_first_wait_ends_at_the_deadline_its_edge_cuts_the_unit_at() {
    for (method, sent) in [
        (reqwest::Method::GET, "a request it may send again"),
        (reqwest::Method::POST, "a request it sends once"),
    ] {
        let (_app, policy) = policy(ResilienceConfig {
            breaker_window: Some(1),
            breaker_failures: 1,
            ..retrying()
        })
        .await;
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
            .with(ResilienceMiddleware::new(Arc::clone(&policy)))
            .with(Stalling)
            .build();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
        let cut = tokio::time::timeout_at(
            deadline,
            nest_rs_core::__private::with_deadline(
                Some(deadline),
                client
                    .request(method, format!("https://127.0.0.1:1{PARCEL}"))
                    .send(),
            ),
        )
        .await;
        let Ok(Err(refused)) = cut else {
            panic!("the policy gave {sent} up at the deadline, before the edge cut it: {cut:?}");
        };
        assert!(
            matches!(refusal(&refused), ResilienceError::TimedOut { timeout, .. }
                if *timeout == Duration::from_millis(300)),
            "{sent}: {refused:?}"
        );
        assert!(
            matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
            "{sent}: the attempt counted in the breaker"
        );
    }
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

/// The `error` of each line on the policy's target saying `message`, none of
/// which quotes `never`.
fn told(logs: &LogCapture, message: &str, never: &[&str]) -> Vec<String> {
    logs.find(nest_rs_resilience::TARGET, message)
        .iter()
        .map(|line| {
            for value in never {
                assert!(!line.quotes(value), "`{value}` in {line:?}");
            }
            line.field("error").unwrap_or_default()
        })
        .collect()
}

/// A status a retry may cure is told by its code alone, in the line of the
/// retry it earns and in the opening of the breaker it opens: the second
/// failure opens the breaker, which refuses the retry after it.
#[tokio::test]
async fn an_answer_counted_as_a_failure_is_told_by_its_status_code_alone() {
    let peer = answering([PeerAnswer::status(503), PeerAnswer::status(502)]).await;
    let (_app, policy) = policy(ResilienceConfig {
        breaker_window: Some(2),
        breaker_failures: 2,
        ..retrying()
    })
    .await;
    let logs = LogCapture::install();

    let refused = client(policy)
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect_err("the second failure opened the breaker");
    assert!(
        matches!(refusal(&refused), ResilienceError::Open { .. }),
        "{refused:?}"
    );

    let never = [PARCEL, "127.0.0.1"];
    assert_eq!(
        told(&logs, "attempt failed; retrying the call", &never),
        ["the dependency answered 503", "the dependency answered 502"]
    );
    assert_eq!(
        told(&logs, "circuit breaker changed state", &never),
        ["the dependency answered 502"]
    );
}

/// A request that reached no answer is told as its failure's chain, without
/// the URL reqwest's own sentence quotes: its path and its query hold what
/// the caller sent.
#[tokio::test]
async fn a_transport_failure_is_told_as_its_chain_without_the_url_it_was_sent_to() {
    const SENT: &str = "s3cr3t";
    let closing = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port");
    let port = closing.local_addr().expect("its address").port();
    let accepting = tokio::spawn(async move {
        while let Ok((accepted, _)) = closing.accept().await {
            drop(accepted);
        }
    });
    let (_app, policy) = policy(ResilienceConfig {
        retries: Some(1),
        breaker_window: Some(1),
        breaker_failures: 1,
        ..retrying()
    })
    .await;
    let logs = LogCapture::install();

    let refused = client(policy)
        .get(format!(
            "https://127.0.0.1:{port}/parcels/{SENT}?token={SENT}"
        ))
        .send()
        .await
        .expect_err("the failure opened the breaker");
    assert!(
        matches!(refusal(&refused), ResilienceError::Open { .. }),
        "{refused:?}"
    );
    accepting.abort();

    let never = [SENT, "/parcels", "for url"];
    let retried = told(&logs, "attempt failed; retrying the call", &never);
    let opened = told(&logs, "circuit breaker changed state", &never);
    assert_eq!(retried.len(), 1, "{retried:?}");
    assert_eq!(opened, retried, "one failure, said the same in both");
    assert!(
        retried[0].starts_with("error sending request: "),
        "reqwest's sentence, then its causes: {retried:?}"
    );
}

/// What an answer's head starts, its body ends: a peer answering `200` and
/// stalling mid-body holds the caller as one that never answered would, so the
/// response the middleware hands back keeps the attempt's bound, the breaker's
/// verdict and the run's slot of the bulkhead until its body ends.
#[tokio::test]
async fn an_answers_body_is_read_within_its_attempts_timeout_and_counts_in_the_breaker() {
    let peer = answering([PeerAnswer::stalling(200)]).await;
    let (_app, policy) = policy(ResilienceConfig {
        timeout: Duration::from_millis(200),
        breaker_window: Some(1),
        breaker_failures: 1,
        ..quiet()
    })
    .await;

    let answered = client(Arc::clone(&policy))
        .post(peer.url(PARCEL))
        .send()
        .await
        .expect("the head answered");
    assert_eq!(answered.status(), StatusCode::OK);
    // Guards against a hang alone: a body no bound covers is read for good.
    let read = tokio::time::timeout(Duration::from_secs(3), answered.bytes()).await;
    assert!(
        matches!(read, Ok(Err(_))),
        "the body's read ends at the attempt's timeout: {read:?}"
    );
    assert!(
        matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
        "a body cut at its bound is a failure the breaker counts: {:?}",
        policy.breaker_state()
    );
}

#[tokio::test]
async fn an_answers_body_holds_its_runs_slot_until_it_is_read_or_dropped() {
    let peer = answering([PeerAnswer::stalling(200)]).await;
    let (_app, policy) = policy(ResilienceConfig {
        concurrency: Some(1),
        ..quiet()
    })
    .await;
    let client = client(policy);

    let unread = client
        .post(peer.url(PARCEL))
        .send()
        .await
        .expect("the head answered");
    let refused = client
        .post(peer.url(PARCEL))
        .send()
        .await
        .expect_err("the one slot is the unread body's");
    assert!(
        matches!(refusal(&refused), ResilienceError::Full { .. }),
        "{refused:?}"
    );
    assert_eq!(peer.requests().len(), 1, "refused unsent");

    drop(unread);
    let next = client
        .post(peer.url(PARCEL))
        .send()
        .await
        .expect("the slot is free once the answer is dropped");
    assert_eq!(next.status(), StatusCode::OK);
}

/// The policy's refusal the read of a body ended on, when it did: what tells
/// the policy's cut from a transport's failure.
fn cut(error: &reqwest::Error) -> Option<&ResilienceError<Infallible>> {
    nest_rs_core::__private::find_in_chain(error)
}

/// What a caller read off reqwest's response before, it reads still — the URL
/// and the body's length, which reqwest's own conversions lose or derive, the
/// status, the version, the headers, the peer's address, the status as an
/// error — and a body read to its end frees the run's slot there, the response
/// still held.
#[tokio::test]
async fn an_answer_with_a_body_is_reqwests_own_response_and_frees_its_slot_at_the_bodys_end() {
    const BODY: &str = "a parcel on its way";
    let peer = answering([PeerAnswer::body(200, BODY), PeerAnswer::body(404, BODY)]).await;
    let (_app, policy) = policy(ResilienceConfig {
        concurrency: Some(1),
        ..quiet()
    })
    .await;
    let client = client(policy);
    let length = u64::try_from(BODY.len()).ok();

    let mut answered = client.get(peer.url(PARCEL)).send().await.expect("answered");
    assert_eq!(answered.url().as_str(), peer.url(PARCEL));
    assert_eq!(answered.content_length(), length);
    assert_eq!(answered.status(), StatusCode::OK);
    assert_eq!(answered.version(), reqwest::Version::HTTP_11);
    assert_eq!(
        answered
            .headers()
            .get("content-length")
            .map(|length| length.as_bytes()),
        Some(BODY.len().to_string().as_bytes())
    );
    assert_eq!(
        answered.remote_addr().map(|peer| peer.port()),
        Some(peer.port())
    );
    let mut read = Vec::new();
    while let Some(chunk) = answered.chunk().await.expect("a chunk") {
        read.extend_from_slice(&chunk);
    }
    assert_eq!(read, BODY.as_bytes());

    let missing = client
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect("the slot is free once the body ended");
    drop(answered);
    let refused = missing
        .error_for_status()
        .expect_err("a status that is an error");
    assert_eq!(refused.status(), Some(StatusCode::NOT_FOUND));
    assert_eq!(
        refused.url().map(reqwest::Url::as_str),
        Some(peer.url(PARCEL).as_str())
    );
}

/// The body is streamed, never buffered: its first part is read while the
/// peer still holds the second back, which comes once the peer is released.
/// A body buffered would be read only once it is whole, so the first read
/// would wait for a part the test has not released yet, until the attempt's
/// bound gave it up.
#[tokio::test]
async fn an_answers_body_is_read_as_it_comes_never_buffered() {
    const FIRST: &str = "the first part, ";
    const SECOND: &str = "then the second";
    let peer = answering([PeerAnswer::parted(200, FIRST, SECOND)]).await;
    let (_app, policy) = policy(quiet()).await;

    let mut answered = client(policy)
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect("the head answered");
    assert_eq!(
        answered.content_length(),
        u64::try_from(FIRST.len() + SECOND.len()).ok()
    );
    let mut read = Vec::new();
    while read.len() < FIRST.len() {
        let chunk = answered.chunk().await.expect("a chunk");
        read.extend_from_slice(&chunk.expect("the first part, the second held back"));
    }
    assert_eq!(read, FIRST.as_bytes());

    peer.release();
    let rest = answered.text().await.expect("the rest");
    assert_eq!(rest, SECOND);
}

/// The timeout the body tests below read under, which each of them outlasts
/// on purpose before it reads or drops: long enough for a part the peer then
/// writes to cross loopback on a loaded machine, the one wait it bounds.
const BODY_TIMEOUT: Duration = Duration::from_millis(300);

/// A policy of [`BODY_TIMEOUT`] whose breaker one failure opens.
fn opening_on_one_failure() -> ResilienceConfig {
    ResilienceConfig {
        timeout: BODY_TIMEOUT,
        breaker_window: Some(1),
        breaker_failures: 1,
        ..quiet()
    }
}

/// The bound is the dependency's, never its caller's: a caller that takes
/// longer than the timeout between two reads, then waits for a part still on
/// its way — as every read does once a transport's buffers are drained — is
/// handed that part, and the breaker counts nothing.
#[tokio::test]
async fn a_caller_slower_than_the_dependency_is_handed_a_part_that_comes_past_the_bounds_instant() {
    const FIRST: &str = "the first part, ";
    const SECOND: &str = "then the second";
    let peer = answering([PeerAnswer::parted(200, FIRST, SECOND)]).await;
    let (_app, policy) = policy(opening_on_one_failure()).await;

    let mut answered = client(Arc::clone(&policy))
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect("the head answered");
    let mut read = Vec::new();
    while read.len() < FIRST.len() {
        let chunk = answered.chunk().await.expect("a chunk");
        read.extend_from_slice(&chunk.expect("the first part"));
    }
    tokio::time::sleep(BODY_TIMEOUT + Duration::from_millis(50)).await;

    peer.release();
    let rest = answered.text().await.expect("the part the read waited for");
    assert_eq!(rest, SECOND);
    assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
}

/// A read its caller abandons, the response dropped past the bound: the
/// answer is judged by what the transport holds at the drop, so a part the
/// dependency delivered meanwhile, which no read was there to take, counts as
/// the head did.
#[tokio::test]
async fn a_read_abandoned_then_dropped_past_the_bound_counts_what_the_dependency_delivered() {
    const FIRST: &str = "the first part, ";
    let peer = answering([PeerAnswer::parted(200, FIRST, "then the second")]).await;
    let (_app, policy) = policy(opening_on_one_failure()).await;

    let mut answered = client(Arc::clone(&policy))
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect("the head answered");
    let mut read = 0;
    while read < FIRST.len() {
        let chunk = answered.chunk().await.expect("a chunk");
        read += chunk.expect("the first part").len();
    }
    let abandoned = tokio::time::timeout(Duration::from_millis(10), answered.chunk()).await;
    assert!(abandoned.is_err(), "the second part is held back");

    peer.release();
    tokio::time::sleep(BODY_TIMEOUT + Duration::from_millis(50)).await;
    drop(answered);
    assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
}

/// A body the policy gives up at the attempt's bound reaches the caller as
/// reqwest's own error, the policy's refusal in its chain with the `503` every
/// edge answers, and frees the run's slot as it is given up, the response
/// still held.
#[tokio::test]
async fn a_body_given_up_at_its_bound_answers_reqwests_error_holding_the_policys_refusal() {
    let peer = answering([PeerAnswer::stalling(200)]).await;
    let (_app, policy) = policy(ResilienceConfig {
        timeout: Duration::from_millis(200),
        concurrency: Some(1),
        ..quiet()
    })
    .await;
    let client = client(policy);

    let mut answered = client
        .post(peer.url(PARCEL))
        .send()
        .await
        .expect("the head answered");
    let declared = answered.content_length();
    let half = answered.chunk().await.expect("read").expect("what came");
    assert_eq!(
        u64::try_from(2 * half.len()).ok(),
        declared,
        "the half the peer wrote before it stalled"
    );
    // Guards against a hang alone: a body no bound covers is read for good.
    let given_up = tokio::time::timeout(Duration::from_secs(3), answered.chunk())
        .await
        .expect("the read ends at the attempt's timeout")
        .expect_err("the rest never came");

    assert!(given_up.is_decode(), "reqwest's own error: {given_up:?}");
    assert!(
        matches!(
            cut(&given_up),
            Some(ResilienceError::TimedOut { timeout, .. }) if *timeout == Duration::from_millis(200)
        ),
        "{given_up:?}"
    );
    let problem = nest_rs_core::__private::find_in_chain::<Problem>(&given_up)
        .expect("the problem rides the chain");
    assert_eq!((problem.status(), problem.code()), (503, code::UNAVAILABLE));

    let next = client
        .post(peer.url(PARCEL))
        .send()
        .await
        .expect("the slot is free once the body is given up");
    assert_eq!(next.status(), StatusCode::OK);
    drop(answered);
}

/// A body the dependency cuts short is a transport failure, never the
/// policy's refusal: it reaches the caller as reqwest's error, and counts in
/// the breaker, whose opening says its chain without the URL it was read from.
#[tokio::test]
async fn a_body_cut_short_by_the_dependency_is_a_failure_told_as_its_chain() {
    let peer = answering([PeerAnswer::truncated(200)]).await;
    let (_app, policy) = policy(ResilienceConfig {
        breaker_window: Some(1),
        breaker_failures: 1,
        ..quiet()
    })
    .await;
    let logs = LogCapture::install();

    let answered = client(Arc::clone(&policy))
        .post(peer.url(PARCEL))
        .send()
        .await
        .expect("the head answered");
    assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
    let failed = answered.bytes().await.expect_err("half a body");

    assert!(failed.is_decode(), "reqwest's own error: {failed:?}");
    assert!(
        cut(&failed).is_none(),
        "no refusal of the policy's: {failed:?}"
    );
    assert!(
        matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
        "{:?}",
        policy.breaker_state()
    );
    let opened = told(
        &logs,
        "circuit breaker changed state",
        &[PARCEL, "127.0.0.1"],
    );
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert!(
        opened[0].starts_with("request or response body error: "),
        "reqwest's sentence, then its causes: {opened:?}"
    );
}

/// A probe follows the body's rules: answered with no body, it closes the
/// breaker as its head comes; answered with one, it keeps the probe's turn
/// while the body is unread, and closes the breaker when the caller, having
/// read the status alone, drops the response.
#[tokio::test]
async fn a_probes_answer_closes_the_breaker_once_its_body_is_read_or_dropped() {
    let peer = answering([PeerAnswer::status(200), PeerAnswer::stalling(200)]).await;
    let (_app, policy) = policy(ResilienceConfig {
        breaker_window: Some(1),
        breaker_failures: 1,
        ..quiet()
    })
    .await;
    let client = client(Arc::clone(&policy));
    let probing = || async {
        policy
            .open_breaker(Duration::from_millis(1))
            .expect("a breaker");
        nest_rs_testing::wait_until(Duration::from_secs(10), || {
            policy.breaker_state() == Some(BreakerState::HalfOpen)
        })
        .await;
    };

    probing().await;
    let bodiless = client.get(peer.url(PARCEL)).send().await.expect("answered");
    assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
    drop(bodiless);

    probing().await;
    let unread = client
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect("the head answered");
    assert_eq!(unread.status(), StatusCode::OK);
    let refused = client
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect_err("the unread body holds the probe's turn");
    assert!(
        matches!(refusal(&refused), ResilienceError::Open { .. }),
        "{refused:?}"
    );
    drop(unread);
    assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
}

/// What is returned is the last attempt's answer: one the run sends again is
/// dropped unread, its slot kept by the run, and a last answer counted as a
/// failure at its head is still the caller's to read.
#[tokio::test]
async fn an_answer_sent_again_is_dropped_and_the_last_is_the_callers_to_read() {
    let peer = answering([
        PeerAnswer::body(503, "down"),
        PeerAnswer::body(200, "up"),
        PeerAnswer::body(503, "down again"),
    ])
    .await;
    let (_app, policy) = policy(ResilienceConfig {
        concurrency: Some(1),
        breaker_window: Some(3),
        breaker_failures: 2,
        ..retrying()
    })
    .await;
    let client = client(Arc::clone(&policy));

    let answered = client.get(peer.url(PARCEL)).send().await.expect("answered");
    assert_eq!(answered.status(), StatusCode::OK);
    assert_eq!(answered.text().await.expect("the last answer's body"), "up");
    assert_eq!(peer.requests().len(), 2);

    let failed = client
        .post(peer.url(PARCEL))
        .send()
        .await
        .expect("the slot is free, and a POST is sent once");
    assert_eq!(failed.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
        "the window's second 503 counted at its head, its body unread"
    );
    assert_eq!(failed.text().await.expect("its body"), "down again");
}
