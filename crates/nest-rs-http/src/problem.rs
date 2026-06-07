//! [RFC 9457](https://www.rfc-editor.org/rfc/rfc9457.html) Problem Details for HTTP APIs.
//!
//! Opt-in per route. A handler that wants a structured `application/problem+json`
//! error body returns `Err(ProblemDetails::not_found().with_detail("…"))`; the
//! [`ResponseError`] impl below renders the JSON envelope and stamps the right
//! `Content-Type`. Features that have their own error enums keep their existing
//! [`ResponseError`] mapping — this helper is for one-off problem responses
//! (a glue route's validation failure, a not-yet-modelled edge case).

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
    pub title: String,
    #[serde(serialize_with = "serialize_status")]
    pub status: StatusCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
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
        }
    }

    fn new(type_uri: &'static str, title: &'static str, status: StatusCode) -> Self {
        Self {
            type_uri: type_uri.into(),
            title: title.into(),
            status,
            detail: None,
            instance: None,
        }
    }

    pub fn bad_request() -> Self {
        Self::new(
            "https://www.rfc-editor.org/rfc/rfc9110#status.400",
            "Bad Request",
            StatusCode::BAD_REQUEST,
        )
    }

    pub fn unauthorized() -> Self {
        Self::new(
            "https://www.rfc-editor.org/rfc/rfc9110#status.401",
            "Unauthorized",
            StatusCode::UNAUTHORIZED,
        )
    }

    pub fn forbidden() -> Self {
        Self::new(
            "https://www.rfc-editor.org/rfc/rfc9110#status.403",
            "Forbidden",
            StatusCode::FORBIDDEN,
        )
    }

    pub fn not_found() -> Self {
        Self::new(
            "https://www.rfc-editor.org/rfc/rfc9110#status.404",
            "Not Found",
            StatusCode::NOT_FOUND,
        )
    }

    pub fn conflict() -> Self {
        Self::new(
            "https://www.rfc-editor.org/rfc/rfc9110#status.409",
            "Conflict",
            StatusCode::CONFLICT,
        )
    }

    pub fn unprocessable() -> Self {
        Self::new(
            "https://www.rfc-editor.org/rfc/rfc9110#status.422",
            "Unprocessable Content",
            StatusCode::UNPROCESSABLE_ENTITY,
        )
    }

    pub fn internal() -> Self {
        Self::new(
            "https://www.rfc-editor.org/rfc/rfc9110#status.500",
            "Internal Server Error",
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_instance(mut self, instance: impl Into<String>) -> Self {
        self.instance = Some(instance.into());
        self
    }

    pub fn with_type(mut self, type_uri: impl Into<String>) -> Self {
        self.type_uri = type_uri.into();
        self
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }
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
