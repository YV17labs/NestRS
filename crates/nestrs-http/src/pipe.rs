//! HTTP binding for nestrs pipes — the poem adapter that applies a
//! [`nestrs_pipes::Pipe`] to a handler parameter between extraction and the
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

use nestrs_pipes::{Pipe, PipeError, ValidationPipe};
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
