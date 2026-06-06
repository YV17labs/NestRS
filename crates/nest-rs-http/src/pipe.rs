//! HTTP binding for nestrs pipes — the poem adapter that applies a
//! [`nest_rs_pipes::Pipe`] to a handler parameter between extraction and the
//! handler.
//!
//! - [`Valid<E>`] (e.g. `Valid<Json<T>>`) validates with `validator::Validate`.
//! - [`Piped<P, E>`] applies pipe `P` to what `E` produced.
//!
//! Both reject with a JSON `400` carrying the [`PipeError`]'s message and any
//! structured `details`.

use std::future::Future;
use std::marker::PhantomData;
use std::ops::Deref;
use std::pin::Pin;

use nest_rs_pipes::{Pipe, PipeError, ValidationPipe};
use poem::http::StatusCode;
use poem::web::{Json, Path, Query};
use poem::{Error, FromRequest, Request, RequestBody, Response, Result};
use validator::Validate;

/// Owned-unwrap for poem extractors, so a pipe can take the value they carry
/// without cloning.
pub trait IntoInner {
    type Inner;
    fn into_inner(self) -> Self::Inner;
}

impl<T> IntoInner for Json<T> {
    type Inner = T;
    fn into_inner(self) -> T {
        self.0
    }
}

impl<T> IntoInner for Path<T> {
    type Inner = T;
    fn into_inner(self) -> T {
        self.0
    }
}

impl<T> IntoInner for Query<T> {
    type Inner = T;
    fn into_inner(self) -> T {
        self.0
    }
}

fn reject(err: PipeError) -> Error {
    let mut body = serde_json::json!({
        "statusCode": 400,
        "error": "Bad Request",
        "message": err.message(),
    });
    if let Some(details) = err.into_details() {
        body["details"] = details;
    }
    Error::from_response(
        Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .content_type("application/json")
            .body(serde_json::to_vec(&body).unwrap_or_default()),
    )
}

/// Extract `E` and unwrap it to its inner value. The inner future is erased to
/// `dyn Future + Send` before awaiting: a generic `async fn` delegating to
/// another's future trips rustc#100013 ("lifetime bound not satisfied"). Boxing
/// it once here keeps the workaround in a single place.
async fn extract_inner<'a, E>(req: &'a Request, body: &mut RequestBody) -> Result<E::Inner>
where
    E: FromRequest<'a> + IntoInner,
{
    let extract: Pin<Box<dyn Future<Output = Result<E>> + Send + '_>> =
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
    pub fn into_inner(self) -> P::Out {
        self.value
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
    async fn from_request(req: &'a Request, body: &mut RequestBody) -> Result<Self> {
        let value = P::transform(extract_inner::<E>(req, body).await?).map_err(reject)?;
        Ok(Self {
            value,
            _marker: PhantomData,
        })
    }
}

/// Validation pipe: extract `E`, validate with `validator::Validate`, reject
/// invalid input with a field-level JSON `400`. `Valid<Json<T>>` is the
/// ergonomic form of `Piped<ValidationPipe<T>, Json<T>>`.
pub struct Valid<E: IntoInner>(E::Inner);

impl<E: IntoInner> Valid<E> {
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
    async fn from_request(req: &'a Request, body: &mut RequestBody) -> Result<Self> {
        let value = ValidationPipe::<E::Inner>::transform(extract_inner::<E>(req, body).await?)
            .map_err(reject)?;
        Ok(Valid(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reject_emits_a_400_json_body_with_the_message() {
        let err = PipeError::new("not a uuid");
        let resp = reject(err).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let bytes = resp.into_body().into_bytes().await.expect("body");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(json["statusCode"], 400);
        assert_eq!(json["error"], "Bad Request");
        assert_eq!(json["message"], "not a uuid");
        assert!(json.get("details").is_none(), "no details on a plain error");
    }

    #[tokio::test]
    async fn reject_carries_structured_details_when_present() {
        let err = PipeError::with_details(
            "validation failed",
            serde_json::json!({ "email": ["not an email"] }),
        );
        let resp = reject(err).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let bytes = resp.into_body().into_bytes().await.expect("body");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(json["message"], "validation failed");
        assert_eq!(json["details"]["email"][0], "not an email");
    }

    // `IntoInner` is the owned-unwrap shim. The three impls are
    // line-for-line identical; pin one for each so a future rename of `.0`
    // or a generic mismatch surfaces immediately.

    #[test]
    fn into_inner_for_json_unwraps_the_payload() {
        let j = Json(42i32);
        assert_eq!(j.into_inner(), 42);
    }

    #[test]
    fn into_inner_for_path_unwraps_the_payload() {
        let p = Path("/users/123".to_string());
        assert_eq!(p.into_inner(), "/users/123");
    }

    #[test]
    fn into_inner_for_query_unwraps_the_payload() {
        #[derive(Debug, PartialEq)]
        struct Q {
            first: u32,
        }
        let q = Query(Q { first: 7 });
        assert_eq!(q.into_inner(), Q { first: 7 });
    }

    // Piped<P, E> exposes `into_inner` and `Deref<Target = P::Out>`. Build a
    // `Piped` directly (the field is private so tests live here, the only
    // module that can see it) and exercise both.

    struct ToUpper;

    impl nest_rs_pipes::Pipe for ToUpper {
        type In = String;
        type Out = String;
        fn transform(input: String) -> std::result::Result<String, PipeError> {
            Ok(input.to_ascii_uppercase())
        }
    }

    #[test]
    fn piped_into_inner_yields_the_transformed_value() {
        let p: Piped<ToUpper, Json<String>> = Piped {
            value: "HELLO".into(),
            _marker: PhantomData,
        };
        assert_eq!(p.into_inner(), "HELLO");
    }

    #[test]
    fn piped_deref_borrows_the_transformed_value() {
        let p: Piped<ToUpper, Json<String>> = Piped {
            value: "world".into(),
            _marker: PhantomData,
        };
        assert_eq!(p.len(), 5);
        assert_eq!(&*p, "world");
    }

    #[test]
    fn valid_into_inner_yields_the_validated_value() {
        let v: Valid<Json<String>> = Valid("ok".into());
        assert_eq!(v.into_inner(), "ok");
    }

    #[test]
    fn valid_deref_borrows_the_validated_value() {
        let v: Valid<Json<String>> = Valid("ok".into());
        assert_eq!(v.len(), 2);
        assert_eq!(&*v, "ok");
    }

    // `from_request` paths. Build a real `Request` with a JSON body and run
    // the extractor by hand — same shape poem invokes at the route boundary.

    use poem::Body;
    use serde::{Deserialize, Serialize};
    use validator::Validate;

    fn json_request<T: Serialize>(payload: &T) -> (Request, poem::RequestBody) {
        let body = Body::from_json(payload).expect("body serializes");
        let req = Request::builder()
            .content_type("application/json")
            .body(body);
        req.split()
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq, Validate)]
    struct Greeting {
        #[validate(length(min = 1))]
        msg: String,
    }

    #[tokio::test]
    async fn piped_from_request_pipes_the_extracted_value() {
        let (req, mut body) = json_request(&"hello".to_string());
        let piped: Piped<ToUpper, Json<String>> =
            Piped::from_request(&req, &mut body).await.expect("happy path");
        assert_eq!(piped.into_inner(), "HELLO");
    }

    struct AlwaysReject;

    impl nest_rs_pipes::Pipe for AlwaysReject {
        type In = String;
        type Out = String;
        fn transform(_: String) -> std::result::Result<String, PipeError> {
            Err(PipeError::new("nope"))
        }
    }

    #[tokio::test]
    async fn piped_from_request_rejects_when_the_pipe_returns_an_error() {
        let (req, mut body) = json_request(&"hello".to_string());
        let err = match Piped::<AlwaysReject, Json<String>>::from_request(&req, &mut body).await {
            Ok(_) => panic!("the pipe should have rejected"),
            Err(e) => e,
        };
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = resp.into_body().into_bytes().await.expect("body");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(json["message"], "nope");
    }

    #[tokio::test]
    async fn valid_from_request_returns_the_validated_payload() {
        let payload = Greeting {
            msg: "hi".into(),
        };
        let (req, mut body) = json_request(&payload);
        let v: Valid<Json<Greeting>> = Valid::from_request(&req, &mut body)
            .await
            .expect("validation passes");
        assert_eq!(v.into_inner(), payload);
    }

    #[tokio::test]
    async fn valid_from_request_rejects_with_400_and_field_details_on_invalid_input() {
        // Empty `msg` fails the `length(min = 1)` rule.
        let payload = Greeting { msg: String::new() };
        let (req, mut body) = json_request(&payload);
        let err = match Valid::<Json<Greeting>>::from_request(&req, &mut body).await {
            Ok(_) => panic!("validation should have rejected"),
            Err(e) => e,
        };
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = resp.into_body().into_bytes().await.expect("body");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(json["statusCode"], 400);
        // The field-level details surface under the offending field name.
        assert!(
            json.get("details").and_then(|d| d.get("msg")).is_some(),
            "details should name the failing field: {json}",
        );
    }
}
