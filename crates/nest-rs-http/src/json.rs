//! [`Json`]: a JSON body read into a type, and a value answered as JSON.

use std::ops::{Deref, DerefMut};

use http::{HeaderValue, StatusCode};
use nest_rs_core::DecodeError;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{JsonError, Result};
use crate::problem::ProblemDetails;
use crate::request::{FromRequest, Request, RequestBody};
use crate::response::{IntoResponse, Response};

/// A JSON body: as a parameter, the request's body read under the
/// transport's cap ([`current_body_limit`](crate::current_body_limit)) and
/// decoded into `T`; as an answer, `T` serialized with
/// `Content-Type: application/json; charset=utf-8`.
///
/// A body without a JSON `Content-Type` (`application/json`, or any
/// `application/*+json`) answers `415`, a body past the cap `413`, and one
/// that does not decode into `T` `400`, said without the value it held.
///
/// ```
/// use nest_rs_http::{FromRequest, HeaderValue, IntoResponse, Json, Request, header};
///
/// #[derive(serde::Deserialize, serde::Serialize)]
/// struct Greeting {
///     text: String,
/// }
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
///
/// let (request, mut body) = Request::builder()
///     .header(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))
///     .body(r#"{"text":"hi"}"#)
///     .split();
/// let Json(greeting) = Json::<Greeting>::from_request(&request, &mut body).await?;
/// assert_eq!(greeting.text, "hi");
///
/// let answer = Json(greeting).into_response();
/// assert_eq!(answer.headers()[header::CONTENT_TYPE], "application/json; charset=utf-8");
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Json<T>(pub T);

impl<T> Json<T> {
    /// The value, out of its wrapper.
    ///
    /// ```
    /// use nest_rs_http::Json;
    ///
    /// assert_eq!(Json(7).into_inner(), 7);
    /// ```
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for Json<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> DerefMut for Json<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<'a, T: DeserializeOwned> FromRequest<'a> for Json<T> {
    async fn from_request(req: &'a Request, body: &mut RequestBody) -> Result<Self> {
        if !req.content_type().is_some_and(is_json) {
            return Err(JsonError::MediaType.into());
        }
        let bytes = body
            .take()?
            .into_bytes_limit(crate::current_body_limit())
            .await?;
        serde_json::from_slice(&bytes)
            .map(Json)
            .map_err(|error| JsonError::Malformed(error).into())
    }
}

impl<T: Serialize + Send> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        match serde_json::to_vec(&self.0) {
            Ok(bytes) => Response::builder()
                .content_type(HeaderValue::from_static("application/json; charset=utf-8"))
                .body(bytes),
            Err(error) => {
                // A `Serialize` impl's own message may quote the value it refused.
                let said = nest_rs_core::error_message(&error);
                tracing::error!(
                    target: crate::target::HTTP,
                    error = %DecodeError::redact(&said, Some(&error)),
                    "a JSON answer did not serialize",
                );
                ProblemDetails::from_status(StatusCode::INTERNAL_SERVER_ERROR).into_response()
            }
        }
    }
}

/// Whether `content_type` names JSON: `application/json`, or a structured
/// syntax suffix `+json` (RFC 6839 §3.1), whatever its parameters.
fn is_json(content_type: &str) -> bool {
    crate::request::essence(content_type).is_some_and(|essence| essence.is("application", "json"))
}

#[cfg(test)]
mod tests {
    use http::header;
    use serde::Deserialize;

    use super::*;
    use crate::body::Body;

    #[derive(Debug, Deserialize, Serialize, PartialEq)]
    struct Order {
        quantity: u32,
    }

    async fn read(content_type: Option<&'static str>, body: &'static str) -> Result<Json<Order>> {
        let mut builder = Request::builder();
        if let Some(content_type) = content_type {
            builder = builder.header(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
        }
        let (request, mut body) = builder.body(body).split();
        Json::<Order>::from_request(&request, &mut body).await
    }

    async fn detail_of(response: Response) -> serde_json::Value {
        let bytes = response.into_body().into_bytes().await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn json_and_its_suffix_are_json_whatever_their_case_and_parameters() {
        for json in [
            "application/json",
            "Application/JSON",
            "application/json; charset=utf-8",
            "application/problem+json",
            "application/vnd.api+JSON ;version=1",
        ] {
            assert!(is_json(json), "{json}");
        }
        for other in [
            "text/json",
            "application/jsonp",
            "application/x-www-form-urlencoded",
            "json",
            "",
        ] {
            assert!(!is_json(other), "{other}");
        }
    }

    #[tokio::test]
    async fn a_json_body_decodes_into_its_type() {
        let Json(order) = read(Some("application/json"), r#"{"quantity":3}"#)
            .await
            .unwrap();
        assert_eq!(order, Order { quantity: 3 });
    }

    #[tokio::test]
    async fn a_body_that_is_not_json_by_its_type_is_415_and_its_type_is_never_quoted() {
        for content_type in [None, Some("text/plain; secret=sk_live_51")] {
            let refused = read(content_type, r#"{"quantity":3}"#).await.unwrap_err();
            assert_eq!(refused.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
            let problem = detail_of(refused.into_response()).await;
            let detail = problem["detail"].as_str().unwrap();
            assert!(detail.contains("application/json"), "{detail}");
            assert!(!detail.contains("sk_live"), "{detail}");
        }
    }

    #[tokio::test]
    async fn a_body_that_does_not_decode_is_400_said_without_its_value() {
        let refused = read(Some("application/json"), r#"{"quantity":"sk_live_51"}"#)
            .await
            .unwrap_err();
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        let problem = detail_of(refused.into_response()).await;
        let detail = problem["detail"].as_str().unwrap();
        assert!(detail.contains("invalid type"), "{detail}");
        assert!(!detail.contains("sk_live"), "{detail}");
    }

    #[tokio::test]
    async fn a_body_past_the_configured_cap_is_413() {
        let (request, mut body) = Request::builder()
            .header(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )
            .body(Body::from(r#"{"quantity":3}"#))
            .split();
        let refused =
            crate::raw_body::with_body_limit(4, Json::<Order>::from_request(&request, &mut body))
                .await
                .unwrap_err();
        assert_eq!(refused.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn a_value_answers_as_json() {
        let answer = Json(Order { quantity: 2 }).into_response();
        assert_eq!(answer.status(), StatusCode::OK);
        assert_eq!(
            answer.headers()[header::CONTENT_TYPE],
            "application/json; charset=utf-8"
        );
        assert_eq!(
            answer.into_body().into_string().await.unwrap(),
            r#"{"quantity":2}"#
        );
    }

    #[tokio::test]
    async fn a_value_that_does_not_serialize_answers_an_opaque_500_and_is_logged() {
        let logs = nest_rs_testing::LogCapture::install();
        let keyed = std::collections::HashMap::from([((1, 2), "sk_live_51")]);
        let answer = Json(keyed).into_response();
        assert_eq!(answer.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = answer.into_body().into_string().await.unwrap();
        assert!(!body.contains("sk_live"), "{body}");
        let event = logs.expect_one("nest_rs::http", "a JSON answer did not serialize");
        assert_eq!(event.level, "error");
    }
}
