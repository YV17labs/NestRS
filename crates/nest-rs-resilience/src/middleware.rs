//! [`ResilienceMiddleware`] — a policy on the reqwest client an app builds
//! itself: every request it sends runs under the policy, its answer's body
//! included, and one the HTTP method or an `Idempotency-Key` declares
//! idempotent is sent again.

use std::convert::Infallible;
use std::future::{Future, poll_fn};
use std::panic::resume_unwind;
use std::pin::{Pin, pin};
use std::sync::Arc;

use http::Extensions;
use nest_rs_core::{Instance, Unnamed};
use nest_rs_reqwest::__private::{Resent, Traced, retry_after};
use reqwest::header::{DATE, HeaderName, HeaderValue, RETRY_AFTER};
use reqwest::{Request, Response, ResponseBuilderExt, StatusCode};
use reqwest_middleware::{Error, Middleware, Next, Result};

use crate::body::ResilienceBody;
use crate::breaker::Failure;
use crate::call::{Calling, Judged, Once, Reads, Uncancelled};
use crate::error::{ResilienceError, ResilienceRefusal};
use crate::held::Held;
use crate::policy::ResiliencePolicy;
use crate::run::Run;
use crate::verdict::{Ended, Verdict};

/// The header a client sends to make a request it could not otherwise replay
/// safe to send again (`draft-ietf-httpapi-idempotency-key-header`).
const IDEMPOTENCY_KEY: HeaderName = HeaderName::from_static("idempotency-key");

/// Runs every request an app's own reqwest client sends under the
/// [`ResiliencePolicy<I>`] it was built with: each attempt given up past the
/// policy's timeout, counted in its breaker and its rate limit, inside its
/// bulkhead.
///
/// Each request it hands on is one attempt, and a redirect the client follows
/// goes out inside it: under its timeout, judged by the answer it ends on, and
/// counted once in the [rate limit](crate::ResilienceConfig::rate_limit). Where
/// the dependency's own limit counts every request it is sent, the client is
/// built with
/// [`redirect::Policy::none()`](reqwest::redirect::Policy::none): a redirect is
/// then the answer, as it came, and the request the app sends after it an
/// attempt of its own.
///
/// A request is sent again — after the policy's backoff, up to its retries —
/// only when it is idempotent — its method is RFC 9110 §9.2.2's `GET`, `HEAD`,
/// `PUT`, `DELETE`, `OPTIONS` or `TRACE`, or `QUERY`, or it carries an
/// `Idempotency-Key` — and its body can be sent twice (a streamed body cannot).
/// What is sent again is a transport failure, a timeout, or a `408`, `429`,
/// `500`, `502`, `503` or `504`, read off the answer's head, and each counts
/// as a failure in the breaker; any other answer is the dependency's answer.
/// The line a retry files and the `warn` of the breaker such a failure opens
/// say it as their `error`: a transport failure by reqwest's kind and what
/// `nest-rs-reqwest` reads of the causes beneath it, never the URL it was sent
/// to nor a word the dependency wrote, and a status by its code alone, never a
/// header or a body. A
/// `429`'s or a `503`'s `Retry-After` is waited whole — in seconds, or an
/// HTTP-date counted from the answer's own `Date` — and one past the policy's
/// backoff ceiling ends the run with that answer. An answer the run sends
/// again is dropped unread before the wait for its retry, which so holds no
/// connection and no stream of it; the last one is returned as it came, and a
/// refusal of the policy's is a [`reqwest_middleware::Error::Middleware`]
/// holding a `ResilienceError<Infallible>`, whose `503` every edge answers.
///
/// **The policy covers the answer's body, read as it comes and never
/// buffered.** The response handed back is reqwest's own, with its URL, its
/// status, its headers and its extensions, its body decoded as the client was
/// built to; until that body ends it holds the run's slot of the bulkhead, the
/// breaker's verdict on a head that was no failure, and what the head left of
/// the attempt's timeout, which runs from the send. That rest bounds how long
/// the caller waits for the dependency, over every read of the body, never how
/// long the caller takes: a read counts against it only while it waits for a
/// part that has not come, until the transport signals one, and the time
/// between two reads, or away from a read the caller abandoned, counts for
/// nothing.
///
/// - read to its end, the answer counts as its head did, and the slot is
///   freed there;
/// - a read that fails is a failure in the breaker, told as its chain, and
///   reaches the caller as reqwest's own error;
/// - a read still waiting once that rest is spent is given up there, a
///   failure counted as a timeout: [`bytes`](Response::bytes),
///   [`chunk`](Response::chunk) and the others answer reqwest's error, whose
///   chain holds the `ResilienceError<Infallible>` a request given up before
///   its head answers, the [`TimedOut`](crate::ResilienceRefusal::TimedOut) that
///   tells the policy's cut from a transport's. A part that came is handed on
///   whenever it is read, so a caller slower than the dependency is never cut,
///   and is charged only the round trip a transport's flow control makes the
///   next part wait for;
/// - dropped before its end — a caller reading a status alone — the answer
///   counts as its head did, and the slot is freed there. One left with a read
///   that has waited past that rest — a read the caller abandoned, or one an
///   edge cut with its unit — or with a read that waited across the deadline
///   of the unit the body is read in, begun before it and dropped at or past
///   it, as that unit's edge cutting its reader drops it whatever the rest has
///   left, or with a read still waiting as the timeout of another policy, the
///   one its reader runs under, drops it, is read once more as it is dropped:
///   a part or the end that came since counts as the head did, a failure as
///   itself, and a body still waiting as a timeout. That deadline is read
///   where the body is dropped, and cuts no read itself: a body handed on past
///   the unit its request was sent in — a handler answering the response's
///   stream, which its edge reads once that unit is over — is read within
///   that rest alone, and a client leaving a read of it, that rest unspent,
///   counts as the head did;
/// - a panic while the body is read is a failure in the breaker, as a panic in
///   the request's own send is, and unwinds on.
///
/// A request sent inside an attempt of its own policy — a
/// [`run`](ResiliencePolicy::run) of it, or a method `#[resilient]` with it —
/// is a call that attempt makes. It shares the attempt's slot and turn of the
/// breaker while no other call does, the first taking its token — the turn
/// only while the breaker has neither opened nor been switched by hand since
/// the attempt was let through, refused unsent once it is open — and counts
/// in that attempt, once, as it ends: a failure it ends on, its body's
/// included, fails the attempt, and one it was sent again past, then
/// answered, fails nothing. A request sent while another shares them takes its own, as one
/// sent alone does, so `CONCURRENCY` and `RATE_LIMIT` bound the requests an
/// attempt fans out. Inside an attempt the policy may still send again, each
/// is sent once, so the two runs' retries do not multiply — though that
/// attempt is sent again only if it then fails as its run retries, so one its
/// `retry_if` refuses, given up past the ceiling or the deadline, or answering
/// `Ok` over the request's error leaves the request sent once; inside one it
/// never sends again — a run once, or the last attempt — it is sent again as
/// any request is. A body read after that attempt has answered holds
/// nothing of the policy.
///
/// So a response is read or dropped, never kept: one held unread holds its
/// slot, and a probe's holds the half-open breaker's one turn, for as long as
/// its caller takes — every call refused beside it told that probe's whole
/// bound, the longest its reads wait for the dependency, since no timer gives
/// a held answer up. A body whose reads wait for the dependency longer than
/// the policy's timeout in all — an event stream idling between events, a
/// download an origin trickles — is cut there, and the cut counts against the
/// dependency: such a client is built without this middleware, or under a
/// policy of its own whose timeout is the transfer's.
///
/// It is added before [`ReqwestMiddleware`](nest_rs_reqwest::ReqwestMiddleware),
/// so each attempt is a client span of its own, numbered by its
/// `http.request.resend_count`; added after it, it refuses every request,
/// unsent, saying so:
///
/// ```
/// use std::sync::Arc;
///
/// use nest_rs_reqwest::ReqwestMiddleware;
/// use nest_rs_resilience::{ResilienceMiddleware, ResiliencePolicy};
///
/// fn client(policy: Arc<ResiliencePolicy>) -> reqwest_middleware::ClientWithMiddleware {
///     reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
///         .with(ResilienceMiddleware::new(policy))
///         .with(ReqwestMiddleware::new())
///         .build()
/// }
/// ```
pub struct ResilienceMiddleware<I = Unnamed> {
    policy: Arc<ResiliencePolicy<I>>,
}

impl<I: Instance> ResilienceMiddleware<I> {
    /// The middleware running every request under `policy`.
    pub const fn new(policy: Arc<ResiliencePolicy<I>>) -> Self {
        Self { policy }
    }
}

impl<I: Instance> std::fmt::Debug for ResilienceMiddleware<I> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResilienceMiddleware")
            .field("policy", &self.policy)
            .finish()
    }
}

#[async_trait::async_trait]
impl<I: Instance> Middleware for ResilienceMiddleware<I> {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<Response> {
        if extensions.get::<Traced>().is_some() {
            return Err(Error::middleware(TracedOutside));
        }
        let replayed = req.method().is_idempotent() || req.headers().contains_key(IDEMPOTENCY_KEY);
        let mut run = pin!(Run::holding(&self.policy.mechanisms));
        poll_fn(|cx| run.as_mut().poll_open(cx))
            .await
            .map_err(|refusal| refused(refusal, run.instance()))?;
        loop {
            if run.resent() > 0 {
                extensions.insert(Resent(run.resent()));
            }
            let (sending, carried) = (next.clone(), &mut *extensions);
            let copy = (replayed && run.may_resend())
                .then(|| req.try_clone())
                .flatten();
            let Some(copy) = copy else {
                let mut call = pin!(sent(move || sending.run(req, carried)));
                let ended = run.as_mut().attempt(call.as_mut()).await;
                return answered(run, call, ended);
            };
            let mut call = pin!(sent(move || sending.run(copy, carried)));
            let mut ended = run.as_mut().attempt(call.as_mut()).await;
            let Some(retry) = run.as_mut().retry(&mut ended, call.failure()) else {
                return answered(run, call, ended);
            };
            // Nothing of an attempt sent again is kept through the wait: it
            // counted as it was judged.
            call.as_mut().let_go();
            run.retrying(&retry);
            run.as_mut().back_off(&retry);
            poll_fn(|cx| run.as_mut().poll_back_off(cx)).await;
        }
    }
}

/// What the middleware answers a request with: a response, or reqwest's error.
type Answer = std::result::Result<Response, ResilienceError<Error>>;

/// One request sent, its answer judged by its status.
type Sending<F> = Judged<Once<F, Uncancelled>, Statuses, Answer>;

/// The request `send` sends, once, judged by the answer's status: a request
/// reads no cancellation token, so none is made for it.
fn sent<F, Fut>(send: F) -> Sending<F>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Response>>,
{
    Judged::new(Once::new(send), Statuses)
}

/// What `run` answers, over at its last attempt, which `ended` so: a response
/// with a body to read hands that body what the run held of the policy; any
/// other answer lets it go here, in a run's order — its attempt in the
/// breaker, then its slot.
fn answered<F, Fut>(
    run: Pin<&mut Run<'_>>,
    call: Pin<&mut Sending<F>>,
    ended: Ended,
) -> Result<Response>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Response>>,
{
    match ended {
        Ended::Judged(_) => match call.take() {
            Some(Ok(response)) if response.content_length() != Some(0) => {
                Ok(covered(response, run.hold()))
            }
            Some(answer) => {
                run.release_outstanding();
                answer
            }
            None => unreachable!("an attempt judged holds its answer"),
        },
        Ended::Refused(refusal) => Err(refused(run.finish(refusal), run.instance())),
        Ended::Unwound(payload) => resume_unwind(payload),
    }
}

/// A request's answer judged by what it says of the dependency: its status,
/// or the failure of its transport.
struct Statuses;

impl Reads<Answer> for Statuses {
    fn read(&self, answer: &Result<Response>, _again: bool) -> Verdict {
        judged(answer)
    }

    fn failure<'a>(&self, answer: &'a Result<Response>) -> Option<Failure<'a>> {
        match answer {
            Ok(response) => status_failure(response.status()).map(Failure::Said),
            Err(error) => Some(Failure::Error(error)),
        }
    }
}

/// `response` as it came, but for its body, read through a
/// [`ResilienceBody`] holding `held` until it ends.
fn covered(response: Response, held: Held) -> Response {
    // reqwest's own conversions drop the URL one way and read it back the
    // other off an extension its builder alone writes.
    let mut url = http::Response::builder().url(response.url().clone());
    let (mut parts, body) = http::Response::<reqwest::Body>::from(response).into_parts();
    let Some(url) = url.extensions_mut() else {
        unreachable!("a builder handed a URL alone holds no error");
    };
    parts.extensions.extend(std::mem::take(url));
    let body = reqwest::Body::wrap(ResilienceBody::new(body, held));
    http::Response::from_parts(parts, body).into()
}

/// What the middleware answers, sending nothing, when `ReqwestMiddleware` runs
/// before it: one span would cover every attempt, and none would be numbered.
#[derive(Debug, thiserror::Error)]
#[error(
    "ResilienceMiddleware was added after ReqwestMiddleware: add it before, so each attempt is a \
     client span of its own"
)]
struct TracedOutside;

/// A transport failure and the statuses a retry may cure fail; any other
/// answer is the dependency's. What each failed on is read where it is told
/// ([`Statuses`]'s `failure`): a transport failure as its chain, without the
/// URL it was sent to, a status by its code.
fn judged(answer: &Result<Response>) -> Verdict {
    let Ok(response) = answer else {
        return Verdict::Failed { asked: None };
    };
    let status = response.status();
    if status_failure(status).is_none() {
        return Verdict::Succeeded;
    }
    let asked = match status {
        StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE => {
            response.headers().get(RETRY_AFTER).and_then(|asked| {
                retry_after(
                    asked.as_bytes(),
                    response.headers().get(DATE).map(HeaderValue::as_bytes),
                )
            })
        }
        _ => None,
    };
    Verdict::Failed { asked }
}

/// What an answer of `status` failed on, for a status a retry may cure: its
/// code, the one value of the answer a line says.
fn status_failure(status: StatusCode) -> Option<&'static str> {
    match status {
        StatusCode::REQUEST_TIMEOUT => Some("the dependency answered 408"),
        StatusCode::TOO_MANY_REQUESTS => Some("the dependency answered 429"),
        StatusCode::INTERNAL_SERVER_ERROR => Some("the dependency answered 500"),
        StatusCode::BAD_GATEWAY => Some("the dependency answered 502"),
        StatusCode::SERVICE_UNAVAILABLE => Some("the dependency answered 503"),
        StatusCode::GATEWAY_TIMEOUT => Some("the dependency answered 504"),
        _ => None,
    }
}

/// `refusal`, made by the policy of `instance`, as reqwest's error carries it.
fn refused(refusal: ResilienceRefusal, instance: &'static str) -> Error {
    Error::middleware(ResilienceError::<Infallible>::refused(refusal, instance))
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::task::{Context, Poll};
    use std::time::Duration;

    use super::*;

    fn answer(status: StatusCode, retry_after: Option<&str>) -> Result<Response> {
        let mut answer = http::Response::builder().status(status);
        if let Some(asked) = retry_after {
            answer = answer.header(RETRY_AFTER, asked);
        }
        Ok(answer.body("").expect("an answer").into())
    }

    /// Each status a retry may cure fails saying its code, a `429` and a `503`
    /// alone asking the wait their `Retry-After` says; any other answer, a
    /// client's error included, is the dependency's.
    #[test]
    fn a_status_a_retry_may_cure_fails_saying_its_code_and_any_other_is_an_answer() {
        for (status, asks) in [
            (StatusCode::REQUEST_TIMEOUT, false),
            (StatusCode::TOO_MANY_REQUESTS, true),
            (StatusCode::INTERNAL_SERVER_ERROR, false),
            (StatusCode::BAD_GATEWAY, false),
            (StatusCode::SERVICE_UNAVAILABLE, true),
            (StatusCode::GATEWAY_TIMEOUT, false),
        ] {
            let answered = answer(status, Some("7"));
            assert_eq!(
                judged(&answered),
                Verdict::Failed {
                    asked: asks.then_some(Duration::from_secs(7)),
                },
                "{status}"
            );
            let said = Statuses
                .failure(&answered)
                .and_then(|failure| failure.said().ok());
            assert_eq!(
                said.as_deref(),
                Some(format!("the dependency answered {}", status.as_str()).as_str()),
            );
        }
        for status in [
            StatusCode::OK,
            StatusCode::SEE_OTHER,
            StatusCode::NOT_FOUND,
            StatusCode::NOT_IMPLEMENTED,
        ] {
            assert_eq!(
                judged(&answer(status, Some("7"))),
                Verdict::Succeeded,
                "{status}"
            );
        }
    }

    /// A failure an inner middleware answers is told as its own chain.
    #[test]
    fn a_middlewares_failure_is_told_as_its_chain() {
        let failed = Err(Error::middleware(std::io::Error::other("no route to host")));
        assert_eq!(judged(&failed), Verdict::Failed { asked: None });
        let said = Statuses
            .failure(&failed)
            .and_then(|failure| failure.said().ok());
        assert_eq!(said.as_deref(), Some("no route to host"));
    }

    /// A dependency counting the requests it is sent, answering each `200`: a
    /// middleware after the policy's.
    struct Counting(Arc<AtomicU32>);

    #[async_trait::async_trait]
    impl Middleware for Counting {
        async fn handle(
            &self,
            _req: Request,
            _extensions: &mut Extensions,
            _next: Next<'_>,
        ) -> Result<Response> {
            self.0.fetch_add(1, Ordering::SeqCst);
            answer(StatusCode::OK, None)
        }
    }

    /// A request its unit has no time left to send is no attempt, so no retry
    /// of it is given up: refused unsent, a request the run would send again
    /// files no line, whether a bulkhead refuses its run as it opens or its
    /// attempt is refused as it is let through.
    #[tokio::test(start_paused = true)]
    async fn a_request_its_unit_has_no_time_left_to_send_gives_up_no_retry() {
        for concurrency in [None, Some(4)] {
            let config = crate::ResilienceConfig {
                retries: Some(2),
                breaker_window: None,
                concurrency,
                ..crate::ResilienceConfig::default()
            };
            let policy =
                Arc::new(ResiliencePolicy::<Unnamed>::new(&config).expect("a valid policy"));
            let sent = Arc::new(AtomicU32::new(0));
            let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
                .with(ResilienceMiddleware::new(policy))
                .with(Counting(Arc::clone(&sent)))
                .build();

            let lines = nest_rs_testing::LogCapture::install();
            let refused = nest_rs_core::__private::with_deadline(
                Some(tokio::time::Instant::now()),
                client.get("https://upstream.example/").send(),
            )
            .await
            .expect_err("no time left to send it");

            let refusal = match &refused {
                Error::Middleware(refusal) => refusal.downcast_ref::<ResilienceError<Infallible>>(),
                Error::Reqwest(_) => None,
            };
            assert!(
                matches!(refusal, Some(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. }) if timeout.is_zero()),
                "{refused:?}"
            );
            assert_eq!(sent.load(Ordering::SeqCst), 0, "nothing was sent");
            let filed: Vec<_> = lines
                .events()
                .into_iter()
                .filter(|line| line.target == crate::TARGET)
                .map(|line| line.message)
                .collect();
            assert_eq!(filed, [""; 0], "concurrency: {concurrency:?}");
        }
    }

    /// A body no part of which ever comes, saying once it is dropped: what an
    /// answer left unread keeps of its connection, or of its stream.
    struct Unread(Arc<AtomicBool>);

    impl http_body::Body for Unread {
        type Data = &'static [u8];
        type Error = std::io::Error;

        fn poll_frame(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<std::result::Result<http_body::Frame<Self::Data>, Self::Error>>> {
            Poll::Pending
        }
    }

    impl Drop for Unread {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// How long the dependency asks to be left alone: under the backoff's
    /// ceiling of these tests, so it is waited whole.
    const ASKED: Duration = Duration::from_secs(5);

    /// A dependency answering `503` over a body it never sends, asking
    /// [`ASKED`]: a middleware after the policy's.
    struct Overloaded(Arc<AtomicBool>);

    #[async_trait::async_trait]
    impl Middleware for Overloaded {
        async fn handle(
            &self,
            _req: Request,
            _extensions: &mut Extensions,
            _next: Next<'_>,
        ) -> Result<Response> {
            let unread = reqwest::Body::wrap(Unread(Arc::clone(&self.0)));
            Ok(http::Response::builder()
                .status(StatusCode::SERVICE_UNAVAILABLE)
                .header(RETRY_AFTER, ASKED.as_secs())
                .body(unread)
                .expect("an answer")
                .into())
        }
    }

    /// An answer the run sends again is let go once its retry is decided,
    /// before the wait the dependency asked: its body, so its connection or
    /// its stream, is dropped there, the attempt already counted in the
    /// breaker, and the slot, the run's, is kept through the wait.
    #[tokio::test(start_paused = true)]
    async fn an_answer_sent_again_is_let_go_before_the_wait_for_its_retry() {
        let let_go = Arc::new(AtomicBool::new(false));
        let config = crate::ResilienceConfig {
            retries: Some(1),
            backoff_ceiling: 2 * ASKED,
            breaker_window: Some(1),
            breaker_failures: 1,
            concurrency: Some(1),
            ..crate::ResilienceConfig::default()
        };
        let policy = Arc::new(ResiliencePolicy::<Unnamed>::new(&config).expect("a valid policy"));
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
            .with(ResilienceMiddleware::new(Arc::clone(&policy)))
            .with(Overloaded(Arc::clone(&let_go)))
            .build();

        let sending =
            tokio::spawn(async move { client.get("https://upstream.example/").send().await });
        tokio::time::sleep(ASKED / 5).await;

        assert!(
            !sending.is_finished(),
            "the run waits the {ASKED:?} the dependency asked"
        );
        assert!(
            let_go.load(Ordering::SeqCst),
            "the 503 is held, unread, through the wait for its retry"
        );
        assert!(
            matches!(
                policy.breaker_state(),
                Some(crate::BreakerState::Open { .. })
            ),
            "the attempt counted before the wait: {:?}",
            policy.breaker_state()
        );
        assert_eq!(
            policy.concurrency_running(),
            Some(1),
            "the slot is the run's, kept through its retries and their waits"
        );
        sending.abort();
    }

    /// A dependency that never sends what is waited for: with `head`, its
    /// answer's head comes at once, `200` over a body no part of which
    /// follows, and without it nothing comes. A middleware after the policy's.
    struct Stalling {
        head: bool,
    }

    #[async_trait::async_trait]
    impl Middleware for Stalling {
        async fn handle(
            &self,
            _req: Request,
            _extensions: &mut Extensions,
            _next: Next<'_>,
        ) -> Result<Response> {
            if !self.head {
                return std::future::pending().await;
            }
            let unread = reqwest::Body::wrap(Unread(Arc::default()));
            Ok(http::Response::builder()
                .body(unread)
                .expect("an answer")
                .into())
        }
    }

    /// A request sent from a call under another policy's shorter timeout is
    /// cut there, before its own bound: it has timed out in the breaker of the
    /// policy on its client, while its head is waited for and while a read of
    /// its body is, so a dependency that stalls opens the breaker naming it
    /// however short the timeout its caller runs under.
    #[tokio::test(start_paused = true)]
    async fn a_request_the_timeout_of_a_policy_around_its_caller_drops_has_timed_out() {
        let short = Duration::from_secs(2);
        let around = ResiliencePolicy::<Unnamed>::new(&crate::ResilienceConfig {
            timeout: short,
            retries: None,
            breaker_window: None,
            ..crate::ResilienceConfig::default()
        })
        .expect("a valid policy");
        assert!(short < crate::ResilienceConfig::default().timeout);

        for head in [false, true] {
            let config = crate::ResilienceConfig {
                retries: None,
                breaker_window: Some(1),
                breaker_failures: 1,
                concurrency: Some(1),
                ..crate::ResilienceConfig::default()
            };
            let policy =
                Arc::new(ResiliencePolicy::<Unnamed>::new(&config).expect("a valid policy"));
            let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
                .with(ResilienceMiddleware::new(Arc::clone(&policy)))
                .with(Stalling { head })
                .build();

            let cut = around
                .run(|| async {
                    let answered = client.get("https://upstream.example/").send().await?;
                    Ok::<_, Error>(answered.bytes().await?)
                })
                .await;
            assert!(
                matches!(cut, Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. }) if timeout == short),
                "head: {head}: {cut:?}"
            );
            assert!(
                matches!(
                    policy.breaker_state(),
                    Some(crate::BreakerState::Open { .. })
                ),
                "head: {head}: {:?}",
                policy.breaker_state()
            );
            assert_eq!(policy.concurrency_running(), Some(0), "head: {head}");
        }
    }

    /// A response whose reader, after work of its own, waits for its body as
    /// the edge cuts its unit at the deadline has timed out in the breaker,
    /// well inside the policy's timeout: the edge drops the body inside the
    /// deadline it cut at, where the body reads it.
    #[tokio::test(start_paused = true)]
    async fn a_response_whose_read_waits_as_its_units_deadline_cuts_its_reader_has_timed_out() {
        let config = crate::ResilienceConfig {
            retries: None,
            breaker_window: Some(1),
            breaker_failures: 1,
            concurrency: Some(1),
            ..crate::ResilienceConfig::default()
        };
        let policy = Arc::new(ResiliencePolicy::<Unnamed>::new(&config).expect("a valid policy"));
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
            .with(ResilienceMiddleware::new(Arc::clone(&policy)))
            .with(Stalling { head: true })
            .build();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        assert!(Duration::from_secs(1) < config.timeout);
        let unit = nest_rs_core::__private::with_deadline(Some(deadline), async {
            let answered = client.get("https://upstream.example/").send().await?;
            tokio::time::sleep(Duration::from_millis(5)).await;
            Ok::<_, Error>(answered.bytes().await?)
        });
        let cut = tokio::time::timeout_at(deadline, unit).await;

        assert!(cut.is_err(), "the edge cut the unit: {cut:?}");
        assert!(
            matches!(
                policy.breaker_state(),
                Some(crate::BreakerState::Open { .. })
            ),
            "{:?}",
            policy.breaker_state()
        );
        assert_eq!(policy.concurrency_running(), Some(0));
    }

    /// A dependency answering `204` once its answer is let through.
    struct Answering(parking_lot::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>);

    #[async_trait::async_trait]
    impl Middleware for Answering {
        async fn handle(
            &self,
            _req: Request,
            _extensions: &mut Extensions,
            _next: Next<'_>,
        ) -> Result<Response> {
            let answered = self.0.lock().take().expect("one request");
            answered.await.expect("the test lets the answer through");
            Ok(http::Response::builder()
                .status(StatusCode::NO_CONTENT)
                .body("")
                .expect("an answer")
                .into())
        }
    }

    /// A request whose answer came within its bound, inside a unit its edge
    /// cuts at a deadline, counts as that answer though its run is dropped
    /// past the bound before it is polled again: the breaker closed, where it
    /// counted a timeout and opened.
    #[tokio::test(start_paused = true)]
    async fn a_request_answered_in_time_and_dropped_past_its_bound_counts_its_answer() {
        let config = crate::ResilienceConfig {
            retries: None,
            breaker_window: Some(1),
            breaker_failures: 1,
            ..crate::ResilienceConfig::default()
        };
        let policy = Arc::new(ResiliencePolicy::<Unnamed>::new(&config).expect("a valid policy"));
        let (answer, answered) = tokio::sync::oneshot::channel();
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
            .with(ResilienceMiddleware::new(Arc::clone(&policy)))
            .with(Answering(parking_lot::Mutex::new(Some(answered))))
            .build();

        let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
        let mut unit = Box::pin(nest_rs_core::__private::with_deadline(
            Some(deadline),
            client.get("https://upstream.example/").send(),
        ));
        assert!(
            unit.as_mut()
                .poll(&mut Context::from_waker(std::task::Waker::noop()))
                .is_pending()
        );
        answer.send(()).expect("the request waits for its answer");
        tokio::time::advance(Duration::from_millis(400)).await;
        drop(unit);

        assert_eq!(
            policy.breaker_state(),
            Some(crate::BreakerState::Closed),
            "the answer that came in time counted"
        );
    }
}
