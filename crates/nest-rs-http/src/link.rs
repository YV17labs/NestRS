//! The `Link` response header (RFC 8288), for a list that has another page.

use std::fmt::Display;

use poem::Response;
use poem::http::{HeaderValue, header::LINK};

/// Stamp `Link: <collection_path?first=N&after=CURSOR>; rel="next"` onto a page
/// another follows.
///
/// A relative reference (RFC 8288 §3.1), resolved against the request: an
/// absolute URI would name the client-controlled `Host`. Pass
/// [`caller_path`](crate::caller_path) as the collection path. A header value
/// that will not build is dropped, never raised.
pub fn set_next_link(resp: &mut Response, collection_path: &str, first: u64, after: impl Display) {
    let target = format!(
        "{collection_path}?first={first}&after={}",
        encode_query_value(&after.to_string()),
    );
    if let Ok(value) = HeaderValue::from_str(&format!("<{target}>; rel=\"next\"")) {
        resp.headers_mut().insert(LINK, value);
    }
}

/// Percent-encode everything but RFC 3986's unreserved characters, so a cursor
/// can never end the query value, the URI or the `<…>` that frames it.
fn encode_query_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
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
