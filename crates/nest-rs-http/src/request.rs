//! [`Request`], its builder, the body a handler sets apart, and
//! [`FromRequest`], the trait a handler parameter reads it through.

use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use http::header::AsHeaderName;
use http::request::Parts as RequestParts;
use http::uri::Scheme;
use http::{Extensions, HeaderMap, HeaderName, HeaderValue, Method, Uri, Version, header};

use crate::body::Body;
use crate::error::{BodyError, BoxError, Result};
use crate::response::HeaderNames;

/// One HTTP request: the `http` crate's head, a [`Body`], and what the
/// connection knows about it — its peer, its scheme, the URI as sent, the route
/// it matched.
///
/// ```
/// use nest_rs_http::{Method, Request, Uri};
///
/// let request = Request::builder()
///     .method(Method::POST)
///     .uri(Uri::from_static("/users"))
///     .body("{}");
/// assert_eq!(request.method(), Method::POST);
/// assert_eq!(request.uri().path(), "/users");
/// ```
pub struct Request {
    head: RequestParts,
    body: Body,
    facts: RequestFacts,
}

/// What the connection and the router know about a request, kept inline.
#[derive(Clone)]
pub(crate) struct RequestFacts {
    pub(crate) peer: Option<SocketAddr>,
    pub(crate) local: Option<SocketAddr>,
    pub(crate) scheme: Scheme,
    pub(crate) original_uri: Uri,
    pub(crate) route: Option<Routed>,
}

impl RequestFacts {
    /// A request seen first at `uri`, from no known socket.
    fn arriving(uri: &Uri) -> Self {
        Self {
            peer: None,
            local: None,
            scheme: uri.scheme().cloned().unwrap_or(Scheme::HTTP),
            original_uri: uri.clone(),
            route: None,
        }
    }
}

/// The template the router matched, and each parameter it decoded.
#[derive(Clone)]
pub(crate) struct Routed {
    template: Arc<str>,
    params: Vec<(Arc<str>, String)>,
}

/// The facts riding a request's extensions while a library that speaks `http`
/// holds it.
#[derive(Clone)]
struct CarriedFacts(RequestFacts);

impl Request {
    /// A builder: `GET /` until told otherwise.
    ///
    /// ```
    /// use nest_rs_http::{Method, Request};
    ///
    /// assert_eq!(Request::builder().finish().method(), Method::GET);
    /// ```
    pub fn builder() -> RequestBuilder {
        RequestBuilder {
            head: http::Request::new(()).into_parts().0,
            peer: None,
        }
    }

    /// The head as the `http` crate holds it: method, URI, version, headers
    /// and extensions.
    ///
    /// ```
    /// use nest_rs_http::{Method, Request};
    ///
    /// let request = Request::builder().method(Method::PUT).finish();
    /// assert_eq!(request.head().method, Method::PUT);
    /// ```
    pub fn head(&self) -> &RequestParts {
        &self.head
    }

    /// The head, to change.
    ///
    /// ```
    /// use nest_rs_http::{Request, Version};
    ///
    /// let mut request = Request::builder().finish();
    /// request.head_mut().version = Version::HTTP_2;
    /// assert_eq!(request.version(), Version::HTTP_2);
    /// ```
    pub fn head_mut(&mut self) -> &mut RequestParts {
        &mut self.head
    }

    /// The method.
    ///
    /// ```
    /// use nest_rs_http::{Method, Request};
    ///
    /// assert_eq!(Request::builder().method(Method::DELETE).finish().method(), Method::DELETE);
    /// ```
    pub fn method(&self) -> &Method {
        &self.head.method
    }

    /// The URI the request is answered at, after any rewrite.
    ///
    /// ```
    /// use nest_rs_http::{Request, Uri};
    ///
    /// let request = Request::builder().uri(Uri::from_static("/a?b=c")).finish();
    /// assert_eq!(request.uri().query(), Some("b=c"));
    /// ```
    pub fn uri(&self) -> &Uri {
        &self.head.uri
    }

    /// The URI, to rewrite; [`original_uri`](Self::original_uri) keeps what
    /// the caller sent.
    ///
    /// ```
    /// use nest_rs_http::{Request, Uri};
    ///
    /// let mut request = Request::builder().uri(Uri::from_static("/users/")).finish();
    /// *request.uri_mut() = Uri::from_static("/users");
    /// assert_eq!(request.original_uri(), "/users/");
    /// ```
    pub fn uri_mut(&mut self) -> &mut Uri {
        &mut self.head.uri
    }

    /// The HTTP version.
    ///
    /// ```
    /// use nest_rs_http::{Request, Version};
    ///
    /// assert_eq!(Request::builder().finish().version(), Version::HTTP_11);
    /// ```
    pub fn version(&self) -> Version {
        self.head.version
    }

    /// The headers.
    ///
    /// ```
    /// use nest_rs_http::{HeaderValue, Request, header};
    ///
    /// let request = Request::builder()
    ///     .header(header::ACCEPT, HeaderValue::from_static("text/html"))
    ///     .finish();
    /// assert_eq!(request.headers()[header::ACCEPT], "text/html");
    /// ```
    pub fn headers(&self) -> &HeaderMap {
        &self.head.headers
    }

    /// The headers, to change.
    ///
    /// ```
    /// use nest_rs_http::{HeaderValue, Request, header};
    ///
    /// let mut request = Request::builder().finish();
    /// request.headers_mut().insert(header::ACCEPT, HeaderValue::from_static("*/*"));
    /// assert_eq!(request.header(header::ACCEPT), Some("*/*"));
    /// ```
    pub fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.head.headers
    }

    /// The value of the header `name`, when it is present and visible ASCII.
    ///
    /// ```
    /// use nest_rs_http::{HeaderValue, Request, header};
    ///
    /// let request = Request::builder()
    ///     .header(header::USER_AGENT, HeaderValue::from_static("curl/8"))
    ///     .finish();
    /// assert_eq!(request.header("user-agent"), Some("curl/8"));
    /// assert_eq!(request.header("referer"), None);
    /// ```
    pub fn header(&self, name: impl AsHeaderName) -> Option<&str> {
        self.head.headers.get(name)?.to_str().ok()
    }

    /// The `Content-Type`, when it is present and visible ASCII.
    ///
    /// ```
    /// use nest_rs_http::{HeaderValue, Request, header};
    ///
    /// let request = Request::builder()
    ///     .header(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))
    ///     .finish();
    /// assert_eq!(request.content_type(), Some("application/json"));
    /// ```
    pub fn content_type(&self) -> Option<&str> {
        self.header(header::CONTENT_TYPE)
    }

    /// Typed values riding with the request: what a guard attaches, what a
    /// library that speaks `http` reads.
    ///
    /// ```
    /// use nest_rs_http::Request;
    ///
    /// let request = Request::builder().extension(42_u32).finish();
    /// assert_eq!(request.extensions().get::<u32>(), Some(&42));
    /// ```
    pub fn extensions(&self) -> &Extensions {
        &self.head.extensions
    }

    /// The extensions, to change.
    ///
    /// ```
    /// use nest_rs_http::Request;
    ///
    /// let mut request = Request::builder().finish();
    /// request.extensions_mut().insert("tenant-7");
    /// assert_eq!(request.extensions().get::<&str>(), Some(&"tenant-7"));
    /// ```
    pub fn extensions_mut(&mut self) -> &mut Extensions {
        &mut self.head.extensions
    }

    /// The TCP peer: `None` in process (a built request has none unless given
    /// one) or on a socket without an address.
    ///
    /// ```
    /// use std::net::SocketAddr;
    ///
    /// use nest_rs_http::Request;
    ///
    /// let peer: SocketAddr = "203.0.113.9:40000".parse()?;
    /// assert_eq!(Request::builder().peer_addr(peer).finish().peer_addr(), Some(peer));
    /// assert_eq!(Request::builder().finish().peer_addr(), None);
    /// # Ok::<(), std::net::AddrParseError>(())
    /// ```
    pub fn peer_addr(&self) -> Option<SocketAddr> {
        self.facts.peer
    }

    /// The address the request arrived at: `None` in process.
    ///
    /// ```
    /// use nest_rs_http::Request;
    ///
    /// assert_eq!(Request::builder().finish().local_addr(), None);
    /// ```
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.facts.local
    }

    /// The scheme the request arrived over.
    ///
    /// ```
    /// use nest_rs_http::{Request, Scheme, Uri};
    ///
    /// let request = Request::builder().uri(Uri::from_static("https://api.example/")).finish();
    /// assert_eq!(request.scheme(), &Scheme::HTTPS);
    /// ```
    pub fn scheme(&self) -> &Scheme {
        &self.facts.scheme
    }

    /// The URI as the caller sent it, before the edge's trailing-slash trim and
    /// a version rewrite.
    ///
    /// ```
    /// use nest_rs_http::{Request, Uri};
    ///
    /// let mut request = Request::builder().uri(Uri::from_static("/v2/users")).finish();
    /// *request.uri_mut() = Uri::from_static("/users");
    /// assert_eq!(request.original_uri(), "/v2/users");
    /// ```
    pub fn original_uri(&self) -> &Uri {
        &self.facts.original_uri
    }

    /// The template the router matched, in nestrs's grammar (`/users/{id}`):
    /// `None` before routing.
    ///
    /// ```
    /// use nest_rs_http::Request;
    ///
    /// assert_eq!(Request::builder().finish().route(), None);
    /// ```
    pub fn route(&self) -> Option<&str> {
        self.facts.route.as_ref().map(|routed| &*routed.template)
    }

    /// The path parameter `name`, percent-decoded once by the router: `None`
    /// before routing or when the template has none of that name.
    ///
    /// ```
    /// use nest_rs_http::Request;
    ///
    /// assert_eq!(Request::builder().finish().path_param("id"), None);
    /// ```
    pub fn path_param(&self, name: &str) -> Option<&str> {
        let routed = self.facts.route.as_ref()?;
        routed
            .params
            .iter()
            .find(|(key, _)| &**key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The body, leaving an empty one in its place.
    ///
    /// ```
    /// use nest_rs_http::Request;
    ///
    /// let mut request = Request::builder().body("payload");
    /// assert!(!request.take_body().is_empty());
    /// assert!(request.take_body().is_empty());
    /// ```
    pub fn take_body(&mut self) -> Body {
        std::mem::take(&mut self.body)
    }

    /// Replaces the body: what a guard that read it to verify a signature
    /// puts back.
    ///
    /// ```
    /// use nest_rs_http::Request;
    ///
    /// let mut request = Request::builder().finish();
    /// request.set_body("restored");
    /// assert!(!request.into_body().is_empty());
    /// ```
    pub fn set_body(&mut self, body: impl Into<Body>) {
        self.body = body.into();
    }

    /// The body, the head dropped.
    ///
    /// ```
    /// use nest_rs_http::Request;
    ///
    /// assert!(Request::builder().finish().into_body().is_empty());
    /// ```
    pub fn into_body(self) -> Body {
        self.body
    }

    /// The head for the extractors and the body apart, which a handler
    /// wrapper does first.
    ///
    /// ```
    /// use nest_rs_http::Request;
    ///
    /// let (head, mut body) = Request::builder().body("data").split();
    /// assert!(head.into_body().is_empty());
    /// assert!(body.take().is_ok());
    /// ```
    pub fn split(mut self) -> (Request, RequestBody) {
        let body = self.take_body();
        (self, RequestBody::new(body))
    }

    /// This request as the `http` crate's, for a library that speaks it; the
    /// peer, the scheme, the URI as sent and the route ride its extensions, and
    /// come back with [`From`].
    ///
    /// ```
    /// use std::net::SocketAddr;
    ///
    /// use nest_rs_http::Request;
    ///
    /// let peer: SocketAddr = "198.51.100.4:5000".parse()?;
    /// let http = Request::builder().peer_addr(peer).finish().into_http();
    /// assert_eq!(Request::from(http).peer_addr(), Some(peer));
    /// # Ok::<(), std::net::AddrParseError>(())
    /// ```
    pub fn into_http(self) -> http::Request<Body> {
        let Self {
            mut head,
            body,
            facts,
        } = self;
        head.extensions.insert(CarriedFacts(facts));
        http::Request::from_parts(head, body)
    }

    /// A request from its parts, its facts already known.
    pub(crate) fn from_raw(head: RequestParts, body: Body, facts: RequestFacts) -> Self {
        Self { head, body, facts }
    }

    /// The parts, and the facts with them.
    pub(crate) fn into_raw(self) -> (RequestParts, Body, RequestFacts) {
        (self.head, self.body, self.facts)
    }

    /// Records the route the router matched: its template, and each
    /// parameter's decoded value by name.
    pub(crate) fn set_route(&mut self, template: Arc<str>, params: Vec<(Arc<str>, String)>) {
        self.facts.route = Some(Routed { template, params });
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("method", &self.head.method)
            .field("path", &self.head.uri.path())
            .field("version", &self.head.version)
            .field("headers", &HeaderNames(&self.head.headers))
            .finish_non_exhaustive()
    }
}

impl<B> From<http::Request<B>> for Request
where
    B: http_body::Body<Data = Bytes> + Send + 'static,
    B::Error: Into<BoxError>,
{
    fn from(request: http::Request<B>) -> Self {
        let (mut head, body) = request.into_parts();
        let facts = match head.extensions.remove::<CarriedFacts>() {
            Some(CarriedFacts(facts)) => facts,
            None => RequestFacts::arriving(&head.uri),
        };
        Self {
            head,
            body: Body::new(body),
            facts,
        }
    }
}

/// Builds a [`Request`]: for a test, and for an endpoint calling another.
///
/// ```
/// use nest_rs_http::{HeaderValue, Request, header};
///
/// let request = Request::builder()
///     .header(header::AUTHORIZATION, HeaderValue::from_static("Bearer t"))
///     .finish();
/// assert!(request.headers().contains_key(header::AUTHORIZATION));
/// ```
pub struct RequestBuilder {
    head: RequestParts,
    peer: Option<SocketAddr>,
}

impl RequestBuilder {
    /// Sets the method.
    ///
    /// ```
    /// use nest_rs_http::{Method, Request};
    ///
    /// assert_eq!(Request::builder().method(Method::PATCH).finish().method(), Method::PATCH);
    /// ```
    pub fn method(mut self, method: Method) -> Self {
        self.head.method = method;
        self
    }

    /// Sets the URI; an absolute one also sets the scheme.
    ///
    /// ```
    /// use nest_rs_http::{Request, Uri};
    ///
    /// let request = Request::builder().uri(Uri::from_static("/health")).finish();
    /// assert_eq!(request.uri(), "/health");
    /// ```
    pub fn uri(mut self, uri: Uri) -> Self {
        self.head.uri = uri;
        self
    }

    /// Appends a header, keeping any value already set under its name.
    ///
    /// ```
    /// use nest_rs_http::{HeaderValue, Request, header};
    ///
    /// let request = Request::builder()
    ///     .header(header::ACCEPT, HeaderValue::from_static("text/html"))
    ///     .header(header::ACCEPT, HeaderValue::from_static("*/*"))
    ///     .finish();
    /// assert_eq!(request.headers().get_all(header::ACCEPT).iter().count(), 2);
    /// ```
    pub fn header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.head.headers.append(name, value);
        self
    }

    /// Adds a typed value riding with the request.
    ///
    /// ```
    /// use nest_rs_http::Request;
    ///
    /// let request = Request::builder().extension(9_i64).finish();
    /// assert_eq!(request.extensions().get::<i64>(), Some(&9));
    /// ```
    pub fn extension<T: Clone + Send + Sync + 'static>(mut self, value: T) -> Self {
        self.head.extensions.insert(value);
        self
    }

    /// Sets the TCP peer the request is said to come from.
    ///
    /// ```
    /// use std::net::SocketAddr;
    ///
    /// use nest_rs_http::Request;
    ///
    /// let peer: SocketAddr = "192.0.2.1:443".parse()?;
    /// assert_eq!(Request::builder().peer_addr(peer).finish().peer_addr(), Some(peer));
    /// # Ok::<(), std::net::AddrParseError>(())
    /// ```
    pub fn peer_addr(mut self, addr: SocketAddr) -> Self {
        self.peer = Some(addr);
        self
    }

    /// The request, with `body`.
    ///
    /// ```
    /// use nest_rs_http::Request;
    ///
    /// assert!(!Request::builder().body("x").into_body().is_empty());
    /// ```
    pub fn body(self, body: impl Into<Body>) -> Request {
        let mut facts = RequestFacts::arriving(&self.head.uri);
        facts.peer = self.peer;
        Request {
            head: self.head,
            body: body.into(),
            facts,
        }
    }

    /// The request, with no body.
    ///
    /// ```
    /// use nest_rs_http::Request;
    ///
    /// assert!(Request::builder().finish().into_body().is_empty());
    /// ```
    pub fn finish(self) -> Request {
        self.body(Body::empty())
    }
}

impl fmt::Debug for RequestBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestBuilder")
            .field("method", &self.head.method)
            .field("path", &self.head.uri.path())
            .field("headers", &HeaderNames(&self.head.headers))
            .finish_non_exhaustive()
    }
}

/// The body a handler wrapper set apart, taken once by the parameter that
/// reads it.
///
/// ```
/// use nest_rs_http::{Body, BodyError, RequestBody};
///
/// let mut body = RequestBody::new(Body::from("once"));
/// assert!(body.take().is_ok());
/// let again = body.take().unwrap_err();
/// assert!(matches!(again.downcast_ref::<BodyError>(), Some(BodyError::AlreadyTaken)));
/// ```
pub struct RequestBody(Option<Body>);

impl RequestBody {
    /// `body`, not yet taken.
    ///
    /// ```
    /// use nest_rs_http::{Body, RequestBody};
    ///
    /// let mut body = RequestBody::new(Body::empty());
    /// assert!(body.take().is_ok());
    /// ```
    pub fn new(body: Body) -> Self {
        Self(Some(body))
    }

    /// The body; a second take is [`BodyError::AlreadyTaken`], a `500`: two
    /// parameters of one handler read the body.
    ///
    /// ```
    /// use nest_rs_http::{Body, RequestBody, StatusCode};
    ///
    /// let mut body = RequestBody::new(Body::from("x"));
    /// let _first = body.take();
    /// assert_eq!(body.take().unwrap_err().status(), StatusCode::INTERNAL_SERVER_ERROR);
    /// ```
    pub fn take(&mut self) -> Result<Body> {
        self.0.take().ok_or_else(|| BodyError::AlreadyTaken.into())
    }
}

impl fmt::Debug for RequestBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestBody")
            .field("taken", &self.0.is_none())
            .finish()
    }
}

/// A handler parameter: read from the request's head and facts, and from the
/// body at most once.
///
/// There is no `Option<T>` reading: it would turn every refusal into `None`;
/// a parameter that may fail reads `Result<T>` and sees the error.
///
/// ```
/// use nest_rs_http::{FromRequest, Request, RequestBody, Result};
///
/// struct UserAgent(String);
///
/// impl<'a> FromRequest<'a> for UserAgent {
///     async fn from_request(req: &'a Request, _body: &mut RequestBody) -> Result<Self> {
///         Ok(Self(req.header("user-agent").unwrap_or("unknown").to_owned()))
///     }
/// }
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
///
/// let (request, mut body) = Request::builder().finish().split();
/// let UserAgent(agent) = UserAgent::from_request(&request, &mut body).await?;
/// assert_eq!(agent, "unknown");
/// # Ok(())
/// # }
/// ```
pub trait FromRequest<'a>: Sized {
    /// Reads `Self` from `req`, taking `body` if it needs it.
    fn from_request(
        req: &'a Request,
        body: &mut RequestBody,
    ) -> impl Future<Output = Result<Self>> + Send;
}

impl<'a> FromRequest<'a> for &'a Request {
    async fn from_request(req: &'a Request, _body: &mut RequestBody) -> Result<Self> {
        Ok(req)
    }
}

impl<'a> FromRequest<'a> for Body {
    async fn from_request(_req: &'a Request, body: &mut RequestBody) -> Result<Self> {
        body.take()
    }
}

impl<'a, T: FromRequest<'a>> FromRequest<'a> for Result<T> {
    async fn from_request(req: &'a Request, body: &mut RequestBody) -> Result<Self> {
        Ok(T::from_request(req, body).await)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use hyper::upgrade::OnUpgrade;
    use hyper_util::rt::TokioIo;

    use super::*;
    use crate::testing::{Upgrading, read_exactly};

    #[tokio::test]
    async fn split_leaves_the_head_without_a_body() {
        let request = Request::builder()
            .method(Method::POST)
            .uri(Uri::from_static("/orders"))
            .header(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"))
            .body("one order");

        let (mut head, mut body) = request.split();

        assert_eq!(head.method(), Method::POST);
        assert_eq!(head.content_type(), Some("text/plain"));
        assert!(head.take_body().is_empty(), "the head keeps no body");
        let taken = body.take().unwrap();
        assert_eq!(taken.into_bytes().await.unwrap(), "one order");
        let again = body.take().unwrap_err();
        assert!(matches!(
            again.downcast_ref::<BodyError>(),
            Some(BodyError::AlreadyTaken)
        ));
    }

    #[tokio::test]
    async fn into_http_then_from_keeps_peer_scheme_route_and_upgrade() {
        let Upgrading { request, mut peer } = Upgrading::open().await;
        let upgrade = request.extensions().get::<OnUpgrade>().cloned().unwrap();
        let addr: SocketAddr = "203.0.113.7:51000".parse().unwrap();
        let mut sent = Request::builder()
            .uri(Uri::from_static("https://api.example/v1/rooms/lobby/"))
            .peer_addr(addr)
            .extension(upgrade)
            .finish();
        sent.set_route(
            "/rooms/{room}".into(),
            vec![("room".into(), "lobby".into())],
        );
        *sent.uri_mut() = Uri::from_static("/rooms/lobby");

        let back = Request::from(sent.into_http());

        assert_eq!(back.peer_addr(), Some(addr));
        assert_eq!(back.scheme(), &Scheme::HTTPS);
        assert_eq!(back.route(), Some("/rooms/{room}"));
        assert_eq!(back.path_param("room"), Some("lobby"));
        assert_eq!(back.original_uri(), "https://api.example/v1/rooms/lobby/");
        assert_eq!(back.uri(), "/rooms/lobby");
        assert!(
            back.extensions().get::<CarriedFacts>().is_none(),
            "the carrier is taken back, not left behind",
        );

        let upgrade = back.extensions().get::<OnUpgrade>().cloned().unwrap();
        peer.switch().await;
        let mut socket = TokioIo::new(
            tokio::time::timeout(Duration::from_secs(5), upgrade)
                .await
                .unwrap()
                .unwrap(),
        );
        peer.send(b"ping").await;
        assert_eq!(read_exactly(&mut socket, 4).await, b"ping");
    }

    #[tokio::test]
    async fn a_parameter_reading_a_result_sees_the_refusal() {
        let (request, mut body) = Request::builder().body("x").split();
        let _first = Body::from_request(&request, &mut body).await.unwrap();
        let second = <Result<Body>>::from_request(&request, &mut body)
            .await
            .unwrap();
        assert!(
            second.is_err(),
            "the second read is refused, and the handler sees it"
        );
    }

    #[test]
    fn debug_names_the_headers_without_their_values() {
        let request = Request::builder()
            .uri(Uri::from_static("/login?token=secret"))
            .header(
                header::AUTHORIZATION,
                HeaderValue::from_static("Bearer secret"),
            )
            .finish();
        let debug = format!("{request:?}");
        assert!(debug.contains("authorization"), "{debug}");
        assert!(!debug.contains("secret"), "{debug}");
    }
}
