//! HTTP binding for nestrs pipes: the extractors as the [`Source`] a
//! [`Piped`] argument reads, and as values that validate as theirs does.
//!
//! - `Piped<P, E>` (e.g. `Piped<ParseUuid, Path<String>>`) applies pipe `P` to
//!   what `E` extracted, and binds its output: `Piped(id)`. `#[routes]`
//!   extracts `E` itself, holds it to its rules, then runs [`piped`].
//! - `Json<T>`, `Query<T>`, `Form<T>`, `Path<T>` and `Header<T>` implement
//!   `validator::Validate` when `T` does, so `#[routes]` validates the value
//!   they carry with no wrapper.
//!
//! Both reject through [`pipe_error`]: an RFC-9457 `application/problem+json`
//! `400` ([`ProblemDetails`]) carrying the [`PipeError`]'s message as `detail`
//! and any field-level errors as an `errors` extension member.

use std::future::Future;
use std::pin::Pin;

use nest_rs_pipes::{Pipe, PipeError, Piped, Source};
use validator::{Validate, ValidationErrors};

use crate::{
    Form, FromRequest, Header, HttpError, Json, Path, ProblemDetails, Query, Request, RequestBody,
};

/// Each single-value extractor is a source a pipe reads, and validates as the
/// value it carries does.
macro_rules! carries_one_value {
    ($($extractor:ident),* $(,)?) => {
        $(
            impl<T> Source for $extractor<T> {
                type Value = T;
                fn into_value(self) -> T {
                    self.0
                }
            }

            impl<T: Validate> Validate for $extractor<T> {
                fn validate(&self) -> Result<(), ValidationErrors> {
                    self.0.validate()
                }
            }
        )*
    };
}

carries_one_value!(Json, Path, Query, Form, Header);

/// Render a [`PipeError`] as the `400` problem document it answers, its
/// field-level errors under the `errors` extension member.
///
/// ```
/// use nest_rs_http::{StatusCode, pipe_error};
/// use nest_rs_pipes::PipeError;
///
/// let refused = pipe_error(&PipeError::new(
///     "query parameter `after`: not a cursor this endpoint issued",
/// ));
/// assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
/// ```
pub fn pipe_error(err: &PipeError) -> HttpError {
    let mut problem = ProblemDetails::bad_request().with_detail(err.message().to_owned());
    if let Some(details) = err.details() {
        problem = problem.with_extension(crate::problem::FIELD_ERRORS, details.clone());
    }
    problem.into()
}

/// Run `P` over the value `extracted` carries — the step `#[routes]` takes once
/// it held the extractor to the rules its value declares.
pub fn piped<P, E>(extracted: E) -> crate::Result<Piped<P, E>>
where
    P: Pipe,
    E: Source<Value = P::In>,
{
    Piped::apply(extracted.into_value()).map_err(|refused| pipe_error(&refused))
}

/// Each extractor of the vocabulary that reads the body reads it under the cap
/// itself. The future is boxed: a generic `async fn` delegating to another's
/// trips rustc#100013.
impl<'a, P, E> FromRequest<'a> for Piped<P, E>
where
    P: Pipe + Send + Sync,
    P::Out: Send,
    E: FromRequest<'a> + Source<Value = P::In>,
{
    async fn from_request(req: &'a Request, body: &mut RequestBody) -> crate::Result<Self> {
        let extract: Pin<Box<dyn Future<Output = crate::Result<E>> + Send + '_>> =
            Box::pin(E::from_request(req, body));
        piped(extract.await?)
    }
}

#[cfg(test)]
mod tests {
    use http::{HeaderValue, StatusCode, header};
    use serde::{Deserialize, Serialize};

    use super::*;

    async fn problem_of(err: HttpError) -> (StatusCode, serde_json::Value, String) {
        let resp = err.into_response();
        let status = resp.status();
        let text = resp.into_body().into_string().await.expect("body");
        let json = serde_json::from_str(&text).unwrap_or_default();
        (status, json, text)
    }

    #[tokio::test]
    async fn a_rejection_is_a_problem_json_400_with_the_message() {
        let resp = pipe_error(&PipeError::new("not a uuid")).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            resp.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
        let json: serde_json::Value =
            serde_json::from_str(&resp.into_body().into_string().await.unwrap()).unwrap();
        assert_eq!(json["status"], 400);
        assert_eq!(json["title"], "Bad Request");
        assert_eq!(json["detail"], "not a uuid");
        assert!(
            json.get("errors").is_none(),
            "no errors on a plain rejection"
        );
    }

    #[tokio::test]
    async fn a_rejection_carries_field_errors_as_an_extension_member() {
        let err = PipeError::with_details(
            "validation failed",
            serde_json::json!({ "email": ["not an email"] }),
        );
        let (_, json, _) = problem_of(pipe_error(&err)).await;
        assert_eq!(json["detail"], "validation failed");
        assert_eq!(json["errors"]["email"][0], "not an email");
    }

    #[test]
    fn each_single_value_extractor_hands_its_value_to_a_pipe() {
        assert_eq!(Json(42).into_value(), 42);
        assert_eq!(Path("/users/123").into_value(), "/users/123");
        assert_eq!(Query(7).into_value(), 7);
        assert_eq!(Form(8).into_value(), 8);
        assert_eq!(Header("abc").into_value(), "abc");
    }

    struct ToUpper;

    impl Pipe for ToUpper {
        type In = String;
        type Out = String;
        fn transform(input: String) -> std::result::Result<String, PipeError> {
            Ok(input.to_ascii_uppercase())
        }
    }

    struct AlwaysReject;

    impl Pipe for AlwaysReject {
        type In = String;
        type Out = String;
        fn transform(_: String) -> std::result::Result<String, PipeError> {
            Err(PipeError::new("nope"))
        }
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq, Validate)]
    struct Greeting {
        #[validate(length(min = 1))]
        msg: String,
    }

    #[test]
    fn each_single_value_extractor_validates_as_its_value_does() {
        let blank = || Greeting { msg: String::new() };
        assert!(Json(blank()).validate().is_err());
        assert!(Path(blank()).validate().is_err());
        assert!(Query(blank()).validate().is_err());
        assert!(Form(blank()).validate().is_err());
        assert!(Header(blank()).validate().is_err());
        assert!(
            Json(Greeting { msg: "hi".into() }).validate().is_ok(),
            "a value keeping its rules passes"
        );
    }

    fn json_request<T: Serialize>(payload: &T) -> (Request, RequestBody) {
        Request::builder()
            .header(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )
            .body(serde_json::to_vec(payload).expect("body serializes"))
            .split()
    }

    #[tokio::test]
    async fn piped_pipes_the_extracted_value_and_binds_the_output() {
        let (req, mut body) = json_request(&"hello".to_owned());
        let Piped(shouted) = Piped::<ToUpper, Json<String>>::from_request(&req, &mut body)
            .await
            .expect("happy path");
        assert_eq!(shouted, "HELLO");
    }

    #[tokio::test]
    async fn piped_rejects_when_the_pipe_returns_an_error() {
        let (req, mut body) = json_request(&"hello".to_owned());
        let Err(err) = Piped::<AlwaysReject, Json<String>>::from_request(&req, &mut body).await
        else {
            panic!("the pipe should have rejected");
        };
        let (status, json, _) = problem_of(err).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["detail"], "nope");
    }

    #[tokio::test]
    async fn piped_rejects_a_body_past_the_configured_limit_with_413() {
        let (req, mut body) = json_request(&"well over four bytes".to_owned());
        let Err(err) = crate::raw_body::with_body_limit(
            4,
            Piped::<ToUpper, Json<String>>::from_request(&req, &mut body),
        )
        .await
        else {
            panic!("body exceeds the cap");
        };
        assert_eq!(err.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn piped_accepts_a_body_within_the_configured_limit() {
        let (req, mut body) = json_request(&"hi".to_owned());
        let Piped(shouted) = crate::raw_body::with_body_limit(
            4096,
            Piped::<ToUpper, Json<String>>::from_request(&req, &mut body),
        )
        .await
        .expect("body fits under the cap");
        assert_eq!(shouted, "HI");
    }

    #[derive(Debug, Serialize, Deserialize, Validate)]
    struct Login {
        #[validate(length(min = 8))]
        password: String,
    }

    #[tokio::test]
    async fn a_validation_refusal_names_the_field_and_never_echoes_the_value() {
        let Err(errors) = (Login {
            password: "hunter2".into(),
        })
        .validate() else {
            panic!("validation should have rejected");
        };
        let (status, json, text) = problem_of(pipe_error(&PipeError::from(errors))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            !text.contains("hunter2"),
            "the submitted value is echoed: {text}"
        );
        assert!(
            json.get("errors").and_then(|d| d.get("password")).is_some(),
            "errors should still name the failing field: {json}",
        );
    }
}
