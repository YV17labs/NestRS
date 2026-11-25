//! Covers `src/middleware.rs`: an app's own reqwest client under a policy sends
//! again what the HTTP method or an `Idempotency-Key` declares idempotent, each
//! attempt a client operation numbered by its resend count, never sends again
//! what it does not, and is refused at once — never sent — while the breaker
//! is open or once its rate has no token free, but for a request whose token
//! comes within the rate's wait, which waits for it; the rate counts a redirect
//! the client follows inside the attempt that met it. It gives each attempt up
//! at its unit's deadline, and tells what an attempt failed on in the retry's
//! line and the breaker's opening: a status by its code, a transport failure
//! without the URL it was sent to, a reqwest error an app's error holds behind
//! a pointer as one it names bare, and a body the client's policy gave up as
//! that policy's timeout. The response it hands back is reqwest's
//! own, its body read as it comes and holding the run's slot, the breaker's
//! verdict and what its head left of the attempt's bound until it ends: the
//! time its reads wait for the dependency, never the time its caller takes,
//! and a read the health edge cuts with the indicator making it a timeout.
//! What its policy tells a subscription of all this names no method: a retry
//! as its line files it, a timeout before the retry it earns, a body given up
//! as the timeout it is, and one the dependency cut short, or dropped by its
//! caller with a read waiting past the bound, by the breaker's opening alone.

use std::convert::Infallible;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use nest_rs_core::client_operation::{ClientOperation, HTTP_REQUEST_RESEND_COUNT, SERVER_PORT};
use nest_rs_core::{Correlation, Problem, code, injectable, instance, module};
use nest_rs_health::{
    HealthConfig, HealthModule, HealthService, IndicatorStatus, ProbeKind, indicators,
};
use nest_rs_reqwest::ReqwestMiddleware;
use nest_rs_resilience::{
    BreakerPhase, BreakerState, ResilienceConfig, ResilienceError, ResilienceEvent,
    ResilienceMiddleware, ResilienceModule, ResiliencePolicy, TARGET, resilient,
};
use nest_rs_testing::{
    HeadlessApp, HttpsPeer, LogCapture, PeerAnswer, TestAuthority, TestCertificate,
};
use reqwest::StatusCode;
use reqwest_middleware::ClientWithMiddleware;

use crate::{held, policy, quiet, told};

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

/// A `503` carrying a body asks its wait as a bodiless one does: the retry is
/// decided for the second its `Retry-After` says, stretched by a fifth at
/// most, where the policy's own backoff is a millisecond or two. The wait
/// itself is not sat through here: the retry's event says it as it is decided.
#[tokio::test]
async fn a_wait_asked_by_a_503_carrying_a_body_is_the_wait_before_its_retry() {
    const ASKED: Duration = Duration::from_secs(1);
    let busy = PeerAnswer::body(503, "busy").header("retry-after", ASKED.as_secs().to_string());
    let peer = answering([busy]).await;
    let (_app, policy) = policy(ResilienceConfig {
        backoff_ceiling: 2 * ASKED,
        ..retrying()
    })
    .await;
    let mut events = policy.events();
    let url = peer.url(PARCEL);
    let sending = tokio::spawn(async move { client(policy).get(url).send().await });

    let decided = tokio::time::timeout(Duration::from_secs(60), events.next())
        .await
        .expect("the retry is decided as the 503's head comes");
    let Some(ResilienceEvent::Retry { wait, error, .. }) = decided else {
        panic!("the retry after the 503: {decided:?}");
    };
    sending.abort();

    assert!(
        (ASKED..=ASKED.mul_f64(1.2)).contains(&wait),
        "the second the peer asked, never the backoff's two milliseconds: {wait:?}"
    );
    assert_eq!(error.as_deref(), Some("the dependency answered 503"));
    assert_eq!(peer.requests().len(), 1);
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

/// How often the bucket of the test below gains its one token, and the
/// longest a request waits for one.
const TOKEN_EVERY: Duration = Duration::from_millis(200);

/// The rate is a bucket of tokens under the middleware too: its one token
/// spent, the next request waits for the token due within the rate's wait and
/// is sent, and a third, whose token comes after that one's, past the wait, is
/// refused unsent with the `503` every edge answers, told the time to it.
#[tokio::test]
async fn a_request_finding_no_token_waits_for_one_within_the_rates_wait_and_is_refused_one_past_it()
{
    let peer = answering([]).await;
    let (_app, policy) = policy(ResilienceConfig {
        rate_limit: Some(1),
        rate_window: TOKEN_EVERY,
        rate_wait: Some(TOKEN_EVERY),
        ..quiet()
    })
    .await;
    let client = client(policy);
    let send = || client.get(peer.url(PARCEL)).send();

    // Polled in this order, each meets the bucket as the one before left it.
    // The third is refused for arriving while the second waits: within
    // `TOKEN_EVERY` of the first, the one bound here real time holds.
    let began = Instant::now();
    let (sent, waited, refused) = tokio::join!(send(), send(), send());
    let took = began.elapsed();

    let sent = sent.expect("the bucket's one token");
    let waited = waited.expect("the next token came within the wait");
    assert_eq!(
        (sent.status(), waited.status()),
        (StatusCode::OK, StatusCode::OK)
    );
    assert!(
        took >= TOKEN_EVERY,
        "a request is not sent before its token: {took:?}"
    );
    let refused = refused.expect_err("its token comes after the waiting request's");
    let ResilienceError::Limited { retry_after, .. } = refusal(&refused) else {
        panic!("the rate refused: {refused:?}");
    };
    assert!(
        TOKEN_EVERY < *retry_after && *retry_after <= 2 * TOKEN_EVERY,
        "the time to its token, the second from the one taken: {retry_after:?}"
    );
    let problem = nest_rs_core::__private::find_in_chain::<Problem>(&refused)
        .expect("the problem rides the chain");
    assert_eq!(
        (problem.status(), problem.code(), problem.retry_after()),
        (503, code::UNAVAILABLE, Some(1)),
        "that time in whole seconds: {problem:?}"
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

/// A timeout the run sends its request again after is told as it is decided,
/// before the retry it earns, whose error is the timeout's sentence; the one
/// the run ends on is told once more, as the refusal its caller is answered.
/// None names a method: no decorated one sent the request.
#[tokio::test(start_paused = true)]
async fn a_timeout_the_run_sends_again_after_is_told_before_its_retry() {
    const BOUND: Duration = Duration::from_millis(100);
    let (_app, policy) = policy(ResilienceConfig {
        timeout: BOUND,
        retries: Some(1),
        ..retrying()
    })
    .await;
    let mut events = policy.events();
    let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
        .with(ResilienceMiddleware::new(policy))
        .with(Stalling)
        .build();

    let given_up = client
        .get(format!("https://127.0.0.1:1{PARCEL}"))
        .send()
        .await
        .expect_err("no answer");
    let timed_out = refusal(&given_up);
    assert!(
        matches!(timed_out, ResilienceError::TimedOut { timeout, .. } if *timeout == BOUND),
        "{given_up:?}"
    );

    let told = told(&mut events);
    let [
        ResilienceEvent::Timeout {
            host: None,
            method: None,
            timeout: first,
            ..
        },
        ResilienceEvent::Retry {
            host: None,
            method: None,
            attempt: 1,
            error,
            ..
        },
        ResilienceEvent::Timeout {
            host: None,
            method: None,
            timeout: last,
            ..
        },
    ] = &told[..]
    else {
        panic!("a timeout, its retry, the retry's timeout: {told:?}");
    };
    assert_eq!(
        (*first, *last, error.clone()),
        (BOUND, BOUND, Some(timed_out.to_string()))
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

/// What the line of a retry says under the middleware, every time: it sends
/// again a transport failure, a timeout or a status judged a failure, never an
/// answer it took, so the line of an `Ok` sent again is none of its own.
const RETRYING: &str = "attempt failed; retrying the call";

/// The `error` of each line on the policy's target saying `message`, none of
/// which quotes `never`.
fn filed(logs: &LogCapture, message: &str, never: &[&str]) -> Vec<String> {
    logs.find(TARGET, message)
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
        filed(&logs, RETRYING, &never),
        ["the dependency answered 503", "the dependency answered 502"]
    );
    assert_eq!(
        filed(&logs, "circuit breaker changed state", &never),
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
    let retried = filed(&logs, RETRYING, &never);
    let opened = filed(&logs, "circuit breaker changed state", &never);
    assert_eq!(retried.len(), 1, "{retried:?}");
    assert_eq!(opened, retried, "one failure, said the same in both");
    assert!(
        retried[0].starts_with("error sending request: "),
        "reqwest's sentence, then its causes: {retried:?}"
    );
}

/// A head judged a failure and sent again is told as the retry its line
/// files: the attempt, the wait and the status by its code alone, naming no
/// method.
#[tokio::test]
async fn a_head_judged_a_failure_and_sent_again_is_told_as_the_retry_its_line_files() {
    let peer = answering([PeerAnswer::status(503)]).await;
    let (_app, policy) = policy(retrying()).await;
    let logs = LogCapture::install();
    let mut events = policy.events();

    let answered = client(policy)
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect("answered");
    assert_eq!(answered.status(), StatusCode::OK, "the resend answered");

    let told = told(&mut events);
    let [
        ResilienceEvent::Retry {
            host: None,
            method: None,
            attempt,
            wait,
            error,
            ..
        },
    ] = &told[..]
    else {
        panic!("one retry, of no decorated method: {told:?}");
    };
    assert_eq!(
        (*attempt, error.as_deref()),
        (1, Some("the dependency answered 503"))
    );
    let line = logs.expect_one(TARGET, RETRYING);
    assert_eq!(
        (
            line.field("host"),
            line.field("method"),
            line.field("attempt"),
            line.field("wait_ms"),
            line.field("error"),
        ),
        (
            None,
            None,
            Some(attempt.to_string()),
            Some(wait.as_nanos().div_ceil(1_000_000).to_string()),
            error.clone(),
        ),
        "the line's milliseconds are the wait's, rounded up"
    );
}

/// A layer of the client's own failing before any answer, its sentence quoting
/// the URL the request was sent to as a client's error does: a middleware
/// after the policy's.
struct Unrouted;

#[derive(Debug, thiserror::Error)]
#[error("no route to {0}")]
struct NoRoute(reqwest::Url);

#[async_trait::async_trait]
impl reqwest_middleware::Middleware for Unrouted {
    async fn handle(
        &self,
        req: reqwest::Request,
        _extensions: &mut http::Extensions,
        _next: reqwest_middleware::Next<'_>,
    ) -> reqwest_middleware::Result<reqwest::Response> {
        Err(reqwest_middleware::Error::middleware(NoRoute(
            req.url().clone(),
        )))
    }
}

/// A retry's event says what its attempt failed on as its line does: the
/// failure's chain, a URL in it cut to its origin, the path and the query,
/// which hold what the caller sent, gone.
#[tokio::test(start_paused = true)]
async fn a_retrys_event_says_its_failures_chain_with_no_url_past_its_origin() {
    const SENT: &str = "s3cr3t";
    const ORIGIN: &str = "https://127.0.0.1:1";
    let (_app, policy) = policy(ResilienceConfig {
        retries: Some(1),
        ..retrying()
    })
    .await;
    let logs = LogCapture::install();
    let mut events = policy.events();
    let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
        .with(ResilienceMiddleware::new(policy))
        .with(Unrouted)
        .build();

    let failed = client
        .get(format!("{ORIGIN}/parcels/{SENT}?token={SENT}"))
        .send()
        .await
        .expect_err("no route");
    assert!(
        failed.to_string().contains(SENT),
        "the caller is handed the layer's own error, as it said it: {failed}"
    );

    let told = told(&mut events);
    let [
        ResilienceEvent::Retry {
            error: Some(error), ..
        },
    ] = &told[..]
    else {
        panic!("one retry, saying its error: {told:?}");
    };
    assert_eq!(*error, format!("no route to {ORIGIN}"));
    assert_eq!(
        filed(&logs, RETRYING, &[SENT, "/parcels"]),
        [error.as_str()],
        "said the same in its line"
    );
}

/// An app's error naming reqwest's as its source behind the pointer that
/// shares it, reqwest's error being no `Clone`: the link it hands out is the
/// pointer, which `std` displays as reqwest's whole sentence.
#[derive(Debug, thiserror::Error)]
#[error("the carrier failed")]
struct Shared(#[source] Arc<reqwest::Error>);

/// An app's error holding the I/O error a reader over a body's stream makes
/// of reqwest's, which displays it whole too.
#[derive(Debug, thiserror::Error)]
#[error("the download failed")]
struct Unread(#[from] std::io::Error);

/// What a run tells of two attempts each failing with what `attempt` makes:
/// the `error` of its retry's line, of the opening of the breaker the second
/// failure opens, and of its retry's event, none of which quotes `never`.
async fn told_of_two_failures<E, Fut>(attempt: impl Fn() -> Fut, never: &[&str]) -> [Vec<String>; 3]
where
    E: std::error::Error + 'static,
    Fut: Future<Output = E>,
{
    let (_app, policy) = policy(ResilienceConfig {
        retries: Some(1),
        breaker_window: Some(2),
        breaker_failures: 2,
        ..retrying()
    })
    .await;
    let logs = LogCapture::install();
    let mut events = policy.events();

    let failed = policy
        .run_idempotent(|_cancel| async { Err::<(), _>(attempt().await) })
        .await
        .expect_err("each attempt fails");
    assert!(matches!(failed, ResilienceError::Failed(_)), "{failed:?}");

    let retried = told(&mut events)
        .into_iter()
        .filter_map(|event| match event {
            ResilienceEvent::Retry { error, .. } => error,
            _ => None,
        })
        .collect();
    [
        filed(&logs, RETRYING, never),
        filed(&logs, "circuit breaker changed state", never),
        retried,
    ]
}

/// A reqwest error an app's error holds behind a pointer, or inside an I/O
/// error, is told as one it names bare: by its kind and what its causes'
/// types tell, never the URL's path, nor the names rustls quotes of the
/// certificate it refuses, in the retry's line, its event and the breaker's
/// opening.
#[tokio::test]
async fn a_reqwest_error_held_behind_a_pointer_is_told_without_its_url_or_a_certificates_names() {
    const SENT: &str = "s3cr3t";
    const CAUSE: &str =
        "error sending request: invalid peer certificate: certificate not valid for name";
    let other = AUTHORITY.server(&["ledger.internal"]);
    let peer = HttpsPeer::answering(&other, [PeerAnswer::status(200)]).await;
    let url = peer.url(&format!("/parcels/{SENT}?token={SENT}"));
    let authority =
        reqwest::Certificate::from_pem(AUTHORITY.pem().as_bytes()).expect("the authority");
    let http = reqwest::Client::builder()
        .tls_certs_only([authority])
        .build()
        .expect("a client");
    let refused = || async { http.get(&url).send().await.expect_err("another name's") };
    let own = Shared(Arc::new(refused().await)).to_string();
    let handed_out = std::error::Error::source(&Shared(Arc::new(refused().await)))
        .map(ToString::to_string)
        .unwrap_or_default();
    assert!(
        !own.contains(SENT) && handed_out.contains(SENT),
        "the link is the pointer, displaying reqwest's own sentence: {handed_out}"
    );
    let never = [SENT, "/parcels", "for url", "ledger.internal", "DnsName"];

    let shared = told_of_two_failures(|| async { Shared(Arc::new(refused().await)) }, &never).await;
    let said = format!("the carrier failed: {CAUSE}");
    assert_eq!(shared, [vec![said.clone()], vec![said.clone()], vec![said]]);

    let read = told_of_two_failures(
        || async { Unread(std::io::Error::other(refused().await)) },
        &never,
    )
    .await;
    let said = format!("the download failed: {CAUSE}");
    assert_eq!(read, [vec![said.clone()], vec![said.clone()], vec![said]]);
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

/// The slot's call is read as running for as long as the body holds it.
#[tokio::test]
async fn an_answers_body_holds_its_runs_slot_until_it_is_read_or_dropped() {
    let peer = answering([PeerAnswer::stalling(200)]).await;
    let (_app, policy) = policy(ResilienceConfig {
        concurrency: Some(1),
        ..quiet()
    })
    .await;
    let client = client(Arc::clone(&policy));

    let unread = client
        .post(peer.url(PARCEL))
        .send()
        .await
        .expect("the head answered");
    assert_eq!(
        policy.concurrency_running(),
        Some(1),
        "the call runs while its answer's body is unread"
    );
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
    assert_eq!(
        policy.concurrency_running(),
        Some(0),
        "dropped unread, its call is counted out"
    );
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
/// error — and a body read to its end frees the run's slot there, its call
/// read as running until then and no longer, the response still held.
#[tokio::test]
async fn an_answer_with_a_body_is_reqwests_own_response_and_frees_its_slot_at_the_bodys_end() {
    const BODY: &str = "a parcel on its way";
    let peer = answering([PeerAnswer::body(200, BODY), PeerAnswer::body(404, BODY)]).await;
    let (_app, policy) = policy(ResilienceConfig {
        concurrency: Some(1),
        ..quiet()
    })
    .await;
    let client = client(Arc::clone(&policy));
    let length = u64::try_from(BODY.len()).ok();

    let mut answered = client.get(peer.url(PARCEL)).send().await.expect("answered");
    assert_eq!(policy.concurrency_running(), Some(1), "its body unread");
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
    assert_eq!(
        policy.concurrency_running(),
        Some(0),
        "read to its end, its call is counted out"
    );

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

/// A response dropped while a read of it has waited past the bound for a part
/// the dependency still holds back is a timeout in the breaker, whose opening
/// says so; and no caller is left to answer, so the policy refuses none: its
/// subscriptions are told the breaker's opening alone, no timeout.
#[tokio::test]
async fn a_response_dropped_with_a_read_waiting_past_the_bound_is_a_timeout_told_to_no_caller() {
    const FIRST: &str = "the first part, ";
    let peer = answering([PeerAnswer::parted(200, FIRST, "then the second")]).await;
    let (_app, policy) = policy(opening_on_one_failure()).await;
    let logs = LogCapture::install();
    let mut events = policy.events();

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
    tokio::time::sleep(BODY_TIMEOUT + Duration::from_millis(50)).await;
    assert_eq!(
        policy.breaker_state(),
        Some(BreakerState::Closed),
        "an abandoned read is given up by no timer: nothing is counted until the drop"
    );
    drop(answered);

    assert!(
        matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
        "the part never came within the bound"
    );
    let opening = logs.expect_one(TARGET, "circuit breaker changed state");
    assert_eq!(
        opening.field("error").as_deref(),
        Some("the call did not answer within its timeout"),
        "{opening:?}"
    );
    let told = told(&mut events);
    assert!(
        matches!(
            &told[..],
            [ResilienceEvent::Transition {
                from: BreakerPhase::Closed,
                to: BreakerPhase::Open,
                manual: false,
                ..
            }]
        ),
        "the opening alone, and no timeout: {told:?}"
    );
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

/// A body given up at its bound is told as the timeout it is, once, saying
/// the attempt's bound and no method, after the opening of the breaker it
/// counted in; the head that answered before it told nothing.
#[tokio::test]
async fn a_body_given_up_at_its_bound_is_told_as_a_timeout_once() {
    let peer = answering([PeerAnswer::stalling(200)]).await;
    let (_app, policy) = policy(ResilienceConfig {
        timeout: Duration::from_millis(200),
        breaker_window: Some(1),
        breaker_failures: 1,
        ..quiet()
    })
    .await;
    let mut events = policy.events();

    let answered = client(policy)
        .post(peer.url(PARCEL))
        .send()
        .await
        .expect("the head answered");
    assert_eq!(told(&mut events), [], "an answer's head tells nothing");
    // Guards against a hang alone: a body no bound covers is read for good.
    let read = tokio::time::timeout(Duration::from_secs(3), answered.bytes()).await;
    assert!(
        matches!(read, Ok(Err(_))),
        "the body's read ends at the attempt's timeout: {read:?}"
    );

    let told = told(&mut events);
    assert!(
        matches!(
            &told[..],
            [
                ResilienceEvent::Transition {
                    from: BreakerPhase::Closed,
                    to: BreakerPhase::Open,
                    manual: false,
                    ..
                },
                ResilienceEvent::Timeout { host: None, method: None, timeout, .. },
            ] if *timeout == Duration::from_millis(200)
        ),
        "{told:?}"
    );
}

/// What a line says of a body the policy gave up: reqwest's kinds, then the
/// policy's own sentence and the `503` it answers, constants of the framework.
const BODY_GIVEN_UP: &str = "error decoding response body: request or response body error: the \
                             call did not answer within its timeout: 503 UNAVAILABLE";

/// A body the policy gives up says so wherever its error is said: the line an
/// edge files for it reads the policy's timeout, never a body that did not
/// decode with its cause left out.
#[tokio::test]
async fn a_body_the_policy_gave_up_says_so_on_a_line() {
    let peer = answering([PeerAnswer::stalling(200)]).await;
    let (_app, policy) = policy(ResilienceConfig {
        timeout: Duration::from_millis(200),
        ..quiet()
    })
    .await;

    let given_up = client(policy)
        .get(peer.url(PARCEL))
        .send()
        .await
        .expect("the head answers")
        .bytes()
        .await
        .expect_err("given up");

    assert_eq!(nest_rs_core::error_message(&given_up), BODY_GIVEN_UP);
    assert_eq!(
        nest_rs_core::error_message(&CarrierFailed::Unread(given_up)),
        format!("the carrier's answer could not be read: {BODY_GIVEN_UP}")
    );
}

/// The policy a decorated method reading a body under the client's own runs
/// under.
#[instance]
struct Reading;

/// What a call to the carrier fails with, naming the client's error as its
/// source.
#[derive(Debug, thiserror::Error)]
enum CarrierFailed {
    #[error("the carrier did not answer")]
    Unanswered(#[from] reqwest_middleware::Error),
    #[error("the carrier's answer could not be read")]
    Unread(#[from] reqwest::Error),
}

/// A host the test builds itself: the policy its method runs under, and a
/// client under a policy of its own.
struct Carrier {
    reading: Arc<ResiliencePolicy<Reading>>,
    http: ClientWithMiddleware,
    parcel: String,
}

impl Carrier {
    #[resilient(reading, idempotent)]
    async fn parcel(&self) -> Result<String, ResilienceError<CarrierFailed>> {
        let answered = self.http.get(&self.parcel).send().await?;
        Ok(answered.text().await?)
    }
}

#[module(imports = [
    ResilienceModule::for_root(ResilienceConfig {
        timeout: Duration::from_millis(200),
        ..quiet()
    }),
    ResilienceModule::for_root((Reading, ResilienceConfig {
        retries: Some(1),
        backoff_first: Duration::from_millis(1),
        backoff_ceiling: Duration::from_millis(2),
        breaker_window: Some(1),
        breaker_failures: 1,
        ..ResilienceConfig::default()
    })),
])]
struct ReadingModule;

/// A decorated method reading a body the client's policy gives up tells that
/// timeout as what its attempt failed on: in the line of the retry it earns,
/// in that retry's event and in the opening of its own breaker.
#[tokio::test]
async fn a_body_the_clients_policy_gave_up_is_told_as_its_timeout_by_the_method_reading_it() {
    let peer = answering([PeerAnswer::stalling(200)]).await;
    let app = HeadlessApp::builder()
        .module::<ReadingModule>()
        .build_headless()
        .await
        .expect("both policies boot");
    let carrier = Carrier {
        reading: held::<Reading>(&app),
        http: client(held::<nest_rs_core::Unnamed>(&app)),
        parcel: peer.url(PARCEL),
    };
    let logs = LogCapture::install();
    let mut events = carrier.reading.events();

    let refused = carrier.parcel().await.expect_err("its breaker opened");
    assert!(
        matches!(refused, ResilienceError::Open { .. }),
        "{refused:?}"
    );

    let said = format!("the carrier's answer could not be read: {BODY_GIVEN_UP}");
    let never = [PARCEL, "127.0.0.1"];
    assert_eq!(filed(&logs, RETRYING, &never), [said.as_str()]);
    assert_eq!(
        filed(&logs, "circuit breaker changed state", &never),
        [said.as_str()]
    );
    let told = told(&mut events);
    assert!(
        matches!(
            &told[..],
            [
                ResilienceEvent::Transition { .. },
                ResilienceEvent::Retry {
                    method: Some("parcel"),
                    error: Some(error),
                    ..
                },
                ResilienceEvent::Refusal { .. },
            ] if *error == said
        ),
        "{told:?}"
    );
}

/// What the indicator below asks the carrier for, and whether its check got
/// as far as reading the answer's body.
struct Watched {
    parcel: String,
    reading: AtomicBool,
}

#[injectable]
struct CarrierIndicator {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    watched: Arc<Watched>,
}

#[indicators]
impl CarrierIndicator {
    /// A request sent under the policy, a few milliseconds of the check's own
    /// once its head answered, then its body read whole.
    #[readiness]
    async fn carrier_answers(&self) -> Result<(), CarrierFailed> {
        let http = client(Arc::clone(&self.upstream));
        let answered = http.get(&self.watched.parcel).send().await?;
        tokio::time::sleep(Duration::from_millis(5)).await;
        self.watched.reading.store(true, Ordering::Relaxed);
        answered.bytes().await?;
        Ok(())
    }
}

/// How long the health edge below lets a check run: long enough for a head
/// to cross loopback on a loaded machine, and well inside the policy's timeout.
const CHECK: Duration = Duration::from_millis(300);

#[module(
    imports = [
        HealthModule::for_root(HealthConfig {
            indicator_timeout: CHECK,
            ..HealthConfig::default()
        }),
        ResilienceModule::for_root(ResilienceConfig {
            breaker_window: Some(1),
            breaker_failures: 1,
            ..quiet()
        }),
    ],
    providers = [CarrierIndicator],
)]
struct CarrierHealthModule;

/// The health edge cutting an indicator drops the response its check reads
/// inside the deadline it cut at: a read waiting there for a body that never
/// ends is a timeout in the breaker of the policy the request was sent under,
/// whatever the check took of its own between the head and the body, and
/// however much of the policy's timeout is left.
#[tokio::test]
async fn a_body_its_indicator_reads_as_the_health_edge_cuts_it_has_timed_out() {
    let peer = answering([PeerAnswer::stalling(200)]).await;
    let app = HeadlessApp::builder()
        .module::<CarrierHealthModule>()
        .provide(Watched {
            parcel: peer.url(PARCEL),
            reading: AtomicBool::new(false),
        })
        .build_headless()
        .await
        .expect("an indicator over a client under the policy boots");
    let health = app.container().get::<HealthService>().expect("provided");
    let watched = app.container().get::<Watched>().expect("seeded");
    let policy = held::<nest_rs_core::Unnamed>(&app);
    let logs = LogCapture::install();

    let started = Instant::now();
    let probed = health.probe(ProbeKind::Readiness).await;

    assert_eq!(probed.status, IndicatorStatus::Down);
    assert!(
        started.elapsed() < quiet().timeout / 2,
        "cut by the health edge, the policy's timeout far from spent"
    );
    assert!(
        watched.reading.load(Ordering::Relaxed),
        "the cut found the check reading its answer's body"
    );
    assert_eq!(peer.requests().len(), 1);
    assert!(
        matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
        "{:?}",
        policy.breaker_state()
    );
    let opening = logs.expect_one(TARGET, "circuit breaker changed state");
    assert_eq!(
        opening.field("error").as_deref(),
        Some("the call did not answer within its timeout"),
        "{opening:?}"
    );
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
    let opened = filed(
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

/// A body the dependency cuts short is no attempt the policy gave up: the
/// failure is told by the opening of the breaker it counts in, and no timeout.
#[tokio::test]
async fn a_body_cut_short_by_the_dependency_is_told_by_the_breakers_opening_alone() {
    let peer = answering([PeerAnswer::truncated(200)]).await;
    let (_app, policy) = policy(ResilienceConfig {
        breaker_window: Some(1),
        breaker_failures: 1,
        ..quiet()
    })
    .await;
    let mut events = policy.events();

    let answered = client(policy)
        .post(peer.url(PARCEL))
        .send()
        .await
        .expect("the head answered");
    let failed = answered.bytes().await.expect_err("half a body");
    assert!(cut(&failed).is_none(), "the transport's own: {failed:?}");

    let told = told(&mut events);
    assert!(
        matches!(
            &told[..],
            [ResilienceEvent::Transition {
                from: BreakerPhase::Closed,
                to: BreakerPhase::Open,
                manual: false,
                ..
            }]
        ),
        "{told:?}"
    );
}

/// A request answered while a retry is left is covered as one sent once is:
/// its head is handed back before its body, which holds the run's slot while
/// unread, is given up at the attempt's bound, and counts in the breaker.
#[tokio::test]
async fn the_body_of_a_request_answered_with_a_retry_left_is_read_under_the_policy_never_buffered()
{
    let peer = answering([PeerAnswer::stalling(200)]).await;
    let (_app, policy) = policy(ResilienceConfig {
        timeout: BODY_TIMEOUT,
        concurrency: Some(1),
        breaker_window: Some(1),
        breaker_failures: 1,
        ..retrying()
    })
    .await;

    // Guards against a hang alone: a body buffered before its head is handed
    // back never comes whole.
    let sent = client(Arc::clone(&policy)).get(peer.url(PARCEL)).send();
    let answered = tokio::time::timeout(Duration::from_secs(3), sent)
        .await
        .expect("the head is handed back before its body")
        .expect("the head answered");
    assert_eq!(peer.requests().len(), 1, "at its first attempt");
    assert_eq!(
        policy.concurrency_running(),
        Some(1),
        "the call runs while its answer's body is unread"
    );

    // Guards against a hang alone: a body no bound covers is read for good.
    let given_up = tokio::time::timeout(Duration::from_secs(3), answered.bytes())
        .await
        .expect("the read ends at the attempt's timeout")
        .expect_err("the rest never came");
    assert!(
        matches!(
            cut(&given_up),
            Some(ResilienceError::TimedOut { timeout, .. }) if *timeout == BODY_TIMEOUT
        ),
        "{given_up:?}"
    );
    assert!(
        matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
        "{:?}",
        policy.breaker_state()
    );
    assert_eq!(policy.concurrency_running(), Some(0));
}

/// A probe follows the body's rules: answered with no body, it closes the
/// breaker as its head comes; answered with one, it keeps the probe's turn
/// while the body is unread, and closes the breaker when the caller, having
/// read the status alone, drops the response. A request refused beside the
/// response held is told the probe's whole bound, the policy's timeout, where
/// one refused beside a probe still unanswered is told what is left of it:
/// nothing counts a held answer down.
#[tokio::test]
async fn a_probes_answer_closes_the_breaker_once_its_body_is_read_or_dropped() {
    let peer = answering([PeerAnswer::status(200), PeerAnswer::stalling(200)]).await;
    let config = ResilienceConfig {
        breaker_window: Some(1),
        breaker_failures: 1,
        ..quiet()
    };
    let timeout = config.timeout;
    let (_app, policy) = policy(config).await;
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
        matches!(
            refusal(&refused),
            ResilienceError::Open { retry_after, .. } if *retry_after == timeout
        ),
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
