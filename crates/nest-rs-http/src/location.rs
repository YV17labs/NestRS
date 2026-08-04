//! The `Location` response header, for a route that mints a resource.
//!
//! RFC 9110 §15.3.2 asks a `201` to name what it created. The value is composed
//! with [`join_path`](crate::join_path) — the same function the transport mounts
//! with and the OpenAPI document documents with — so the URI a caller is handed
//! back cannot drift from the one that route actually serves.
//!
//! It lives here rather than beside `#[crud]`'s data layer because nothing about
//! it is ORM-shaped: a collection path, an id, a `Response`. The id arrives as
//! `impl Display` so this crate needs no `uuid` of its own.

use std::fmt::Display;

use poem::http::{HeaderValue, Uri, header::LOCATION};
use poem::{Request, Response};

/// The URI the caller addressed, captured at the transport edge.
///
/// poem's own `original_uri()` cannot serve this: it is populated only on the
/// hyper path, and a request built through `Request::builder()` — which
/// `TestClient`, and therefore every `TestApp` suite, goes through — carries a
/// `Default` state whose URI is `/`. Reading it would make a `201` name
/// `/<id>` in process and `/orgs/<id>` on the wire: the in-process witness
/// would fail on a route that is correct, which is the opposite of what a boot
/// test is for.
#[derive(Clone)]
pub(crate) struct CallerUri(pub(crate) Uri);

/// The path the caller addressed — the collection a `#[crud]` create posted to.
///
/// Read from the edge's capture rather than off `uri()`, which the router
/// rewrites: a global prefix is mounted with `Route::nest`, which strips itself
/// off before the handler runs. Same value in process and on the wire, so a
/// `TestApp` assertion about the `201`'s `Location` means what it says.
pub fn caller_path(req: &Request) -> &str {
    match req.extensions().get::<CallerUri>() {
        Some(CallerUri(uri)) => uri.path(),
        // The edge captures on every routed request, so this is an endpoint
        // driven without one — a bare `Route` in a unit test. `uri()` rather
        // than `original_uri()`: whoever built the request populated it, and
        // the only segment it can be missing is a global prefix, which a tree
        // assembled without the edge has not applied either.
        None => req.uri().path(),
    }
}

/// Stamp `Location: <collection_path>/<id>` onto a create response.
///
/// An **absolute-path reference**, which RFC 9110 §10.2.2 permits and which
/// costs nothing in trust: an absolute URI would have to name a host, and the
/// only host a request carries is the `Host` header — the one field a client
/// controls. Pass the collection path as the caller sent it — [`caller_path`]
/// — so a global prefix or a `/v1` segment is already part of the answer.
///
/// A header value that will not build is dropped rather than raised: an id that
/// renders as ASCII and a routed path cannot produce one, and a `201` whose body
/// already carries the id is not worth failing a completed insert over. Same
/// posture as the pagination cursor on the `#[crud]` list route.
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

    /// The composition cases `join_path` guarantees, asserted through this
    /// function so the `201`'s URI is pinned at the surface a caller sees — a
    /// prefixed or versioned collection keeps its segments, and a trailing
    /// slash cannot double the separator.
    #[test]
    fn the_location_is_the_collection_path_plus_the_id() {
        assert_eq!(location_of("/posts"), format!("/posts/{ID}"));
        assert_eq!(location_of("/api/v1/posts"), format!("/api/v1/posts/{ID}"));
        assert_eq!(location_of("/posts/"), format!("/posts/{ID}"));
        assert_eq!(location_of("/"), format!("/{ID}"));
    }

    /// The capture wins over `uri()` — the case that separates a request the
    /// router has already stripped a global prefix from (`/posts`) from what
    /// the caller actually addressed (`/api/posts`).
    #[test]
    fn caller_path_reads_the_edges_capture() {
        let mut req = Request::builder().uri_str("/posts").finish();
        req.extensions_mut()
            .insert(CallerUri(Uri::from_static("/api/posts")));

        assert_eq!(caller_path(&req), "/api/posts");
    }

    /// No edge, no capture: `uri()` answers. `original_uri()` would say `/`
    /// here — poem populates it on the hyper path only — which is the whole
    /// reason this function exists.
    #[test]
    fn caller_path_falls_back_to_the_request_uri() {
        let req = Request::builder().uri_str("/posts").finish();

        assert_eq!(caller_path(&req), "/posts");
        assert_eq!(req.original_uri().path(), "/");
    }
}
