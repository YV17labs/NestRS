//! The HTTP library the transport adapts, poem, behind one crate-private type:
//! the router the route table compiles into, the CORS and compression
//! answered around the edge, and the query, form and multipart decoders the
//! extractors read through. What those do on the wire is pinned by black-box
//! suites (`tests/integration/{routes,cors,compression,query,form,multipart}.rs`),
//! so a later library replaces this file and passes them unchanged
//! (`.claude/decisions/http-server.md`).

use std::str::FromStr;
use std::sync::Arc;

use anyhow::Context;
use bytes::Bytes;
use http::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_RANGE, CONTENT_TYPE, Entry, VARY};
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use poem::error::{NotFoundError, ParseFormError, ParseJsonError, ParseMultipartError};
use poem::middleware::{Compression, Cors};
use poem::web::{Compress, CompressionAlgo};
use poem::{EndpointExt, FromRequest, IntoResponse, PathPattern};
use serde::de::DeserializeOwned;
use tokio::io::AsyncRead;

use crate::accept_encoding::{AcceptEncoding, Coding};
use crate::allow::{Chosen, HeadAsGet, Methods, head_of};
use crate::body::Body;
use crate::cors::{FRAMEWORK_SENT, HttpCors, listed};
use crate::endpoint::{BoxEndpoint, Endpoint};
use crate::error::{
    FormError, HttpError, JsonError, MultipartError, PathParameterError, Result, RouteError,
};
use crate::matched::MatchedRoute;
use crate::poem_bridge::{
    body_to_poem, error_from_poem, error_to_poem, from_poem, note_route, request_from_poem,
    request_to_poem, response_from_poem, response_to_poem, to_poem,
};
use crate::request::{Request, Routed};
use crate::response::Response;
use crate::routes::{Address, Dispatch, Leaf};

/// poem, as the transport uses it.
pub(crate) struct PoemAdapter;

/// A CORS policy as poem's middleware takes it, checked at boot.
pub(crate) struct CorsPolicy(Cors);

/// A poem endpoint, erased.
type PoemEndpoint = poem::endpoint::BoxEndpoint<'static, poem::Response>;

impl PoemAdapter {
    /// The route table, compiled into poem's router, answering nestrs's
    /// requests: a request no address claims answers `404` as an `Err`, which
    /// the bands under
    /// [`ERROR_RESOLVE`](crate::interceptor::priority::ERROR_RESOLVE) still see.
    pub(crate) fn router(table: Vec<Address>) -> std::result::Result<BoxEndpoint, RouteError> {
        let mut route = poem::Route::new();
        for address in table {
            let pattern = poem_pattern(address.template.as_str());
            let template = address.template.as_str().to_owned();
            let owner = address.owner.to_string();
            route = route
                .try_at(pattern, Matched::new(address))
                .map_err(|refused| RouteError::Router {
                    template,
                    owner,
                    reason: refused.to_string(),
                })?;
        }
        Ok(BoxEndpoint::new(Router(route)))
    }

    /// The policy `cors` describes, or the boot's refusal naming the value it
    /// cannot take. A `*` in `origins`, `methods` or `headers` admits every
    /// value, which poem spells as an empty list; explicit values map one to one.
    pub(crate) fn cors_policy(cors: &HttpCors) -> anyhow::Result<CorsPolicy> {
        cors.reject_wildcards_with_credentials()?;
        let mut policy = Cors::new();
        for origin in listed(&cors.origins) {
            policy = policy.allow_origin(origin);
        }
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
        Ok(CorsPolicy(policy))
    }

    /// `inner` behind the response compression `Accept-Encoding` negotiates,
    /// when `compression` is on, and behind `cors`, when set: compression inside
    /// CORS, so a preflight is never encoded, both in one crossing.
    pub(crate) fn around(
        inner: BoxEndpoint,
        cors: Option<CorsPolicy>,
        compression: bool,
    ) -> BoxEndpoint {
        let mut endpoint: PoemEndpoint = to_poem(inner).boxed();
        if compression {
            endpoint = Compressed {
                decoding: Restored(endpoint).with(Compression::new()),
            }
            .boxed();
        }
        if let Some(CorsPolicy(cors)) = cors {
            endpoint = VariesOnOrigin(endpoint.with(cors)).boxed();
        }
        BoxEndpoint::new(from_poem(endpoint))
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
pub(crate) fn media_type_refusal(error: &poem::Error) -> Option<HttpError> {
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

/// An endpoint still written on poem, registered through the hidden
/// `route_poem`: it reads poem's own route parameters.
pub(crate) struct PoemLeaf(PoemEndpoint);

impl PoemLeaf {
    pub(crate) fn new<E>(endpoint: E) -> Self
    where
        E: poem::IntoEndpoint,
        E::Endpoint: 'static,
    {
        Self(endpoint.into_endpoint().map_to_response().boxed())
    }

    /// The leaf as a nestrs endpoint: the request crosses back into poem with
    /// what poem matched.
    pub(crate) fn into_endpoint(self) -> BoxEndpoint {
        BoxEndpoint::new(from_poem(self.0))
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
    Any(Leaf),
    Methods(Methods<Leaf>),
}

/// One address of the table, as poem's router reaches it: it records the
/// template it was registered under, in nestrs's spelling, and each parameter
/// percent-decoded once, then dispatches on the method.
struct Matched {
    template: Arc<str>,
    names: Box<[Arc<str>]>,
    answers: Answers,
}

impl Matched {
    fn new(address: Address) -> Self {
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
                Chosen::NotAllowed => return Ok(response_to_poem(methods.not_allowed())),
            },
        };
        match leaf {
            Leaf::Endpoint(endpoint) => {
                let mut request = request_from_poem(req);
                request.set_route(Arc::clone(&self.template), params);
                let resp = endpoint.call(request).await.map_err(error_to_poem)?;
                Ok(response_to_poem(if head { head_of(resp) } else { resp }))
            }
            Leaf::Poem(PoemLeaf(endpoint)) => {
                note_route(&mut req, Routed::new(Arc::clone(&self.template), params));
                let resp = endpoint.call(req).await?;
                Ok(if head {
                    response_to_poem(head_of(response_from_poem(resp)))
                } else {
                    resp
                })
            }
        }
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
/// a pattern.
struct VariesOnOrigin<E>(E);

impl<E: poem::Endpoint> poem::Endpoint for VariesOnOrigin<E> {
    type Output = poem::Response;

    async fn call(&self, req: poem::Request) -> poem::Result<poem::Response> {
        let mut resp = match self.0.call(req).await {
            Ok(resp) => resp.into_response(),
            // Rendered as the normalizer outside `around` renders an `Err`, so
            // the refusal of an unlisted origin carries `Vary` too.
            Err(error) => response_to_poem(error_from_poem(error).answer()),
        };
        vary_on(resp.headers_mut(), http::header::ORIGIN);
        Ok(resp)
    }
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::cors::WILDCARD;

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
        let router = PoemAdapter::router(vec![Address {
            template: crate::RouteTemplate::parse("/orgs/@{org}/files/{*rest}").unwrap(),
            owner: Arc::from("FilesController"),
            dispatch: Dispatch::Methods(vec![(
                Method::GET,
                Leaf::Endpoint(BoxEndpoint::new(echo)),
            )]),
        }])
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

        let missed = router
            .call(
                Request::builder()
                    .uri("/elsewhere".parse().unwrap())
                    .finish(),
            )
            .await
            .unwrap_err();
        assert_eq!(missed.status(), StatusCode::NOT_FOUND);
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
        type Layer = (&'static str, fn() -> Option<CorsPolicy>, bool);
        let layers: [Layer; 2] = [
            (
                "cors",
                || PoemAdapter::cors_policy(&cfg(&["*"])).ok(),
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
}
