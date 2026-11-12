//! The `Location` response header, for a route that mints a resource.
//!
//! RFC 9110 §15.3.2 asks a `201` to name what it created. The value is composed
//! with [`join_path`](crate::transport::join_path), as the transport mounts the route, on
//! the path the caller addressed, read off the edge by whichever tail answered
//! the `Created`.

use http::{HeaderValue, Uri, header::LOCATION};

use crate::request::Request;
use crate::response::Response;

/// The URI the caller addressed, captured at the transport edge before any
/// rewrite in front of the router.
#[derive(Clone)]
pub(crate) struct CallerUri(pub(crate) Uri);

/// The URI the caller addressed.
///
/// Read from the edge's capture: a header or media-type version selector
/// rewrites `uri()` before the handler runs, and the router strips a global
/// prefix off it.
pub(crate) fn caller_uri(req: &Request) -> &Uri {
    match req.extensions().get::<CallerUri>() {
        Some(CallerUri(uri)) => uri,
        // No edge: a unit test calling the endpoint bare, with no selector in front.
        None => req.uri(),
    }
}

/// The path the caller addressed — the collection a create posted to.
pub(crate) fn caller_path(req: &Request) -> &str {
    caller_uri(req).path()
}

/// Stamp `Location: <collection_path>/<id>` onto a create response, the id
/// percent-encoded as one path segment.
///
/// An absolute-path reference (RFC 9110 §10.2.2): an absolute URI would name
/// the client-controlled `Host`. The collection path is [`caller_path`].
///
/// An id that is empty, `.` or `..` names no segment — a client resolves a dot
/// segment away, encoded or not (RFC 3986 §5.2.4) — so no `Location` is
/// written, and that is logged at `warn`.
pub(crate) fn set_created_location(resp: &mut Response, collection_path: &str, id: &str) {
    if matches!(id, "" | "." | "..") {
        tracing::warn!(
            target: crate::target::HTTP,
            id_len = id.len(),
            "a created resource's id names no path segment; its `Location` is not written",
        );
        return;
    }
    let location =
        crate::transport::join_path(collection_path, &crate::router::percent_encoded(id));
    if let Ok(value) = HeaderValue::from_str(&location) {
        resp.headers_mut().insert(LOCATION, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "018f3f9c-0000-7000-8000-000000000001";

    fn location_of(collection_path: &str) -> String {
        location_of_id(collection_path, ID)
    }

    fn location_of_id(collection_path: &str, id: &str) -> String {
        let mut resp = Response::builder().finish();
        set_created_location(&mut resp, collection_path, id);
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
    fn an_id_is_one_path_segment_whatever_it_holds() {
        for (id, location) in [
            ("a b", "/orgs/a%20b"),
            ("x?y=1", "/orgs/x%3Fy%3D1"),
            ("a/b", "/orgs/a%2Fb"),
            ("#top", "/orgs/%23top"),
            ("café", "/orgs/caf%C3%A9"),
            ("100%", "/orgs/100%25"),
            ("Ab-1.2_~", "/orgs/Ab-1.2_~"),
        ] {
            assert_eq!(location_of_id("/orgs", id), location, "{id}");
        }
    }

    #[test]
    fn an_id_no_segment_can_name_writes_no_location() {
        for id in ["", ".", ".."] {
            let mut resp = Response::builder().finish();
            set_created_location(&mut resp, "/orgs", id);
            assert!(resp.headers().get(LOCATION).is_none(), "{id:?}");
        }
    }

    #[test]
    fn caller_path_reads_the_edges_capture() {
        let mut req = Request::builder().uri(Uri::from_static("/posts")).finish();
        req.extensions_mut()
            .insert(CallerUri(Uri::from_static("/api/posts")));

        assert_eq!(caller_path(&req), "/api/posts");
    }

    #[test]
    fn caller_path_falls_back_to_the_request_uri() {
        let req = Request::builder().uri(Uri::from_static("/posts")).finish();

        assert_eq!(caller_path(&req), "/posts");
    }
}
