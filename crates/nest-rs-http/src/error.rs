//! Every error type of the HTTP edge: the vocabulary's ([`BodyError`],
//! [`HttpError`], [`ResponseError`]) and the header binding's.

use std::borrow::Cow;
use std::convert::Infallible;
use std::error::Error;
use std::fmt;
use std::time::Duration;

use http::{Extensions, StatusCode};

use crate::headers::one_of;
use crate::problem::ProblemDetails;
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

/// How a cause answers on the wire.
type Render = fn(BoxError, StatusCode) -> Response;

enum Cause {
    /// Nothing beneath the status.
    Status,
    /// A response answered as it is.
    Response(Response),
    /// An error rendered as a problem document of the status.
    Error(BoxError),
    /// A [`ResponseError`], rendered by its own `as_response`.
    Typed { error: BoxError, render: Render },
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
            Cause::Typed { error, render } | Cause::Wrapped { error, render } => {
                render(error, status)
            }
        };
        response.extensions_mut().extend(extensions);
        response
    }
}

/// A [`ResponseError`]'s own rendering, behind the box that erased its type.
fn render_typed<E>(error: BoxError, status: StatusCode) -> Response
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
