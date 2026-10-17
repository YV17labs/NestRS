//! Raw request body extractor with a size guard.
//!
//! [`RawBody`] reads the whole body into [`Bytes`], capped at the transport's
//! configured cap ([`current_body_limit`], [`RawBody::DEFAULT_LIMIT`] — 2 MiB —
//! unless configured); past it, `413 Payload Too Large`.
//! For code that needs the exact bytes. It reads after routing and the guards,
//! like every reader: the edge wraps the body and reads none of it, so a guard
//! verifying a webhook's signature reads it the same way, under the same cap,
//! and sets the bytes back for the handler.

use std::ops::Deref;

use bytes::Bytes;
use http::StatusCode;
use poem::error::ReadBodyError;

use crate::request::{FromRequest, Request, RequestBody};

tokio::task_local! {
    /// The edge's configured whole-body cap for the current request. Not
    /// carried on `RequestContinuation`: its readers are extractors.
    static BODY_LIMIT: usize;
}

/// Run `fut` with the edge's configured cap ambient.
pub(crate) fn with_body_limit<F: std::future::Future>(
    limit: usize,
    fut: F,
) -> impl std::future::Future<Output = F::Output> {
    BODY_LIMIT.scope(limit, fut)
}

/// The whole-body byte cap a reader — an extractor, a guard reading the body —
/// buffers at most: the transport's configured cap on a request it serves,
/// [`RawBody::DEFAULT_LIMIT`] off one.
///
/// ```
/// use nest_rs_http::{RawBody, current_body_limit};
///
/// assert_eq!(current_body_limit(), RawBody::DEFAULT_LIMIT);
/// ```
pub fn current_body_limit() -> usize {
    BODY_LIMIT
        .try_with(|limit| *limit)
        .unwrap_or(RawBody::DEFAULT_LIMIT)
}

/// Whole request body as `Bytes`, bounded by the transport's configured cap
/// ([`current_body_limit`]).
#[derive(Debug, Clone)]
pub struct RawBody(pub Bytes);

impl RawBody {
    /// Default cap: 2 MiB.
    pub const DEFAULT_LIMIT: usize = 2 * 1024 * 1024;

    /// Take ownership of the buffered body bytes.
    pub fn into_inner(self) -> Bytes {
        self.0
    }

    /// Read the body under a caller-chosen byte cap, for an extractor of
    /// its own that wants a tighter one than the transport's: `413` past it.
    ///
    /// ```
    /// use nest_rs_http::{RawBody, Request, StatusCode};
    /// # #[nest_rs_core::main]
    /// # async fn main() -> anyhow::Result<()> {
    ///
    /// let (_, mut body) = Request::builder().body("twelve bytes").split();
    /// let refused = RawBody::extract_with_limit(&mut body, 8).await.unwrap_err();
    /// assert_eq!(refused.status(), StatusCode::PAYLOAD_TOO_LARGE);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn extract_with_limit(body: &mut RequestBody, limit: usize) -> crate::Result<Self> {
        Ok(Self(body.take()?.into_bytes_limit(limit).await?))
    }

    /// [`extract_with_limit`](Self::extract_with_limit) over poem's body, which
    /// `#[routes]` still hands its extractors.
    async fn extract_poem_with_limit(
        body: &mut poem::RequestBody,
        limit: usize,
    ) -> poem::Result<Self> {
        let raw = body.take()?;
        match raw.into_bytes_limit(limit).await {
            Ok(bytes) => Ok(Self(bytes)),
            Err(ReadBodyError::PayloadTooLarge) => {
                Err(poem::Error::from_status(StatusCode::PAYLOAD_TOO_LARGE))
            }
            Err(err) => Err(err.into()),
        }
    }
}

impl Deref for RawBody {
    type Target = Bytes;
    fn deref(&self) -> &Bytes {
        &self.0
    }
}

impl<'a> poem::FromRequest<'a> for RawBody {
    async fn from_request(
        _req: &'a poem::Request,
        body: &mut poem::RequestBody,
    ) -> poem::Result<Self> {
        Self::extract_poem_with_limit(body, crate::current_body_limit()).await
    }
}

impl<'a> FromRequest<'a> for RawBody {
    async fn from_request(_req: &'a Request, body: &mut RequestBody) -> crate::Result<Self> {
        Self::extract_with_limit(body, crate::current_body_limit()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Body;

    fn request_with_body(payload: impl Into<Body>) -> (Request, RequestBody) {
        Request::builder().body(payload).split()
    }

    /// The poem twin `#[routes]` still calls reads under the same cap.
    #[tokio::test]
    async fn the_poem_reading_answers_the_same_413() {
        let (req, mut body) = poem::Request::builder().body(vec![b'x'; 64]).split();
        let err = with_body_limit(
            32,
            <RawBody as poem::FromRequest>::from_request(&req, &mut body),
        )
        .await
        .expect_err("over the ambient cap");
        assert_eq!(err.into_response().status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn happy_path_reads_the_full_payload() {
        let (req, mut body) = request_with_body("hello world");
        let raw = RawBody::from_request(&req, &mut body).await.expect("read");
        assert_eq!(&raw.0[..], b"hello world");
        assert_eq!(raw.len(), 11);
    }

    #[tokio::test]
    async fn empty_body_yields_empty_bytes() {
        let (req, mut body) = request_with_body(Body::empty());
        let raw = RawBody::from_request(&req, &mut body).await.expect("read");
        assert!(raw.0.is_empty());
    }

    #[tokio::test]
    async fn oversize_body_returns_413_payload_too_large() {
        let payload = vec![b'x'; RawBody::DEFAULT_LIMIT + 1];
        let (req, mut body) = request_with_body(payload);
        let err = RawBody::from_request(&req, &mut body)
            .await
            .expect_err("over the cap");
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn extract_with_limit_enforces_the_caller_cap() {
        let payload = vec![b'x'; 64];
        let (_req, mut body) = request_with_body(payload);
        let err = RawBody::extract_with_limit(&mut body, 32)
            .await
            .expect_err("over the tight cap");
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn extract_with_limit_passes_when_payload_fits() {
        let payload = vec![b'x'; 32];
        let (_req, mut body) = request_with_body(payload);
        let raw = RawBody::extract_with_limit(&mut body, 32)
            .await
            .expect("fits");
        assert_eq!(raw.0.len(), 32);
    }

    #[tokio::test]
    async fn ambient_limit_overrides_the_default() {
        let (req, mut body) = Request::builder().body(vec![b'x'; 64]).split();
        let err = crate::raw_body::with_body_limit(32, RawBody::from_request(&req, &mut body))
            .await
            .expect_err("over the ambient cap");
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn ambient_limit_passes_when_payload_fits() {
        let (req, mut body) = Request::builder().body(vec![b'x'; 32]).split();
        let raw = crate::raw_body::with_body_limit(32, RawBody::from_request(&req, &mut body))
            .await
            .expect("fits");
        assert_eq!(raw.0.len(), 32);
    }

    #[tokio::test]
    async fn missing_ambient_limit_falls_back_to_default() {
        let (req, mut body) = request_with_body("hi");
        let raw = RawBody::from_request(&req, &mut body).await.expect("fits");
        assert_eq!(&raw.0[..], b"hi");
    }

    /// `/mcp` and `/graphql` open their own request scope inside the edge's.
    #[tokio::test]
    async fn a_nested_request_scope_does_not_clear_the_edges_cap() {
        let scope = std::sync::Arc::new(nest_rs_core::RequestScope::new(
            nest_rs_core::Container::builder().build(),
        ));
        let (req, mut body) = Request::builder().body(vec![b'x'; 64]).split();

        let err = crate::raw_body::with_body_limit(
            32,
            nest_rs_core::with_request_scope(
                Some(scope),
                nest_rs_core::Correlation::minted(None),
                RawBody::from_request(&req, &mut body),
            ),
        )
        .await
        .expect_err("the edge's cap survives the self-mount's own scope");
        assert_eq!(err.into_response().status(), StatusCode::PAYLOAD_TOO_LARGE,);
    }
}
