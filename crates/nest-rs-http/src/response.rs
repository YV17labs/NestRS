//! [`Response`], its builder, and [`IntoResponse`]: what an endpoint answers.

use std::fmt;

use bytes::Bytes;
use http::response::Parts as ResponseParts;
use http::{Extensions, HeaderMap, HeaderName, HeaderValue, StatusCode, Version, header};

use crate::body::Body;
use crate::error::{BoxError, HttpError, Result};

/// One HTTP response: the `http` crate's head and a [`Body`].
///
/// ```
/// use nest_rs_http::{Response, StatusCode};
///
/// let response = Response::builder().status(StatusCode::CREATED).body("made");
/// assert_eq!(response.status(), StatusCode::CREATED);
/// ```
pub struct Response {
    head: ResponseParts,
    body: Body,
}

impl Response {
    /// A builder, `200 OK` with no header until told otherwise.
    ///
    /// ```
    /// use nest_rs_http::{Response, StatusCode};
    ///
    /// assert_eq!(Response::builder().finish().status(), StatusCode::OK);
    /// ```
    pub fn builder() -> ResponseBuilder {
        ResponseBuilder { head: empty_head() }
    }

    /// A response from its head and body.
    ///
    /// ```
    /// use nest_rs_http::{Body, Response};
    ///
    /// let (head, _) = Response::builder().finish().into_parts();
    /// let response = Response::from_parts(head, Body::from("again"));
    /// assert!(!response.into_body().is_empty());
    /// ```
    pub fn from_parts(head: ResponseParts, body: Body) -> Self {
        Self { head, body }
    }

    /// The head and the body apart.
    ///
    /// ```
    /// use nest_rs_http::{Response, StatusCode};
    ///
    /// let (head, body) = Response::builder().status(StatusCode::ACCEPTED).finish().into_parts();
    /// assert_eq!(head.status, StatusCode::ACCEPTED);
    /// assert!(body.is_empty());
    /// ```
    pub fn into_parts(self) -> (ResponseParts, Body) {
        (self.head, self.body)
    }

    /// The status.
    ///
    /// ```
    /// use nest_rs_http::{Response, StatusCode};
    ///
    /// assert_eq!(Response::default().status(), StatusCode::OK);
    /// ```
    pub fn status(&self) -> StatusCode {
        self.head.status
    }

    /// Replaces the status.
    ///
    /// ```
    /// use nest_rs_http::{Response, StatusCode};
    ///
    /// let mut response = Response::default();
    /// response.set_status(StatusCode::NO_CONTENT);
    /// assert_eq!(response.status(), StatusCode::NO_CONTENT);
    /// ```
    pub fn set_status(&mut self, status: StatusCode) {
        self.head.status = status;
    }

    /// The HTTP version.
    ///
    /// ```
    /// use nest_rs_http::{Response, Version};
    ///
    /// assert_eq!(Response::default().version(), Version::HTTP_11);
    /// ```
    pub fn version(&self) -> Version {
        self.head.version
    }

    /// The headers.
    ///
    /// ```
    /// use nest_rs_http::{HeaderValue, Response, header};
    ///
    /// let response = Response::builder()
    ///     .content_type(HeaderValue::from_static("text/csv"))
    ///     .finish();
    /// assert_eq!(response.headers()[header::CONTENT_TYPE], "text/csv");
    /// ```
    pub fn headers(&self) -> &HeaderMap {
        &self.head.headers
    }

    /// The headers, to change.
    ///
    /// ```
    /// use nest_rs_http::{HeaderValue, Response, header};
    ///
    /// let mut response = Response::default();
    /// response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    /// assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    /// ```
    pub fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.head.headers
    }

    /// Typed values riding with the response.
    ///
    /// ```
    /// use nest_rs_http::Response;
    ///
    /// let response = Response::builder().extension(3_u8).finish();
    /// assert_eq!(response.extensions().get::<u8>(), Some(&3));
    /// ```
    pub fn extensions(&self) -> &Extensions {
        &self.head.extensions
    }

    /// The extensions, to change.
    ///
    /// ```
    /// use nest_rs_http::Response;
    ///
    /// let mut response = Response::default();
    /// response.extensions_mut().insert("marked");
    /// assert_eq!(response.extensions().get::<&str>(), Some(&"marked"));
    /// ```
    pub fn extensions_mut(&mut self) -> &mut Extensions {
        &mut self.head.extensions
    }

    /// The body, leaving an empty one in its place.
    ///
    /// ```
    /// use nest_rs_http::Response;
    ///
    /// let mut response = Response::builder().body("once");
    /// assert!(!response.take_body().is_empty());
    /// assert!(response.take_body().is_empty());
    /// ```
    pub fn take_body(&mut self) -> Body {
        std::mem::take(&mut self.body)
    }

    /// Replaces the body.
    ///
    /// ```
    /// use nest_rs_http::Response;
    ///
    /// let mut response = Response::default();
    /// response.set_body("now");
    /// assert!(!response.into_body().is_empty());
    /// ```
    pub fn set_body(&mut self, body: impl Into<Body>) {
        self.body = body.into();
    }

    /// The body, the head dropped.
    ///
    /// ```
    /// use nest_rs_http::Response;
    ///
    /// assert!(Response::default().into_body().is_empty());
    /// ```
    pub fn into_body(self) -> Body {
        self.body
    }

    /// This response as the `http` crate's, for a library that speaks it.
    ///
    /// ```
    /// use nest_rs_http::{Response, StatusCode};
    ///
    /// let response = Response::builder().status(StatusCode::GONE).finish().into_http();
    /// assert_eq!(Response::from(response).status(), StatusCode::GONE);
    /// ```
    pub fn into_http(self) -> http::Response<Body> {
        http::Response::from_parts(self.head, self.body)
    }
}

impl Default for Response {
    fn default() -> Self {
        Self::builder().finish()
    }
}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.head.status)
            .field("version", &self.head.version)
            .field("headers", &HeaderNames(&self.head.headers))
            .finish_non_exhaustive()
    }
}

impl<B> From<http::Response<B>> for Response
where
    B: http_body::Body<Data = Bytes> + Send + 'static,
    B::Error: Into<BoxError>,
{
    fn from(response: http::Response<B>) -> Self {
        let (head, body) = response.into_parts();
        Self {
            head,
            body: Body::new(body),
        }
    }
}

/// Builds a [`Response`]. Every name and value is already typed, so nothing it
/// is given can be dropped for not parsing.
///
/// ```
/// use nest_rs_http::{HeaderName, HeaderValue, Response, StatusCode};
///
/// let response = Response::builder()
///     .status(StatusCode::OK)
///     .header(HeaderName::from_static("x-trace"), HeaderValue::from_static("1"))
///     .body("traced");
/// assert_eq!(response.headers()["x-trace"], "1");
/// ```
pub struct ResponseBuilder {
    head: ResponseParts,
}

impl ResponseBuilder {
    /// Sets the status.
    ///
    /// ```
    /// use nest_rs_http::{Response, StatusCode};
    ///
    /// let response = Response::builder().status(StatusCode::NOT_MODIFIED).finish();
    /// assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    /// ```
    pub fn status(mut self, status: StatusCode) -> Self {
        self.head.status = status;
        self
    }

    /// Appends a header, keeping any value already set under its name.
    ///
    /// ```
    /// use nest_rs_http::{HeaderValue, Response, header};
    ///
    /// let response = Response::builder()
    ///     .header(header::SET_COOKIE, HeaderValue::from_static("a=1"))
    ///     .header(header::SET_COOKIE, HeaderValue::from_static("b=2"))
    ///     .finish();
    /// assert_eq!(response.headers().get_all(header::SET_COOKIE).iter().count(), 2);
    /// ```
    pub fn header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.head.headers.append(name, value);
        self
    }

    /// Sets `Content-Type`, replacing any value already set.
    ///
    /// ```
    /// use nest_rs_http::{HeaderValue, Response, header};
    ///
    /// let response = Response::builder()
    ///     .content_type(HeaderValue::from_static("application/json"))
    ///     .finish();
    /// assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    /// ```
    pub fn content_type(mut self, value: HeaderValue) -> Self {
        self.head.headers.insert(header::CONTENT_TYPE, value);
        self
    }

    /// Adds a typed value riding with the response.
    ///
    /// ```
    /// use nest_rs_http::Response;
    ///
    /// let response = Response::builder().extension(1_u16).finish();
    /// assert_eq!(response.extensions().get::<u16>(), Some(&1));
    /// ```
    pub fn extension<T: Clone + Send + Sync + 'static>(mut self, value: T) -> Self {
        self.head.extensions.insert(value);
        self
    }

    /// The response, with `body`.
    ///
    /// ```
    /// use nest_rs_http::Response;
    ///
    /// assert!(!Response::builder().body("filled").into_body().is_empty());
    /// ```
    pub fn body(self, body: impl Into<Body>) -> Response {
        Response {
            head: self.head,
            body: body.into(),
        }
    }

    /// The response, with no body.
    ///
    /// ```
    /// use nest_rs_http::Response;
    ///
    /// assert!(Response::builder().finish().into_body().is_empty());
    /// ```
    pub fn finish(self) -> Response {
        self.body(Body::empty())
    }
}

impl fmt::Debug for ResponseBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResponseBuilder")
            .field("status", &self.head.status)
            .field("headers", &HeaderNames(&self.head.headers))
            .finish_non_exhaustive()
    }
}

/// A value a handler, a guard or a layer answers with.
///
/// ```
/// use nest_rs_http::{IntoResponse, StatusCode};
///
/// let response = (StatusCode::CREATED, "made").into_response();
/// assert_eq!(response.status(), StatusCode::CREATED);
/// ```
pub trait IntoResponse: Send {
    /// The response this value answers.
    fn into_response(self) -> Response;
}

impl IntoResponse for Response {
    fn into_response(self) -> Response {
        self
    }
}

impl IntoResponse for Body {
    fn into_response(self) -> Response {
        Response::builder().body(self)
    }
}

impl IntoResponse for String {
    fn into_response(self) -> Response {
        text(self)
    }
}

impl IntoResponse for &'static str {
    fn into_response(self) -> Response {
        text(self)
    }
}

impl IntoResponse for Bytes {
    fn into_response(self) -> Response {
        octets(self)
    }
}

impl IntoResponse for Vec<u8> {
    fn into_response(self) -> Response {
        octets(self)
    }
}

impl IntoResponse for () {
    fn into_response(self) -> Response {
        Response::default()
    }
}

impl IntoResponse for StatusCode {
    fn into_response(self) -> Response {
        Response::builder().status(self).finish()
    }
}

impl<T: IntoResponse> IntoResponse for (StatusCode, T) {
    fn into_response(self) -> Response {
        let (status, inner) = self;
        let mut response = inner.into_response();
        response.set_status(status);
        response
    }
}

/// What a handler wrapper turns a handler's return value into: a value is
/// `Ok`, a `Result`'s error becomes an [`HttpError`].
pub trait IntoResult<T: IntoResponse> {
    /// The value as a result.
    fn into_result(self) -> Result<T>;
}

impl<T: IntoResponse> IntoResult<T> for T {
    fn into_result(self) -> Result<T> {
        Ok(self)
    }
}

impl<T: IntoResponse, E: Into<HttpError>> IntoResult<T> for std::result::Result<T, E> {
    fn into_result(self) -> Result<T> {
        self.map_err(Into::into)
    }
}

fn text(body: impl Into<Body>) -> Response {
    Response::builder()
        .content_type(HeaderValue::from_static("text/plain; charset=utf-8"))
        .body(body)
}

fn octets(body: impl Into<Body>) -> Response {
    Response::builder()
        .content_type(HeaderValue::from_static("application/octet-stream"))
        .body(body)
}

/// An empty head: `200 OK`, HTTP/1.1, no header.
fn empty_head() -> ResponseParts {
    http::Response::new(()).into_parts().0
}

/// A header map's names alone: a `Debug` line never prints a value, where a
/// credential or a cookie travels.
pub(crate) struct HeaderNames<'a>(pub(crate) &'a HeaderMap);

impl fmt::Debug for HeaderNames<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.0.keys()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_response_builder_keeps_every_header() {
        let response = Response::builder()
            .header(header::SET_COOKIE, HeaderValue::from_static("a=1"))
            .header(header::SET_COOKIE, HeaderValue::from_static("b=2"))
            .header(
                HeaderName::from_static("x-binary"),
                HeaderValue::from_bytes(&[0x80, 0xff]).unwrap(),
            )
            .content_type(HeaderValue::from_static("text/plain"))
            .content_type(HeaderValue::from_static("application/json"))
            .finish();

        let cookies: Vec<_> = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .collect();
        assert_eq!(
            cookies,
            ["a=1", "b=2"],
            "an appended header keeps the one before"
        );
        assert_eq!(response.headers()["x-binary"].as_bytes(), [0x80, 0xff]);
        assert_eq!(
            response
                .headers()
                .get_all(header::CONTENT_TYPE)
                .iter()
                .count(),
            1,
            "Content-Type is replaced, never doubled",
        );
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    }

    #[test]
    fn a_status_pair_keeps_the_inner_responses_headers() {
        let response = (StatusCode::ACCEPTED, "queued").into_response();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/plain; charset=utf-8",
        );
    }

    #[test]
    fn debug_names_the_headers_without_their_values() {
        let response = Response::builder()
            .header(
                header::SET_COOKIE,
                HeaderValue::from_static("session=secret"),
            )
            .finish();
        let debug = format!("{response:?}");
        assert!(debug.contains("set-cookie"), "{debug}");
        assert!(!debug.contains("secret"), "{debug}");
    }

    #[test]
    fn a_result_error_becomes_an_http_error() {
        let failed: std::result::Result<&'static str, StatusCode> = Err(StatusCode::CONFLICT);
        let error = match failed.into_result() {
            Err(error) => error,
            Ok(_) => panic!("an Err stays an Err"),
        };
        assert_eq!(error.status(), StatusCode::CONFLICT);
        assert!(matches!("ok".into_result(), Ok("ok")));
    }
}
