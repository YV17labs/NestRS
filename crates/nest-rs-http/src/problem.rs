//! [RFC 9457](https://www.rfc-editor.org/rfc/rfc9457.html) Problem Details for HTTP APIs.
//!
//! The single error format at the HTTP boundary: every modelled failure the
//! transport renders — a handler's `ServiceError`, an inline [`ProblemDetails`],
//! a validation rejection, a guard denial, a domain exception filter — is an
//! `application/problem+json` body. A handler that wants a structured error
//! returns `Err(ProblemDetails::not_found().with_detail("…"))`; the
//! [`ResponseError`] impl below renders the JSON envelope and stamps the
//! `Content-Type`. [`normalize_error_response`] is the transport-edge boundary
//! that lifts any leftover raw plain-text error (an unmounted-route 404, a 413,
//! an extractor's bad-path-id 400) onto the same envelope, so no route returns
//! a bare-text or foreign-shaped body for a modelled failure. RFC 9457
//! extension members (e.g. field-level errors under `errors`) ride via
//! [`ProblemDetails::with_extension`].

use poem::error::ResponseError;
use poem::http::{StatusCode, header};
use poem::{IntoResponse, Response};
use serde::Serialize;

/// Body of an `application/problem+json` response. Stable URIs for the
/// well-known constructors live at `https://www.rfc-editor.org/rfc/rfc9457` —
/// `type` is the only field a client may key on, so the URIs are stable across
/// releases (and overridable via [`with_type`](Self::with_type) when an app
/// wants to publish its own type registry).
#[derive(Debug, Clone, Serialize)]
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
    /// RFC 9457 **extension members** — problem-specific data flattened
    /// alongside the standard fields (e.g. field-level validation errors under
    /// an `errors` key). Empty ⇒ no extra keys serialized.
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
    /// Convert a `Display` value to the problem `detail` field. Chainable
    /// shorthand for `.with_detail(err.to_string())`.
    pub fn from_error(status: StatusCode, title: impl Into<String>, err: impl ToString) -> Self {
        Self {
            type_uri: "about:blank".into(),
            title: title.into(),
            status,
            detail: Some(err.to_string()),
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

    /// Build a problem from a bare [`StatusCode`], routing the well-known 4xx/5xx
    /// through their canonical constructor (stable `type` URI + `title`) and
    /// falling back to `about:blank` + the status' reason phrase for anything
    /// else. The single seam the transport-edge mapper uses to lift a raw poem
    /// error (an unmounted-route 404, a 413, a 405) onto the RFC-9457 envelope.
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

    /// Attach an RFC 9457 **extension member** — arbitrary problem-specific data
    /// serialized alongside the standard fields. Field-level validation errors
    /// ride here (e.g. `.with_extension("errors", json!({ "email": [...] }))`).
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
/// The transport-edge boundary the framework installs around the whole route
/// tree. A `< 400` response, or one already rendered as
/// `application/problem+json` (a [`ProblemDetails`], a `ServiceError`, a guard
/// denial, a domain exception filter), passes through untouched. A `>= 400`
/// response whose body is poem's default `text/plain` (or empty) — an
/// unmounted-route `404`, a `413`, a `405`, a bad-path-id `400` an extractor
/// rejected — is rebuilt as `problem+json` keyed on its status, **preserving
/// the original headers** (`WWW-Authenticate`, `Retry-After`, …). A
/// client-error (`4xx`) body rides through as `detail`; a server-error (`5xx`)
/// body is dropped so a driver or panic message never reaches the wire. A
/// deliberately-typed body (`application/json`, `text/html`, …) is left alone.
pub async fn normalize_error_response(resp: Response) -> Response {
    let status = resp.status();
    if !(status.is_client_error() || status.is_server_error()) {
        return resp;
    }
    // A response a `Filter`/`ExceptionFilter` produced by mapping a handler
    // error is tagged `MappedError` — its body is a deliberate wire contract
    // (an app's own envelope, a custom status), never a raw transport error to
    // rewrite.
    if resp
        .extensions()
        .get::<nest_rs_core::MappedError>()
        .is_some()
    {
        return resp;
    }
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let already_problem = content_type
        .as_deref()
        .is_some_and(|ct| ct.starts_with("application/problem+json"));
    // Only poem's default plain-text (or a bodyless) error is a raw transport
    // error; anything a handler deliberately typed is left untouched.
    let is_raw_transport_error = content_type
        .as_deref()
        .is_none_or(|ct| ct.starts_with("text/plain"));
    if already_problem || !is_raw_transport_error {
        return resp;
    }

    let (parts, body) = resp.into_parts();
    let mut problem = ProblemDetails::from_status(status);
    if status.is_client_error()
        && let Ok(bytes) = body.into_bytes().await
        && let Ok(text) = std::str::from_utf8(&bytes)
        && !text.trim().is_empty()
    {
        problem = problem.with_detail(text.trim().to_owned());
    }
    let mut response = problem.as_response();
    // Carry the original response's headers across — the new body owns
    // content-type / content-length, everything else (auth challenges, rate
    // limits) survives. Content/transfer-encoding are dropped too: the
    // replacement body is fresh and uncompressed, so a stale `Content-Encoding`
    // copied from a compressed original would make the client fail to decode it.
    for (name, value) in parts.headers.iter() {
        if name == header::CONTENT_TYPE
            || name == header::CONTENT_LENGTH
            || name == header::CONTENT_ENCODING
            || name == header::TRANSFER_ENCODING
        {
            continue;
        }
        response.headers_mut().append(name.clone(), value.clone());
    }
    response
}

impl std::fmt::Display for ProblemDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The error chain only ever shows in logs; the JSON body is the real
        // response. Keep the `Display` form short and structured.
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
        // Serialize the body once; on the off-chance serialization fails we
        // fall back to an empty object so the status code still surfaces.
        let body = serde_json::to_vec(self).unwrap_or_else(|_| b"{}".to_vec());
        Response::builder()
            .status(self.status)
            .header(header::CONTENT_TYPE, "application/problem+json")
            .body(body)
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
        // Each well-known constructor must ship with a stable, non-blank URI;
        // a client may key on it.
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
        // Body round-trips through serde with the right shape.
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
        // RFC 9457 extension members sit at the top level next to type/title/
        // status — not nested under a wrapper key.
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
        // An uncommon status falls back to about:blank + its reason phrase.
        let teapot = ProblemDetails::from_status(StatusCode::IM_A_TEAPOT);
        assert_eq!(teapot.type_uri, "about:blank");
        assert_eq!(teapot.title, "I'm a teapot");
    }

    #[tokio::test]
    async fn normalize_lifts_a_raw_plain_text_transport_error() {
        // A raw poem status error renders plain text by default; the edge
        // boundary lifts it onto problem+json keyed on the status, carrying the
        // (safe) client-error message through as `detail`.
        let raw = poem::Error::from_string("id must be a UUID v7", StatusCode::BAD_REQUEST)
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
        assert_eq!(v["detail"], "id must be a UUID v7");
    }

    #[tokio::test]
    async fn normalize_passes_through_an_existing_problem() {
        // A response already rendered as problem+json is left untouched — its
        // detail survives.
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
        // A handler that typed its own 4xx `application/json` body is not a raw
        // transport error — the boundary must not clobber it.
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
        // A `Filter`/`ExceptionFilter` mapping tags its response `MappedError`;
        // its deliberate (plain-text) body must survive the edge boundary.
        let mut mapped = Response::builder()
            .status(StatusCode::IM_A_TEAPOT)
            .body("edge-mapped".as_bytes().to_vec());
        mapped.extensions_mut().insert(nest_rs_core::MappedError);
        let resp = normalize_error_response(mapped).await;
        assert_eq!(resp.status(), StatusCode::IM_A_TEAPOT);
        let bytes = resp.into_body().into_bytes().await.expect("body");
        assert_eq!(&bytes[..], b"edge-mapped");
    }

    #[tokio::test]
    async fn normalize_drops_server_error_detail() {
        // A 5xx must never echo the error text — only the canonical title.
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
        // A bodyless 401 (no content-type) is a raw transport error; the
        // boundary lifts it while preserving an auth challenge header.
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
        // The compression layer sits inside this boundary, so a raw error it
        // already stamped `Content-Encoding: gzip` must not carry that header
        // onto the fresh, uncompressed problem+json body — else the client
        // fails to decode it (ERR_CONTENT_DECODING_FAILED).
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
        // RFC 9457: `detail` and `instance` are optional, so a minimal problem
        // serializes only the four core fields.
        let v: serde_json::Value =
            serde_json::from_slice(&ProblemDetails::not_found().as_response_body()).unwrap();
        assert!(v.get("detail").is_none(), "absent detail must be omitted");
        assert!(
            v.get("instance").is_none(),
            "absent instance must be omitted",
        );
    }

    impl ProblemDetails {
        // Tiny test helper: drain the response into bytes so the assertions
        // above can inspect the serialized JSON without going through poem's
        // async body type.
        fn as_response_body(&self) -> Vec<u8> {
            serde_json::to_vec(self).unwrap()
        }
    }
}
