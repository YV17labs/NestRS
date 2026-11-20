//! [`ResilienceMiddleware`] — a policy on the reqwest client an app builds
//! itself: every request it sends runs under the policy, and one the HTTP
//! method or an `Idempotency-Key` declares idempotent is sent again.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use http::Extensions;
use nest_rs_core::{Instance, Unnamed};
use nest_rs_reqwest::__private::Resent;
use reqwest::header::{HeaderName, RETRY_AFTER};
use reqwest::{Method, Request, Response, StatusCode};
use reqwest_middleware::{Error, Middleware, Next, Result};

use crate::error::{Refusal, ResilienceError};
use crate::policy::{Attempted, ResiliencePolicy, Verdict};

/// The header a client sends to make a request it could not otherwise replay
/// safe to send again (`draft-ietf-httpapi-idempotency-key-header`).
const IDEMPOTENCY_KEY: HeaderName = HeaderName::from_static("idempotency-key");

/// Runs every request an app's own reqwest client sends under the
/// [`ResiliencePolicy<I>`] it was built with: each attempt given up past the
/// policy's timeout, counted in its breaker, inside its bulkhead.
///
/// A request is sent again — after the policy's backoff, up to its retries —
/// only when it is idempotent: its method is one RFC 9110 §9.2.2 defines so
/// (`GET`, `HEAD`, `PUT`, `DELETE`, `OPTIONS`, `TRACE`), or it carries an
/// `Idempotency-Key`, and its body can be sent twice (a streamed body cannot).
/// What is sent again is a transport failure, a timeout, or a `408`, `429`,
/// `500`, `502`, `503` or `504`, and each counts as a failure in the breaker;
/// any other answer is the dependency's answer. A `Retry-After` in seconds on a
/// `429` or a `503` is waited whole, and one past the policy's backoff ceiling
/// ends the run with that answer. The last answer is returned as it came; a
/// refusal of the policy's is a [`reqwest_middleware::Error::Middleware`]
/// holding a `ResilienceError<Infallible>`, whose `503` every edge answers.
///
/// Added before [`ReqwestMiddleware`](nest_rs_reqwest::ReqwestMiddleware), so
/// each attempt is a client span of its own, numbered by its
/// `http.request.resend_count`:
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
        let replayed = declares_idempotent(&req);
        let mut run = self.policy.begin(replayed).await.map_err(refused)?;
        loop {
            extensions.insert(Resent(run.resent()));
            let copy = if replayed { req.try_clone() } else { None };
            let (sending, carried) = (next.clone(), &mut *extensions);
            let attempted = match copy {
                Some(copy) => {
                    run.attempt(move |_| sending.run(copy, carried), judged)
                        .await
                }
                None => {
                    return answered(
                        run.attempt(move |_| sending.run(req, carried), judged)
                            .await,
                    );
                }
            };
            if !run.again(&attempted).await {
                return answered(attempted);
            }
        }
    }
}

/// Whether `request` may be sent twice: its method is idempotent, or it
/// carries an idempotency key.
fn declares_idempotent(request: &Request) -> bool {
    matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::PUT | Method::DELETE | Method::OPTIONS | Method::TRACE
    ) || request.headers().contains_key(IDEMPOTENCY_KEY)
}

/// How an answer counts: a transport failure and the statuses a retry may cure
/// fail, any other answer succeeds.
fn judged(answer: &Result<Response>) -> Verdict {
    let Ok(response) = answer else {
        return Verdict::Failed { asked: None };
    };
    match response.status() {
        StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE => Verdict::Failed {
            asked: asked_wait(response),
        },
        StatusCode::REQUEST_TIMEOUT
        | StatusCode::INTERNAL_SERVER_ERROR
        | StatusCode::BAD_GATEWAY
        | StatusCode::GATEWAY_TIMEOUT => Verdict::Failed { asked: None },
        _ => Verdict::Succeeded,
    }
}

/// The `Retry-After` `response` asks in seconds (RFC 9110 §10.2.3's
/// `delay-seconds`), a value past what fits read as the largest; an HTTP-date
/// asks nothing this reads.
fn asked_wait(response: &Response) -> Option<Duration> {
    let digits = response.headers().get(RETRY_AFTER)?.to_str().ok()?.trim();
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(Duration::from_secs(digits.parse().unwrap_or(u64::MAX)))
}

/// The run's last attempt, as reqwest-middleware answers it.
fn answered(attempted: Attempted<Result<Response>>) -> Result<Response> {
    match attempted {
        Ok((answer, _)) => answer,
        Err(refusal) => Err(refused(refusal)),
    }
}

/// The policy's refusal, as reqwest-middleware's error.
fn refused(refusal: Refusal) -> Error {
    Error::middleware(ResilienceError::<Infallible>::from(refusal))
}
