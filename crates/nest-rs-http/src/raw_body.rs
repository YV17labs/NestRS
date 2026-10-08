//! Raw request body extractor with a size guard.
//!
//! [`RawBody`] reads the whole body into [`Bytes`], capped at
//! [`RawBody::DEFAULT_LIMIT`] (2 MiB) unless the transport edge carries a
//! configured cap ([`current_body_limit`]); past it, `413 Payload Too Large`.
//! For a handler that needs the exact bytes, such as a webhook verifying a
//! signature.

use std::ops::Deref;

use bytes::Bytes;
use poem::error::ReadBodyError;
use poem::http::StatusCode;
use poem::{Error, FromRequest, Request, RequestBody, Result};

tokio::task_local! {
    /// The edge's configured whole-body cap for the current request. Not
    /// carried on `RequestContinuation`: its readers are extractors.
    static BODY_LIMIT: usize;
}

/// Run `fut` with the edge's configured cap ambient; `None` installs nothing and
/// leaves readers on [`RawBody::DEFAULT_LIMIT`].
pub(crate) async fn with_body_limit<F: std::future::Future>(
    limit: Option<usize>,
    fut: F,
) -> F::Output {
    match limit {
        Some(limit) => BODY_LIMIT.scope(limit, fut).await,
        None => fut.await,
    }
}

/// The transport's configured whole-body byte cap, when one is installed.
/// Body readers fall back to their own default on `None`.
pub fn current_body_limit() -> Option<usize> {
    BODY_LIMIT.try_with(|limit| *limit).ok()
}

/// Whole request body as `Bytes`, bounded by [`RawBody::DEFAULT_LIMIT`] (or
/// by the transport edge's configured cap, when one is installed).
#[derive(Debug, Clone)]
pub struct RawBody(pub Bytes);

impl RawBody {
    /// Default cap: 2 MiB.
    pub const DEFAULT_LIMIT: usize = 2 * 1024 * 1024;

    /// Take ownership of the buffered body bytes.
    pub fn into_inner(self) -> Bytes {
        self.0
    }

    /// Extract with a caller-chosen byte cap.
    pub async fn extract_with_limit(body: &mut RequestBody, limit: usize) -> Result<Self> {
        let raw = body.take()?;
        match raw.into_bytes_limit(limit).await {
            Ok(bytes) => Ok(Self(bytes)),
            Err(ReadBodyError::PayloadTooLarge) => {
                Err(Error::from_status(StatusCode::PAYLOAD_TOO_LARGE))
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

impl<'a> FromRequest<'a> for RawBody {
    async fn from_request(_req: &'a Request, body: &mut RequestBody) -> Result<Self> {
        let limit = crate::current_body_limit().unwrap_or(Self::DEFAULT_LIMIT);
        Self::extract_with_limit(body, limit).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use poem::Body;

    fn request_with_body(payload: impl Into<Body>) -> (Request, RequestBody) {
        Request::builder().body(payload).split()
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
        let err =
            crate::raw_body::with_body_limit(Some(32), RawBody::from_request(&req, &mut body))
                .await
                .expect_err("over the ambient cap");
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn ambient_limit_passes_when_payload_fits() {
        let (req, mut body) = Request::builder().body(vec![b'x'; 32]).split();
        let raw =
            crate::raw_body::with_body_limit(Some(32), RawBody::from_request(&req, &mut body))
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
            Some(32),
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
