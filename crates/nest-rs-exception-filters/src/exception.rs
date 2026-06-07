//! [`ExceptionFilter`] — catches a single typed exception.

use std::error::Error as StdError;

use async_trait::async_trait;
use nest_rs_core::Layer;
use poem::Response;

/// Catches a typed exception thrown by a handler and maps it to a [`Response`].
///
/// `ExceptionFilter` extends [`Layer`] so it plugs into the same dedup-by-`TypeId`
/// chain as guards, interceptors, pipes, and filters. Each impl declares the
/// concrete error type it claims via [`Self::Exception`]; non-matching errors
/// fall through to the next exception filter, then to any outer
/// [`Filter`](nest_rs_filters::Filter), then back to poem's default error
/// renderer.
///
/// The bound on [`Self::Exception`] is the contract `poem::Error::downcast`
/// requires: anything carryable as a `Box<dyn std::error::Error + Send + Sync>`
/// works.
#[async_trait]
pub trait ExceptionFilter: Layer {
    /// The concrete exception this filter catches.
    type Exception: StdError + Send + Sync + 'static;

    async fn catch(&self, exception: Self::Exception) -> Response;
}
