//! The `Location` response header, for a route that mints a resource.
//!
//! RFC 9110 §15.3.2 asks a `201` to name what it created. The value is composed
//! with [`join_path`](crate::join_path), as the transport mounts the route.

use std::fmt::Display;

use poem::http::{HeaderValue, Uri, header::LOCATION};
use poem::{Request, Response};

/// The URI the caller addressed, captured at the transport edge.
///
/// poem's `original_uri()` is populated only on the hyper path: a request built
/// through `Request::builder()`, as every `TestApp` sends, reads `/`.
#[derive(Clone)]
pub(crate) struct CallerUri(pub(crate) Uri);

/// The path the caller addressed — the collection a `#[crud]` create posted to.
///
/// Read from the edge's capture: `Route::nest` strips a global prefix off
/// `uri()` before the handler runs.
pub fn caller_path(req: &Request) -> &str {
    match req.extensions().get::<CallerUri>() {
        Some(CallerUri(uri)) => uri.path(),
        // No edge: a bare `Route` in a unit test, which applied no global prefix either.
        None => req.uri().path(),
    }
}

/// Stamp `Location: <collection_path>/<id>` onto a create response.
///
/// An absolute-path reference (RFC 9110 §10.2.2): an absolute URI would name
/// the client-controlled `Host`. Pass [`caller_path`] as the collection path.
///
/// A header value that will not build is dropped, never raised.
pub fn set_created_location(resp: &mut Response, collection_path: &str, id: impl Display) {
    let location = crate::join_path(collection_path, &id.to_string());
    if let Ok(value) = HeaderValue::from_str(&location) {
        resp.headers_mut().insert(LOCATION, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "018f3f9c-0000-7000-8000-000000000001";

    fn location_of(collection_path: &str) -> String {
        let mut resp = Response::builder().finish();
        set_created_location(&mut resp, collection_path, ID);
        resp.headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .expect("the header lands on the response")
            .to_owned()
    }

    #[test]
    fn the_location_is_the_collection_path_plus_the_id() {
        assert_eq!(location_of("/posts"), format!("/posts/{ID}"));
        assert_eq!(location_of("/api/v1/posts"), format!("/api/v1/posts/{ID}"));
        assert_eq!(location_of("/posts/"), format!("/posts/{ID}"));
        assert_eq!(location_of("/"), format!("/{ID}"));
    }

    #[test]
    fn caller_path_reads_the_edges_capture() {
        let mut req = Request::builder().uri_str("/posts").finish();
        req.extensions_mut()
            .insert(CallerUri(Uri::from_static("/api/posts")));

        assert_eq!(caller_path(&req), "/api/posts");
    }

    #[test]
    fn caller_path_falls_back_to_the_request_uri() {
        let req = Request::builder().uri_str("/posts").finish();

        assert_eq!(caller_path(&req), "/posts");
        assert_eq!(req.original_uri().path(), "/");
    }
}
