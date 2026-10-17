//! The method set one address of the route table serves, and what it answers a
//! method outside it: `405` with the `Allow` it must carry
//! ([RFC 9110 §15.5.6](https://www.rfc-editor.org/rfc/rfc9110#status.405)).
//!
//! `HEAD` is served by `GET` whenever `GET` is registered and `HEAD` is not
//! (RFC 9110 §9.3.2), so it is advertised beside it; `OPTIONS` never, since
//! nothing here serves it. An entry answering every method (`route_any`)
//! dispatches on the method itself: `PUT /graphql` answers the surface's own
//! `405`.

use http::header::{ALLOW, CONTENT_LENGTH};
use http::{HeaderValue, Method, StatusCode};

use crate::body::Body;
use crate::problem::ProblemDetails;
use crate::response::{IntoResponse, Response};

/// The entries one address answers, one per method, in registration order.
pub(crate) struct Methods<L> {
    entries: Vec<(Method, L)>,
    allow: HeaderValue,
}

/// What a method selects out of [`Methods`].
pub(crate) enum Chosen<'a, L> {
    /// The entry registered for the method.
    Entry(&'a L),
    /// `HEAD`, served by the `GET` entry: answered without its content.
    HeadOf(&'a L),
    /// No entry: `405`.
    NotAllowed,
}

impl<L> Methods<L> {
    /// The table `entries` describe; no method appears twice in them.
    pub(crate) fn new(entries: Vec<(Method, L)>) -> Self {
        let allow = allow_value(entries.iter().map(|(method, _)| method));
        // Cannot fail: every `Method::as_str()` is an RFC 9110 token.
        let allow = HeaderValue::try_from(allow).unwrap_or(HeaderValue::from_static(""));
        Self { entries, allow }
    }

    /// The entry `method` selects.
    pub(crate) fn choose(&self, method: &Method) -> Chosen<'_, L> {
        if let Some((_, entry)) = self.entries.iter().find(|(m, _)| m == method) {
            return Chosen::Entry(entry);
        }
        if *method != Method::HEAD {
            return Chosen::NotAllowed;
        }
        self.entries
            .iter()
            .find(|(m, _)| *m == Method::GET)
            .map_or(Chosen::NotAllowed, |(_, entry)| Chosen::HeadOf(entry))
    }

    /// The `405` this address answers a method it does not serve: an `Ok`
    /// problem document, so a handler's own `405` error stays an error through
    /// the bands.
    pub(crate) fn not_allowed(&self) -> Response {
        let mut resp = ProblemDetails::from_status(StatusCode::METHOD_NOT_ALLOWED).into_response();
        resp.headers_mut().insert(ALLOW, self.allow.clone());
        resp
    }
}

/// The `Allow` field value a method set advertises: the registered methods in
/// order, with `HEAD` beside the `GET` that serves it.
fn allow_value<'a>(methods: impl Iterator<Item = &'a Method> + Clone) -> String {
    let explicit_head = methods.clone().any(|method| *method == Method::HEAD);
    let mut listed: Vec<&str> = Vec::new();
    for method in methods {
        listed.push(method.as_str());
        if *method == Method::GET && !explicit_head {
            listed.push(Method::HEAD.as_str());
        }
    }
    listed.join(", ")
}

/// `resp`, the `GET` entry's answer to a `HEAD`, without its content: the same
/// header fields, `Content-Length` included when the content's size is known
/// (RFC 9110 §9.3.2). hyper drops a `HEAD` body on the wire; an in-process
/// client would read it.
pub(crate) fn head_of(resp: Response) -> Response {
    let (mut head, body) = resp.into_parts();
    if !head.headers.contains_key(CONTENT_LENGTH)
        && let Some(length) = http_body::Body::size_hint(&body).exact()
    {
        head.headers
            .insert(CONTENT_LENGTH, HeaderValue::from(length));
    }
    Response::from_parts(head, Body::empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn methods(listed: &[Method]) -> Methods<usize> {
        Methods::new(listed.iter().cloned().zip(0..).collect())
    }

    #[test]
    fn a_get_carries_the_head_it_serves() {
        assert_eq!(methods(&[Method::GET]).allow, "GET, HEAD");
    }

    #[test]
    fn methods_are_advertised_in_registration_order() {
        let table = methods(&[Method::POST, Method::GET, Method::DELETE]);
        assert_eq!(table.allow, "POST, GET, HEAD, DELETE");
    }

    #[test]
    fn a_table_without_a_get_advertises_no_head() {
        let table = methods(&[Method::POST, Method::PATCH, Method::PUT]);
        assert_eq!(table.allow, "POST, PATCH, PUT");
    }

    #[test]
    fn a_registered_head_is_its_own_entry() {
        let table = methods(&[Method::GET, Method::HEAD]);
        assert_eq!(table.allow, "GET, HEAD");
        assert!(matches!(table.choose(&Method::HEAD), Chosen::Entry(1)));
    }

    #[test]
    fn head_is_served_by_get_and_nothing_else_is_guessed() {
        let table = methods(&[Method::POST, Method::GET]);
        assert!(matches!(table.choose(&Method::HEAD), Chosen::HeadOf(1)));
        assert!(matches!(table.choose(&Method::POST), Chosen::Entry(0)));
        assert!(matches!(table.choose(&Method::PUT), Chosen::NotAllowed));
        assert!(matches!(
            methods(&[Method::POST]).choose(&Method::HEAD),
            Chosen::NotAllowed
        ));
    }

    #[tokio::test]
    async fn a_405_is_a_problem_document_carrying_allow() {
        let resp = methods(&[Method::GET, Method::POST]).not_allowed();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(resp.headers()[ALLOW], "GET, HEAD, POST");
        assert_eq!(
            resp.headers()[http::header::CONTENT_TYPE],
            "application/problem+json"
        );
    }

    #[tokio::test]
    async fn a_head_answer_keeps_the_length_and_drops_the_content() {
        let answered = head_of(Response::builder().body("hello"));
        assert_eq!(answered.headers()[CONTENT_LENGTH], "5");
        assert_eq!(answered.into_body().into_string().await.unwrap(), "");

        let streamed = Body::from_stream(futures_util::stream::iter([Ok::<_, std::io::Error>(
            bytes::Bytes::from_static(b"x"),
        )]));
        let answered = head_of(Response::builder().body(streamed));
        assert!(
            !answered.headers().contains_key(CONTENT_LENGTH),
            "a length nobody knows is not invented"
        );
    }
}
