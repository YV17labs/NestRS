//! [`ResilienceMiddleware`] — a policy on the reqwest client an app builds
//! itself: every request it sends runs under the policy, and one the HTTP
//! method or an `Idempotency-Key` declares idempotent is sent again.

use std::convert::Infallible;
use std::sync::Arc;

use http::Extensions;
use nest_rs_core::__private::within_until;
use nest_rs_core::{Instance, Unnamed};
use nest_rs_reqwest::__private::{Resent, Traced, retry_after};
use reqwest::header::{DATE, HeaderName, HeaderValue, RETRY_AFTER};
use reqwest::{Request, Response, StatusCode};
use reqwest_middleware::{Error, Middleware, Next, Result};

use crate::error::{Refusal, ResilienceError};
use crate::policy::{Attempted, Once, ResiliencePolicy, Verdict};

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
/// `500`, `502`, `503` or `504`, and each counts as a failure in the breaker;
/// any other answer is the dependency's answer. A `429`'s or a `503`'s
/// `Retry-After` is waited whole — in seconds, or an HTTP-date counted from the
/// answer's own `Date` — and one past the policy's backoff ceiling ends the run
/// with that answer. The last answer is returned as it came; a
/// refusal of the policy's is a [`reqwest_middleware::Error::Middleware`]
/// holding a `ResilienceError<Infallible>`, whose `503` every edge answers.
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
        let mut run = self.policy.begin(None).await.map_err(refused)?;
        loop {
            if run.resent > 0 {
                extensions.insert(Resent(run.resent));
            }
            let (sending, carried) = (next.clone(), &mut *extensions);
            let Some(copy) = (replayed && run.may_resend())
                .then(|| req.try_clone())
                .flatten()
            else {
                let (attempted, ()) = run
                    .attempt(
                        Once(move |_| sending.run(req, carried)),
                        judged,
                        within_until,
                    )
                    .await;
                return answered(run.finish(attempted));
            };
            let (attempted, ()) = run
                .attempt(
                    Once(move |_| sending.run(copy, carried)),
                    judged,
                    within_until,
                )
                .await;
            if !run.again(&attempted).await {
                return answered(run.finish(attempted));
            }
        }
    }
}

/// What the middleware answers, sending nothing, when `ReqwestMiddleware` runs
/// before it: one span would cover every attempt, and none would be numbered.
#[derive(Debug, thiserror::Error)]
#[error(
    "ResilienceMiddleware was added after ReqwestMiddleware: add it before, so each attempt is a \
     client span of its own"
)]
struct TracedOutside;

/// A transport failure and the statuses a retry may cure fail; any other answer
/// is the dependency's.
fn judged(answer: &Result<Response>) -> Verdict {
    let Ok(response) = answer else {
        return Verdict::Failed { asked: None };
    };
    match response.status() {
        StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE => Verdict::Failed {
            asked: response.headers().get(RETRY_AFTER).and_then(|asked| {
                retry_after(
                    asked.as_bytes(),
                    response.headers().get(DATE).map(HeaderValue::as_bytes),
                )
            }),
        },
        StatusCode::REQUEST_TIMEOUT
        | StatusCode::INTERNAL_SERVER_ERROR
        | StatusCode::BAD_GATEWAY
        | StatusCode::GATEWAY_TIMEOUT => Verdict::Failed { asked: None },
        _ => Verdict::Succeeded,
    }
}

fn answered(attempted: Attempted<Result<Response>>) -> Result<Response> {
    match attempted {
        Ok((answer, _)) => answer,
        Err(refusal) => Err(refused(refusal)),
    }
}

fn refused(refusal: Refusal) -> Error {
    Error::middleware(ResilienceError::<Infallible>::from(refusal))
}
