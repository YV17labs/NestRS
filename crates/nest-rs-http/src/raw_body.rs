//! Raw request body extractor with a size guard.
//!
//! [`RawBody`] reads the whole body into [`Bytes`], capped at
//! [`RawBody::DEFAULT_LIMIT`] (2 MiB) unless the request carries a
//! [`RawBodyLimit`] in its extensions (installed by `HttpModule` from
//! `HttpConfig.max_body_bytes`). Past the limit the extractor rejects with
//! `413 Payload Too Large` — never silently truncates, never buffers
//! unbounded memory.
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

/// Per-request byte cap for [`RawBody`], read from the request extensions.
/// `HttpModule` installs this from `HttpConfig.max_body_bytes`; absent ⇒ the
/// extractor falls back to [`RawBody::DEFAULT_LIMIT`]. Public so middleware
/// can pin a per-route cap by inserting it into `req.extensions_mut()`.
#[derive(Debug, Clone, Copy)]
pub struct RawBodyLimit(pub usize);

/// Whole request body as `Bytes`, bounded by [`RawBody::DEFAULT_LIMIT`] (or by
/// the [`RawBodyLimit`] in the request extensions, when installed).
#[derive(Debug, Clone)]
pub struct RawBody(pub Bytes);

impl RawBody {
    /// Default cap: 2 MiB. Generous for webhook payloads, tight enough to
    /// resist a memory-exhaustion attempt from a single request.
    pub const DEFAULT_LIMIT: usize = 2 * 1024 * 1024;

    /// Consume the wrapper and return the inner bytes.
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
    async fn from_request(req: &'a Request, body: &mut RequestBody) -> Result<Self> {
        let limit = req
            .extensions()
            .get::<RawBodyLimit>()
            .map(|l| l.0)
            .unwrap_or(Self::DEFAULT_LIMIT);
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
    async fn request_extension_limit_overrides_the_default() {
        // 64-byte payload, 32-byte extension cap → 413, mirroring the
        // `extract_with_limit` behaviour driven through the extractor.
        let mut req = Request::builder().body(vec![b'x'; 64]);
        req.extensions_mut().insert(RawBodyLimit(32));
        let (req, mut body) = req.split();
        let err = RawBody::from_request(&req, &mut body)
            .await
            .expect_err("over the extension cap");
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn request_extension_limit_passes_when_payload_fits() {
        // Same 32-byte payload + cap, but threaded through the extension —
        // pins that the extractor reads the cap rather than the constant.
        let mut req = Request::builder().body(vec![b'x'; 32]);
        req.extensions_mut().insert(RawBodyLimit(32));
        let (req, mut body) = req.split();
        let raw = RawBody::from_request(&req, &mut body).await.expect("fits");
        assert_eq!(raw.0.len(), 32);
    }

    #[tokio::test]
    async fn missing_extension_falls_back_to_default_limit() {
        // No `RawBodyLimit` in extensions ⇒ DEFAULT_LIMIT applies. A tiny
        // payload under the constant must still pass.
        let (req, mut body) = request_with_body("hi");
        let raw = RawBody::from_request(&req, &mut body).await.expect("fits");
        assert_eq!(&raw.0[..], b"hi");
    }
}
