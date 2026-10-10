//! A route `#[api(deprecated = "…")]` marks: the `Deprecation` header
//! (RFC 9745) on what it answers, and the facts the document states.

use poem::http::{HeaderName, HeaderValue};
use poem::{Endpoint, EndpointExt, IntoResponse, Response};

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
pub fn deprecated_route<E: Endpoint + 'static>(
    endpoint: E,
    header: &'static str,
) -> impl Endpoint<Output = Response> {
    endpoint
        .map_to_response()
        .around(move |ep, req| async move {
            let mut resp = ep.call(req).await?.into_response();
            resp.headers_mut()
                .insert(DEPRECATION, HeaderValue::from_static(header));
            Ok(resp)
        })
}

#[cfg(test)]
mod tests {
    use poem::test::TestClient;
    use poem::{Route, get, handler};

    use super::*;

    #[handler]
    fn list() -> &'static str {
        "[]"
    }

    #[tokio::test]
    async fn every_answer_of_a_deprecated_route_says_since_when() {
        let resp =
            TestClient::new(Route::new().at("/", get(deprecated_route(list, "@1791417600"))))
                .get("/")
                .send()
                .await;
        resp.assert_status_is_ok();
        resp.assert_header("deprecation", "@1791417600");
    }
}
