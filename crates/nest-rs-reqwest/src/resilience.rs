//! [`ResilienceMiddleware`] — an app's reqwest client sent through a
//! resilience [`Policy`]: timeout, breaker, bulkhead, and retries where the
//! request is idempotent.

use std::error::Error as StdError;
use std::fmt;

use nest_rs_resilience::{Policy, ResilienceError};
use reqwest::{Method, Request, Response, StatusCode};
use reqwest_middleware::{Error, Middleware, Next, Result};

/// The header whose presence declares a non-idempotent request safe to send
/// again: the server deduplicates on it.
const IDEMPOTENCY_KEY: &str = "idempotency-key";

/// Sends every request through a resilience [`Policy`]: the policy's
/// timeout, deadline, circuit breaker and bulkhead apply to every request, and
/// its retries to an **idempotent** one alone — a method RFC 9110 §9.2.2 names
/// idempotent (`GET`, `HEAD`, `OPTIONS`, `TRACE`, `PUT`, `DELETE`), or any
/// request carrying an `Idempotency-Key` header — whose body can be cloned.
///
/// A transport error, a `408`, a `429` and a `5xx` other than `501` count as
/// the dependency failing: retried, and recorded by the breaker. The last
/// attempt's response is answered as it is, so the caller still reads its
/// status. A policy's refusal (an open breaker, a full bulkhead, a timeout) is
/// a [`reqwest_middleware::Error::Middleware`] holding the
/// [`ResilienceError`].
///
/// Each attempt runs the rest of the chain with a copy of the request's
/// extensions; what a later middleware writes into them is not seen here.
///
/// ```
/// use nest_rs_reqwest::{ReqwestMiddleware, ResilienceMiddleware};
/// use nest_rs_resilience::presets;
///
/// let carrier = presets::outbound_http("carrier").build()?;
/// let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
///     .with(ReqwestMiddleware::new())
///     .with(ResilienceMiddleware::new(carrier))
///     .build();
/// # Ok::<(), nest_rs_resilience::InvalidPolicy>(())
/// ```
#[derive(Clone, Debug)]
pub struct ResilienceMiddleware {
    policy: Policy,
}

impl ResilienceMiddleware {
    /// Sends every request through `policy`.
    pub const fn new(policy: Policy) -> Self {
        Self { policy }
    }

    /// The policy requests are sent through.
    pub const fn policy(&self) -> &Policy {
        &self.policy
    }
}

/// Whether `request` may be sent again: an idempotent method, or an
/// `Idempotency-Key` the server deduplicates on.
fn idempotent(request: &Request) -> bool {
    matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE | Method::PUT | Method::DELETE
    ) || request.headers().contains_key(IDEMPOTENCY_KEY)
}

/// Whether `status` is the dependency failing rather than answering.
fn failing(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || (status.is_server_error() && status != StatusCode::NOT_IMPLEMENTED)
}

/// What one attempt ended on, short of a response the caller takes as it is.
enum Attempt {
    /// A response answering a failing status.
    Status(Response),
    /// The chain's own error.
    Error(Error),
}

/// The variant a refusal's mapped error stands in, which a refusal never
/// holds.
#[derive(Debug)]
struct NotAttempted;

impl fmt::Display for NotAttempted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the request was not answered")
    }
}

impl StdError for NotAttempted {}

#[async_trait::async_trait]
impl Middleware for ResilienceMiddleware {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut http::Extensions,
        next: Next<'_>,
    ) -> Result<Response> {
        // A request whose body streams cannot be sent twice: once, through
        // the policy all the same.
        let retriable = idempotent(&req) && req.try_clone().is_some();
        let mut original = Some(req);
        let shared = &*extensions;
        let call = self.policy.call();
        let call = if retriable { call.idempotent() } else { call };
        let answered = call
            .failure_if(|attempt: &Attempt| match attempt {
                Attempt::Status(_) => true,
                Attempt::Error(error) => !matches!(error, Error::Middleware(_)),
            })
            .run(|_cancel| {
                let request = original
                    .as_ref()
                    .and_then(Request::try_clone)
                    .or_else(|| original.take());
                let mut extensions = shared.clone();
                let next = next.clone();
                async move {
                    let Some(request) = request else {
                        return Err(Attempt::Error(Error::middleware(NotAttempted)));
                    };
                    match next.run(request, &mut extensions).await {
                        Ok(response) if failing(response.status()) => {
                            Err(Attempt::Status(response))
                        }
                        Ok(response) => Ok(response),
                        Err(error) => Err(Attempt::Error(error)),
                    }
                }
            })
            .await;
        match answered {
            Ok(response) => Ok(response),
            Err(ResilienceError::Failed {
                error: Attempt::Status(response),
                ..
            }) => Ok(response),
            Err(ResilienceError::Failed {
                error: Attempt::Error(error),
                ..
            }) => Err(error),
            Err(refused) => Err(Error::middleware(refused.map_inner(|_| NotAttempted))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: Method) -> Request {
        Request::new(method, "https://example.com/".parse().expect("a URL"))
    }

    #[test]
    fn only_an_idempotent_request_is_retriable() {
        assert!(idempotent(&request(Method::GET)));
        assert!(idempotent(&request(Method::PUT)));
        assert!(!idempotent(&request(Method::POST)));
        let mut keyed = request(Method::POST);
        keyed
            .headers_mut()
            .insert(IDEMPOTENCY_KEY, http::HeaderValue::from_static("k-1"));
        assert!(idempotent(&keyed));
    }

    #[test]
    fn a_failing_status_is_the_dependency_failing() {
        for status in [408, 429, 500, 502, 503, 504] {
            assert!(
                failing(StatusCode::from_u16(status).expect("a status")),
                "{status}"
            );
        }
        for status in [200, 204, 400, 404, 409, 501] {
            assert!(
                !failing(StatusCode::from_u16(status).expect("a status")),
                "{status}"
            );
        }
    }
}
