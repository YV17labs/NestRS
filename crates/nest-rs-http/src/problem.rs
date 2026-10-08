//! [RFC 9457](https://www.rfc-editor.org/rfc/rfc9457.html) Problem Details for HTTP APIs,
//! the single error format at the HTTP boundary.
//!
//! A handler returns `Err(ProblemDetails::not_found().with_detail("…"))`;
//! [`normalize_error_response`] lifts any leftover plain-text transport error (an
//! unmounted-route 404, a 413) onto the same `application/problem+json` envelope.

use nest_rs_core::DecodeError;
use poem::error::{ParseQueryError, ResponseError};
use poem::http::{StatusCode, header};
use poem::{IntoResponse, Response};
use serde::Serialize;

/// Body of an `application/problem+json` response. `type` is the only field a
/// client may key on, so the constructors' URIs are stable across releases.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct ProblemDetails {
    /// Stable URI identifying the problem type. `about:blank` means the client
    /// should ignore the type and key on `status` + `title` instead.
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
        Self {
            type_uri: "about:blank".into(),
            title: title.into(),
            status,
            detail: Some(DecodeError::redact(&err.to_string(), Some(err)).into_owned()),
            instance: None,
            extensions: serde_json::Map::new(),
        }
    }

    fn new(type_uri: &'static str, title: &'static str, status: StatusCode) -> Self {
        Self {
            type_uri: type_uri.into(),
            title: title.into(),
            status,
            detail: None,
            instance: None,
            extensions: serde_json::Map::new(),
        }
    }

    /// Build a problem from a bare [`StatusCode`]: a well-known code through its
    /// canonical constructor, anything else as `about:blank` and its reason phrase.
    pub fn from_status(status: StatusCode) -> Self {
        match status {
            StatusCode::BAD_REQUEST => Self::bad_request(),
            StatusCode::UNAUTHORIZED => Self::unauthorized(),
            StatusCode::FORBIDDEN => Self::forbidden(),
            StatusCode::NOT_FOUND => Self::not_found(),
            StatusCode::CONFLICT => Self::conflict(),
            StatusCode::UNPROCESSABLE_ENTITY => Self::unprocessable(),
            StatusCode::INTERNAL_SERVER_ERROR => Self::internal(),
            other => Self {
                type_uri: "about:blank".into(),
                title: other.canonical_reason().unwrap_or("Error").into(),
                status: other,
                detail: None,
                instance: None,
                extensions: serde_json::Map::new(),
            },
        }
    }

    /// A `400 Bad Request` problem.
    pub fn bad_request() -> Self {
        Self::new(
            "https://www.rfc-editor.org/rfc/rfc9110#status.400",
            "Bad Request",
            StatusCode::BAD_REQUEST,
        )
    }

    /// A `401 Unauthorized` problem.
    pub fn unauthorized() -> Self {
        Self::new(
            "https://www.rfc-editor.org/rfc/rfc9110#status.401",
            "Unauthorized",
            StatusCode::UNAUTHORIZED,
        )
    }

    /// A `403 Forbidden` problem.
    pub fn forbidden() -> Self {
        Self::new(
            "https://www.rfc-editor.org/rfc/rfc9110#status.403",
            "Forbidden",
            StatusCode::FORBIDDEN,
        )
    }

    /// A `404 Not Found` problem.
    pub fn not_found() -> Self {
        Self::new(
            "https://www.rfc-editor.org/rfc/rfc9110#status.404",
            "Not Found",
            StatusCode::NOT_FOUND,
        )
    }

    /// A `409 Conflict` problem.
    pub fn conflict() -> Self {
        Self::new(
            "https://www.rfc-editor.org/rfc/rfc9110#status.409",
            "Conflict",
            StatusCode::CONFLICT,
        )
    }

    /// A `422 Unprocessable Content` problem — well-formed but semantically invalid.
    pub fn unprocessable() -> Self {
        Self::new(
            "https://www.rfc-editor.org/rfc/rfc9110#status.422",
            "Unprocessable Content",
            StatusCode::UNPROCESSABLE_ENTITY,
        )
    }

    /// A `500 Internal Server Error` problem.
    pub fn internal() -> Self {
        Self::new(
            "https://www.rfc-editor.org/rfc/rfc9110#status.500",
            "Internal Server Error",
            StatusCode::INTERNAL_SERVER_ERROR,
        )
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

/// Normalize a response carrying a raw (non-problem) transport error onto the
/// single RFC-9457 `application/problem+json` envelope.
///
/// Only a `>= 400` response with a `text/plain` or empty body is rebuilt, keeping
/// its headers. A `4xx` body becomes `detail`, said without any value it quotes
/// ([`DecodeError::redact`]); a `5xx` body is dropped so a driver or panic
/// message never reaches the wire.
pub async fn normalize_error_response(resp: Response) -> Response {
    let status = resp.status();
    if !(status.is_client_error() || status.is_server_error()) {
        return resp;
    }
    // A filter-mapped error is a deliberate wire contract, never rewritten.
    if resp.extensions().get::<crate::MappedError>().is_some() {
        return resp;
    }
    if !is_raw_text(&resp) {
        return resp;
    }

    let (parts, body) = resp.into_parts();
    let mut problem = ProblemDetails::from_status(status);
    // A body rendered from an `Err` has been through `render_error`; one a
    // handler or a layer wrote as text is read by serde's wording alone.
    if status.is_client_error()
        && let Ok(bytes) = body.into_bytes().await
        && let Ok(text) = std::str::from_utf8(&bytes)
        && !text.trim().is_empty()
    {
        problem = problem.with_detail(DecodeError::redact(text.trim(), None).into_owned());
    }
    let mut response = problem.as_response();
    // The fresh body is uncompressed: a stale `Content-Encoding` would fail decoding.
    // An original header replaces the envelope's default (a bare `Bearer`), since a
    // client reads the first value; later occurrences append (`Set-Cookie`).
    let mut replaced = std::collections::HashSet::new();
    for (name, value) in parts.headers.iter() {
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
    response
}

/// Whether `resp` is poem's default rendering of an error — `text/plain`, or no
/// body type at all — rather than a problem or a body somebody typed.
fn is_raw_text(resp: &Response) -> bool {
    resp.headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_none_or(|ct| ct.starts_with("text/plain"))
}

/// An `Err` as the response it renders: poem's own rendering, except that a raw
/// text body says each decode failure in the error's chain without its value.
///
/// `Query<T>`'s rejection is transparent over its serde error, so that one is
/// read off the type.
pub(crate) fn render_error(err: poem::Error) -> Response {
    let sentence = err.to_string();
    let mut said = DecodeError::redact(&sentence, Some(&err)).into_owned();
    if let Some(query) = err.downcast_ref::<ParseQueryError>() {
        said = DecodeError::redact(&said, Some(&query.0)).into_owned();
    }
    let redacted = (said != sentence).then_some(said);
    let resp = err.into_response();
    match redacted {
        Some(said) if is_raw_text(&resp) => {
            let (parts, _) = resp.into_parts();
            Response::from_parts(parts, poem::Body::from_string(said))
        }
        _ => resp,
    }
}

/// The [`ERROR_RESOLVE`](crate::interceptor::priority::ERROR_RESOLVE) band: the
/// route tree, with a still-unhandled `Err` rendered by [`render_error`] so every
/// band above observes a response.
pub(crate) struct ResolvedErrors<E>(pub(crate) E);

impl<E: poem::Endpoint> poem::Endpoint for ResolvedErrors<E> {
    type Output = Response;

    async fn call(&self, req: poem::Request) -> poem::Result<Response> {
        Ok(match self.0.call(req).await {
            Ok(out) => out.into_response(),
            Err(err) => render_error(err),
        })
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
        let body = serde_json::to_vec(self).unwrap_or_else(|_| b"{}".to_vec());
        let mut builder = Response::builder()
            .status(self.status)
            .header(header::CONTENT_TYPE, "application/problem+json");
        // RFC 9110 §11.6.1 / RFC 6750 §3: a 401 MUST carry a challenge.
        if self.status == StatusCode::UNAUTHORIZED {
            builder = builder.header(header::WWW_AUTHENTICATE, "Bearer");
        }
        builder.body(body)
    }
}

impl IntoResponse for ProblemDetails {
    fn into_response(self) -> Response {
        self.as_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn constructors_preset_type_uri() {
        assert!(
            ProblemDetails::not_found()
                .type_uri
                .starts_with("https://www.rfc-editor.org/rfc/rfc9110"),
        );
        assert!(ProblemDetails::unprocessable().type_uri.contains("rfc9110"),);
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
        let normalized = normalize_error_response(raw).await;

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

    #[test]
    fn from_status_routes_well_known_codes_to_their_constructor() {
        assert_eq!(
            ProblemDetails::from_status(StatusCode::NOT_FOUND).type_uri,
            ProblemDetails::not_found().type_uri,
        );
        assert_eq!(
            ProblemDetails::from_status(StatusCode::CONFLICT).status,
            StatusCode::CONFLICT,
        );
        let teapot = ProblemDetails::from_status(StatusCode::IM_A_TEAPOT);
        assert_eq!(teapot.type_uri, "about:blank");
        assert_eq!(teapot.title, "I'm a teapot");
    }

    #[tokio::test]
    async fn normalize_lifts_a_raw_plain_text_transport_error() {
        let raw = poem::Error::from_string(nest_rs_core::UUID_V7_REQUIRED, StatusCode::BAD_REQUEST)
            .into_response();
        let resp = normalize_error_response(raw).await;
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
        let resp = normalize_error_response(existing).await;
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
        let resp = normalize_error_response(typed).await;
        assert_eq!(
            resp.headers()
                .get(header::CONTENT_TYPE)
                .map(|v| v.as_bytes()),
            Some(b"application/json".as_slice()),
        );
    }

    #[tokio::test]
    async fn normalize_leaves_a_filter_mapped_response_alone() {
        let mut mapped = Response::builder()
            .status(StatusCode::IM_A_TEAPOT)
            .body("edge-mapped".as_bytes().to_vec());
        mapped.extensions_mut().insert(crate::MappedError);
        let resp = normalize_error_response(mapped).await;
        assert_eq!(resp.status(), StatusCode::IM_A_TEAPOT);
        let bytes = resp.into_body().into_bytes().await.expect("body");
        assert_eq!(&bytes[..], b"edge-mapped");
    }

    #[tokio::test]
    async fn normalize_drops_server_error_detail() {
        let raw = poem::Error::from_string(
            "connection to db-primary refused",
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response();
        let resp = normalize_error_response(raw).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = resp.into_body().into_bytes().await.expect("body");
        let text = std::str::from_utf8(&bytes).expect("utf8");
        assert!(
            !text.contains("db-primary"),
            "server-error detail must not leak the driver message: {text}",
        );
        let v: serde_json::Value = serde_json::from_slice(&bytes).expect("problem json");
        assert_eq!(v["status"], 500);
        assert_eq!(v["title"], "Internal Server Error");
        assert!(v.get("detail").is_none(), "no detail on a 500");
    }

    #[tokio::test]
    async fn normalize_preserves_headers_on_a_bodyless_error() {
        let raw = Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header("WWW-Authenticate", "Bearer")
            .finish();
        let resp = normalize_error_response(raw).await;
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
        let resp = normalize_error_response(raw).await;
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
