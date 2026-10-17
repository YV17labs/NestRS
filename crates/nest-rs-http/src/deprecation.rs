//! A route `#[api(deprecated = "…")]` marks: the `Deprecation` header
//! (RFC 9745) on what it answers, and the facts the document states.

use http::{HeaderName, HeaderValue};

use crate::endpoint::Endpoint;
use crate::error::Result;
use crate::request::Request;
use crate::response::Response;

/// RFC 9745 §2's field name.
const DEPRECATION: HeaderName = HeaderName::from_static("deprecation");

/// What a deprecated route declares, as `#[routes]` reads it off
/// `#[api(deprecated = "…")]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeprecationMeta {
    /// The RFC 3339 full-date the route was deprecated on.
    pub since: &'static str,
    /// The `Deprecation` header value: `@` and that date's Unix time.
    pub header: &'static str,
}

/// Stamp `Deprecation` onto every response the route answers, a guard's
/// refusal included; a handler's `Err` is rendered past the route and carries
/// none.
pub fn deprecated_route<E: Endpoint>(endpoint: E, header: &'static str) -> impl Endpoint {
    DeprecatedRoute { endpoint, header }
}

/// A route `#[api(deprecated = "…")]` marks, as [`deprecated_route`] wraps it.
struct DeprecatedRoute<E> {
    endpoint: E,
    header: &'static str,
}

impl<E: Endpoint> Endpoint for DeprecatedRoute<E> {
    async fn call(&self, req: Request) -> Result<Response> {
        let mut resp = self.endpoint.call(req).await?;
        resp.headers_mut()
            .insert(DEPRECATION, HeaderValue::from_static(self.header));
        Ok(resp)
    }
}

#[cfg(test)]
mod tests {
    use crate::endpoint::endpoint_fn;

    use super::*;

    #[tokio::test]
    async fn every_answer_of_a_deprecated_route_says_since_when() {
        let list = endpoint_fn(|_req| async { Ok(Response::builder().body("[]")) });
        let resp = deprecated_route(list, "@1791417600")
            .call(Request::builder().finish())
            .await
            .expect("answers");
        assert_eq!(resp.status(), http::StatusCode::OK);
        assert_eq!(resp.headers()["deprecation"], "@1791417600");
    }
}
