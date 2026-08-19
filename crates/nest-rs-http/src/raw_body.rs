//! Raw request body extractor with a size guard.
//!
//! [`RawBody`] reads the whole body into [`Bytes`], capped at
//! [`RawBody::DEFAULT_LIMIT`] (2 MiB) unless the transport edge carries a
//! configured cap (`HttpConfig.max_body_bytes`, read back through
//! [`current_body_limit`](crate::current_body_limit)). Past the limit the
//! extractor rejects with `413 Payload Too Large` — never silently
//! truncates, never buffers unbounded memory.
//!
//! Webhook-style handlers (Stripe, GitHub, …) that need the exact byte string
//! to verify a signature are the canonical use case; anything that can deserialize
//! through `Json<T>` should use that instead.
//!
//! Use [`RawBody::extract_with_limit`] for a tighter cap on a specific route.

use std::ops::Deref;

use bytes::Bytes;
use poem::error::ReadBodyError;
use poem::http::StatusCode;
use poem::{Error, FromRequest, Request, RequestBody, Result};

tokio::task_local! {
    /// The edge's configured whole-body cap for the current request.
    ///
    /// **HTTP's, and only HTTP's.** It rode in `nest_rs_core`'s request context
    /// for a while, which cost the other five edges a positional `None` at
    /// fourteen call sites for a concept their protocol does not have — and read
    /// as "every edge has considered this" while none of them could supply it. A
    /// whole *body* is something one transport has.
    ///
    /// Not carried on [`RequestContinuation`](nest_rs_core::RequestContinuation):
    /// what re-installs around a streaming body is the request's *identity*, and
    /// the two readers of this cap are extractors, which have both run by the
    /// time a body is written.
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
    /// Default cap: 2 MiB. Generous for webhook payloads, tight enough to
    /// resist a memory-exhaustion attempt from a single request.
    pub const DEFAULT_LIMIT: usize = 2 * 1024 * 1024;

    /// Take ownership of the buffered body bytes.
    pub fn into_inner(self) -> Bytes {
        self.0
    }

    /// Extract with a caller-chosen byte cap. Use when a handler knows its
    /// payload should stay well under the default.
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
        assert_eq!(raw.len(), 11); // Deref<Target = Bytes>
    }

    #[tokio::test]
    async fn empty_body_yields_empty_bytes() {
        let (req, mut body) = request_with_body(Body::empty());
        let raw = RawBody::from_request(&req, &mut body).await.expect("read");
        assert!(raw.0.is_empty());
    }

    #[tokio::test]
    async fn oversize_body_returns_413_payload_too_large() {
        // Exceed the default limit by 1 byte.
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
        // Well under the default, but past the tighter cap.
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
        // 64-byte payload, 32-byte ambient cap → 413, mirroring the
        // `extract_with_limit` behaviour driven through the extractor.
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
        // Same 32-byte payload + cap, threaded through the ambient context —
        // pins that the extractor reads the cap rather than the constant.
        let (req, mut body) = Request::builder().body(vec![b'x'; 32]).split();
        let raw =
            crate::raw_body::with_body_limit(Some(32), RawBody::from_request(&req, &mut body))
                .await
                .expect("fits");
        assert_eq!(raw.0.len(), 32);
    }

    #[tokio::test]
    async fn missing_ambient_limit_falls_back_to_default() {
        // No ambient cap ⇒ DEFAULT_LIMIT applies. A tiny
        // payload under the constant must still pass.
        let (req, mut body) = request_with_body("hi");
        let raw = RawBody::from_request(&req, &mut body).await.expect("fits");
        assert_eq!(&raw.0[..], b"hi");
    }

    /// A self-mounted endpoint opens its own request scope inside the edge's —
    /// `/mcp` and `/graphql` both do — and that must not cost it the cap.
    ///
    /// It did, and this is the regression: the cap used to be a field on the
    /// kernel's request context, so a nested `with_request_scope` installed a
    /// whole new context with no cap in it and every whole-body reader below
    /// silently fell back to `DEFAULT_LIMIT`. A deployment that pinned
    /// `max_body_bytes` had it on its controllers and not on its self-mounts,
    /// with nothing to say so. The cap is this transport's own task-local now,
    /// and a scope re-installed inside it does not reach it.
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
                nest_rs_core::Correlation::mint(),
                RawBody::from_request(&req, &mut body),
            ),
        )
        .await
        .expect_err("the edge's cap survives the self-mount's own scope");
        assert_eq!(err.into_response().status(), StatusCode::PAYLOAD_TOO_LARGE,);
    }
}
