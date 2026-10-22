//! The HTTP library the transport adapts, poem, behind one crate-private type:
//! the router the route table compiles into, the CORS and compression
//! answered around the edge, the query, form and multipart decoders the
//! extractors read through, and the crossings between nestrs's request,
//! response, body and error and poem's, which never leave this file. What
//! those do on the wire is pinned by black-box suites
//! (`tests/integration/{routes,cors,compression,query,form,multipart}.rs`),
//! so a later library replaces this file and passes them unchanged
//! (`.claude/decisions/http-server.md`).
#![expect(
    clippy::disallowed_types,
    reason = "poem is the HTTP adapter's, and this file is the one that names it"
)]

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::Context;
use bytes::Bytes;
use http::header::{
    ACCEPT_ENCODING, ACCESS_CONTROL_ALLOW_METHODS, CONTENT_ENCODING, CONTENT_RANGE, CONTENT_TYPE,
    Entry, VARY,
};
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use http_body_util::combinators::BoxBody;
use hyper::upgrade::OnUpgrade;
use nest_rs_core::{DecodeError, Problem};
use poem::error::{
    CorsError, NotFoundError, ParseFormError, ParseJsonError, ParseMultipartError, ParseQueryError,
};
use poem::middleware::{Compression, Cors};
use poem::web::{Compress, CompressionAlgo, LocalAddr, RemoteAddr};
use poem::{EndpointExt, FromRequest, IntoResponse, PathPattern};
use serde::de::DeserializeOwned;
use tokio::io::AsyncRead;
use tokio::time::Instant;

use crate::accept_encoding::{AcceptEncoding, Coding};
use crate::allow::{Chosen, HeadAsGet, Methods, head_of};
use crate::body::Body;
use crate::cors::{
    CorsRefusal, FRAMEWORK_SENT, HttpCors, admitted_by_default, holds_wildcard, listed,
};
use crate::endpoint::{BoxEndpoint, Endpoint};
use crate::error::{
    BoxError, FormError, HttpError, JsonError, MultipartError, PathParameterError, Result,
    RouteError,
};
use crate::fallback::NotAllowed;
use crate::matched::MatchedRoute;
use crate::problem::{ProblemDetails, Said, is_raw_text, problem_response, replacing};
use crate::request::{Request, RequestFacts, Routed};
use crate::response::Response;
use crate::routes::{Address, Dispatch};

/// poem, as the transport uses it.
pub(crate) struct PoemAdapter;

/// A CORS policy as poem's middleware takes it, checked at boot: it answers
/// nothing until it knows the route table it serves ([`CorsPolicy::serving`]).
pub(crate) struct CorsPolicy {
    cors: Cors,
    methods: Admitted,
}

/// A CORS policy serving one route table, every method it admits listed or
/// said as `*`: the one [`PoemAdapter::around`] answers.
pub(crate) struct ServedCorsPolicy {
    cors: Cors,
    /// Whether it admits every method, said as `*`.
    wildcard: bool,
}

/// The methods a CORS policy admits.
enum Admitted {
    /// The ones it lists.
    Listed,
    /// Every one, said as `*`.
    Every,
    /// The ones the route table serves, read once it is built.
    Routed,
}

impl CorsPolicy {
    /// The policy as it serves the route table: one listing no method admits
    /// the CORS-safelisted ones and those `routed` names, or every method a
    /// page sends when an address answers `every_method` itself.
    pub(crate) fn serving<'a>(
        self,
        routed: impl IntoIterator<Item = &'a Method>,
        every_method: bool,
    ) -> ServedCorsPolicy {
        let (cors, wildcard) = match self.methods {
            Admitted::Routed => (
                admitted_by_default(routed, every_method)
                    .into_iter()
                    .fold(self.cors, Cors::allow_method),
                false,
            ),
            Admitted::Listed => (self.cors, false),
            Admitted::Every => (self.cors, true),
        };
        ServedCorsPolicy { cors, wildcard }
    }
}

/// A poem endpoint, erased.
type PoemEndpoint = poem::endpoint::BoxEndpoint<'static, poem::Response>;

impl PoemAdapter {
    /// The route table, compiled into poem's router, answering nestrs's
    /// requests: a path no address claims, and a method an address does not
    /// serve, reach `unrouted` — the fallback site answering `404` and `405` —
    /// with the request whole.
    pub(crate) fn router(
        table: Vec<Address>,
        unrouted: BoxEndpoint,
    ) -> std::result::Result<BoxEndpoint, RouteError> {
        let mut route = poem::Route::new();
        // A root catch-all of the table's own answers every unclaimed path.
        let claims_everything = table
            .iter()
            .any(|address| address.template.is_root_catch_all());
        let mut root = None;
        for address in table {
            let pattern = poem_pattern(address.template.as_str());
            let template = address.template.as_str().to_owned();
            let owner = address.owner.to_string();
            let matched = Arc::new(Matched::new(address, unrouted.clone()));
            if template == "/" {
                root = Some(Arc::clone(&matched));
            }
            route = route
                .try_at(pattern, matched)
                .map_err(|refused| RouteError::Router {
                    template,
                    owner,
                    reason: refused.to_string(),
                })?;
        }
        if !claims_everything {
            route = route.at(UNROUTED, Unmatched { unrouted, root });
        }
        Ok(BoxEndpoint::new(Router(route)))
    }

    /// The policy `cors` describes, or the boot's refusal naming the value it
    /// cannot take. A `*` in `origins`, `methods` or `headers` admits every
    /// value, which poem spells as an empty list; explicit values map one to
    /// one; no method at all admits what the routes serve
    /// ([`CorsPolicy::serving`]).
    pub(crate) fn cors_policy(cors: &HttpCors) -> anyhow::Result<CorsPolicy> {
        cors.reject_wildcards_with_credentials()?;
        cors.reject_forbidden_methods()?;
        let mut policy = Cors::new();
        for origin in listed(&cors.origins) {
            policy = policy.allow_origin(origin);
        }
        let methods = if holds_wildcard(&cors.methods) {
            Admitted::Every
        } else if cors.methods.is_empty() {
            Admitted::Routed
        } else {
            Admitted::Listed
        };
        for m in listed(&cors.methods) {
            let method = Method::from_bytes(m.as_bytes())
                .with_context(|| format!("invalid HTTP method in CORS config: `{m}`"))?;
            policy = policy.allow_method(method);
        }
        for h in listed(&cors.headers) {
            let header = HeaderName::from_str(h)
                .with_context(|| format!("invalid header name in CORS allow-list: `{h}`"))?;
            policy = policy.allow_header(header);
        }
        for h in &cors.exposed_headers {
            let header = HeaderName::from_str(h)
                .with_context(|| format!("invalid header name in CORS expose-list: `{h}`"))?;
            policy = policy.expose_header(header);
        }
        for h in FRAMEWORK_SENT {
            policy = policy.expose_header(HeaderName::from_static(h));
        }
        if cors.credentials {
            policy = policy.allow_credentials(true);
        }
        if let Some(age) = cors.max_age {
            let secs: i32 = age
                .as_secs()
                .try_into()
                .context("CORS max_age overflows i32 seconds (~68 years); pick a smaller value")?;
            policy = policy.max_age(secs);
        }
        Ok(CorsPolicy {
            cors: policy,
            methods,
        })
    }

    /// `inner` behind the response compression `Accept-Encoding` negotiates,
    /// when `compression` is on, and behind `cors`, when set: compression inside
    /// CORS, so a preflight is never encoded, both in one crossing.
    pub(crate) fn around(
        inner: BoxEndpoint,
        cors: Option<ServedCorsPolicy>,
        compression: bool,
    ) -> BoxEndpoint {
        let mut endpoint: PoemEndpoint = ToPoem(inner).boxed();
        if compression {
            endpoint = Compressed {
                decoding: Restored(endpoint).with(Compression::new()),
            }
            .boxed();
        }
        if let Some(ServedCorsPolicy { cors, wildcard }) = cors {
            endpoint = Cross {
                inner: endpoint.with(cors),
                wildcard,
            }
            .boxed();
        }
        BoxEndpoint::new(FromPoem(endpoint))
    }

    /// `uri`'s query string decoded into `T`, or what the decoder refused, in
    /// its words.
    pub(crate) fn query<T: DeserializeOwned>(uri: &Uri) -> Result<T, serde::de::value::Error> {
        poem::Request::builder()
            .uri(uri.clone())
            .finish()
            .params::<T>()
            .map_err(|refused| refused.0)
    }

    /// An `application/x-www-form-urlencoded` body, already read, decoded into
    /// `T`.
    pub(crate) async fn form<T: DeserializeOwned>(bytes: Bytes) -> Result<T> {
        let (request, mut body) = poem::Request::builder()
            .method(Method::POST)
            .content_type("application/x-www-form-urlencoded")
            .body(bytes)
            .split();
        match poem::web::Form::<T>::from_request(&request, &mut body).await {
            Ok(poem::web::Form(value)) => Ok(value),
            Err(refused) => Err(match refused.downcast::<ParseFormError>() {
                Ok(ParseFormError::UrlDecode(malformed)) => FormError::Malformed(malformed).into(),
                Ok(other) => error_from_poem(other.into()),
                Err(other) => error_from_poem(other),
            }),
        }
    }

    /// The parts of a `multipart/form-data` body, read as they arrive, the
    /// boundary read off `content_type`.
    pub(crate) async fn multipart(
        content_type: HeaderValue,
        body: Body,
    ) -> Result<MultipartReader> {
        let (request, mut body) = poem::Request::builder()
            .method(Method::POST)
            .header(CONTENT_TYPE, content_type)
            .body(body_to_poem(body))
            .split();
        poem::web::Multipart::from_request(&request, &mut body)
            .await
            .map(MultipartReader)
            .map_err(multipart_refusal)
    }
}

/// A multipart body's parts, as the library reads them.
pub(crate) struct MultipartReader(poem::web::Multipart);

impl MultipartReader {
    /// The next part, or `None` past the last.
    pub(crate) async fn next_field(&mut self) -> Result<Option<FieldReader>> {
        self.0
            .next_field()
            .await
            .map(|field| field.map(FieldReader))
            .map_err(|refused| multipart_refusal(refused.into()))
    }
}

/// One part of a multipart body, as the library reads it.
pub(crate) struct FieldReader(poem::web::Field);

impl FieldReader {
    pub(crate) fn name(&self) -> Option<&str> {
        self.0.name()
    }

    pub(crate) fn file_name(&self) -> Option<&str> {
        self.0.file_name()
    }

    pub(crate) fn content_type(&self) -> Option<&str> {
        self.0.content_type()
    }

    /// The whole part.
    pub(crate) async fn bytes(self) -> Result<Bytes> {
        self.0
            .bytes()
            .await
            .map(Bytes::from)
            .map_err(|refused| multipart_refusal(refused.into()))
    }

    /// The part's bytes as they arrive.
    pub(crate) fn into_async_read(self) -> impl AsyncRead + Send + 'static {
        self.0.into_async_read()
    }
}

/// A media-type refusal of the library's own extractors, said in nestrs's words:
/// the type the body had to be, never the one the client sent, which the
/// library's sentence quotes. `None` for any other error.
fn media_type_refusal(error: &poem::Error) -> Option<HttpError> {
    if let Some(ParseJsonError::InvalidContentType(_) | ParseJsonError::ContentTypeRequired) =
        error.downcast_ref::<ParseJsonError>()
    {
        return Some(JsonError::MediaType.into());
    }
    if let Some(ParseFormError::InvalidContentType(_) | ParseFormError::ContentTypeRequired) =
        error.downcast_ref::<ParseFormError>()
    {
        return Some(FormError::MediaType.into());
    }
    if let Some(
        ParseMultipartError::InvalidContentType(_) | ParseMultipartError::ContentTypeRequired,
    ) = error.downcast_ref::<ParseMultipartError>()
    {
        return Some(MultipartError::MediaType.into());
    }
    None
}

/// What the library's multipart reader refused, said in nestrs's words: its
/// own may name what the client sent.
fn multipart_refusal(refused: poem::Error) -> HttpError {
    match refused.downcast::<ParseMultipartError>() {
        Ok(
            ParseMultipartError::ContentTypeRequired | ParseMultipartError::InvalidContentType(_),
        ) => MultipartError::MediaType.into(),
        Ok(malformed) => MultipartError::Malformed(Box::new(malformed)).into(),
        Err(other) => error_from_poem(other),
    }
}

/// The catch-all poem matches below every other route: what no address claims.
const UNROUTED: &str = "/*__nestrs_unrouted";

/// What poem's catch-all reaches: the request, whole, for the fallback site.
///
/// poem matches a root catch-all before the root's own route when nothing of
/// the path is left, so `/` is handed back to the table's root address.
struct Unmatched {
    unrouted: BoxEndpoint,
    root: Option<Arc<Matched>>,
}

impl poem::Endpoint for Unmatched {
    type Output = poem::Response;

    async fn call(&self, req: poem::Request) -> poem::Result<poem::Response> {
        if let Some(root) = &self.root
            && req.uri().path() == "/"
        {
            return poem::Endpoint::call(root.as_ref(), req).await;
        }
        let resp = self
            .unrouted
            .call(request_from_poem(req))
            .await
            .map_err(error_to_poem)?;
        Ok(response_to_poem(resp))
    }
}

/// poem's router answering nestrs's requests.
struct Router(poem::Route);

impl Endpoint for Router {
    async fn call(&self, req: Request) -> Result<Response> {
        match poem::Endpoint::call(&self.0, request_to_poem(req)).await {
            Ok(resp) => Ok(response_from_poem(resp)),
            // An entry's own `404` carries the pattern poem stamps on it; the
            // router's carries none.
            Err(err) if err.is::<NotFoundError>() && err.data::<PathPattern>().is_none() => {
                Err(HttpError::from_status(StatusCode::NOT_FOUND))
            }
            Err(err) => Err(error_from_poem(err)),
        }
    }
}

/// What answers at one address once poem's router matched it.
enum Answers {
    Any(BoxEndpoint),
    Methods(Methods<BoxEndpoint>),
}

/// One address of the table, as poem's router reaches it: it records the
/// template it was registered under, in nestrs's spelling, and each parameter
/// percent-decoded once, then dispatches on the method.
struct Matched {
    template: Arc<str>,
    names: Box<[Arc<str>]>,
    answers: Answers,
    /// What answers a method the address does not serve.
    unrouted: BoxEndpoint,
}

impl Matched {
    fn new(address: Address, unrouted: BoxEndpoint) -> Self {
        let names = address
            .template
            .parameters()
            .chain(address.template.catch_all())
            .map(Arc::from)
            .collect();
        let answers = match address.dispatch {
            Dispatch::Any(leaf) => Answers::Any(leaf),
            Dispatch::Methods(entries) => Answers::Methods(Methods::new(entries)),
        };
        Self {
            template: Arc::from(address.template.as_str()),
            names,
            answers,
            unrouted,
        }
    }

    /// Each parameter poem matched, by name, percent-decoded once by poem's
    /// router — `%2F` is a `/` inside the value. poem drops a parameter whose
    /// decoded bytes are not UTF-8: the client's `400`.
    fn params(&self, req: &poem::Request) -> Result<Vec<(Arc<str>, String)>> {
        let mut params = Vec::with_capacity(self.names.len());
        for name in &self.names {
            let value = req.raw_path_param(name).ok_or_else(|| {
                HttpError::new(
                    PathParameterError {
                        name: Arc::clone(name),
                    },
                    StatusCode::BAD_REQUEST,
                )
            })?;
            params.push((Arc::clone(name), value.to_owned()));
        }
        Ok(params)
    }
}

impl poem::Endpoint for Matched {
    type Output = poem::Response;

    async fn call(&self, mut req: poem::Request) -> poem::Result<poem::Response> {
        if let Some(slot) = req.extensions().get::<MatchedRoute>() {
            slot.note(&self.template);
        }
        let params = self.params(&req).map_err(error_to_poem)?;
        let (leaf, head) = match &self.answers {
            Answers::Any(leaf) => (leaf, false),
            Answers::Methods(methods) => match methods.choose(req.method()) {
                Chosen::Entry(leaf) => (leaf, false),
                Chosen::HeadOf(leaf) => {
                    req.set_method(Method::GET);
                    req.extensions_mut().insert(HeadAsGet);
                    (leaf, true)
                }
                Chosen::NotAllowed => {
                    let mut request = request_from_poem(req);
                    request
                        .extensions_mut()
                        .insert(NotAllowed(methods.allow().clone()));
                    let resp = self.unrouted.call(request).await.map_err(error_to_poem)?;
                    return Ok(response_to_poem(resp));
                }
            },
        };
        let mut request = request_from_poem(req);
        request.set_route(Arc::clone(&self.template), params);
        let resp = leaf.call(request).await.map_err(error_to_poem)?;
        Ok(response_to_poem(if head { head_of(resp) } else { resp }))
    }
}

/// A template [`RouteTemplate::parse`](crate::RouteTemplate::parse) accepted, in poem's spelling: `{name}`
/// is `:name`, `{*name}` is `*name`, `{{` is `{`.
///
/// A translation, not a check: what is not a template comes out as mechanical
/// as it went in, so it is handed only what the grammar read.
fn poem_pattern(template: &str) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(at) = rest.find(['{', '}']) {
        out.push_str(&rest[..at]);
        let brace = rest.as_bytes()[at];
        let after = &rest[at + 1..];
        if after.as_bytes().first() == Some(&brace) {
            out.push(char::from(brace));
            rest = &after[1..];
            continue;
        }
        if brace == b'}' {
            out.push('}');
            rest = after;
            continue;
        }
        let close = after.find('}').unwrap_or(after.len());
        match after[..close].strip_prefix('*') {
            Some(name) => {
                out.push('*');
                out.push_str(name);
            }
            None => {
                out.push(':');
                out.push_str(&after[..close]);
            }
        }
        rest = after.get(close + 1..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

/// The request's `Accept-Encoding`, set aside while poem's middleware decodes
/// the request body: the coding was negotiated here, and poem's own reading of
/// the header would compress what the negotiation refused.
#[derive(Clone)]
struct Offered(Vec<HeaderValue>);

/// Responses compressed with the coding `Accept-Encoding` weighs highest
/// (RFC 9110 §12.5.3), an event stream left as written: a compressor holds an
/// event back until its buffer fills.
struct Compressed<E> {
    /// poem's middleware, which decodes a request body its `Content-Encoding`
    /// names and, the request's `Accept-Encoding` set aside, compresses nothing.
    decoding: E,
}

impl<E: poem::Endpoint<Output = poem::Response>> poem::Endpoint for Compressed<E> {
    type Output = poem::Response;

    async fn call(&self, mut req: poem::Request) -> poem::Result<poem::Response> {
        let coding = AcceptEncoding::of(req.headers()).and_then(|offered| offered.negotiate());
        if let Entry::Occupied(offered) = req.headers_mut().entry(ACCEPT_ENCODING) {
            let offered = Offered(offered.remove_entry_mult().1.collect());
            req.extensions_mut().insert(offered);
        }
        let mut resp = self.decoding.call(req).await?;
        if !compressible(&resp) {
            return Ok(resp);
        }
        vary_on(resp.headers_mut(), ACCEPT_ENCODING);
        Ok(match coding {
            Some(coding) => Compress::new(resp, algorithm(coding)).into_response(),
            None => resp,
        })
    }
}

/// The request with its `Accept-Encoding` back, for what answers it.
struct Restored(PoemEndpoint);

impl poem::Endpoint for Restored {
    type Output = poem::Response;

    async fn call(&self, mut req: poem::Request) -> poem::Result<poem::Response> {
        if let Some(Offered(offered)) = req.extensions_mut().remove::<Offered>() {
            for value in offered {
                req.headers_mut().append(ACCEPT_ENCODING, value);
            }
        }
        self.0.call(req).await
    }
}

/// Whether `resp` may be compressed: it has content to encode, none encoded
/// already, is no part of a representation — `Content-Range` counts unencoded
/// bytes (RFC 9110 §14.4) — and is no event stream.
fn compressible(resp: &poem::Response) -> bool {
    let status = resp.status();
    let bodiless = status.is_informational()
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED;
    let partial =
        status == StatusCode::PARTIAL_CONTENT || resp.headers().contains_key(CONTENT_RANGE);
    let event_stream = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("text/event-stream"));
    !bodiless && !partial && !event_stream && !resp.headers().contains_key(CONTENT_ENCODING)
}

/// poem's name for `coding`.
fn algorithm(coding: Coding) -> CompressionAlgo {
    match coding {
        Coding::Zstd => CompressionAlgo::ZSTD,
        Coding::Brotli => CompressionAlgo::BR,
        Coding::Gzip => CompressionAlgo::GZIP,
        Coding::Deflate => CompressionAlgo::DEFLATE,
    }
}

/// An answer under a CORS policy, which depends on the request's `Origin`
/// whatever it lists, a refusal included: a shared cache keys it on the origin
/// (WHATWG Fetch, *CORS protocol and HTTP caches*), where poem says so only for
/// a pattern. A refusal files its one `warn`, and a policy admitting every
/// method says so as Fetch spells it, `*`, where poem lists methods no page
/// can send.
struct Cross<E> {
    inner: E,
    wildcard: bool,
}

impl<E: poem::Endpoint> poem::Endpoint for Cross<E> {
    type Output = poem::Response;

    async fn call(&self, req: poem::Request) -> poem::Result<poem::Response> {
        let mut resp = match self.inner.call(req).await {
            Ok(resp) => resp.into_response(),
            // Rendered as the normalizer outside `around` renders an `Err`, so
            // the refusal of an unlisted origin carries `Vary` too.
            Err(error) => {
                if let Some(refused) = cors_refusal(&error) {
                    refused.file();
                }
                response_to_poem(error_from_poem(error).answer())
            }
        };
        if self.wildcard && resp.headers().contains_key(ACCESS_CONTROL_ALLOW_METHODS) {
            resp.headers_mut()
                .insert(ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("*"));
        }
        vary_on(resp.headers_mut(), http::header::ORIGIN);
        Ok(resp)
    }
}

/// The check of the policy poem's CORS layer refused `error` for, if it did.
fn cors_refusal(error: &poem::Error) -> Option<CorsRefusal> {
    Some(match error.downcast_ref::<CorsError>()? {
        CorsError::OriginNotAllowed => CorsRefusal::Origin,
        CorsError::MethodNotAllowed => CorsRefusal::Method,
        CorsError::HeadersNotAllowed => CorsRefusal::Headers,
    })
}

/// Add `name` to `Vary`, unless it is already there or `Vary` is `*`.
fn vary_on(headers: &mut HeaderMap, name: HeaderName) {
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

// The crossings between nestrs's vocabulary and poem's: nothing is lost either
// way, since what one side has no place for rides the other's extensions,
// privately, and comes back on the return crossing.

/// What poem keeps private about a request it built — its peer, its upgrade,
/// its route's parameters — emptied of the head nestrs carries, so the way back
/// restores it.
#[derive(Clone)]
struct PoemState(Arc<Mutex<Option<poem::RequestParts>>>);

impl PoemState {
    fn new(parts: poem::RequestParts) -> Self {
        Self(Arc::new(Mutex::new(Some(parts))))
    }

    fn take(self) -> Option<poem::RequestParts> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// hyper's upgrade, cloned before poem moves the original into its private
/// state: either side may take the socket, both await one receiver.
#[derive(Clone)]
struct CarriedUpgrade(OnUpgrade);

/// What a poem request has no place for: the route the router matched, and the
/// instant the request arrived, which the edge counts its budget from after the
/// CORS or compression layer ran before it.
#[derive(Clone, Default)]
struct Unplaced {
    route: Option<Routed>,
    arrival: Option<Instant>,
}

/// A poem request as nestrs's.
fn request_from_poem(request: poem::Request) -> Request {
    let peer = request.remote_addr().as_socket_addr().copied();
    let local = request.local_addr().as_socket_addr().copied();
    let scheme = request.scheme().clone();
    let original_uri = request.original_uri().clone();
    let (mut parts, body) = request.into_parts();
    let (mut head, ()) = http::Request::new(()).into_parts();
    head.method = std::mem::take(&mut parts.method);
    head.uri = std::mem::take(&mut parts.uri);
    head.version = parts.version;
    head.headers = std::mem::take(&mut parts.headers);
    head.extensions = std::mem::take(&mut parts.extensions);
    let Unplaced { route, arrival } = head.extensions.remove::<Unplaced>().unwrap_or_default();
    if let Some(CarriedUpgrade(upgrade)) = head.extensions.remove::<CarriedUpgrade>() {
        head.extensions.insert(upgrade);
    }
    head.extensions.insert(PoemState::new(parts));
    let facts = RequestFacts {
        peer,
        local,
        scheme,
        original_uri,
        route,
        arrival,
    };
    Request::from_raw(head, body_from_poem(body), facts)
}

/// A nestrs request as poem's.
fn request_to_poem(request: Request) -> poem::Request {
    let (mut head, body, facts) = request.into_raw();
    let mut parts = match head
        .extensions
        .remove::<PoemState>()
        .and_then(PoemState::take)
    {
        Some(mut parts) => {
            parts.method = head.method;
            parts.uri = head.uri;
            parts.version = head.version;
            parts.headers = head.headers;
            parts.extensions = head.extensions;
            parts
        }
        None => {
            let upgrade = head.extensions.get::<OnUpgrade>().cloned();
            // poem reads the URI as sent off the head, then takes the current one.
            let current = std::mem::replace(&mut head.uri, facts.original_uri);
            let mut parts = poem::RequestParts::from((
                head,
                LocalAddr(addr(facts.local)),
                RemoteAddr(addr(facts.peer)),
                facts.scheme,
            ));
            parts.uri = current;
            if let Some(upgrade) = upgrade {
                parts.extensions.insert(CarriedUpgrade(upgrade));
            }
            parts
        }
    };
    if facts.route.is_some() || facts.arrival.is_some() {
        parts.extensions.insert(Unplaced {
            route: facts.route,
            arrival: facts.arrival,
        });
    }
    poem::Request::from_parts(parts, body_to_poem(body))
}

/// A poem response as nestrs's.
fn response_from_poem(response: poem::Response) -> Response {
    let (head, body) = http::Response::<BoxBody<Bytes, io::Error>>::from(response).into_parts();
    Response::from_parts(head, Body::from_boxed(body))
}

/// A nestrs response as poem's.
fn response_to_poem(response: Response) -> poem::Response {
    let (head, body) = response.into_parts();
    poem::Response::from_parts(
        poem::ResponseParts {
            status: head.status,
            version: head.version,
            headers: head.headers,
            extensions: head.extensions,
        },
        body_to_poem(body),
    )
}

/// A poem error as nestrs's: an [`HttpError`] that crossed into poem comes
/// back whole, with what poem's side added to it; any other answers as poem
/// renders it, its `set_data` values kept inside it — they reach the response,
/// never [`HttpError::extensions`].
fn error_from_poem(error: poem::Error) -> HttpError {
    let status = error.status();
    let Some(carried) = error.downcast_ref::<Carried>() else {
        return HttpError::wrapping(Box::new(error), status, render_poem);
    };
    let taken = carried.take();
    // poem hands its error's extensions over only on the response it renders;
    // the emptied carrier renders an empty one.
    let added = error.into_response().into_parts().0.extensions;
    let mut error = taken.unwrap_or_else(|| HttpError::from_status(status));
    error.extensions_mut().extend(added);
    error
}

/// A nestrs error as poem's: a poem error that crossed into nestrs goes back
/// whole when nestrs added nothing to it; any other is carried, rendered by
/// nestrs when poem answers it. poem's `downcast_ref` and `is` see only the
/// carrier, and its `source()` ends there: a poem layer matching errors by
/// type misses a carried one.
fn error_to_poem(error: HttpError) -> poem::Error {
    match error.into_wrapped::<poem::Error>() {
        Ok(error) => error,
        Err(error) => poem::Error::from(Carried {
            status: error.status(),
            error: Mutex::new(Some(error)),
        }),
    }
}

/// A poem body as nestrs's: both wrap one box, which moves.
fn body_from_poem(body: poem::Body) -> Body {
    Body::from_boxed(body.into())
}

/// A nestrs body as poem's: both wrap one box, which moves.
fn body_to_poem(body: Body) -> poem::Body {
    poem::Body::from(body.into_boxed())
}

/// A poem endpoint answering nestrs's requests.
struct FromPoem<E>(E);

impl<E: poem::Endpoint + 'static> Endpoint for FromPoem<E> {
    async fn call(&self, req: Request) -> Result<Response> {
        self.0
            .call(request_to_poem(req))
            .await
            .map(|output| response_from_poem(poem::IntoResponse::into_response(output)))
            .map_err(error_from_poem)
    }
}

/// A nestrs endpoint answering poem's requests.
struct ToPoem<E>(E);

impl<E: Endpoint> poem::Endpoint for ToPoem<E> {
    type Output = poem::Response;

    async fn call(&self, req: poem::Request) -> poem::Result<poem::Response> {
        self.0
            .call(request_from_poem(req))
            .await
            .map(response_to_poem)
            .map_err(error_to_poem)
    }
}

/// An [`HttpError`] inside a poem error: taken out when it crosses back, or
/// rendered when poem answers it.
struct Carried {
    status: StatusCode,
    error: Mutex<Option<HttpError>>,
}

impl Carried {
    fn take(&self) -> Option<HttpError> {
        self.error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

impl fmt::Debug for Carried {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Carried")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// The whole chain, said once: the cause cannot be lent out of the lock as a
/// `source`, so a log line on poem's side reads it here.
impl fmt::Display for Carried {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &*self.error.lock().unwrap_or_else(PoisonError::into_inner) {
            Some(error) => f.write_str(&nest_rs_core::error_message(error)),
            None => fmt::Display::fmt(&self.status, f),
        }
    }
}

impl std::error::Error for Carried {}

impl poem::error::ResponseError for Carried {
    fn status(&self) -> StatusCode {
        self.status
    }

    fn as_response(&self) -> poem::Response {
        match self.take() {
            Some(error) => response_to_poem(error.into_response()),
            None => poem::Response::builder().status(self.status).finish(),
        }
    }
}

/// A poem error answers as poem renders it, each decode failure in its chain
/// said without its value.
fn render_poem(error: BoxError, status: StatusCode) -> Response {
    match error.downcast::<poem::Error>() {
        Ok(error) => response_from_poem(render_error(*error)),
        // Unreachable: the box was filled with a poem error by `error_from_poem`.
        Err(_) => crate::IntoResponse::into_response(ProblemDetails::from_status(status)),
    }
}

fn addr(socket: Option<SocketAddr>) -> poem::Addr {
    socket.map_or_else(poem::Addr::default, poem::Addr::SocketAddr)
}

/// An `Err` poem raised as the response it renders, by one rule: an error that
/// renders itself — poem's `ResponseError`, a ready response, an
/// [`HttpError`] — answers as it renders, but a media-type refusal of poem's
/// own extractors, which quotes the client's `Content-Type`, answers nestrs's
/// sentence naming only the type expected. Any other poem
/// renders as its own sentence, so it is read instead: a [`Problem`] its chain
/// carries answers its document, and a `5xx` answers opaquely; what either
/// answer withholds of the chain is filed once at `error`. A raw text body left
/// says each decode failure in the error's chain without its value.
///
/// `Query<T>`'s rejection is transparent over its serde error, so that one is
/// read off the type.
fn render_error(err: poem::Error) -> poem::Response {
    // Taken out and rendered as nestrs renders it: poem's own rendering would
    // replace the response's extensions with its error's.
    if err.is::<Carried>() {
        return response_to_poem(error_from_poem(err).answer());
    }
    if let Some(refused) = media_type_refusal(&err) {
        return response_to_poem(refused.answer());
    }
    let source = std::error::Error::source(&err);
    let problem = source
        .and_then(nest_rs_core::__private::find_in_chain::<Problem>)
        .cloned();
    let withheld = source.and_then(|chain| match &problem {
        Some(problem) => nest_rs_core::__private::withheld(problem, chain),
        None => nest_rs_core::__private::Withheld::behind(err.status().as_u16(), chain),
    });
    let sentence = err.to_string();
    let mut said = DecodeError::redact(&sentence, Some(&err)).into_owned();
    if let Some(query) = err.downcast_ref::<ParseQueryError>() {
        said = DecodeError::redact(&said, Some(&query.0)).into_owned();
    }
    let redacted = (said != sentence).then_some(said);
    let mut resp = err.into_response();
    if (problem.is_some() || withheld.is_some()) && is_raw_text(resp.headers()) {
        let body = resp.take_body();
        if !body.is_empty() {
            let (parts, _) = resp.into_parts();
            if let Some(withheld) = withheld {
                crate::error::file(withheld);
            }
            let answer = match problem {
                Some(problem) => {
                    let mut answer = problem_response(&problem);
                    answer.extensions_mut().extend(parts.extensions);
                    answer
                }
                None => replacing(
                    ProblemDetails::from_status(parts.status),
                    &parts.headers,
                    parts.extensions,
                ),
            };
            return response_to_poem(answer);
        }
        resp.set_body(body);
    }
    let mut resp = match redacted {
        Some(said) if is_raw_text(resp.headers()) => {
            let (parts, _) = resp.into_parts();
            poem::Response::from_parts(parts, poem::Body::from_string(said))
        }
        _ => resp,
    };
    if is_raw_text(resp.headers()) {
        resp.extensions_mut().insert(Said);
    }
    resp
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use hyper_util::rt::TokioIo;

    use super::*;
    use crate::cors::WILDCARD;
    use crate::error::BodyError;
    use crate::testing::{Upgrading, internal_document, read_exactly};

    #[derive(Debug, Clone, PartialEq)]
    struct Marker(&'static str);

    #[derive(Debug, thiserror::Error, PartialEq)]
    #[error("the kettle is busy")]
    struct Busy;

    /// `GET uri`, as poem's own builder makes it.
    fn poem_get(uri: &'static str) -> poem::Request {
        poem::Request::builder().uri(Uri::from_static(uri)).finish()
    }

    /// The body of `response`, read as text.
    async fn poem_text(response: poem::Response) -> String {
        response.into_body().into_string().await.unwrap()
    }

    fn cfg(origins: &[&str]) -> HttpCors {
        HttpCors {
            origins: origins.iter().map(|s| (*s).to_owned()).collect(),
            ..Default::default()
        }
    }

    /// The entry the router reaches records its template in nestrs's spelling,
    /// never poem's, each parameter by name, and notes the template for the
    /// edge; a path no entry claims is the router's own `404`.
    #[tokio::test]
    async fn an_entry_records_the_template_it_was_registered_under() {
        let echo = crate::endpoint_fn(|req: Request| async move {
            Ok(Response::builder().body(format!(
                "{} {} {}",
                req.route().unwrap_or("-"),
                req.path_param("org").unwrap_or("-"),
                req.path_param("rest").unwrap_or("-"),
            )))
        });
        let unrouted = BoxEndpoint::new(crate::endpoint_fn(|req: Request| async move {
            let allowed = req
                .extensions()
                .get::<NotAllowed>()
                .map(|allow| allow.0.clone());
            Ok(Response::builder()
                .status(StatusCode::IM_A_TEAPOT)
                .body(match allowed {
                    Some(allow) => format!("unrouted {} allow {allow:?}", req.uri().path()),
                    None => format!("unrouted {}", req.uri().path()),
                }))
        }));
        let router = PoemAdapter::router(
            vec![Address {
                template: crate::RouteTemplate::parse("/orgs/@{org}/files/{*rest}").unwrap(),
                owner: Arc::from("FilesController"),
                dispatch: Dispatch::Methods(vec![(Method::GET, BoxEndpoint::new(echo))]),
            }],
            unrouted,
        )
        .unwrap();

        let slot = MatchedRoute::default();
        let request = Request::builder()
            .uri("/orgs/@acme/files/a%20b/c".parse().unwrap())
            .extension(slot.clone())
            .finish();
        let resp = router.call(request).await.unwrap();
        assert_eq!(
            resp.into_body().into_string().await.unwrap(),
            "/orgs/@{org}/files/{*rest} acme a b/c"
        );
        assert_eq!(slot.route().as_deref(), Some("/orgs/@{org}/files/{*rest}"));

        for (path, answered) in [("/elsewhere", "unrouted /elsewhere"), ("/", "unrouted /")] {
            let missed = router
                .call(Request::builder().uri(path.parse().unwrap()).finish())
                .await
                .expect("the fallback site answers it");
            assert_eq!(missed.status(), StatusCode::IM_A_TEAPOT);
            assert_eq!(missed.into_body().into_string().await.unwrap(), answered);
        }

        let not_allowed = router
            .call(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/orgs/@acme/files/x".parse().unwrap())
                    .finish(),
            )
            .await
            .expect("the fallback site answers it");
        assert_eq!(
            not_allowed.into_body().into_string().await.unwrap(),
            r#"unrouted /orgs/@acme/files/x allow "GET, HEAD""#
        );
    }

    #[test]
    fn a_template_is_handed_to_poem_in_its_spelling() {
        for (ours, poems) in [
            ("/{id}", "/:id"),
            ("/@{handle}", "/@:handle"),
            ("/{*rest}", "/*rest"),
            ("/users/{id}/posts/{post_id}", "/users/:id/posts/:post_id"),
            ("/files/x{*rest}", "/files/x*rest"),
            ("/b/{{x}}", "/b/{x}"),
            ("/plain", "/plain"),
            ("/", "/"),
        ] {
            let template = crate::RouteTemplate::parse(ours).unwrap();
            assert_eq!(poem_pattern(template.as_str()), poems, "{ours}");
        }
    }

    #[test]
    fn a_cors_policy_accepts_an_empty_config() {
        PoemAdapter::cors_policy(&cfg(&[])).expect("empty config builds");
    }

    #[test]
    fn a_cors_policy_accepts_a_basic_origin_list() {
        PoemAdapter::cors_policy(&cfg(&["https://app.example.com"])).expect("valid config");
    }

    fn err_string(result: anyhow::Result<CorsPolicy>) -> String {
        match result {
            Ok(_) => panic!("expected an error"),
            Err(err) => err.to_string(),
        }
    }

    #[test]
    fn a_cors_policy_rejects_an_invalid_method() {
        // Spaces aren't token characters in RFC 9110 §9 — `Method::from_bytes`
        // refuses them.
        let cfg = HttpCors {
            origins: vec!["*".into()],
            methods: vec!["BAD METHOD".into()],
            ..Default::default()
        };
        let err = err_string(PoemAdapter::cors_policy(&cfg));
        assert!(err.contains("invalid HTTP method"), "got: {err}");
    }

    #[test]
    fn a_cors_policy_rejects_an_invalid_header_name() {
        let cfg = HttpCors {
            origins: vec!["*".into()],
            headers: vec!["bad header!".into()],
            ..Default::default()
        };
        let err = err_string(PoemAdapter::cors_policy(&cfg));
        assert!(err.contains("invalid header name"), "got: {err}");
    }

    #[test]
    fn a_cors_policy_rejects_a_max_age_that_overflows_i32_seconds() {
        let cfg = HttpCors {
            origins: vec!["*".into()],
            max_age: Some(Duration::from_secs(u64::MAX)),
            ..Default::default()
        };
        let err = err_string(PoemAdapter::cors_policy(&cfg));
        assert!(err.contains("max_age overflows"), "got: {err}");
    }

    #[test]
    fn a_cors_policy_accepts_credentials_and_max_age_and_exposed_headers() {
        let cfg = HttpCors {
            origins: vec!["https://app.example.com".into()],
            methods: vec!["GET".into(), "POST".into()],
            headers: vec!["content-type".into(), "x-trace-id".into()],
            exposed_headers: vec!["x-trace-id".into()],
            credentials: true,
            max_age: Some(Duration::from_secs(60 * 60)),
        };
        PoemAdapter::cors_policy(&cfg).expect("a fully-specified config builds");
    }

    #[test]
    fn a_cors_policy_rejects_an_invalid_exposed_header() {
        let cfg = HttpCors {
            origins: vec!["*".into()],
            exposed_headers: vec!["bad header!".into()],
            ..Default::default()
        };
        let err = err_string(PoemAdapter::cors_policy(&cfg));
        assert!(err.contains("invalid header name"), "got: {err}");
        assert!(err.contains("expose-list"), "must name the list: {err}");
    }

    #[test]
    fn credentials_refuse_a_wildcard_in_every_list_the_rule_binds() {
        type Case = (&'static str, &'static str, fn(&mut HttpCors));
        let cases: [Case; 4] = [
            ("origins", "Access-Control-Allow-Origin", |c| {
                c.origins = vec![WILDCARD.into()]
            }),
            ("headers", "Access-Control-Allow-Headers", |c| {
                c.headers = vec![WILDCARD.into()]
            }),
            ("methods", "Access-Control-Allow-Methods", |c| {
                c.methods = vec![WILDCARD.into()]
            }),
            ("exposed_headers", "Access-Control-Expose-Headers", |c| {
                c.exposed_headers = vec![WILDCARD.into()]
            }),
        ];
        for (field, header, wildcard) in cases {
            let mut cfg = HttpCors {
                origins: vec!["https://app.example.com".into()],
                credentials: true,
                ..Default::default()
            };
            wildcard(&mut cfg);
            let err = err_string(PoemAdapter::cors_policy(&cfg));
            assert!(err.contains(field), "must name the list: {err}");
            assert!(err.contains(header), "must name the header: {err}");
        }
    }

    #[test]
    fn a_wildcard_without_credentials_still_builds() {
        PoemAdapter::cors_policy(&HttpCors {
            origins: vec![WILDCARD.into()],
            headers: vec![WILDCARD.into()],
            methods: vec![WILDCARD.into()],
            exposed_headers: vec![WILDCARD.into()],
            credentials: false,
            ..Default::default()
        })
        .expect("a wildcard policy with no credentials is legal");
    }

    /// The edge inside either layer counts its budget from the instant the
    /// accept loop stamped, not from its own call after the crossing.
    #[tokio::test]
    async fn the_arrival_crosses_each_layer_to_the_edge() {
        let arrival = tokio::time::Instant::now() - Duration::from_secs(3);
        type Layer = (&'static str, fn() -> Option<ServedCorsPolicy>, bool);
        let layers: [Layer; 2] = [
            (
                "cors",
                || {
                    PoemAdapter::cors_policy(&cfg(&["*"]))
                        .ok()
                        .map(|policy| policy.serving(std::iter::empty(), false))
                },
                false,
            ),
            ("compression", || None, true),
        ];
        for (layer, cors, compression) in layers {
            let edge = crate::endpoint_fn(|mut req: crate::Request| async move {
                let seen = req.take_arrival();
                Ok(crate::Response::builder().body(format!("{seen:?}")))
            });
            let around = PoemAdapter::around(BoxEndpoint::new(edge), cors(), compression);
            let (head, ()) = http::Request::new(()).into_parts();
            let facts = crate::request::RequestFacts {
                peer: None,
                local: None,
                scheme: http::uri::Scheme::HTTP,
                original_uri: head.uri.clone(),
                route: None,
                arrival: Some(arrival),
            };
            let request = crate::Request::from_raw(head, crate::Body::empty(), facts);
            let resp = crate::endpoint::Endpoint::call(&around, request)
                .await
                .expect("the layers answer");
            assert_eq!(
                resp.into_body().into_string().await.unwrap(),
                format!("{:?}", Some(arrival)),
                "{layer} loses the arrival"
            );
        }
    }

    #[test]
    fn a_header_is_added_to_vary_once() {
        let mut headers = HeaderMap::new();
        vary_on(&mut headers, http::header::ORIGIN);
        vary_on(&mut headers, http::header::ORIGIN);
        headers.append(
            VARY,
            HeaderValue::from_static("Accept-Language, accept-encoding"),
        );
        vary_on(&mut headers, ACCEPT_ENCODING);
        let vary: Vec<_> = headers.get_all(VARY).iter().collect();
        assert_eq!(vary, ["origin", "Accept-Language, accept-encoding"]);

        let mut any = HeaderMap::new();
        any.insert(VARY, HeaderValue::from_static("*"));
        vary_on(&mut any, http::header::ORIGIN);
        assert_eq!(
            any.get_all(VARY).iter().count(),
            1,
            "`*` varies on everything"
        );
    }

    /// A request crosses into poem and back with its head, its facts, its
    /// route, what rides its extensions and its upgrade, which the entry the
    /// router reaches takes.
    #[tokio::test]
    async fn a_request_crosses_into_poem_and_back_whole() {
        let Upgrading { request, mut peer } = Upgrading::open().await;
        let mut sent = Request::from(request);
        sent.extensions_mut().insert(Marker("request"));
        sent.set_route(
            "/rooms/{room}".into(),
            vec![("room".into(), "lobby".into())],
        );
        *sent.uri_mut() = "/chat/".parse().unwrap();

        let back = request_from_poem(request_to_poem(request_from_poem(request_to_poem(sent))));

        assert_eq!(back.extensions().get::<Marker>(), Some(&Marker("request")));
        assert_eq!(back.original_uri(), "/chat");
        assert_eq!(back.uri(), "/chat/");
        assert_eq!(back.route(), Some("/rooms/{room}"));
        assert_eq!(back.path_param("room"), Some("lobby"));
        let upgrade = back
            .extensions()
            .get::<OnUpgrade>()
            .cloned()
            .expect("nestrs holds the upgrade");
        peer.switch().await;
        let mut socket = TokioIo::new(
            tokio::time::timeout(Duration::from_secs(5), upgrade)
                .await
                .unwrap()
                .unwrap(),
        );
        peer.send(b"nest").await;
        assert_eq!(read_exactly(&mut socket, 4).await, b"nest");
    }

    /// The peer, local address and scheme poem holds cross into nestrs's
    /// facts and back.
    #[test]
    fn a_requests_addresses_and_scheme_cross_both_ways() {
        let peer: SocketAddr = "203.0.113.9:40001".parse().unwrap();
        let local: SocketAddr = "10.0.0.2:443".parse().unwrap();
        let (head, ()) = http::Request::new(()).into_parts();
        let parts = poem::RequestParts::from((
            head,
            LocalAddr(poem::Addr::SocketAddr(local)),
            RemoteAddr(poem::Addr::SocketAddr(peer)),
            http::uri::Scheme::HTTPS,
        ));
        let sent = poem::Request::from_parts(parts, poem::Body::empty());

        let crossed = request_from_poem(sent);
        assert_eq!(crossed.peer_addr(), Some(peer));
        assert_eq!(crossed.local_addr(), Some(local));
        assert_eq!(crossed.scheme(), &http::uri::Scheme::HTTPS);

        let back = request_to_poem(crossed);
        assert_eq!(back.remote_addr().as_socket_addr(), Some(&peer));
        assert_eq!(back.local_addr().as_socket_addr(), Some(&local));
        assert_eq!(back.scheme(), &http::uri::Scheme::HTTPS);
    }

    #[test]
    fn a_poem_error_crosses_into_nestrs_and_back_whole() {
        let mut raised = poem::Error::new(Busy, StatusCode::IM_A_TEAPOT);
        raised.set_data(Marker("error"));
        let crossed = error_from_poem(raised);
        assert_eq!(crossed.status(), StatusCode::IM_A_TEAPOT);
        assert_eq!(
            crossed.downcast_ref::<Busy>(),
            Some(&Busy),
            "a downcast reaches into poem's error"
        );
        let back = error_to_poem(crossed);
        assert_eq!(back.status(), StatusCode::IM_A_TEAPOT);
        assert_eq!(back.data::<Marker>(), Some(&Marker("error")));
        assert_eq!(back.downcast::<Busy>().ok(), Some(Busy));
    }

    #[tokio::test]
    async fn an_http_error_crosses_into_poem_and_comes_back_whole() {
        let mut raised = HttpError::from(BodyError::TooLarge { limit: 8 });
        raised.extensions_mut().insert(Marker("nestrs"));

        let mut crossed = error_to_poem(raised);
        assert_eq!(crossed.status(), StatusCode::PAYLOAD_TOO_LARGE);
        crossed.set_data(7_u32);
        let back = error_from_poem(crossed);

        assert_eq!(back.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(back.extensions().get::<Marker>(), Some(&Marker("nestrs")));
        assert_eq!(
            back.extensions().get::<u32>(),
            Some(&7),
            "what poem's side added is kept"
        );
        assert!(matches!(
            back.downcast::<BodyError>(),
            Ok(BodyError::TooLarge { limit: 8 })
        ));
    }

    #[tokio::test]
    async fn a_poem_error_nestrs_added_to_goes_back_with_both_sides_kept() {
        let crossed_in = || {
            let mut crossed = error_from_poem(poem::Error::new(Busy, StatusCode::IM_A_TEAPOT));
            crossed.extensions_mut().insert(Marker("nestrs"));
            crossed
        };

        let back = error_from_poem(error_to_poem(crossed_in()));
        assert_eq!(back.extensions().get::<Marker>(), Some(&Marker("nestrs")));
        assert_eq!(
            back.downcast_ref::<Busy>(),
            Some(&Busy),
            "the poem cause is still reached"
        );

        let rendered = render_error(error_to_poem(crossed_in()));
        assert_eq!(rendered.status(), StatusCode::IM_A_TEAPOT);
        assert_eq!(
            rendered.data::<Marker>(),
            Some(&Marker("nestrs")),
            "what nestrs added reaches the response"
        );
        assert_eq!(
            rendered.into_body().into_string().await.unwrap(),
            "the kettle is busy",
            "the poem cause answers as poem renders it"
        );
    }

    #[tokio::test]
    async fn an_http_error_poem_renders_answers_nestrs_problem_document() {
        let mut raised = HttpError::from_status(StatusCode::UNAUTHORIZED);
        raised.extensions_mut().insert(Marker("challenge"));

        let rendered = render_error(error_to_poem(raised));

        assert_eq!(rendered.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(rendered.content_type(), Some("application/problem+json"));
        assert_eq!(rendered.header("www-authenticate"), Some("Bearer"));
        assert_eq!(rendered.data::<Marker>(), Some(&Marker("challenge")));

        let by_poem =
            error_to_poem(HttpError::from_status(StatusCode::UNAUTHORIZED)).into_response();
        assert_eq!(by_poem.content_type(), Some("application/problem+json"));
        assert_eq!(by_poem.header("www-authenticate"), Some("Bearer"));
    }

    #[tokio::test]
    async fn a_response_crosses_both_ways_with_its_head_and_body() {
        let sent = Response::builder()
            .status(StatusCode::CREATED)
            .header(
                http::header::LOCATION,
                HeaderValue::from_static("/orders/7"),
            )
            .extension(Marker("response"))
            .body("made");

        let back = response_from_poem(response_to_poem(sent));

        assert_eq!(back.status(), StatusCode::CREATED);
        assert_eq!(back.headers()[http::header::LOCATION], "/orders/7");
        assert_eq!(back.extensions().get::<Marker>(), Some(&Marker("response")));
        assert_eq!(back.into_body().into_string().await.unwrap(), "made");
    }

    #[tokio::test]
    async fn endpoints_answer_across_the_crossing() {
        let nestrs = crate::endpoint_fn(|req: Request| async move {
            Ok(Response::builder().body(format!("nestrs saw {}", req.uri().path())))
        });
        let answer = poem::Endpoint::get_response(&ToPoem(nestrs), poem_get("/here")).await;
        assert_eq!(poem_text(answer).await, "nestrs saw /here");

        let failing = poem::endpoint::make(|_req: poem::Request| async {
            Err::<String, _>(poem::Error::new(Busy, StatusCode::SERVICE_UNAVAILABLE))
        });
        let refused = FromPoem(failing)
            .call(Request::builder().finish())
            .await
            .unwrap_err();
        assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(refused.is::<Busy>());
    }

    /// poem's own body extractors quote the client's `Content-Type` in their
    /// `415`; rendered by the edge, the refusal names only the type expected.
    #[tokio::test]
    async fn a_foreign_media_type_refusal_never_quotes_the_clients_content_type() {
        let marker = "text/x-quoted-marker".to_owned();
        for (refused, expected) in [
            (
                poem::Error::from(ParseJsonError::InvalidContentType(marker.clone())),
                "application/json",
            ),
            (
                poem::Error::from(ParseFormError::InvalidContentType(marker.clone())),
                "application/x-www-form-urlencoded",
            ),
            (
                poem::Error::from(ParseMultipartError::InvalidContentType(marker.clone())),
                "multipart/form-data",
            ),
        ] {
            let resp = response_from_poem(render_error(refused));
            assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
            let body = resp.into_body().into_string().await.expect("a body");
            assert!(!body.contains("x-quoted-marker"), "{body}");
            assert!(body.contains(expected), "{body}");
        }
    }

    /// A plain-text error poem raises says the framework's own sentence, which
    /// the normalizer lifts onto a problem document as its `detail`.
    #[tokio::test]
    async fn a_plain_text_error_poem_raises_is_lifted_onto_the_problem_document() {
        let raw = render_error(poem::Error::from_string(
            nest_rs_core::UUID_V7_REQUIRED,
            StatusCode::BAD_REQUEST,
        ));
        let resp = crate::normalize_error_response(response_from_poem(raw)).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(resp.headers()[CONTENT_TYPE], "application/problem+json");
        let bytes = resp.into_body().into_bytes().await.expect("body");
        let v: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(v["status"], 400);
        assert_eq!(v["detail"], nest_rs_core::UUID_V7_REQUIRED);
    }

    /// An error the edge cannot read an answer in, rendered with its chain
    /// withheld, answers the one opaque document.
    #[tokio::test]
    async fn an_unknown_server_error_renders_the_one_opaque_document() {
        let unknown = poem::Error::new(
            io::Error::other("connection to db-primary refused"),
            StatusCode::INTERNAL_SERVER_ERROR,
        );
        let resp = response_from_poem(render_error(unknown));
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = resp.into_body().into_bytes().await.expect("body");
        let v: serde_json::Value = serde_json::from_slice(&bytes).expect("problem json");
        assert_eq!(v, internal_document());
    }
}
