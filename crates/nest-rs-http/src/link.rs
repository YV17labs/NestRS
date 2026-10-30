//! The `Link` response header (RFC 8288), for a list that has another page.

use http::{HeaderValue, header::LINK};

use crate::response::Response;

/// Stamp `Link: <collection_path?first=N&after=CURSOR>; rel="next"` onto a page
/// another follows.
///
/// A relative reference (RFC 8288 §3.1), resolved against the request: an
/// absolute URI would name the client-controlled `Host`. The collection path
/// is the one the caller addressed. A header value that will not build is
/// dropped, never raised.
pub(crate) fn set_next_link(resp: &mut Response, collection_path: &str, first: u64, after: &str) {
    let link = format!(
        "<{collection_path}?first={first}&after={}>; rel=\"next\"",
        crate::router::percent_encoded(after),
    );
    if let Ok(value) = HeaderValue::from_str(&link) {
        resp.headers_mut().insert(LINK, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link_of(collection_path: &str, after: &str) -> String {
        let mut resp = Response::builder().finish();
        set_next_link(&mut resp, collection_path, 20, after);
        resp.headers()
            .get(LINK)
            .and_then(|value| value.to_str().ok())
            .expect("the header lands on the response")
            .to_owned()
    }

    #[test]
    fn the_next_link_is_the_collection_path_with_the_page_query() {
        let cursor = "018f3f9c-0000-7000-8000-000000000001";
        assert_eq!(
            link_of("/posts", cursor),
            format!("</posts?first=20&after={cursor}>; rel=\"next\""),
        );
        assert_eq!(
            link_of("/api/v1/posts", cursor),
            format!("</api/v1/posts?first=20&after={cursor}>; rel=\"next\""),
        );
    }

    #[test]
    fn a_cursor_cannot_break_out_of_its_query_value() {
        assert_eq!(
            link_of("/posts", "a b&c>; rel=\"prev\""),
            "</posts?first=20&after=a%20b%26c%3E%3B%20rel%3D%22prev%22>; rel=\"next\"",
        );
    }
}
