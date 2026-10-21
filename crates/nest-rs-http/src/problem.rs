//! [RFC 9457](https://www.rfc-editor.org/rfc/rfc9457.html) Problem Details for HTTP APIs,
//! the single error format at the HTTP boundary.
//!
//! A handler returns `Err(ProblemDetails::not_found().with_detail("…"))`;
//! [`normalize_error_response`] lifts any leftover plain-text transport error (an
//! unmounted-route 404, a 413) onto the same `application/problem+json` envelope.

use http::{HeaderValue, StatusCode, header};
use nest_rs_core::problem::code;
use nest_rs_core::{Code, DecodeError, Problem};
use poem::error::{ParseQueryError, ResponseError};
use poem::{IntoResponse, Response};
use serde::Serialize;

use crate::HttpError;

// Defined beside the 401 that carries it, so `ProblemDetails` reaches no
// poem-typed file; `challenge` re-exports it as RFC 6750's grammar.
/// The scheme name, matched case-insensitively per RFC 7235 §2.1.
pub const BEARER: &str = "Bearer";

/// The extension member a problem document carries its [`Code`] in.
const CODE: &str = "code";

/// The response a [`Problem`] answers: its document, and `Retry-After` when it
/// names a wait (RFC 9110 §10.2.3).
pub(crate) fn problem_response(problem: &Problem) -> crate::Response {
    let mut response = crate::IntoResponse::into_response(ProblemDetails::from_problem(problem));
    if let Some(seconds) = problem.retry_after() {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(seconds));
    }
    response
}

/// Body of an `application/problem+json` response.
///
/// Every problem the framework builds from a status alone has one `type`,
/// `about:blank`, and the status's reason phrase as its `title`: RFC 9457
/// §4.2.1's spelling of a problem that means nothing beyond its status. A
/// `5xx` adds the `code` member of its class and nothing else, so a server
/// failure reads the same whichever part of the framework answered it. A
/// problem type of an app's own, which a client may key on, is set with
/// [`with_type`](Self::with_type).
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct ProblemDetails {
    /// URI identifying the problem type. `about:blank` means the problem says
    /// nothing beyond its `status`, which a client keys on instead.
    #[serde(rename = "type")]
    pub type_uri: String,
    /// Short, human-readable summary of the problem type (stable per `type`).
    pub title: String,
    /// The HTTP status code, mirrored into the body as a number.
    #[serde(serialize_with = "serialize_status")]
    pub status: StatusCode,
    /// Human-readable explanation specific to this occurrence; omitted when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// URI identifying this specific occurrence (e.g. the request path); omitted
    /// when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    /// RFC 9457 **extension members**, flattened alongside the standard fields.
    #[serde(flatten)]
    pub extensions: serde_json::Map<String, serde_json::Value>,
}

fn serialize_status<S: serde::Serializer>(
    status: &StatusCode,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_u16(status.as_u16())
}

impl ProblemDetails {
    /// A problem whose `detail` is `err`'s own sentence, any decode failure its
    /// chain holds said without its value ([`DecodeError::redact`]).
    pub fn from_error(
        status: StatusCode,
        title: impl Into<String>,
        err: &(dyn std::error::Error + 'static),
    ) -> Self {
        Self::from_status(status)
            .with_title(title)
            .with_detail(DecodeError::redact(&err.to_string(), Some(err)).into_owned())
    }

    /// The HTTP document of a [`Problem`]: its status's own problem, its
    /// [`detail`](Problem::detail) on a `4xx`, and its code as the `code`
    /// extension member.
    ///
    /// ```
    /// use nest_rs_core::Problem;
    /// use nest_rs_core::problem::code;
    /// use nest_rs_http::ProblemDetails;
    ///
    /// let document = ProblemDetails::from_problem(
    ///     &Problem::new(409, code::CONFLICT).with_detail("the handle is taken"),
    /// );
    /// assert_eq!(document.status.as_u16(), 409);
    /// assert_eq!(document.detail.as_deref(), Some("the handle is taken"));
    /// assert_eq!(document.extensions["code"], "CONFLICT");
    /// ```
    pub fn from_problem(problem: &Problem) -> Self {
        // `Problem::new` holds its status to 400..=599, every one of them valid.
        let status =
            StatusCode::from_u16(problem.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let document = Self::from_status(status).with_extension(CODE, problem.code().as_str());
        match problem.detail() {
            Some(detail) => document.with_detail(detail),
            None => document,
        }
    }

    /// The problem an error answers with `status`: the status's own problem,
    /// whose `detail` on a `4xx` is `error`'s sentence said without any value
    /// it quotes ([`DecodeError::redact`]), and which a `5xx` answers without
    /// one, so a driver's message never reaches the wire.
    pub(crate) fn answering(status: StatusCode, error: &(dyn std::error::Error + 'static)) -> Self {
        let problem = Self::from_status(status);
        if !status.is_client_error() {
            return problem;
        }
        let sentence = error.to_string();
        let said = DecodeError::redact(sentence.trim(), Some(error));
        if said.is_empty() {
            problem
        } else {
            problem.with_detail(said.into_owned())
        }
    }

    /// The problem a bare [`StatusCode`] answers: `about:blank` and the
    /// status's reason phrase (RFC 9457 §4.2.1); a `5xx` also carries the
    /// `code` of its class, the document [`from_problem`](Self::from_problem)
    /// gives that class's [`Problem`], so every server failure the framework
    /// does not describe answers one document.
    ///
    /// ```
    /// use nest_rs_http::{ProblemDetails, StatusCode};
    ///
    /// let failed = ProblemDetails::from_status(StatusCode::INTERNAL_SERVER_ERROR);
    /// assert_eq!(failed.extensions["code"], "INTERNAL");
    /// assert!(failed.detail.is_none());
    /// let busy = ProblemDetails::from_status(StatusCode::SERVICE_UNAVAILABLE);
    /// assert_eq!(busy.extensions["code"], "UNAVAILABLE");
    /// assert!(ProblemDetails::from_status(StatusCode::NOT_FOUND).extensions.is_empty());
    /// ```
    pub fn from_status(status: StatusCode) -> Self {
        let problem = Self {
            type_uri: "about:blank".into(),
            title: reason_phrase(status).into(),
            status,
            detail: None,
            instance: None,
            extensions: serde_json::Map::new(),
        };
        match server_error_class(status) {
            Some(class) => problem.with_extension(CODE, class.as_str()),
            None => problem,
        }
    }

    /// A `400 Bad Request` problem.
    pub fn bad_request() -> Self {
        Self::from_status(StatusCode::BAD_REQUEST)
    }

    /// A `401 Unauthorized` problem.
    pub fn unauthorized() -> Self {
        Self::from_status(StatusCode::UNAUTHORIZED)
    }

    /// A `403 Forbidden` problem.
    pub fn forbidden() -> Self {
        Self::from_status(StatusCode::FORBIDDEN)
    }

    /// A `404 Not Found` problem.
    pub fn not_found() -> Self {
        Self::from_status(StatusCode::NOT_FOUND)
    }

    /// A `409 Conflict` problem.
    pub fn conflict() -> Self {
        Self::from_status(StatusCode::CONFLICT)
    }

    /// A `422 Unprocessable Content` problem — well-formed but semantically invalid.
    pub fn unprocessable() -> Self {
        Self::from_status(StatusCode::UNPROCESSABLE_ENTITY)
    }

    /// A `500 Internal Server Error` problem, under the `INTERNAL` code: what
    /// every failure the client is not told of answers.
    pub fn internal() -> Self {
        Self::from_status(StatusCode::INTERNAL_SERVER_ERROR)
    }

    /// Set the occurrence-specific `detail` message.
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Set the `instance` URI identifying this occurrence.
    pub fn with_instance(mut self, instance: impl Into<String>) -> Self {
        self.instance = Some(instance.into());
        self
    }

    /// Override the `type` URI — e.g. to point at an app's own type registry.
    pub fn with_type(mut self, type_uri: impl Into<String>) -> Self {
        self.type_uri = type_uri.into();
        self
    }

    /// Override the human-readable `title`.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Attach an RFC 9457 **extension member**, serialized alongside the standard
    /// fields (e.g. `.with_extension("errors", json!({ "email": [...] }))`).
    pub fn with_extension(
        mut self,
        key: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.extensions.insert(key.into(), value.into());
        self
    }
}

/// The code a server failure answering `status` is known by: `UNAVAILABLE`
/// for a `503`, the server unable to serve the request in time, and
/// `INTERNAL` for any other `5xx` — core's two classes, as its
/// `Problem::client_message` reads them. `None` for a client error.
fn server_error_class(status: StatusCode) -> Option<Code> {
    match status {
        StatusCode::SERVICE_UNAVAILABLE => Some(code::UNAVAILABLE),
        status if status.is_server_error() => Some(code::INTERNAL),
        _ => None,
    }
}

/// The reason phrase RFC 9110 gives `status`, where the `http` crate still
/// says the RFC 7231 one.
fn reason_phrase(status: StatusCode) -> &'static str {
    match status {
        StatusCode::PAYLOAD_TOO_LARGE => "Content Too Large",
        StatusCode::UNPROCESSABLE_ENTITY => "Unprocessable Content",
        other => other.canonical_reason().unwrap_or("Error"),
    }
}

/// Normalize a response carrying a raw (non-problem) transport error onto the
/// single RFC-9457 `application/problem+json` envelope.
///
/// Only a `>= 400` response with a `text/plain` or empty body is rebuilt, keeping
/// its headers. Its text becomes `detail` only on a `4xx` the framework wrote
/// itself, its own sentence with any decode failure said without its value
/// ([`DecodeError::redact`]); any other text — a library's, a layer's, a
/// handler's — may quote what the client sent, and answers its status's
/// problem alone, as a `5xx` always does.
pub async fn normalize_error_response(resp: crate::Response) -> crate::Response {
    let status = resp.status();
    if !(status.is_client_error() || status.is_server_error()) {
        return resp;
    }
    if !is_raw_text(resp.headers()) {
        return resp;
    }

    let said = resp.extensions().get::<Said>().is_some();
    let (parts, body) = resp.into_parts();
    let mut problem = ProblemDetails::from_status(status);
    if status.is_client_error()
        && said
        && let Ok(bytes) = body.into_bytes().await
        && let Ok(text) = std::str::from_utf8(&bytes)
        && !text.trim().is_empty()
    {
        problem = problem.with_detail(DecodeError::redact(text.trim(), None).into_owned());
    }
    replacing(problem, &parts.headers, parts.extensions)
}

/// Marks a raw-text error body the framework wrote — its own sentence, already
/// said without the values it quoted — as the one text a problem's `detail`
/// may carry.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Said;

/// `response` with its raw-text body marked as a sentence the framework owns:
/// one a sibling crate checked quotes nothing the request carried, which the
/// normalizer then keeps as the problem's `detail`.
pub fn framework_sentence(mut response: crate::Response) -> crate::Response {
    response.extensions_mut().insert(Said);
    response
}

/// `problem` as the answer, keeping the head of the response it replaces: its
/// headers, and its extensions, which the layers outside it read.
fn replacing(
    problem: ProblemDetails,
    headers: &http::HeaderMap,
    extensions: http::Extensions,
) -> crate::Response {
    let mut response = crate::ResponseError::as_response(&problem);
    // The fresh body is uncompressed: a stale `Content-Encoding` would fail decoding.
    // An original header replaces the envelope's default (a bare `Bearer`), since a
    // client reads the first value; later occurrences append (`Set-Cookie`).
    let mut replaced = std::collections::HashSet::new();
    for (name, value) in headers.iter() {
        if name == header::CONTENT_TYPE
            || name == header::CONTENT_LENGTH
            || name == header::CONTENT_ENCODING
            || name == header::TRANSFER_ENCODING
        {
            continue;
        }
        if replaced.insert(name.clone()) {
            response.headers_mut().insert(name.clone(), value.clone());
        } else {
            response.headers_mut().append(name.clone(), value.clone());
        }
    }
    response.extensions_mut().extend(extensions);
    response
}

/// Whether a response with `headers` is a default rendering of an error —
/// `text/plain`, or no body type at all — rather than a problem or a body
/// somebody typed.
fn is_raw_text(headers: &http::HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_none_or(|ct| ct.starts_with("text/plain"))
}

/// An `Err` as the response it renders, by one rule: an error that renders
/// itself — a [`ResponseError`], a ready response, an
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
pub(crate) fn render_error(err: poem::Error) -> Response {
    let err = match crate::poem_bridge::take_carried(err) {
        Ok(carried) => return crate::poem_bridge::response_to_poem(carried.answer()),
        Err(err) => err,
    };
    if let Some(refused) = crate::poem_adapter::media_type_refusal(&err) {
        return crate::poem_bridge::response_to_poem(refused.answer());
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
            return crate::poem_bridge::response_to_poem(answer);
        }
        resp.set_body(body);
    }
    let mut resp = match redacted {
        Some(said) if is_raw_text(resp.headers()) => {
            let (parts, _) = resp.into_parts();
            Response::from_parts(parts, poem::Body::from_string(said))
        }
        _ => resp,
    };
    if is_raw_text(resp.headers()) {
        resp.extensions_mut().insert(Said);
    }
    resp
}

/// The outermost layer when something wraps the edge: every answer, an `Err`
/// included, normalized onto the problem document.
pub(crate) struct Normalized<E>(pub(crate) E);

impl<E: crate::Endpoint> crate::Endpoint for Normalized<E> {
    async fn call(&self, req: crate::Request) -> crate::Result<crate::Response> {
        let answer = self.0.call(req).await.unwrap_or_else(HttpError::answer);
        Ok(normalize_error_response(answer).await)
    }
}

impl std::fmt::Display for ProblemDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.status.as_u16(), self.title)?;
        if let Some(detail) = &self.detail {
            write!(f, " — {detail}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ProblemDetails {}

impl ResponseError for ProblemDetails {
    fn status(&self) -> StatusCode {
        self.status
    }

    fn as_response(&self) -> Response {
        crate::poem_bridge::response_to_poem(crate::ResponseError::as_response(self))
    }
}

impl IntoResponse for ProblemDetails {
    fn into_response(self) -> Response {
        self.as_response()
    }
}

impl crate::ResponseError for ProblemDetails {
    fn status(&self) -> StatusCode {
        self.status
    }

    fn as_response(&self) -> crate::Response {
        let body = serde_json::to_vec(self).unwrap_or_else(|_| b"{}".to_vec());
        let mut builder = crate::Response::builder()
            .status(self.status)
            .content_type(HeaderValue::from_static("application/problem+json"));
        // RFC 9110 §11.6.1 / RFC 6750 §3: a 401 MUST carry a challenge.
        if self.status == StatusCode::UNAUTHORIZED {
            builder = builder.header(header::WWW_AUTHENTICATE, HeaderValue::from_static(BEARER));
        }
        builder.body(body)
    }
}

impl crate::IntoResponse for ProblemDetails {
    fn into_response(self) -> crate::Response {
        crate::ResponseError::as_response(&self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::internal_document;

    /// A response poem rendered, as the normalizer takes it.
    fn crossed(response: Response) -> crate::Response {
        crate::poem_bridge::response_from_poem(response)
    }

    /// poem's own body extractors quote the client's `Content-Type` in their
    /// `415`; rendered by the edge, the refusal names only the type expected.
    #[tokio::test]
    async fn a_foreign_media_type_refusal_never_quotes_the_clients_content_type() {
        let marker = "text/x-quoted-marker".to_owned();
        for (refused, expected) in [
            (
                poem::Error::from(poem::error::ParseJsonError::InvalidContentType(
                    marker.clone(),
                )),
                "application/json",
            ),
            (
                poem::Error::from(poem::error::ParseFormError::InvalidContentType(
                    marker.clone(),
                )),
                "application/x-www-form-urlencoded",
            ),
            (
                poem::Error::from(poem::error::ParseMultipartError::InvalidContentType(
                    marker.clone(),
                )),
                "multipart/form-data",
            ),
        ] {
            let resp = crossed(render_error(refused));
            assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
            let body = resp.into_body().into_string().await.expect("a body");
            assert!(!body.contains("x-quoted-marker"), "{body}");
            assert!(body.contains(expected), "{body}");
        }
    }

    #[test]
    fn constructors_set_status_and_title() {
        assert_eq!(
            ProblemDetails::bad_request().status,
            StatusCode::BAD_REQUEST,
        );
        assert_eq!(ProblemDetails::bad_request().title, "Bad Request");
        assert_eq!(
            ProblemDetails::unauthorized().status,
            StatusCode::UNAUTHORIZED,
        );
        assert_eq!(ProblemDetails::forbidden().status, StatusCode::FORBIDDEN);
        assert_eq!(ProblemDetails::not_found().status, StatusCode::NOT_FOUND);
        assert_eq!(ProblemDetails::conflict().status, StatusCode::CONFLICT);
        assert_eq!(
            ProblemDetails::unprocessable().status,
            StatusCode::UNPROCESSABLE_ENTITY,
        );
        assert_eq!(
            ProblemDetails::internal().status,
            StatusCode::INTERNAL_SERVER_ERROR,
        );
    }

    #[test]
    fn with_detail_adds_field() {
        let p = ProblemDetails::not_found().with_detail("user 42 missing");
        assert_eq!(p.detail.as_deref(), Some("user 42 missing"));
        let v: serde_json::Value = serde_json::from_slice(&p.as_response_body()).unwrap();
        assert_eq!(v["detail"], "user 42 missing");
        assert_eq!(v["status"], 404);
        assert_eq!(v["title"], "Not Found");
    }

    #[test]
    fn with_instance_adds_field() {
        let p = ProblemDetails::conflict().with_instance("/orders/17");
        assert_eq!(p.instance.as_deref(), Some("/orders/17"));
        let v: serde_json::Value = serde_json::from_slice(&p.as_response_body()).unwrap();
        assert_eq!(v["instance"], "/orders/17");
    }

    #[test]
    fn with_type_and_title_override_defaults() {
        let p = ProblemDetails::bad_request()
            .with_type("urn:problem:order-invalid")
            .with_title("Order invalid");
        assert_eq!(p.type_uri, "urn:problem:order-invalid");
        assert_eq!(p.title, "Order invalid");
    }

    #[test]
    fn every_401_carries_the_www_authenticate_challenge() {
        let resp = ProblemDetails::unauthorized().as_response();
        assert_eq!(
            resp.headers()
                .get(header::WWW_AUTHENTICATE)
                .map(|v| v.as_bytes()),
            Some(b"Bearer".as_slice()),
        );
        // A 403 takes none: a challenge would invite a pointless retry.
        assert!(
            ProblemDetails::forbidden()
                .as_response()
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .is_none(),
        );
    }

    #[tokio::test]
    async fn a_richer_challenge_replaces_the_envelope_default_rather_than_stacking() {
        let raw = Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(
                header::WWW_AUTHENTICATE,
                "Bearer resource_metadata=\"https://api.example.com/.well-known/oauth-protected-resource\"",
            )
            .body("nope");
        let normalized = normalize_error_response(crossed(raw)).await;

        let challenges: Vec<_> = normalized
            .headers()
            .get_all(header::WWW_AUTHENTICATE)
            .iter()
            .collect();
        assert_eq!(challenges.len(), 1, "exactly one challenge: {challenges:?}");
        assert!(
            challenges[0]
                .to_str()
                .expect("ascii")
                .contains("resource_metadata"),
            "the specific challenge is the one that survives",
        );
    }

    #[test]
    fn response_uses_problem_json_content_type() {
        let resp = ProblemDetails::not_found().as_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            resp.headers()
                .get(header::CONTENT_TYPE)
                .map(|v| v.as_bytes()),
            Some(b"application/problem+json".as_slice()),
        );
    }

    #[test]
    fn with_extension_flattens_alongside_standard_members() {
        let p = ProblemDetails::bad_request()
            .with_detail("validation failed")
            .with_extension("errors", serde_json::json!({ "email": ["not an email"] }));
        let v: serde_json::Value = serde_json::from_slice(&p.as_response_body()).unwrap();
        assert_eq!(v["status"], 400);
        assert_eq!(v["detail"], "validation failed");
        assert_eq!(v["errors"]["email"][0], "not an email");
    }

    /// RFC 9457 §4.2.1: a problem that says nothing beyond its status is
    /// `about:blank`, titled with the status's phrase — one rule for every
    /// status the framework answers, the named constructors included.
    #[test]
    fn every_status_the_framework_answers_is_about_blank_with_its_phrase() {
        for (status, title) in [
            (StatusCode::BAD_REQUEST, "Bad Request"),
            (StatusCode::UNAUTHORIZED, "Unauthorized"),
            (StatusCode::FORBIDDEN, "Forbidden"),
            (StatusCode::NOT_FOUND, "Not Found"),
            (StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed"),
            (StatusCode::REQUEST_TIMEOUT, "Request Timeout"),
            (StatusCode::CONFLICT, "Conflict"),
            (StatusCode::PAYLOAD_TOO_LARGE, "Content Too Large"),
            (StatusCode::UNSUPPORTED_MEDIA_TYPE, "Unsupported Media Type"),
            (StatusCode::UNPROCESSABLE_ENTITY, "Unprocessable Content"),
            (StatusCode::TOO_MANY_REQUESTS, "Too Many Requests"),
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error"),
            (StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable"),
            (StatusCode::IM_A_TEAPOT, "I'm a teapot"),
        ] {
            let problem = ProblemDetails::from_status(status);
            assert_eq!(
                (problem.type_uri.as_str(), problem.title.as_str()),
                ("about:blank", title),
                "{status}",
            );
        }
        for named in [
            ProblemDetails::bad_request(),
            ProblemDetails::unauthorized(),
            ProblemDetails::forbidden(),
            ProblemDetails::not_found(),
            ProblemDetails::conflict(),
            ProblemDetails::unprocessable(),
            ProblemDetails::internal(),
        ] {
            let bare = ProblemDetails::from_status(named.status);
            assert_eq!(
                (named.type_uri, named.title),
                (bare.type_uri, bare.title),
                "{}",
                named.status,
            );
        }
    }

    /// A raw `401` a handler marked as no call for a bearer token — a wrong
    /// password — keeps its marks once lifted onto the envelope, and the one
    /// challenge RFC 9110 §11.6.1 owes every `401`.
    #[tokio::test]
    async fn normalize_keeps_the_marks_a_raw_error_carries() {
        for marked in [true, false] {
            let raw = crate::Response::builder().status(StatusCode::UNAUTHORIZED);
            let raw = match marked {
                true => raw.extension(crate::challenge::NoBearerChallenge),
                false => raw,
            };
            let resp = normalize_error_response(raw.body("invalid credentials")).await;
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(
                resp.headers()[header::CONTENT_TYPE],
                "application/problem+json"
            );
            assert_eq!(resp.headers()[header::WWW_AUTHENTICATE], BEARER);
            assert_eq!(
                resp.extensions()
                    .get::<crate::challenge::NoBearerChallenge>()
                    .is_some(),
                marked,
            );
        }
    }

    #[tokio::test]
    async fn normalize_lifts_a_raw_plain_text_transport_error() {
        let raw = render_error(poem::Error::from_string(
            nest_rs_core::UUID_V7_REQUIRED,
            StatusCode::BAD_REQUEST,
        ));
        let resp = normalize_error_response(crossed(raw)).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            resp.headers()
                .get(header::CONTENT_TYPE)
                .map(|v| v.as_bytes()),
            Some(b"application/problem+json".as_slice()),
        );
        let bytes = resp.into_body().into_bytes().await.expect("body");
        let v: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(v["status"], 400);
        assert_eq!(v["detail"], nest_rs_core::UUID_V7_REQUIRED);
    }

    #[tokio::test]
    async fn normalize_passes_through_an_existing_problem() {
        let existing = ProblemDetails::conflict()
            .with_detail("dup name")
            .as_response();
        let resp = normalize_error_response(crossed(existing)).await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let bytes = resp.into_body().into_bytes().await.expect("body");
        let v: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(v["detail"], "dup name");
    }

    #[tokio::test]
    async fn normalize_leaves_a_deliberate_json_error_body_alone() {
        let typed = Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .content_type("application/json")
            .body(br#"{"custom":"envelope"}"#.to_vec());
        let resp = normalize_error_response(crossed(typed)).await;
        assert_eq!(
            resp.headers()
                .get(header::CONTENT_TYPE)
                .map(|v| v.as_bytes()),
            Some(b"application/json".as_slice()),
        );
    }

    /// A layer or a library writing what the client sent into a `4xx` text:
    /// the problem holds no part of it.
    #[tokio::test]
    async fn normalize_keeps_no_text_the_framework_did_not_write() {
        let echoed = Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body("unknown session x-quoted-marker".as_bytes().to_vec());
        let resp = normalize_error_response(crossed(echoed)).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = resp.into_body().into_bytes().await.expect("body");
        let text = std::str::from_utf8(&bytes).expect("utf8");
        assert!(!text.contains("x-quoted-marker"), "{text}");
        let v: serde_json::Value = serde_json::from_slice(&bytes).expect("problem json");
        assert_eq!(v["title"], "Bad Request");
        assert!(v.get("detail").is_none(), "{v}");
    }

    #[tokio::test]
    async fn normalize_drops_server_error_detail() {
        let raw = poem::Error::from_string(
            "connection to db-primary refused",
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response();
        let resp = normalize_error_response(crossed(raw)).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = resp.into_body().into_bytes().await.expect("body");
        let text = std::str::from_utf8(&bytes).expect("utf8");
        assert!(
            !text.contains("db-primary"),
            "server-error detail must not leak the driver message: {text}",
        );
        let v: serde_json::Value = serde_json::from_slice(&bytes).expect("problem json");
        assert_eq!(v, internal_document(), "the one opaque document");
    }

    /// A server failure answered from its status alone is its class's
    /// problem, `code` included and no `detail`: `UNAVAILABLE` for a `503`,
    /// `INTERNAL` for any other `5xx`, as core's classes read them. A `4xx`
    /// carries no code it was not given.
    #[test]
    fn a_server_status_answers_its_class_s_problem() {
        for (status, class) in [
            (StatusCode::INTERNAL_SERVER_ERROR, code::INTERNAL),
            (StatusCode::NOT_IMPLEMENTED, code::INTERNAL),
            (StatusCode::BAD_GATEWAY, code::INTERNAL),
            (StatusCode::SERVICE_UNAVAILABLE, code::UNAVAILABLE),
            (StatusCode::GATEWAY_TIMEOUT, code::INTERNAL),
        ] {
            let bare = serde_json::to_value(ProblemDetails::from_status(status)).unwrap();
            let problem = Problem::new(status.as_u16(), class);
            let classed = serde_json::to_value(ProblemDetails::from_problem(&problem)).unwrap();
            assert_eq!(bare, classed, "{status}");
            assert_eq!(bare["code"], class.as_str(), "{status}");
            assert!(bare.get("detail").is_none(), "{status}");
        }
        assert_eq!(
            serde_json::to_value(ProblemDetails::internal()).unwrap(),
            internal_document(),
        );
        let failed = ProblemDetails::from_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal Server Error",
            &std::fmt::Error,
        );
        assert_eq!(failed.extensions["code"], "INTERNAL");
        for client in [
            ProblemDetails::bad_request(),
            ProblemDetails::not_found(),
            ProblemDetails::from_status(StatusCode::METHOD_NOT_ALLOWED),
        ] {
            assert!(client.extensions.get("code").is_none(), "{}", client.status);
        }
    }

    /// The `503` the request deadline answers, an empty body, is lifted onto
    /// its class's document and keeps its `Retry-After`.
    #[tokio::test]
    async fn normalize_answers_the_deadline_s_503_as_its_class_s_document() {
        let raw = crate::deadline::timed_out(std::time::Duration::from_secs(30));
        let resp = normalize_error_response(raw).await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(resp.headers()[header::RETRY_AFTER], "30");
        let bytes = resp.into_body().into_bytes().await.expect("body");
        let v: serde_json::Value = serde_json::from_slice(&bytes).expect("problem json");
        assert_eq!(
            v,
            serde_json::json!({
                "type": "about:blank",
                "title": "Service Unavailable",
                "status": 503,
                "code": "UNAVAILABLE",
            }),
        );
    }

    /// An error the edge cannot read an answer in, rendered with its chain
    /// withheld, answers the one opaque document.
    #[tokio::test]
    async fn an_unknown_server_error_renders_the_one_opaque_document() {
        let unknown = poem::Error::new(
            std::io::Error::other("connection to db-primary refused"),
            StatusCode::INTERNAL_SERVER_ERROR,
        );
        let resp = crossed(render_error(unknown));
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = resp.into_body().into_bytes().await.expect("body");
        let v: serde_json::Value = serde_json::from_slice(&bytes).expect("problem json");
        assert_eq!(v, internal_document());
    }

    #[tokio::test]
    async fn normalize_preserves_headers_on_a_bodyless_error() {
        let raw = Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header("WWW-Authenticate", "Bearer")
            .finish();
        let resp = normalize_error_response(crossed(raw)).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers()
                .get(header::CONTENT_TYPE)
                .map(|v| v.as_bytes()),
            Some(b"application/problem+json".as_slice()),
        );
        assert_eq!(
            resp.headers().get("WWW-Authenticate").map(|v| v.as_bytes()),
            Some(b"Bearer".as_slice()),
            "an auth challenge header must survive normalization",
        );
    }

    #[tokio::test]
    async fn normalize_drops_a_stale_content_encoding_from_the_original() {
        // The compression layer sits inside this boundary.
        let raw = Response::builder()
            .status(StatusCode::GATEWAY_TIMEOUT)
            .header(header::CONTENT_ENCODING, "gzip")
            .header(header::TRANSFER_ENCODING, "chunked")
            .content_type("text/plain")
            .body("upstream timed out");
        let resp = normalize_error_response(crossed(raw)).await;
        assert_eq!(resp.status(), StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(
            resp.headers()
                .get(header::CONTENT_TYPE)
                .map(|v| v.as_bytes()),
            Some(b"application/problem+json".as_slice()),
        );
        assert!(
            resp.headers().get(header::CONTENT_ENCODING).is_none(),
            "a stale Content-Encoding must not survive onto the rewritten body",
        );
        assert!(
            resp.headers().get(header::TRANSFER_ENCODING).is_none(),
            "a stale Transfer-Encoding must not survive onto the rewritten body",
        );
    }

    #[test]
    fn detail_and_instance_omitted_when_absent() {
        let v: serde_json::Value =
            serde_json::from_slice(&ProblemDetails::not_found().as_response_body()).unwrap();
        assert!(v.get("detail").is_none(), "absent detail must be omitted");
        assert!(
            v.get("instance").is_none(),
            "absent instance must be omitted",
        );
    }

    impl ProblemDetails {
        fn as_response_body(&self) -> Vec<u8> {
            serde_json::to_vec(self).unwrap()
        }
    }
}
