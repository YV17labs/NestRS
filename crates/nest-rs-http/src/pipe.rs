//! HTTP binding for nestrs pipes — the extractor wrappers that apply a
//! [`nest_rs_pipes::Pipe`] to a handler parameter between extraction and the
//! handler.
//!
//! - [`Valid<E>`] (e.g. `Valid<Json<T>>`) validates with `validator::Validate`.
//! - [`Piped<P, E>`] applies pipe `P` to what `E` produced.
//!
//! Both reject through [`pipe_error`]: an RFC-9457 `application/problem+json`
//! `400` ([`ProblemDetails`]) carrying the [`PipeError`]'s message as `detail`
//! and any field-level errors as an `errors` extension member.

use std::future::Future;
use std::marker::PhantomData;
use std::ops::Deref;
use std::pin::Pin;

use nest_rs_pipes::{Pipe, PipeError, ValidationPipe};
use validator::Validate;

use crate::{
    Form, FromRequest, Header, HttpError, Json, Path, ProblemDetails, Query, Request, RequestBody,
};

/// Owned-unwrap for an extractor, so a pipe can take the value it carries
/// without cloning.
pub trait IntoInner {
    /// The value the wrapping extractor carries.
    type Inner;
    /// Unwrap the extractor into the value it carries, by value.
    fn into_inner(self) -> Self::Inner;
}

/// `IntoInner` for each single-value extractor: its value, out of its wrapper.
macro_rules! into_inner {
    ($($extractor:ty),* $(,)?) => {
        $(
            impl<T> IntoInner for $extractor {
                type Inner = T;
                fn into_inner(self) -> T {
                    self.0
                }
            }
        )*
    };
}

into_inner!(Json<T>, Path<T>, Query<T>, Form<T>, Header<T>);

/// Render a [`PipeError`] as the `400` problem document it answers, its
/// field-level errors under the `errors` extension member.
///
/// ```
/// use nest_rs_http::{StatusCode, pipe_error};
/// use nest_rs_pipes::PipeError;
///
/// let refused = pipe_error(&PipeError::new("after is not a cursor this endpoint issued"));
/// assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
/// ```
pub fn pipe_error(err: &PipeError) -> HttpError {
    let mut problem = ProblemDetails::bad_request().with_detail(err.message().to_owned());
    if let Some(details) = err.details() {
        problem = problem.with_extension("errors", details.clone());
    }
    problem.into()
}

/// Extract `E` and unwrap it to its inner value; each extractor of the
/// vocabulary that reads the body reads it under the cap itself. The future is
/// boxed: a generic `async fn` delegating to another's trips rustc#100013.
async fn extract_inner<'a, E>(req: &'a Request, body: &mut RequestBody) -> crate::Result<E::Inner>
where
    E: FromRequest<'a> + IntoInner,
{
    let extract: Pin<Box<dyn Future<Output = crate::Result<E>> + Send + '_>> =
        Box::pin(E::from_request(req, body));
    Ok(extract.await?.into_inner())
}

/// Applies pipe `P` to the value extractor `E` produces, handing the handler
/// the transformed `P::Out`.
pub struct Piped<P: Pipe, E> {
    value: P::Out,
    _marker: PhantomData<fn() -> E>,
}

impl<P: Pipe, E> Piped<P, E> {
    /// Take ownership of the pipe's output value.
    pub fn into_inner(self) -> P::Out {
        self.value
    }

    fn of(value: P::Out) -> Self {
        Self {
            value,
            _marker: PhantomData,
        }
    }
}

impl<P: Pipe, E> Deref for Piped<P, E> {
    type Target = P::Out;
    fn deref(&self) -> &P::Out {
        &self.value
    }
}

impl<'a, P, E> FromRequest<'a> for Piped<P, E>
where
    P: Pipe + Send + Sync,
    P::Out: Send,
    E: FromRequest<'a> + IntoInner<Inner = P::In>,
{
    async fn from_request(req: &'a Request, body: &mut RequestBody) -> crate::Result<Self> {
        let input = extract_inner::<E>(req, body).await?;
        P::transform(input)
            .map(Self::of)
            .map_err(|refused| pipe_error(&refused))
    }
}

/// Validation pipe: extract `E`, validate with `validator::Validate`, reject
/// invalid input with a field-level JSON `400`. `Valid<Json<T>>` is the
/// ergonomic form of `Piped<ValidationPipe<T>, Json<T>>`.
///
/// The field is public so a handler can destructure it:
/// `Valid(input): Valid<Json<CreateUser>>`.
pub struct Valid<E: IntoInner>(pub E::Inner);

impl<E: IntoInner> Valid<E> {
    /// Take ownership of the validated inner value.
    pub fn into_inner(self) -> E::Inner {
        self.0
    }
}

impl<E: IntoInner> Deref for Valid<E> {
    type Target = E::Inner;
    fn deref(&self) -> &E::Inner {
        &self.0
    }
}

impl<'a, E> FromRequest<'a> for Valid<E>
where
    E: FromRequest<'a> + IntoInner,
    E::Inner: Validate,
{
    async fn from_request(req: &'a Request, body: &mut RequestBody) -> crate::Result<Self> {
        let input = extract_inner::<E>(req, body).await?;
        ValidationPipe::<E::Inner>::transform(input)
            .map(Valid)
            .map_err(|refused| pipe_error(&refused))
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
    fn into_inner_unwraps_each_single_value_extractor() {
        assert_eq!(IntoInner::into_inner(Json(42)), 42);
        assert_eq!(IntoInner::into_inner(Path("/users/123")), "/users/123");
        assert_eq!(IntoInner::into_inner(Query(7)), 7);
        assert_eq!(IntoInner::into_inner(Form(8)), 8);
        assert_eq!(IntoInner::into_inner(Header("abc")), "abc");
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

    #[test]
    fn piped_and_valid_hand_back_and_lend_their_value() {
        let p: Piped<ToUpper, Json<String>> = Piped::of("HELLO".into());
        assert_eq!(p.len(), 5);
        assert_eq!(p.into_inner(), "HELLO");
        let v: Valid<Json<String>> = Valid("ok".into());
        assert_eq!(&*v, "ok");
        assert_eq!(v.into_inner(), "ok");
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq, Validate)]
    struct Greeting {
        #[validate(length(min = 1))]
        msg: String,
    }

    #[derive(Debug, Serialize, Deserialize, Validate)]
    struct Login {
        #[validate(length(min = 8))]
        password: String,
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
    async fn piped_pipes_the_extracted_value() {
        let (req, mut body) = json_request(&"hello".to_owned());
        let piped = Piped::<ToUpper, Json<String>>::from_request(&req, &mut body)
            .await
            .expect("happy path");
        assert_eq!(piped.into_inner(), "HELLO");
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
    async fn valid_returns_the_validated_payload() {
        let payload = Greeting { msg: "hi".into() };
        let (req, mut body) = json_request(&payload);
        let v = Valid::<Json<Greeting>>::from_request(&req, &mut body)
            .await
            .expect("validation passes");
        assert_eq!(v.into_inner(), payload);
    }

    #[tokio::test]
    async fn valid_rejects_a_body_past_the_configured_limit_with_413() {
        let (req, mut body) = json_request(&Greeting {
            msg: "well over four bytes".into(),
        });
        let Err(err) = crate::raw_body::with_body_limit(
            4,
            Valid::<Json<Greeting>>::from_request(&req, &mut body),
        )
        .await
        else {
            panic!("body exceeds the cap");
        };
        assert_eq!(err.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn valid_accepts_a_body_within_the_configured_limit() {
        let payload = Greeting { msg: "hi".into() };
        let (req, mut body) = json_request(&payload);
        let v = crate::raw_body::with_body_limit(
            4096,
            Valid::<Json<Greeting>>::from_request(&req, &mut body),
        )
        .await
        .expect("body fits under the cap");
        assert_eq!(v.into_inner(), payload);
    }

    #[tokio::test]
    async fn valid_rejects_with_400_and_field_details_on_invalid_input() {
        let (req, mut body) = json_request(&Greeting { msg: String::new() });
        let Err(err) = Valid::<Json<Greeting>>::from_request(&req, &mut body).await else {
            panic!("validation should have rejected");
        };
        let (status, json, _) = problem_of(err).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["status"], 400);
        assert!(
            json.get("errors").and_then(|d| d.get("msg")).is_some(),
            "errors should name the failing field: {json}",
        );
    }

    #[tokio::test]
    async fn a_valid_rejection_does_not_echo_the_submitted_value_in_the_400_body() {
        let (req, mut body) = json_request(&Login {
            password: "hunter2".into(),
        });
        let Err(err) = Valid::<Json<Login>>::from_request(&req, &mut body).await else {
            panic!("validation should have rejected");
        };
        let (status, json, text) = problem_of(err).await;
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
