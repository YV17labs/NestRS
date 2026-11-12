//! `Vary` ([RFC 9110 §12.5.5](https://www.rfc-editor.org/rfc/rfc9110#field.vary)):
//! the request headers an answer was chosen by, which a cache keys it on.

use http::header::VARY;
use http::{HeaderMap, HeaderName, HeaderValue};

/// Add `name` to `Vary`, unless it is already there or `Vary` is `*`; a value
/// already set is kept, so each layer adds what it read.
pub(crate) fn vary_on(headers: &mut HeaderMap, name: HeaderName) {
    let varied = headers
        .get_all(VARY)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .any(|field| field == "*" || field.eq_ignore_ascii_case(name.as_str()));
    if !varied {
        headers.append(VARY, HeaderValue::from(name));
    }
}

#[cfg(test)]
mod tests {
    use http::header::{ACCEPT_ENCODING, ORIGIN};

    use super::*;

    #[test]
    fn a_header_is_added_to_vary_once() {
        let mut headers = HeaderMap::new();
        vary_on(&mut headers, ORIGIN);
        vary_on(&mut headers, ORIGIN);
        headers.append(
            VARY,
            HeaderValue::from_static("Accept-Language, accept-encoding"),
        );
        vary_on(&mut headers, ACCEPT_ENCODING);
        let vary: Vec<_> = headers.get_all(VARY).iter().collect();
        assert_eq!(vary, ["origin", "Accept-Language, accept-encoding"]);

        let mut any = HeaderMap::new();
        any.insert(VARY, HeaderValue::from_static("*"));
        vary_on(&mut any, ORIGIN);
        assert_eq!(
            any.get_all(VARY).iter().count(),
            1,
            "`*` varies on everything"
        );
    }
}
