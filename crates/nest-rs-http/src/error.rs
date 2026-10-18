//! Every error type of the HTTP edge: the vocabulary's ([`BodyError`],
//! [`HttpError`], [`ResponseError`]), the route grammar's
//! ([`RouteTemplateError`]), the browser-origin check's ([`OriginRefused`]),
//! the WebSocket handshake's and the header binding's.

use std::borrow::Cow;
use std::convert::Infallible;
use std::error::Error;
use std::fmt;
use std::time::Duration;

use http::{Extensions, StatusCode};

use nest_rs_core::__private::Withheld;
use nest_rs_core::Problem;
use nest_rs_core::problem::code;

use crate::headers::one_of;
use crate::origin::OriginScheme;
use crate::problem::{ProblemDetails, problem_response};
use crate::response::{IntoResponse, Response};

/// Any error, boxed: a body's, or the cause an [`HttpError`] carries.
pub(crate) type BoxError = Box<dyn Error + Send + Sync>;

/// `std::result::Result` whose error defaults to [`HttpError`]: what an
/// endpoint, a handler and an extractor answer with.
///
/// ```
/// use nest_rs_http::{HttpError, Result, StatusCode};
///
/// fn find(id: u64) -> Result<&'static str> {
///     if id == 1 { Ok("ada") } else { Err(HttpError::from_status(StatusCode::NOT_FOUND)) }
/// }
///
/// assert_eq!(find(2).unwrap_err().status(), StatusCode::NOT_FOUND);
/// ```
pub type Result<T, E = HttpError> = std::result::Result<T, E>;

/// Why a body read stopped. A variant names the bound it crossed, never the
/// bytes it read.
///
/// ```
/// use nest_rs_http::{BodyError, ResponseError, StatusCode};
///
/// let refused = BodyError::TooLarge { limit: 1024 };
/// assert_eq!(refused.status(), StatusCode::PAYLOAD_TOO_LARGE);
/// assert_eq!(refused.to_string(), "the body is larger than 1024 bytes");
/// ```
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum BodyError {
    /// The body is larger than the bound it was read under: `413`.
    #[error("the body is larger than {limit} bytes")]
    TooLarge {
        /// The bound, in bytes.
        limit: usize,
    },
    /// No part of the body arrived within the bound: `408`.
    #[error("no part of the body arrived for {after:?}")]
    Stalled {
        /// How long the read waited.
        after: Duration,
    },
    /// The body was taken before: a handler-signature mistake, `500`.
    #[error("the request body was already read")]
    AlreadyTaken,
    /// The body is not UTF-8 text: `400`.
    #[error("the body is not valid UTF-8")]
    NotUtf8,
    /// The peer reset the connection or the stream failed: `400`.
    #[error("the body stream failed")]
    Io(#[source] std::io::Error),
}

impl ResponseError for BodyError {
    fn status(&self) -> StatusCode {
        match self {
            Self::TooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Stalled { .. } => StatusCode::REQUEST_TIMEOUT,
            Self::AlreadyTaken => StatusCode::INTERNAL_SERVER_ERROR,
            Self::NotUtf8 | Self::Io(_) => StatusCode::BAD_REQUEST,
        }
    }
}

/// An error that knows its HTTP answer, so `?` on it answers its status.
///
/// ```
/// use nest_rs_http::{HttpError, ResponseError, StatusCode};
///
/// #[derive(Debug, thiserror::Error)]
/// #[error("the order is already paid")]
/// struct AlreadyPaid;
///
/// impl ResponseError for AlreadyPaid {
///     fn status(&self) -> StatusCode {
///         StatusCode::CONFLICT
///     }
/// }
///
/// let error = HttpError::from(AlreadyPaid);
/// assert_eq!(error.status(), StatusCode::CONFLICT);
/// assert!(error.is::<AlreadyPaid>());
/// ```
pub trait ResponseError {
    /// The status this error answers.
    fn status(&self) -> StatusCode;

    /// The response this error answers: by default an RFC 9457 problem document
    /// of its status, whose `detail` on a `4xx` is the error's sentence said
    /// without any value it quotes, and which a `5xx` answers without one.
    fn as_response(&self) -> Response
    where
        Self: Error + Send + Sync + Sized + 'static,
    {
        ProblemDetails::answering(self.status(), self).into_response()
    }
}

/// A [`Problem`] answers its status, its document with the `code` member and
/// its `Retry-After`, so `?` on one answers it.
///
/// ```
/// use nest_rs_core::Problem;
/// use nest_rs_core::problem::code;
/// use nest_rs_http::{HttpError, StatusCode};
///
/// let error = HttpError::from(Problem::new(409, code::CONFLICT));
/// assert_eq!(error.status(), StatusCode::CONFLICT);
/// ```
impl ResponseError for Problem {
    fn status(&self) -> StatusCode {
        // `Problem::new` holds its status to 400..=599, every one of them valid.
        StatusCode::from_u16(Problem::status(self)).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
    }

    fn as_response(&self) -> Response {
        problem_response(self)
    }
}

/// An HTTP failure: a status, the error that caused it, and typed extensions
/// copied onto the response it renders.
///
/// It answers an RFC 9457 problem document unless it holds a ready response or
/// a [`ResponseError`] that renders its own; its cause stays reachable by
/// [`downcast_ref`](Self::downcast_ref) and as its
/// [`source`](std::error::Error::source), for the log.
///
/// ```
/// use nest_rs_http::{HttpError, StatusCode};
///
/// let error = HttpError::new(std::fmt::Error, StatusCode::BAD_REQUEST);
/// assert_eq!(error.status(), StatusCode::BAD_REQUEST);
/// assert_eq!(error.into_response().status(), StatusCode::BAD_REQUEST);
/// ```
pub struct HttpError(Box<Repr>);

struct Repr {
    status: StatusCode,
    cause: Cause,
    extensions: Extensions,
}

/// How another library's cause answers on the wire, consumed.
type Render = fn(BoxError, StatusCode) -> Response;

/// How a [`ResponseError`] answers on the wire, from a borrow.
type RenderTyped = fn(&BoxError, StatusCode) -> Response;

enum Cause {
    /// Nothing beneath the status.
    Status,
    /// A response answered as it is.
    Response(Response),
    /// An error rendered as a problem document of the status.
    Error(BoxError),
    /// A [`ResponseError`], rendered by its own `as_response`.
    Typed {
        error: BoxError,
        render: RenderTyped,
    },
    /// Another HTTP library's error crossing into this one: it renders itself,
    /// and a downcast reaches the cause it wraps.
    Wrapped { error: BoxError, render: Render },
    /// `500`, opaque on the wire, its chain kept for the log.
    Anyhow(anyhow::Error),
}

impl HttpError {
    /// `source` answering `status`.
    ///
    /// ```
    /// use nest_rs_http::{HttpError, StatusCode};
    ///
    /// let error = HttpError::new(std::fmt::Error, StatusCode::BAD_GATEWAY);
    /// assert!(error.is::<std::fmt::Error>());
    /// ```
    pub fn new(source: impl Error + Send + Sync + 'static, status: StatusCode) -> Self {
        Self::with(status, Cause::Error(Box::new(source)))
    }

    /// A bare `status`, with nothing beneath it.
    ///
    /// ```
    /// use nest_rs_http::{HttpError, StatusCode};
    ///
    /// let error = HttpError::from_status(StatusCode::NOT_FOUND);
    /// assert_eq!(error.to_string(), "404 Not Found");
    /// ```
    pub fn from_status(status: StatusCode) -> Self {
        Self::with(status, Cause::Status)
    }

    /// A ready `response`, answered as it is.
    ///
    /// ```
    /// use nest_rs_http::{HttpError, Response, StatusCode};
    ///
    /// let teapot = Response::builder().status(StatusCode::IM_A_TEAPOT).finish();
    /// let error = HttpError::from_response(teapot);
    /// assert_eq!(error.into_response().status(), StatusCode::IM_A_TEAPOT);
    /// ```
    pub fn from_response(response: Response) -> Self {
        Self::with(response.status(), Cause::Response(response))
    }

    /// Another library's error, rendered by `render`; a downcast also reaches
    /// the error it wraps, one level down.
    pub(crate) fn wrapping(error: BoxError, status: StatusCode, render: Render) -> Self {
        Self::with(status, Cause::Wrapped { error, render })
    }

    fn with(status: StatusCode, cause: Cause) -> Self {
        Self(Box::new(Repr {
            status,
            cause,
            extensions: Extensions::new(),
        }))
    }

    /// The status this error answers.
    ///
    /// ```
    /// use nest_rs_http::{HttpError, StatusCode};
    ///
    /// assert_eq!(HttpError::from(StatusCode::GONE).status(), StatusCode::GONE);
    /// ```
    pub fn status(&self) -> StatusCode {
        self.0.status
    }

    /// Whether the cause is a `T`.
    ///
    /// ```
    /// use nest_rs_http::{HttpError, StatusCode};
    ///
    /// let error = HttpError::new(std::fmt::Error, StatusCode::BAD_REQUEST);
    /// assert!(error.is::<std::fmt::Error>());
    /// assert!(!error.is::<std::io::Error>());
    /// ```
    pub fn is<T: Error + Send + Sync + 'static>(&self) -> bool {
        self.downcast_ref::<T>().is_some()
    }

    /// The cause, when it is a `T`.
    ///
    /// ```
    /// use nest_rs_http::{HttpError, StatusCode};
    ///
    /// let error = HttpError::new(std::fmt::Error, StatusCode::BAD_REQUEST);
    /// assert_eq!(error.downcast_ref::<std::fmt::Error>(), Some(&std::fmt::Error));
    /// ```
    pub fn downcast_ref<T: Error + Send + Sync + 'static>(&self) -> Option<&T> {
        match &self.0.cause {
            Cause::Error(error) | Cause::Typed { error, .. } => error.downcast_ref::<T>(),
            Cause::Wrapped { error, .. } => error
                .downcast_ref::<T>()
                .or_else(|| error.source()?.downcast_ref::<T>()),
            Cause::Anyhow(error) => error.downcast_ref::<T>(),
            Cause::Status | Cause::Response(_) => None,
        }
    }

    /// The cause itself when it is a `T`, otherwise this error back. Unlike
    /// [`downcast_ref`](Self::downcast_ref), it does not reach inside another
    /// library's error.
    ///
    /// ```
    /// use nest_rs_http::{HttpError, StatusCode};
    ///
    /// let error = HttpError::new(std::fmt::Error, StatusCode::BAD_REQUEST);
    /// let error = error.downcast::<std::io::Error>().unwrap_err();
    /// assert_eq!(error.downcast::<std::fmt::Error>().ok(), Some(std::fmt::Error));
    /// ```
    pub fn downcast<T: Error + Send + Sync + 'static>(mut self) -> std::result::Result<T, Self> {
        self.0.cause = match std::mem::replace(&mut self.0.cause, Cause::Status) {
            Cause::Error(error) => match error.downcast::<T>() {
                Ok(cause) => return Ok(*cause),
                Err(error) => Cause::Error(error),
            },
            Cause::Typed { error, render } => match error.downcast::<T>() {
                Ok(cause) => return Ok(*cause),
                Err(error) => Cause::Typed { error, render },
            },
            Cause::Wrapped { error, render } => match error.downcast::<T>() {
                Ok(cause) => return Ok(*cause),
                Err(error) => Cause::Wrapped { error, render },
            },
            Cause::Anyhow(error) => match error.downcast::<T>() {
                Ok(cause) => return Ok(cause),
                Err(error) => Cause::Anyhow(error),
            },
            other => other,
        };
        Err(self)
    }

    /// The cause, when it is another library's error of type `T` to which this
    /// side added nothing: it goes back to that library whole.
    pub(crate) fn into_wrapped<T: Error + Send + Sync + 'static>(
        self,
    ) -> std::result::Result<T, Self> {
        let unwrappable = matches!(&self.0.cause, Cause::Wrapped { error, .. } if error.is::<T>())
            && self.0.extensions.is_empty();
        if unwrappable {
            self.downcast::<T>()
        } else {
            Err(self)
        }
    }

    /// Values copied onto the response this error renders, each replacing the
    /// response's own value of its type.
    ///
    /// ```
    /// use nest_rs_http::{HttpError, StatusCode};
    ///
    /// #[derive(Clone)]
    /// struct Retryable;
    ///
    /// let mut error = HttpError::from_status(StatusCode::SERVICE_UNAVAILABLE);
    /// error.extensions_mut().insert(Retryable);
    /// assert!(error.extensions().get::<Retryable>().is_some());
    /// assert!(error.into_response().extensions().get::<Retryable>().is_some());
    /// ```
    pub fn extensions(&self) -> &Extensions {
        &self.0.extensions
    }

    /// The extensions, to add to: see [`extensions`](Self::extensions).
    ///
    /// ```
    /// use nest_rs_http::{HttpError, StatusCode};
    ///
    /// let mut error = HttpError::from_status(StatusCode::UNAUTHORIZED);
    /// error.extensions_mut().insert(7_u8);
    /// assert_eq!(error.extensions().get::<u8>(), Some(&7));
    /// ```
    pub fn extensions_mut(&mut self) -> &mut Extensions {
        &mut self.0.extensions
    }

    /// The response this error answers: a ready response as it is, a
    /// [`ResponseError`]'s own rendering, otherwise an RFC 9457 problem document
    /// whose `detail` on a `4xx` is the cause's sentence said without any value
    /// it quotes, and which a `5xx` answers without one. The extensions are
    /// copied on last.
    ///
    /// ```
    /// use nest_rs_http::{HttpError, StatusCode, header};
    ///
    /// let response = HttpError::from_status(StatusCode::NOT_FOUND).into_response();
    /// assert_eq!(response.headers()[header::CONTENT_TYPE], "application/problem+json");
    /// ```
    pub fn into_response(self) -> Response {
        let Repr {
            status,
            cause,
            extensions,
        } = *self.0;
        let mut response = match cause {
            Cause::Status | Cause::Anyhow(_) => ProblemDetails::from_status(status).into_response(),
            Cause::Response(response) => response,
            Cause::Error(error) => ProblemDetails::answering(status, &*error).into_response(),
            Cause::Typed { error, render } => render(&error, status),
            Cause::Wrapped { error, render } => render(error, status),
        };
        response.extensions_mut().extend(extensions);
        response
    }
}

impl HttpError {
    /// The response this error answers at the edge: as
    /// [`into_response`](Self::into_response) renders it, except that a
    /// [`Problem`] its cause carries answers its own document; the chain a `5xx`
    /// answer withholds is filed once at `error`.
    pub(crate) fn answer(self) -> Response {
        match self.0.cause {
            Cause::Response(_) | Cause::Wrapped { .. } => self.into_response(),
            _ => self.answer_ref(),
        }
    }

    /// The response this error answers, as [`answer`](Self::answer) renders
    /// it but filing nothing, and the error to keep beside it: itself, unless
    /// rendering took what it held — a ready response, another library's
    /// error — and its status is what is left. Whoever answers last files what
    /// the client was not told ([`withheld_behind`](Self::withheld_behind)).
    pub(crate) fn rendered_kept(self) -> (Response, HttpError) {
        match self.0.cause {
            Cause::Response(_) | Cause::Wrapped { .. } => {
                let status = self.0.status;
                (self.into_response(), HttpError::from_status(status))
            }
            _ => (self.rendered_ref(), self),
        }
    }

    /// [`answer`](Self::answer), from a borrow: a ready response or another
    /// library's error, which only their owner renders, answers its status's
    /// problem.
    pub(crate) fn answer_ref(&self) -> Response {
        let response = self.rendered_ref();
        if let Some(withheld) = self.withheld_behind(response.status()) {
            file(withheld);
        }
        response
    }

    /// The response [`answer_ref`](Self::answer_ref) renders, filing nothing.
    pub(crate) fn rendered_ref(&self) -> Response {
        let status = self.0.status;
        let problem = self
            .chain()
            .and_then(nest_rs_core::__private::find_in_chain::<Problem>);
        let mut response = match (problem, &self.0.cause) {
            (Some(problem), _) => problem_response(problem),
            (None, Cause::Error(error)) => {
                ProblemDetails::answering(status, &**error).into_response()
            }
            (None, Cause::Typed { error, render }) => render(error, status),
            (None, _) => ProblemDetails::from_status(status).into_response(),
        };
        response.extensions_mut().extend(self.0.extensions.clone());
        response
    }

    /// What a `status` answer keeps from the client of this error: the chain
    /// it carries behind a `5xx`, unless that is a bare [`Problem`]. A status
    /// alone, a ready response and an error that renders itself carry none.
    pub(crate) fn withheld_behind(&self, status: StatusCode) -> Option<Withheld> {
        let chain = self.chain().filter(|chain| !chain.is::<Problem>())?;
        Withheld::behind(status.as_u16(), chain)
    }

    /// The error chain this error renders from, when it is one the edge reads.
    fn chain(&self) -> Option<&(dyn Error + 'static)> {
        match &self.0.cause {
            Cause::Anyhow(error) => Some(error.as_ref()),
            Cause::Error(error) => Some(&**error),
            Cause::Status | Cause::Response(_) | Cause::Typed { .. } | Cause::Wrapped { .. } => {
                None
            }
        }
    }
}

/// The operator's line for a failure the answer withholds: its whole chain.
pub(crate) fn failed(said: &str) {
    tracing::error!(
        target: crate::target::HTTP,
        error = %said,
        "request failed",
    );
}

/// File what an answer withheld: a failure at `error`, a `503` — the server
/// unable to serve the request in time — at `warn`.
pub(crate) fn file(withheld: Withheld) {
    match withheld {
        Withheld::Failed(said) => failed(&said),
        Withheld::Unavailable(said) => tracing::warn!(
            target: crate::target::HTTP,
            error = %said,
            "request unavailable",
        ),
    }
}

/// A [`ResponseError`]'s own rendering, behind the box that erased its type.
fn render_typed<E>(error: &BoxError, status: StatusCode) -> Response
where
    E: ResponseError + Error + Send + Sync + 'static,
{
    match error.downcast_ref::<E>() {
        Some(error) => error.as_response(),
        // Unreachable: the box was filled with an `E` by `From<E>`.
        None => ProblemDetails::from_status(status).into_response(),
    }
}

impl fmt::Debug for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("HttpError");
        debug.field("status", &self.0.status);
        match &self.0.cause {
            Cause::Status => debug.field("cause", &"none"),
            Cause::Response(_) => debug.field("cause", &"a ready response"),
            Cause::Error(error) | Cause::Typed { error, .. } | Cause::Wrapped { error, .. } => {
                debug.field("cause", error)
            }
            Cause::Anyhow(error) => debug.field("cause", error),
        };
        debug.finish()
    }
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0.cause {
            Cause::Status | Cause::Response(_) => fmt::Display::fmt(&self.0.status, f),
            Cause::Error(error) | Cause::Typed { error, .. } | Cause::Wrapped { error, .. } => {
                fmt::Display::fmt(error, f)
            }
            Cause::Anyhow(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for HttpError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.0.cause {
            Cause::Error(error) | Cause::Typed { error, .. } | Cause::Wrapped { error, .. } => {
                Some(&**error)
            }
            Cause::Anyhow(error) => Some(error.as_ref()),
            Cause::Status | Cause::Response(_) => None,
        }
    }
}

impl<E: ResponseError + Error + Send + Sync + 'static> From<E> for HttpError {
    fn from(error: E) -> Self {
        let status = error.status();
        Self::with(
            status,
            Cause::Typed {
                error: Box::new(error),
                render: render_typed::<E>,
            },
        )
    }
}

impl From<StatusCode> for HttpError {
    fn from(status: StatusCode) -> Self {
        Self::from_status(status)
    }
}

impl From<anyhow::Error> for HttpError {
    fn from(error: anyhow::Error) -> Self {
        Self::with(StatusCode::INTERNAL_SERVER_ERROR, Cause::Anyhow(error))
    }
}

impl From<Infallible> for HttpError {
    fn from(never: Infallible) -> Self {
        match never {}
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        HttpError::into_response(self)
    }
}

/// Why [`RouteTemplate::parse`](crate::RouteTemplate::parse) refuses a
/// template: the template, then the reason — the sentence `#[routes]` prints
/// after its site, word for word.
///
/// ```
/// use nest_rs_http::RouteTemplate;
///
/// let refused = RouteTemplate::parse("/files/*rest").unwrap_err();
/// assert_eq!(refused.template(), "/files/*rest");
/// assert_eq!(
///     refused.to_string(),
///     "`/files/*rest`: `*rest` is the 6.x catch-all syntax — write `/files/{*rest}`",
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{written}`: {refusal}")]
pub struct RouteTemplateError {
    written: String,
    refusal: TemplateRefusal,
}

impl RouteTemplateError {
    pub(crate) fn new(written: &str, refusal: TemplateRefusal) -> Self {
        Self {
            written: written.to_owned(),
            refusal,
        }
    }

    /// The template as it was written.
    pub fn template(&self) -> &str {
        &self.written
    }
}

/// What the route grammar refuses, worded as `nest_rs_codegen::RoutePath`
/// words it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum TemplateRefusal {
    #[error("`:{name}` is the 6.x parameter syntax — write `{rewrite}`")]
    Parameter6x { name: String, rewrite: String },
    #[error("a `:` is the 6.x parameter syntax, and names none — a parameter is `{{name}}`")]
    UnnamedParameter6x,
    #[error("`{written}` is the 6.x catch-all syntax — write `{rewrite}`")]
    CatchAll6x { written: String, rewrite: String },
    #[error(
        "a route parameter carries no pattern — parse it with its type (`Path<u64>`) or a pipe"
    )]
    Pattern,
    #[error("`{{{inside}}}` ends its segment — a parameter reads up to the next `/`")]
    EndsItsSegment { inside: String },
    #[error("`{{{inside}}}` comes last — a catch-all takes the rest of the path")]
    ComesLast { inside: String },
    #[error("`{{}}` names no parameter — write `{{name}}`")]
    Unnamed,
    #[error("`{{*}}` names no catch-all — write `{{*rest}}`")]
    UnnamedCatchAll,
    #[error("a `{{` opens a parameter that no `}}` closes — a literal brace is `{{{{`")]
    Unclosed,
    #[error("a `}}` closes no parameter — a literal brace is `}}}}`")]
    Unopened,
    #[error(
        "it is mounted at one literal address, so it holds no parameter, no brace, and no `:`, \
         `*` or `<`"
    )]
    NotLiteral,
}

/// What the route table refuses at boot, each refusal naming who registered
/// what: the transport's `configure` fails with it.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RouteError {
    /// A path outside the route grammar, or a template where one literal
    /// address is due.
    #[error("{owner} mounts {refused}")]
    Template {
        owner: String,
        refused: RouteTemplateError,
    },
    /// The global prefix outside the grammar.
    #[error("{owner} {refused}")]
    Prefix {
        owner: String,
        refused: RouteTemplateError,
    },
    /// Two owners of one exclusive namespace.
    #[error(
        "duplicate {kind} {template:?}: {first} and {owner} both mount there — a {kind} is its \
         exclusive namespace; {remedy}"
    )]
    Exclusive {
        kind: &'static str,
        template: String,
        first: String,
        owner: String,
        remedy: String,
    },
    /// Two spellings of one address.
    #[error(
        "duplicate route path: {first} mounts {first_template:?} and {second} mounts \
         {second_template:?}, one address — an address is mounted once and each parameter \
         binds by its name; give each controller route a distinct full path"
    )]
    OneAddress {
        first: String,
        first_template: String,
        second: String,
        second_template: String,
    },
    /// A self-mount's path a controller already holds.
    #[error(
        "duplicate mount path {path:?}: controller {controller} and {label} endpoint {owner} both \
         mount there — a mount path is its owner's exclusive namespace; give each one a distinct \
         path"
    )]
    MountPath {
        path: String,
        controller: String,
        label: String,
        owner: String,
    },
    /// One address claimed by two registrants.
    #[error(
        "{second} registers {template:?}, which {first} already answers — an address is mounted \
         once, by one owner; give each one a distinct path"
    )]
    SharedAddress {
        template: String,
        first: String,
        second: String,
    },
    /// One method, or every method, registered twice at one address.
    #[error(
        "{owner} registers {template:?} twice for {method} — an address answers each method \
         once, and an address answering every method answers no other registration"
    )]
    Twice {
        owner: String,
        method: String,
        template: String,
    },
    /// A self-mount registering a path it did not declare.
    #[error(
        "{owner} registers {template:?}, which it does not declare — a self-mount answers at its \
         path, below it, and at what `also_mounts` declares ({declared}); declare the path with \
         `also_mounts`"
    )]
    Undeclared {
        owner: String,
        template: String,
        declared: String,
    },
    /// A site whose layers did not compose: it would run without one it
    /// declares, or with its guards out of phase order.
    #[error("{site} cannot mount: its layers do not compose")]
    Layers {
        site: String,
        #[source]
        source: BoxError,
    },
    /// The fallback's path outside the grammar.
    #[error("the router fallback {owner} answers at {refused}")]
    FallbackPath {
        owner: String,
        refused: RouteTemplateError,
    },
    /// The fallback under a prefix someone owns.
    #[error(
        "the router fallback {owner} answers at {path:?}, under {prefix:?}, which {prefix_owner} \
         owns: the route table claims everything there, so the fallback would answer nothing — \
         give it a path outside {prefix:?}"
    )]
    FallbackUnder {
        owner: String,
        path: String,
        prefix: String,
        prefix_owner: String,
    },
    /// Two fallbacks.
    #[error(
        "two router fallbacks: {first} and {second} — the router has one fallback, which answers \
         whatever nothing else claims; import one of them"
    )]
    TwoFallbacks { first: String, second: String },
    /// A template the router library refused although the grammar read it.
    #[error("the router cannot mount {template:?} for {owner}: {reason}")]
    Router {
        template: String,
        owner: String,
        reason: String,
    },
}

/// A path parameter whose percent-decoded bytes are not UTF-8: said with its
/// name, never its value.
#[derive(Debug, thiserror::Error)]
#[error("the path parameter `{name}` is not UTF-8 once percent-decoded")]
pub(crate) struct PathParameterError {
    pub(crate) name: std::sync::Arc<str>,
}

/// What [`Json`](crate::Json) refused.
#[derive(Debug, thiserror::Error)]
pub(crate) enum JsonError {
    /// No `Content-Type`, or one that is not JSON: `415`.
    #[error("{}", MediaType("`application/json` or a `+json` type"))]
    MediaType,
    /// The body is not the JSON the type reads: `400`, said without its value.
    #[error("the JSON body does not decode: {0}")]
    Malformed(#[source] serde_json::Error),
}

impl ResponseError for JsonError {
    fn status(&self) -> StatusCode {
        match self {
            Self::MediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::Malformed(_) => StatusCode::BAD_REQUEST,
        }
    }
}

/// What [`Query`](crate::Query) refused: the query string is not what the type
/// reads, `400`, in serde's words said without its value.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct QueryError(#[source] pub(crate) serde::de::value::Error);

impl ResponseError for QueryError {
    fn status(&self) -> StatusCode {
        StatusCode::BAD_REQUEST
    }
}

/// What [`Form`](crate::Form) refused.
#[derive(Debug, thiserror::Error)]
pub(crate) enum FormError {
    /// A body whose `Content-Type` is missing or not a form's: `415`.
    #[error("{}", MediaType("`application/x-www-form-urlencoded`"))]
    MediaType,
    /// The form is not what the type reads: `400`, said without its value.
    #[error("url decode: {0}")]
    Malformed(#[source] serde::de::value::Error),
}

impl ResponseError for FormError {
    fn status(&self) -> StatusCode {
        match self {
            Self::MediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::Malformed(_) => StatusCode::BAD_REQUEST,
        }
    }
}

/// What [`Multipart`](crate::Multipart) refused.
#[derive(Debug, thiserror::Error)]
pub(crate) enum MultipartError {
    /// No `Content-Type`, or one that is not `multipart/form-data`: `415`.
    #[error("{}", MediaType("`multipart/form-data`"))]
    MediaType,
    /// The body does not parse as parts, or a part's text is not UTF-8:
    /// `400`. What the parser said is kept for the log, never answered: it
    /// may name what the client sent.
    #[error("the multipart body is malformed")]
    Malformed(#[source] BoxError),
}

impl ResponseError for MultipartError {
    fn status(&self) -> StatusCode {
        match self {
            Self::MediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::Malformed(_) => StatusCode::BAD_REQUEST,
        }
    }
}

/// What a body extractor says of a `Content-Type` it does not read: the media
/// type it expected, never the one sent, which the client chose.
struct MediaType(&'static str);

impl fmt::Display for MediaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the request's content type is not {}", self.0)
    }
}

/// What a path binding can fail on. A value is never carried: a refusal names
/// the parameter and what its type reads.
#[derive(Debug)]
pub(crate) enum PathError {
    /// The parameter is present but its text is not what the field's type
    /// reads: `400`.
    Malformed {
        name: String,
        expected: Cow<'static, str>,
    },
    /// serde recognised the shape and refused the content, before the
    /// parameter was known; [`against`](Self::against) names it.
    Unexpected(Cow<'static, str>),
    /// A type's own refusal, whose message may quote the value: only the fact
    /// is carried.
    Refused,
    /// The handler reads a parameter the route does not declare: a
    /// handler-signature mistake, `500`.
    Undeclared(String),
    /// The route declares a parameter the handler's type refuses to leave
    /// unread: a handler-signature mistake, `500`.
    Unread(String),
    /// The handler reads another number of parameters than the route
    /// declares: a handler-signature mistake, `500`.
    Arity { declared: usize, read: usize },
    /// The handler reads a parameter as a shape one segment cannot hold: a
    /// handler-signature mistake, `500`.
    Unsupported(&'static str),
}

impl PathError {
    pub(crate) fn malformed(name: &str, expected: impl Into<Cow<'static, str>>) -> Self {
        Self::Malformed {
            name: name.to_owned(),
            expected: expected.into(),
        }
    }

    /// Attribute a content refusal to the parameter it was read from: serde's
    /// constructors cannot know it. Idempotent on named variants.
    pub(crate) fn against(self, name: &str) -> Self {
        match self {
            Self::Unexpected(expected) => Self::malformed(name, expected),
            Self::Refused => Self::malformed(name, ACCEPTED),
            named => named,
        }
    }
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed { name, expected } => {
                write!(f, "path parameter `{name}` is not {expected}")
            }
            Self::Unexpected(expected) => write!(f, "path parameter is not {expected}"),
            Self::Refused => write!(f, "path parameter is not {ACCEPTED}"),
            Self::Undeclared(name) => {
                write!(
                    f,
                    "the handler reads `{name}`, a path parameter its route does not declare"
                )
            }
            Self::Unread(name) => {
                write!(
                    f,
                    "the route declares `{name}`, a path parameter the handler does not read"
                )
            }
            Self::Arity { declared, read } => write!(
                f,
                "the handler reads {read} path parameters where its route declares {declared}",
            ),
            Self::Unsupported(shape) => {
                write!(
                    f,
                    "the handler reads a path parameter as {shape}, which one segment cannot hold"
                )
            }
        }
    }
}

impl serde::de::Error for PathError {
    /// The message is dropped unread: it is the type's, and may quote the
    /// parameter it refused.
    fn custom<T: fmt::Display>(_msg: T) -> Self {
        Self::Refused
    }

    fn missing_field(field: &'static str) -> Self {
        Self::Undeclared(field.to_owned())
    }

    /// serde's default message opens with the value read off the path.
    fn unknown_variant(_variant: &str, expected: &'static [&'static str]) -> Self {
        Self::Unexpected(one_of(expected).into())
    }

    /// A key is a parameter's name, the route's own: a struct refusing it
    /// leaves a declared parameter unread.
    fn unknown_field(field: &str, _expected: &'static [&'static str]) -> Self {
        Self::Unread(field.to_owned())
    }

    /// serde's default quotes the value in full.
    fn invalid_value(
        _unexpected: serde::de::Unexpected<'_>,
        expected: &dyn serde::de::Expected,
    ) -> Self {
        Self::Unexpected(expected.to_string().into())
    }

    /// serde's default quotes the value in full.
    fn invalid_type(
        _unexpected: serde::de::Unexpected<'_>,
        expected: &dyn serde::de::Expected,
    ) -> Self {
        Self::Unexpected(expected.to_string().into())
    }
}

impl std::error::Error for PathError {}

impl ResponseError for PathError {
    fn status(&self) -> StatusCode {
        match self {
            Self::Malformed { .. } | Self::Unexpected(_) | Self::Refused => StatusCode::BAD_REQUEST,
            Self::Undeclared(_) | Self::Unread(_) | Self::Arity { .. } | Self::Unsupported(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}

/// What an [`OriginRefused`] says, to the client and in its error: never the
/// origin sent.
pub(crate) const ORIGIN_REFUSED: &str =
    "the request's origin is neither this server's own nor one its CORS policy lists";

/// A browser `Origin` the [`OriginPolicy`](crate::__private::OriginPolicy)
/// refused: it answers `403` with a problem document that names no origin, and
/// keeps no value the request carried — the `Origin` sent and the `Host` it was
/// compared to are both the client's — only why, by kind, for the edge's `warn`
/// ([`__origin_refused!`](crate::__origin_refused)).
#[derive(Debug, thiserror::Error)]
#[error("{}", ORIGIN_REFUSED)]
pub struct OriginRefused {
    reason: OriginRefusedReason,
    scheme: Option<OriginScheme>,
    trusted_proxy: bool,
}

impl OriginRefused {
    pub(crate) fn new(
        reason: OriginRefusedReason,
        scheme: Option<OriginScheme>,
        trusted_proxy: bool,
    ) -> Self {
        Self {
            reason,
            scheme,
            trusted_proxy,
        }
    }

    /// The first check the `Origin` failed.
    pub fn reason(&self) -> OriginRefusedReason {
        self.reason
    }

    /// The sent origin's scheme, when the field parsed as a scheme, host and
    /// port.
    pub fn scheme(&self) -> Option<OriginScheme> {
        self.scheme
    }

    /// Whether the request came from a peer `trusted_proxies` lists, whose
    /// forwarded host and scheme the request's own origin was read from.
    pub fn trusted_proxy(&self) -> bool {
        self.trusted_proxy
    }

    /// The settings that would admit the origin, each named with the
    /// deployment's prefix; `None` when no setting admits it, the field being
    /// no origin at all.
    pub fn hint(&self) -> Option<String> {
        let origins = nest_rs_config::var_name("http", "CORS_ORIGINS");
        match self.reason {
            OriginRefusedReason::OriginNotVisibleAscii
            | OriginRefusedReason::OriginNotSerialized
            | OriginRefusedReason::OriginRepeated => None,
            OriginRefusedReason::OriginNull => Some(format!(
                "an opaque origin is admitted only where {origins} lists `null`"
            )),
            _ if self.trusted_proxy => Some(format!("list the page's origin in {origins}")),
            _ => Some(format!(
                "list the page's origin in {origins}, or the proxy in front of this server \
                 in {}",
                nest_rs_config::var_name("http", "TRUSTED_PROXIES"),
            )),
        }
    }
}

/// Why an `Origin` was refused, in RFC 6454's vocabulary: the first check it
/// failed — the field itself, then its comparison with the request's own
/// origin, scheme, host and port in §5's order. Each word is the variant in
/// snake case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OriginRefusedReason {
    /// The field holds a byte outside visible ASCII.
    OriginNotVisibleAscii,
    /// The field is visible ASCII but no origin serialization (§6.2).
    OriginNotSerialized,
    /// The request carries the field more than once (§7.3).
    OriginRepeated,
    /// The field is `null`, an opaque origin, which no listed origin admits.
    OriginNull,
    /// The scheme is not the request's own.
    SchemeDiffers,
    /// The host is not the request's own.
    HostDiffers,
    /// The port is not the request's own.
    PortDiffers,
    /// The request named no host, so it has no origin of its own.
    HostMissing,
}

impl OriginRefusedReason {
    /// The word the line files.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OriginNotVisibleAscii => "origin_not_visible_ascii",
            Self::OriginNotSerialized => "origin_not_serialized",
            Self::OriginRepeated => "origin_repeated",
            Self::OriginNull => "origin_null",
            Self::SchemeDiffers => "scheme_differs",
            Self::HostDiffers => "host_differs",
            Self::PortDiffers => "port_differs",
            Self::HostMissing => "host_missing",
        }
    }
}

/// Files `refused` at `warn` on `target`, said by kind: the one line each edge
/// with a browser-origin check files on its own target.
#[doc(hidden)]
#[macro_export]
macro_rules! __origin_refused {
    (target: $target:expr, $refused:expr $(,)?) => {{
        let __refused: &$crate::__private::OriginRefused = $refused;
        $crate::__private::tracing::warn!(
            target: $target,
            reason = __refused.reason().as_str(),
            scheme = __refused
                .scheme()
                .map($crate::__private::OriginScheme::as_str),
            trusted_proxy = __refused.trusted_proxy(),
            hint = __refused.hint().as_deref(),
            "browser origin refused",
        );
    }};
}

impl ResponseError for OriginRefused {
    fn status(&self) -> StatusCode {
        StatusCode::FORBIDDEN
    }

    fn as_response(&self) -> Response {
        problem_response(&Problem::new(403, code::FORBIDDEN).with_detail(ORIGIN_REFUSED))
    }
}

/// Why a WebSocket opening handshake was refused (RFC 6455 §4.2.1): each
/// answers a problem document that names what the handshake lacks, never a
/// value the request sent.
#[cfg(feature = "websocket")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HandshakeRefused {
    /// Not a `GET` — a `HEAD` the route table answers through its `GET` entry
    /// included: `400`. The `405` naming what an address serves is the route
    /// table's, which knows it.
    #[error("a WebSocket handshake is a GET request")]
    MethodNotGet,
    /// `Upgrade` lists no `websocket`: `400`.
    #[error("a WebSocket handshake carries `Upgrade: websocket`")]
    UpgradeNotWebSocket,
    /// `Connection` lists no `upgrade`: `400`.
    #[error("a WebSocket handshake carries `Connection: Upgrade`")]
    ConnectionNotUpgrade,
    /// `Sec-WebSocket-Key` is absent, repeated, or not 16 bytes in base64:
    /// `400`.
    #[error("a WebSocket handshake carries one `Sec-WebSocket-Key` of 16 bytes in base64")]
    KeyMalformed,
    /// `Sec-WebSocket-Version` is absent, repeated, or not `13`: `426` with
    /// `Sec-WebSocket-Version: 13` (§4.4).
    #[error("this server speaks WebSocket version 13")]
    VersionUnsupported,
    /// The connection under the request cannot switch protocols — HTTP/1.0,
    /// or no connection at all, as in process: `426`.
    #[error("this connection cannot switch protocols; a WebSocket opens over HTTP/1.1")]
    NotUpgradable,
    /// `Sec-WebSocket-Protocol` offers none of the subprotocols the endpoint
    /// speaks, listed here: `400`.
    #[error("a WebSocket handshake here offers one of the subprotocols {}", .0.join(", "))]
    SubprotocolNotOffered(&'static [&'static str]),
}

#[cfg(feature = "websocket")]
impl ResponseError for HandshakeRefused {
    fn status(&self) -> StatusCode {
        match self {
            Self::VersionUnsupported | Self::NotUpgradable => StatusCode::UPGRADE_REQUIRED,
            Self::MethodNotGet
            | Self::UpgradeNotWebSocket
            | Self::ConnectionNotUpgrade
            | Self::KeyMalformed
            | Self::SubprotocolNotOffered(_) => StatusCode::BAD_REQUEST,
        }
    }

    fn as_response(&self) -> Response {
        use http::HeaderValue;
        use http::header::{CONNECTION, SEC_WEBSOCKET_VERSION, UPGRADE};

        let mut response = ProblemDetails::from_status(self.status())
            .with_detail(self.to_string())
            .into_response();
        let headers = response.headers_mut();
        match self {
            // RFC 9110 §15.5.22: a `426` names the protocol to switch to.
            Self::VersionUnsupported | Self::NotUpgradable => {
                headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
                headers.insert(CONNECTION, HeaderValue::from_static("upgrade"));
                if *self == Self::VersionUnsupported {
                    headers.insert(SEC_WEBSOCKET_VERSION, HeaderValue::from_static("13"));
                }
            }
            Self::MethodNotGet
            | Self::UpgradeNotWebSocket
            | Self::ConnectionNotUpgrade
            | Self::KeyMalformed
            | Self::SubprotocolNotOffered(_) => {}
        }
        response
    }
}

/// What a header a type refused in its own words is said not to be.
const ACCEPTED: &str = "a value its type accepts";

/// What a header binding can fail on. Each variant names the header; none
/// carries its value.
#[derive(Debug)]
pub(crate) enum HeaderError {
    /// A field without a default (i.e. not `Option<_>`) whose header is absent.
    Missing(String),
    /// The header is present but its value is not what the field's type needs.
    Malformed {
        name: String,
        expected: Cow<'static, str>,
    },
    /// serde recognised the shape and refused the content. It carries what was
    /// *expected*, never what was read; [`against`](Self::against) names the header.
    Unexpected(Cow<'static, str>),
    /// A type's own refusal (`deserialize_with`, a `Deserialize` impl calling
    /// `custom`); its message may quote the value, so only the fact is carried.
    Refused,
    /// A field naming something that cannot be a header name — the developer's
    /// mistake, surfaced on a request.
    NotAHeaderName(String),
}

impl HeaderError {
    pub(crate) fn not_a_header_name(field: &str) -> Self {
        Self::NotAHeaderName(field.to_owned())
    }

    pub(crate) fn malformed(name: &str, expected: impl Into<Cow<'static, str>>) -> Self {
        Self::Malformed {
            name: name.to_owned(),
            expected: expected.into(),
        }
    }

    /// Attribute a content refusal to the header it was read from: serde's
    /// constructors are static and cannot know it. Idempotent on named variants.
    pub(crate) fn against(self, name: &str) -> Self {
        match self {
            Self::Unexpected(expected) => Self::malformed(name, expected),
            Self::Refused => Self::malformed(name, ACCEPTED),
            named => named,
        }
    }
}

impl fmt::Display for HeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(name) => write!(f, "missing required header `{name}`"),
            Self::Malformed { name, expected } => {
                write!(f, "header `{name}` is not {expected}")
            }
            Self::Unexpected(expected) => write!(f, "header value is not {expected}"),
            Self::Refused => write!(f, "header value is not {ACCEPTED}"),
            Self::NotAHeaderName(field) => write!(
                f,
                "`{field}` is not a valid header name, so no request can carry it — fix the \
                 field's `#[serde(rename = \"…\")]`",
            ),
        }
    }
}

impl serde::de::Error for HeaderError {
    /// The message is dropped unread: it is the type's, and may quote the
    /// header it refused.
    fn custom<T: fmt::Display>(_msg: T) -> Self {
        Self::Refused
    }

    fn missing_field(field: &'static str) -> Self {
        Self::Missing(field.to_owned())
    }

    /// serde's default message opens with the value read off the wire, which
    /// must not be echoed.
    fn unknown_variant(_variant: &str, expected: &'static [&'static str]) -> Self {
        Self::Unexpected(one_of(expected).into())
    }

    /// One rule over every value-interpolating constructor.
    fn unknown_field(_field: &str, expected: &'static [&'static str]) -> Self {
        Self::Unexpected(one_of(expected).into())
    }

    /// serde's default quotes the value in full.
    fn invalid_value(
        _unexpected: serde::de::Unexpected<'_>,
        expected: &dyn serde::de::Expected,
    ) -> Self {
        Self::Unexpected(expected.to_string().into())
    }

    /// serde's default quotes the value in full.
    fn invalid_type(
        _unexpected: serde::de::Unexpected<'_>,
        expected: &dyn serde::de::Expected,
    ) -> Self {
        Self::Unexpected(expected.to_string().into())
    }
}

impl std::error::Error for HeaderError {}

nest_rs_core::inventory::submit! { nest_rs_core::SafeLink::new(say_h2) }

/// A GOAWAY error of h2's, said as h2 says it without the debug data its
/// `Display` appends — the peer's own bytes; every other h2 error says nothing
/// it was sent.
fn say_h2(link: &(dyn Error + 'static)) -> Option<String> {
    let error = link.downcast_ref::<h2::Error>()?;
    if !error.is_go_away() {
        return None;
    }
    let initiator = if error.is_remote() {
        "received"
    } else if error.is_library() {
        "detected"
    } else {
        "sent by user"
    };
    let reason = error
        .reason()
        .map_or_else(|| "no reason".to_owned(), |reason| reason.to_string());
    Some(format!("connection error {initiator}: {reason}"))
}

#[cfg(test)]
mod tests {
    use http::{HeaderValue, header};

    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct Marker(&'static str);

    #[derive(Debug, Clone, PartialEq)]
    struct Untouched;

    #[derive(Debug, thiserror::Error, PartialEq)]
    #[error("order {0} is already paid")]
    struct AlreadyPaid(u32);

    async fn problem_of(response: Response) -> serde_json::Value {
        let bytes = response.into_body().into_bytes().await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn extensions_on_an_error_overwrite_the_response_extension_when_rendered() {
        let ready = Response::builder()
            .status(StatusCode::CONFLICT)
            .extension(Marker("response"))
            .extension(Untouched)
            .finish();
        let mut error = HttpError::from_response(ready);
        error.extensions_mut().insert(Marker("error"));

        let rendered = error.into_response();

        assert_eq!(
            rendered.extensions().get::<Marker>(),
            Some(&Marker("error"))
        );
        assert_eq!(
            rendered.extensions().get::<Untouched>(),
            Some(&Untouched),
            "a value of another type stays",
        );
    }

    #[test]
    fn downcast_round_trips_the_source() {
        let error = HttpError::new(AlreadyPaid(7), StatusCode::CONFLICT);
        assert_eq!(error.downcast_ref::<AlreadyPaid>(), Some(&AlreadyPaid(7)));

        let error = error.downcast::<std::fmt::Error>().unwrap_err();
        assert_eq!(
            error.status(),
            StatusCode::CONFLICT,
            "a miss hands the error back"
        );
        assert_eq!(error.downcast::<AlreadyPaid>().ok(), Some(AlreadyPaid(7)));

        let typed = HttpError::from(BodyError::NotUtf8);
        assert!(typed.is::<BodyError>());
        assert!(matches!(
            typed.downcast::<BodyError>(),
            Ok(BodyError::NotUtf8)
        ));

        let from_anyhow = HttpError::from(anyhow::Error::new(AlreadyPaid(9)));
        assert_eq!(
            from_anyhow.downcast::<AlreadyPaid>().ok(),
            Some(AlreadyPaid(9))
        );

        assert!(!HttpError::from_status(StatusCode::NOT_FOUND).is::<AlreadyPaid>());
    }

    #[tokio::test]
    async fn an_error_renders_a_problem_document_without_a_round_trip() {
        let decode = serde_json::from_str::<u32>("\"hunter2\"").unwrap_err();
        let rendered = HttpError::new(decode, StatusCode::BAD_REQUEST).into_response();
        assert_eq!(rendered.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            rendered.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
        let problem = problem_of(rendered).await;
        assert_eq!(problem["status"], 400);
        let detail = problem["detail"].as_str().unwrap();
        assert!(detail.contains("invalid type"), "{detail}");
        assert!(
            !detail.contains("hunter2"),
            "a 4xx detail never quotes the value: {detail}"
        );

        let failure = std::io::Error::other("connection to db-primary refused");
        let rendered = HttpError::new(failure, StatusCode::BAD_GATEWAY).into_response();
        assert_eq!(rendered.status(), StatusCode::BAD_GATEWAY);
        let problem = problem_of(rendered).await;
        assert_eq!(problem["status"], 502);
        assert!(
            problem.get("detail").is_none(),
            "a 5xx answers without a detail"
        );

        let rendered = HttpError::from_status(StatusCode::UNAUTHORIZED).into_response();
        assert_eq!(rendered.headers()[header::WWW_AUTHENTICATE], "Bearer");
        assert_eq!(
            rendered.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
    }

    #[tokio::test]
    async fn a_response_error_renders_its_own_answer() {
        let refused = HttpError::from(BodyError::TooLarge { limit: 64 }).into_response();
        assert_eq!(refused.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            problem_of(refused).await["detail"],
            "the body is larger than 64 bytes"
        );

        let custom = crate::ProblemDetails::conflict().with_type("urn:problem:paid");
        let rendered = HttpError::from(custom).into_response();
        assert_eq!(rendered.status(), StatusCode::CONFLICT);
        assert_eq!(problem_of(rendered).await["type"], "urn:problem:paid");

        let teapot = Response::builder()
            .status(StatusCode::IM_A_TEAPOT)
            .header(header::RETRY_AFTER, HeaderValue::from_static("5"))
            .finish();
        let rendered = HttpError::from_response(teapot).into_response();
        assert_eq!(rendered.status(), StatusCode::IM_A_TEAPOT);
        assert_eq!(rendered.headers()[header::RETRY_AFTER], "5");
    }

    #[test]
    fn an_http_error_is_a_std_error_with_its_cause_as_source() {
        let error: Box<dyn Error + Send + Sync> =
            Box::new(HttpError::new(AlreadyPaid(3), StatusCode::CONFLICT));

        assert_eq!(error.to_string(), "order 3 is already paid");
        let source = error.source().expect("the cause is the source");
        assert_eq!(source.downcast_ref::<AlreadyPaid>(), Some(&AlreadyPaid(3)));
        assert_eq!(
            nest_rs_core::error_message(&*error),
            "order 3 is already paid",
            "the chain says the cause once",
        );
        assert!(HttpError::from_status(StatusCode::GONE).source().is_none());
    }

    #[tokio::test]
    async fn an_anyhow_error_answers_an_opaque_500_and_keeps_its_chain_for_the_log() {
        let failure = anyhow::anyhow!("pool exhausted on db-primary").context("loading order 42");
        let error = HttpError::from(failure);

        assert_eq!(error.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let logged = nest_rs_core::error_message(&error);
        assert!(logged.contains("loading order 42"), "{logged}");
        assert!(logged.contains("pool exhausted on db-primary"), "{logged}");

        let rendered = error.into_response();
        assert_eq!(rendered.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = rendered.into_body().into_bytes().await.unwrap();
        let body = std::str::from_utf8(&bytes).unwrap();
        assert!(
            !body.contains("db-primary"),
            "nothing of the chain reaches the wire: {body}"
        );
        assert!(!body.contains("order 42"), "{body}");
    }
}
