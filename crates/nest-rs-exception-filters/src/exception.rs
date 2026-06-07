//! [`ExceptionFilter`] — catches a single typed exception, NestJS-style.

use async_trait::async_trait;
use nest_rs_core::Layer;
use poem::Response;

/// Catches a typed exception thrown by a handler and maps it to a [`Response`].
///
/// `ExceptionFilter` extends [`Layer`] so it plugs into the same dedup-by-`TypeId`
/// chain as guards, interceptors, pipes, and filters. Each impl declares the
/// concrete error type it claims via [`Self::Exception`]; non-matching errors
/// fall through to any outer exception filter / [`Filter`](nest_rs_filters::Filter).
#[async_trait]
pub trait ExceptionFilter: Layer {
    /// The concrete exception this filter catches.
    type Exception: Send + 'static;

    async fn catch(&self, exception: Self::Exception) -> Response;
}
